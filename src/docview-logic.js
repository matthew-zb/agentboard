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

/**
 * Decoded path part of a local href (`?query` dropped, `\` → `/`), or null when unusable:
 * malformed escapes, control characters, protocol-relative / UNC (`//host`, `\\host`, also
 * percent-encoded).
 */
function localPath(raw) {
  let s = String(raw);
  const q = s.indexOf('?');
  if (q >= 0) s = s.slice(0, q);
  try {
    s = decodeURIComponent(s);
  } catch (_) {
    return null;
  }
  s = s.replace(/\\/g, '/');
  if (s.startsWith('//')) return null;
  if (/[\u0000-\u001f]/.test(s)) return null;
  return s;
}

/** Elements whose activation could navigate: links, image-map areas, anything with an href (SVG xlink:href too). */
export const LINK_SELECTOR = 'a, area, [href], [*|href]';

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
 * An `<img src>` in document `currentRel`: `{type:'local', rel}` for a relative path that
 * stays inside the root (then loaded through `doc_image`, which re-validates it), else
 * `{type:'drop'}` — data:, remote, rooted `/x`, UNC / device paths (also percent-encoded),
 * drive letters; the image is replaced by its alt text.
 */
export function resolveAssetSrc(currentRel, src) {
  const s = String(src ?? '').trim();
  if (!s || SCHEME.test(s)) return { type: 'drop' };
  const hashAt = s.indexOf('#');
  const p = localPath(hashAt >= 0 ? s.slice(0, hashAt) : s);
  if (p == null || p.startsWith('/') || p.includes(':')) return { type: 'drop' };
  const rel = joinRel(dirOf(currentRel), p);
  return rel ? { type: 'local', rel } : { type: 'drop' };
}

/** Text shown instead of an image that is not loaded. */
export function imgAltText(alt) {
  const a = String(alt ?? '').trim();
  return a ? `[이미지: ${a}]` : '[이미지]';
}

/**
 * Element id of a heading in pane `paneId`: `doc-<pane>-<slug>`, so no document can produce
 * an id the app uses (`status`, `groups`, `parking`, …) or a named global (`mermaid`).
 */
