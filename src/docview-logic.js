// Document tab pure logic (Task 14, spec §5-2): link / image resolution inside workspaceRoot,
// navigation history, table of contents, sanitizer configuration + string pre-filter, key map,
// in-document search. No DOM, no Tauri: unit-tested in Node. docview.js renders.
//
// Paths here are workspace-root relative with `/` separators (`rel`, as `doc_read` returns
// them). They are a convenience only: the backend re-validates every path it is given.

/** `a/b/c.md` → `c.md`. */
export function docTitle(rel) {
  const s = String(rel || '');
  return s.slice(s.lastIndexOf('/') + 1);
}

function dirOf(rel) {
  const s = String(rel || '');
  const i = s.lastIndexOf('/');
  return i < 0 ? '' : s.slice(0, i);
}

/**
 * Join relative path `p` (`/` separated, already decoded) onto directory `base` and normalize
 * `.` / `..`. null when the result would leave the root, or when a segment is unusable
 * (empty result, NUL, a drive colon).
 */
export function joinRel(base, p) {
  const out = String(base || '')
    .split('/')
    .filter((s) => s && s !== '.');
  for (const seg of String(p).split('/')) {
    if (!seg || seg === '.') continue;
    if (seg === '..') {
      if (!out.length) return null;
      out.pop();
      continue;
    }
    if (/[\u0000-\u001f:]/.test(seg)) return null;
    out.push(seg);
  }
  return out.length ? out.join('/') : null;
}

const SCHEME = /^[a-z][a-z0-9+.-]*:/i;

/** Decoded path part of a local href (`?query` dropped), or null when unusable. */
function localPath(raw) {
  let s = String(raw).replace(/\\/g, '/');
  if (s.startsWith('//')) return null; // protocol-relative / UNC
  const q = s.indexOf('?');
  if (q >= 0) s = s.slice(0, q);
  try {
    s = decodeURIComponent(s);
  } catch (_) {
    return null;
  }
  if (/[\u0000-\u001f]/.test(s)) return null;
  return s;
}

/**
 * What a click on a link in document `currentRel` does:
 * - `{type:'anchor', id}` — `#id` in this document;
 * - `{type:'doc', rel, hash}` — a `.md` file inside the root (relative to the document, or
 *   to the root when it starts with `/`), opened in the same tab;
 * - `{type:'external', url}` — http(s), opened in the system browser;
 * - `{type:'ignore'}` — everything else (file:, javascript:, data:, drive paths, other files,
 *   paths leaving the root).
 */
export function resolveDocLink(currentRel, href) {
  const h = String(href ?? '').trim();
  if (!h) return { type: 'ignore' };
  if (h.startsWith('#')) {
    try {
      return { type: 'anchor', id: decodeURIComponent(h.slice(1)) };
    } catch (_) {
      return { type: 'ignore' };
    }
  }
  if (/^https?:/i.test(h)) {
    try {
      const u = new URL(h);
      if ((u.protocol === 'http:' || u.protocol === 'https:') && u.hostname) return { type: 'external', url: u.href };
    } catch (_) {
      /* not a URL */
    }
    return { type: 'ignore' };
  }
  if (SCHEME.test(h)) return { type: 'ignore' }; // javascript:, file:, data:, mailto:, C:\…
  const hashAt = h.indexOf('#');
  const pathPart = hashAt >= 0 ? h.slice(0, hashAt) : h;
  let hash = '';
  if (hashAt >= 0) {
    try {
      hash = decodeURIComponent(h.slice(hashAt + 1));
    } catch (_) {
      hash = '';
    }
  }
  const p = localPath(pathPart);
  if (p == null || !/\.md$/i.test(p)) return { type: 'ignore' };
  const rel = p.startsWith('/') ? joinRel('', p) : joinRel(dirOf(currentRel), p);
  return rel ? { type: 'doc', rel, hash } : { type: 'ignore' };
}

/**
 * An `<img src>` in document `currentRel`: `{type:'local', rel}` (inside the root → asset
 * URL), `{type:'keep'}` (data: image), `{type:'drop'}` (remote, other schemes, outside).
 */
export function resolveAssetSrc(currentRel, src) {
  const s = String(src ?? '').trim();
  if (!s) return { type: 'drop' };
  if (/^data:image\//i.test(s)) return { type: 'keep' };
  if (SCHEME.test(s)) return { type: 'drop' };
  const hashAt = s.indexOf('#');
  const p = localPath(hashAt >= 0 ? s.slice(0, hashAt) : s);
  if (p == null) return { type: 'drop' };
  const rel = p.startsWith('/') ? joinRel('', p) : joinRel(dirOf(currentRel), p);
  return rel ? { type: 'local', rel } : { type: 'drop' };
}

/**
 * Link navigation history of one document tab. Entries `{rel, scroll}`; `push(entry, scroll)` /
 * `back(scroll)` / `forward(scroll)` take the scroll position of the document being left (it is
 * restored on return) and return the entry to show (null when there is none). Pushing the
 * current document again is a no-op; a push drops the forward entries; at most `cap` kept.
 */
export function createHistory(first, cap = 100) {
  let list = [{ ...first }];
  let i = 0;
  return {
    current: () => list[i],
    canBack: () => i > 0,
    canForward: () => i < list.length - 1,
    entries: () => list.map((e) => ({ ...e })),
    push(entry, scroll = 0) {
      if (entry.rel === list[i].rel) return list[i];
      list[i].scroll = scroll;
      list = list.slice(0, i + 1);
      list.push({ ...entry });
      if (list.length > cap) list = list.slice(list.length - cap);
      i = list.length - 1;
      return list[i];
    },
    back(scroll = 0) {
      if (i === 0) return null;
      list[i].scroll = scroll;
      i--;
      return { ...list[i] };
    },
    forward(scroll = 0) {
      if (i >= list.length - 1) return null;
      list[i].scroll = scroll;
      i++;
      return { ...list[i] };
    },
    /** Remember the current document's scroll position. */
    setScroll(scroll) {
      list[i].scroll = scroll;
    },
  };
}

/** GitHub-like heading slug: lowercase, letters/numbers/space/_/- kept, spaces → `-`. */
export function slugify(text) {
  const s = String(text ?? '')
    .toLowerCase()
    .trim()
    .replace(/[^\p{L}\p{N}\s_-]/gu, '')
    .trim()
    .replace(/\s+/g, '-');
  return s || 'section';
}

/**
 * Table of contents from `[{level, text}]` (document order): unique ids (`x`, `x-1`, `x-2`, …,
 * also avoiding clashes with an earlier literal `x-1`), labels indented two spaces per level
 * below the shallowest heading.
 */
export function extractToc(headings) {
  const list = headings || [];
  const min = list.length ? Math.min(...list.map((h) => h.level)) : 1;
  const used = new Set();
  return list.map((h) => {
    const base = slugify(h.text);
    let id = base;
    for (let n = 1; used.has(id); n++) id = `${base}-${n}`;
    used.add(id);
    const text = String(h.text ?? '');
    return { level: h.level, text, id, label: '  '.repeat(Math.max(0, h.level - min)) + (text || '(제목 없음)') };
  });
}

// ---------------------------------------------------------------- sanitizer

const FORBID_ACTIVE = [
  'script', 'style', 'iframe', 'frame', 'frameset', 'object', 'embed', 'applet', 'form', 'input',
  'button', 'textarea', 'select', 'option', 'base', 'meta', 'link', 'template', 'portal',
  'audio', 'video', 'source', 'track', 'noscript', 'dialog', 'foreignobject', 'use',
];
const FORBID_ATTR_COMMON = ['target', 'formaction', 'action', 'srcset', 'ping', 'background', 'poster', 'xlink:href', 'autofocus', 'tabindex', 'contenteditable', 'accesskey'];

/**
 * DOMPurify config for marked's output (agent-written markdown + raw HTML): no scripts,
 * no active/embedded content, no forms, no inline style, no data-* attributes, ids prefixed
 * (`user-content-`) against DOM clobbering. Event-handler attributes are always removed by
 * DOMPurify; the string pre-filter removes them too.
 */
export const SANITIZE_CONFIG = Object.freeze({
  USE_PROFILES: Object.freeze({ html: true }),
  FORBID_TAGS: Object.freeze([...FORBID_ACTIVE, 'svg', 'math']),
  FORBID_ATTR: Object.freeze([...FORBID_ATTR_COMMON, 'style']),
  ALLOW_DATA_ATTR: false,
  ALLOW_ARIA_ATTR: true,
  ALLOW_UNKNOWN_PROTOCOLS: false,
  SANITIZE_DOM: true,
  SANITIZE_NAMED_PROPS: true,
  WHOLE_DOCUMENT: false,
});

/**
 * Second pass over the rendered body after KaTeX (defence in depth: KaTeX 0.16.9 output is
 * re-sanitized). Allows KaTeX's MathML, SVG (stretchy symbols) and inline `style` — the first
 * pass already removed every svg/math element and style attribute the document itself had.
 */
export const FINAL_SANITIZE_CONFIG = Object.freeze({
  USE_PROFILES: Object.freeze({ html: true, svg: true, mathMl: true }),
  FORBID_TAGS: Object.freeze([...FORBID_ACTIVE]),
  FORBID_ATTR: Object.freeze([...FORBID_ATTR_COMMON]),
  ALLOW_DATA_ATTR: false,
  ALLOW_ARIA_ATTR: true,
  ALLOW_UNKNOWN_PROTOCOLS: false,
  SANITIZE_DOM: true,
  SANITIZE_NAMED_PROPS: false,
  WHOLE_DOCUMENT: false,
});

/**
 * String pre-filter run before DOMPurify (belt and braces, not the sanitizer): drops
 * script/style/iframe/object/embed elements (also unclosed ones), `on*=` attributes and
 * `javascript:` URLs inside tags. Text outside tags is untouched.
 */
export function prefilterHtml(html) {
  let s = String(html ?? '');
  for (const t of ['script', 'style', 'iframe', 'object', 'embed', 'noscript', 'template']) {
    s = s.replace(new RegExp(`<${t}\\b[\\s\\S]*?<\\/${t}\\s*>`, 'gi'), '');
    s = s.replace(new RegExp(`<\\/?${t}\\b[^>]*>?`, 'gi'), '');
  }
  // inside a tag: on…= attributes (after whitespace or `/`) and javascript: URLs
  s = s.replace(/<[^>]*>/g, (tag) => {
    let t = tag;
    let prev;
    do {
      prev = t;
      t = t
        .replace(/\s+on[a-z0-9_-]+\s*=\s*("[^"]*"|'[^']*'|[^\s>]*)/gi, '')
        .replace(/\/on[a-z0-9_-]+\s*=\s*("[^"]*"|'[^']*'|[^\s>]*)/gi, '/');
    } while (t !== prev);
    return t.replace(/j\s*a\s*v\s*a\s*s\s*c\s*r\s*i\s*p\s*t\s*:/gi, 'blocked:');
  });
  return s;
}

// ---------------------------------------------------------------- keys

const KEYS = {
  KeyJ: 'lineDown',
  ArrowDown: 'lineDown',
  KeyK: 'lineUp',
  ArrowUp: 'lineUp',
  Space: 'pageDown',
  PageDown: 'pageDown',
  PageUp: 'pageUp',
  KeyG: 'top',
  Home: 'top',
  End: 'bottom',
  Slash: 'search',
  KeyN: 'next',
  KeyO: 'toc',
  BracketLeft: 'back',
  BracketRight: 'forward',
  KeyR: 'reload',
};
const SHIFTED = { Space: 'pageUp', KeyG: 'bottom', KeyN: 'prev' };
const REPEATS = new Set(['lineDown', 'lineUp', 'pageDown', 'pageUp', 'next', 'prev']);

/**
 * Document-pane action for a keydown (by `KeyboardEvent.code`), or null. Ctrl/Alt/Meta chords
 * stay with the app key router (which runs first anyway); Shift only for Space/G/N; IME
 * composition ignored; held keys repeat only for scrolling and n/N.
 */
export function docKeyAction(e) {
  if (!e || e.ctrlKey || e.altKey || e.metaKey || e.isComposing) return null;
  const a = e.shiftKey ? SHIFTED[e.code] || null : KEYS[e.code] || null;
  if (a && e.repeat && !REPEATS.has(a)) return null;
  return a;
}

/** Start offsets of non-overlapping, case-insensitive matches of `query` in `text`. */
export function findMatches(text, query) {
  const q = String(query ?? '').trim().toLowerCase();
  if (!q) return [];
  const t = String(text ?? '').toLowerCase();
  const out = [];
  for (let i = t.indexOf(q); i >= 0; i = t.indexOf(q, i + q.length)) out.push(i);
  return out;
}

/**
 * Inbox detail, issue-file list mode (Task 14 hook in the inbox applet): j/k/↑↓ move,
 * Enter opens (a `.md` file → document tab), Esc / Tab leave the list. Chords and auto-repeated
 * Enter are ignored.
 */
export function fileModeKey(e) {
  if (!e || e.ctrlKey || e.altKey || e.metaKey || e.isComposing) return null;
  if (e.code === 'Tab') return 'leave';
  if (e.shiftKey) return null;
  switch (e.code) {
    case 'KeyJ':
    case 'ArrowDown':
      return 'down';
    case 'KeyK':
    case 'ArrowUp':
      return 'up';
    case 'Enter':
    case 'NumpadEnter':
      return e.repeat ? null : 'open';
    case 'Escape':
      return 'leave';
    default:
      return null;
  }
}