export function headingId(paneId, slug) {
  return `doc-${String(paneId).replace(/[^A-Za-z0-9_-]/g, '')}-${slug}`;
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
  'area', 'map', 'picture',
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

/** Remove `<t …>…</t>` elements (an unclosed one up to the end) in one linear scan. */
function dropElements(s, t) {
  const lower = s.toLowerCase();
  const open = `<${t}`;
  let out = '';
  let i = 0;
  for (let j = lower.indexOf(open, i); j >= 0; j = lower.indexOf(open, i)) {
    const after = lower.charAt(j + open.length);
    if (after && /[a-z0-9-]/.test(after)) {
      out += s.slice(i, j + 1); // e.g. <styles>: not this element
      i = j + 1;
      continue;
    }
    out += s.slice(i, j);
    const close = lower.indexOf(`</${t}`, j + open.length);
    if (close < 0) return out; // unclosed: everything after it goes
    const end = lower.indexOf('>', close);
    if (end < 0) return out;
    i = end + 1;
  }
  return out + s.slice(i);
}

const isWs = (c) => c === ' ' || c === '\t' || c === '\n' || c === '\r' || c === '\f';

/**
 * Inside one tag (`<…>`): attributes named `on…` are dropped (with the whitespace before them)
 * and `javascript:` is neutralised. A single left-to-right scan (no backtracking regex), so
 * long whitespace runs cost linear time.
 */
function cleanTag(tag) {
  const n = tag.length - 1; // index of the closing '>'
  let i = 1;
  while (i < n && !isWs(tag[i]) && tag[i] !== '/') i++; // tag name
  const parts = [tag.slice(0, i)];
  let pending = ''; // separators seen since the last kept part
  while (i < n) {
    const c = tag[i];
    if (isWs(c)) {
      const j0 = i;
      while (i < n && isWs(tag[i])) i++;
      pending += tag.slice(j0, i);
      continue;
    }
    if (c === '/') {
      parts.push(pending, '/');
      pending = '';
      i++;
      continue;
    }
    const start = i;
    while (i < n && !isWs(tag[i]) && tag[i] !== '/' && tag[i] !== '=') i++;
    const name = tag.slice(start, i);
    let k = i;
    while (k < n && isWs(tag[k])) k++;
    if (k < n && tag[k] === '=') {
      k++;
      while (k < n && isWs(tag[k])) k++;
      if (k < n && (tag[k] === '"' || tag[k] === "'")) {
        const close = tag.indexOf(tag[k], k + 1);
        k = close < 0 || close >= n ? n : close + 1;
      } else {
        while (k < n && !isWs(tag[k])) k++;
      }
      i = k;
    }
    if (i === start) i++; // a stray '=' : keep moving
    if (/^on/i.test(name)) {
      pending = ''; // drop the attribute and the whitespace before it
    } else {
      parts.push(pending, tag.slice(start, i));
      pending = '';
    }
  }
  parts.push(pending, tag.slice(n));
  return parts.join('').replace(/j\s*a\s*v\s*a\s*s\s*c\s*r\s*i\s*p\s*t\s*:/gi, 'blocked:');
}

/**
 * String pre-filter run before DOMPurify (belt and braces, not the sanitizer): drops
 * script/style/iframe/object/embed elements (also unclosed ones), `on*=` attributes and
 * `javascript:` URLs inside tags. Text outside tags is untouched. Linear in the input (no
 * backtracking regex over the whole document).
 */
export function prefilterHtml(html) {
  let s = String(html ?? '');
  for (const t of ['script', 'style', 'iframe', 'object', 'embed', 'noscript', 'template']) {
    s = dropElements(s, t);
    s = s.replace(new RegExp(`</${t}\\b[^<>]*>?`, 'gi'), '');
  }
  let out = '';
  let i = 0;
  for (let lt = s.indexOf('<'); lt >= 0; lt = s.indexOf('<', i)) {
    const gt = s.indexOf('>', lt);
    if (gt < 0) break; // no more complete tags
    out += s.slice(i, lt) + cleanTag(s.slice(lt, gt + 1));
    i = gt + 1;
  }
  return out + s.slice(i);
}

// ---------------------------------------------------------------- mermaid output

/**
 * DOMPurify config for mermaid's rendered SVG (mermaid 'strict' still lets `<img>` through its
 * HTML labels): SVG + filters + HTML (foreignObject labels), its own `<style>` kept, but no
 * images of any kind, no links, scripts or embedded content. `href` / `xlink:href` / `src`
 * survive only as in-document anchors ({@link mermaidAttrAllowed}, a DOMPurify hook) and CSS
 * `url()` only as `url(#…)` ({@link stripCssUrls}).
 */
export const MERMAID_SANITIZE_CONFIG = Object.freeze({
  USE_PROFILES: Object.freeze({ svg: true, svgFilters: true, html: true }),
  FORBID_TAGS: Object.freeze([
    'img', 'image', 'feimage', 'use', 'a', 'area', 'map', 'picture', 'script', 'iframe', 'frame',
    'object', 'embed', 'form', 'input', 'button', 'textarea', 'select', 'link', 'meta', 'base',
    'audio', 'video', 'source', 'track', 'portal', 'template', 'noscript',
  ]),
  FORBID_ATTR: Object.freeze(['srcset', 'ping', 'formaction', 'action', 'background', 'poster', 'target']),
  ALLOW_DATA_ATTR: false,
  ALLOW_UNKNOWN_PROTOCOLS: false,
  SANITIZE_DOM: true,
  SANITIZE_NAMED_PROPS: false,
});

/**
 * Id (or name) `value` inside the SVG of mermaid render `rid` (`mermaid-svg-N`): kept when it is
 * in that render's namespace (`rid` itself or `rid` followed by a non-digit), otherwise prefixed
 * `rid-` — so a diagram can never produce an app id (`status`, `parking`, …) or a named global.
 */
export function mermaidId(rid, value) {
  const v = String(value ?? '');
  const own = v.startsWith(rid) && (v.length === rid.length || !/[0-9]/.test(v[rid.length]));
  return own ? v : `${rid}-${v}`;
}

/**
 * References to renamed ids (`map`: old → new): `url(#old)` anywhere in `text`, or `text` being
 * exactly `#old` (an href). Other text is unchanged.
 */
export function rewriteMermaidRefs(text, map) {
  const t = String(text ?? '');
  if (t.startsWith('#') && map.has(t.slice(1))) return `#${map.get(t.slice(1))}`;
  return t.replace(/url\(\s*(['"]?)#([^'")\s]+)\1\s*\)/g, (m, _q, id) => (map.has(id) ? `url(#${map.get(id)})` : m));
}

/** Mermaid output attribute filter: no `on*`; href / xlink:href / src only as `#anchor`. */
export function mermaidAttrAllowed(name, value) {
  const n = String(name || '').toLowerCase();
  if (n.startsWith('on')) return false;
  if (n === 'href' || n === 'xlink:href' || n === 'src') {
    const v = String(value ?? '').trim();
    return v.length > 1 && v.startsWith('#');
  }
  return true;
}

/** CSS text with `@import`, `image-set()` and every `url()` not pointing at `#id` removed. */
export function stripCssUrls(css) {
  return String(css ?? '')
    .replace(/@import[^;]*;?/gi, '')
    .replace(/(?:-webkit-)?image-set\([^)]*\)/gi, 'none')
    .replace(/url\(\s*(['"]?)(.*?)\1\s*\)/gi, (m, _q, inner) => (String(inner).trim().startsWith('#') ? m : 'none'));
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
