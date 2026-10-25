// Document tab view (Task 14, spec §5-2): a read-only markdown viewer mounted by pane.js into an
// applet pane `{type:'applet', kind:'doc'}` (the pane's `docPath` is the document as opened).
//
// Rendering (mdviewer's engine, vendored, no CDN): marked (GFM) with highlight.js inside code
// blocks → string pre-filter → DOMPurify (first pass; every <img src> is taken off and kept
// aside) → KaTeX auto-render (trust: false) → DOMPurify again (KaTeX output) → heading ids
// (`doc-<pane>-<slug>`) + table of contents → images: a relative path inside the root is read by
// `doc_image` (validated in Rust, returned as a data: URL), anything else becomes its alt text →
// mermaid (securityLevel 'strict', loaded only when a document has a ```mermaid block; its SVG is
// sanitized again before it is inserted). The markdown is untrusted (agents write it): nothing
// from it is ever executed, links never navigate the WebView (relative .md → this tab, http(s) →
// system browser through the backend, anything else ignored; the backend also refuses any
// navigation away from the app and every new window).
//
// Keys (document pane focused; app shortcuts are handled first by the key router): j/k line,
// Space/Shift+Space page, g/G top/bottom, / search (n/N), o table of contents, [ ] back/forward,
// r reload. The file is polled by the backend (1 s); a change reloads keeping the scroll position.
// No top-level DOM access (Node-importable); the engine is imported on first use.
import * as api from './api.js';
import {
  resolveDocLink,
  resolveAssetSrc,
  createHistory,
  extractToc,
  slugify,
  SANITIZE_CONFIG,
  FINAL_SANITIZE_CONFIG,
  prefilterHtml,
  docKeyAction,
  findMatches,
  docTitle,
  headingId,
  imgAltText,
  LINK_SELECTOR,
  MERMAID_SANITIZE_CONFIG,
  mermaidAttrAllowed,
  stripCssUrls,
  mermaidId,
  rewriteMermaidRefs,
} from './docview-logic.js';
import { isFocusTrapKey, focusLost } from './inbox-logic.js';

const LINE_PX = 40;
const HLJS_LANGS = [
  'bash', 'c', 'cpp', 'csharp', 'css', 'diff', 'dockerfile', 'dos', 'go', 'ini', 'java', 'javascript',
  'json', 'kotlin', 'lua', 'makefile', 'markdown', 'php', 'plaintext', 'powershell', 'python', 'ruby',
  'rust', 'scss', 'shell', 'sql', 'swift', 'typescript', 'xml', 'yaml',
];
const AUTO_HIGHLIGHT_MAX = 20000; // unlabeled code blocks larger than this are not auto-detected
const AUTO_HIGHLIGHT_BUDGET = 200000; // auto-detected bytes per document render
let autoBudget = AUTO_HIGHLIGHT_BUDGET;
const KATEX_OPTIONS = {
  delimiters: [
    { left: '$$', right: '$$', display: true },
    { left: '\\[', right: '\\]', display: true },
    { left: '$', right: '$', display: false },
    { left: '\\(', right: '\\)', display: false },
  ],
  ignoredTags: ['script', 'noscript', 'style', 'textarea', 'pre', 'code', 'option'],
  throwOnError: false,
  trust: false,
  strict: 'ignore',
  maxExpand: 500,
  maxSize: 20,
};

// ---------------------------------------------------------------- engine (lazy)

let enginePromise = null;

const escHtml = (s) =>
  String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);

/** marked + DOMPurify + highlight.js + KaTeX, imported once. */
function engine() {
  if (!enginePromise) {
    enginePromise = (async () => {
      const [{ Marked }, purifyMod, hljsMod, autoRenderMod, ...langs] = await Promise.all([
        import('./vendor/marked.esm.js'),
        import('./vendor/purify.es.mjs'),
        import('./vendor/hljs/core.mjs'),
        import('./vendor/katex/contrib/auto-render.mjs'),
        ...HLJS_LANGS.map((l) => import(`./vendor/hljs/languages/${l}.js`)),
      ]);
      const hljs = hljsMod.default;
      HLJS_LANGS.forEach((l, i) => hljs.registerLanguage(l, langs[i].default));
      hljs.configure({ ignoreUnescapedHTML: true });
      const marked = new Marked({
        gfm: true,
        breaks: true,
        renderer: {
          code(code, info) {
            const lang = String(info || '').trim().split(/\s+/)[0].toLowerCase();
            if (lang === 'mermaid') return `<pre class="mermaid-src">${escHtml(code)}</pre>\n`;
            let body;
            let cls = 'hljs';
            if (lang && hljs.getLanguage(lang)) {
              body = hljs.highlight(code, { language: lang, ignoreIllegals: true }).value;
              cls += ` language-${lang.replace(/[^a-z0-9+#_-]/g, '')}`;
            } else if (!lang && code.length <= AUTO_HIGHLIGHT_MAX && code.length <= autoBudget) {
              autoBudget -= code.length;
              body = hljs.highlightAuto(code).value;
            } else {
              body = escHtml(code);
            }
            return `<pre><code class="${cls}">${body}</code></pre>\n`;
          },
        },
      });
      const purify = purifyMod.default(window); // own instance: the hook below stays private
      return { marked, purify, renderMath: autoRenderMod.default };
    })();
    enginePromise.catch(() => {
      enginePromise = null; // retry on the next render
    });
  }
  return enginePromise;
}

let mermaidPromise = null;

/** The mermaid IIFE build (≈3.5 MB), injected once on first use; securityLevel 'strict'. */
function loadMermaid() {
  if (!mermaidPromise) {
    mermaidPromise = new Promise((resolve, reject) => {
      if (globalThis.mermaid) return resolve(globalThis.mermaid);
      const s = document.createElement('script');
      s.src = 'vendor/mermaid.min.js';
      s.async = true;
      s.onload = () => (globalThis.mermaid ? resolve(globalThis.mermaid) : reject(new Error('mermaid를 불러오지 못했습니다')));
      s.onerror = () => reject(new Error('mermaid를 불러오지 못했습니다'));
      document.head.appendChild(s);
    }).then((m) => {
      m.initialize({
        startOnLoad: false,
        securityLevel: 'strict', // + our own DOMPurify pass over the SVG (renderMermaidIn)
        // labels as SVG text: no foreignObject HTML (DOMPurify drops foreignObject anyway)
        htmlLabels: false,
        flowchart: { htmlLabels: false },
        theme: 'dark',
        darkMode: true,
        fontFamily: '"Segoe UI", "Malgun Gothic", sans-serif',
      });
      return m;
    });
    mermaidPromise.catch(() => {
      mermaidPromise = null;
    });
  }
  return mermaidPromise;
}

/** `<span>` with the image's alt text, standing in for an image that is not shown. */
function altSpan(img) {
  const s = document.createElement('span');
  s.className = 'doc-img-alt';
  s.textContent = imgAltText(img.getAttribute('alt'));
  s.title = '작업 폴더 안의 상대 경로 이미지만 표시합니다';
  return s;
}

/**
 * Render markdown `text` of document `rel` (shown in pane `paneId`) into a DocumentFragment, its
 * table of contents (`{id → heading element}` in `anchors`) and the images to load: every
 * `<img>` loses its `src` in the first pass (no request is ever made for a document-written
 * URL); a relative path inside the root is kept for `doc_image`, anything else becomes text.
 */
async function renderMarkdown(text, rel, paneId) {
  const { marked, purify, renderMath } = await engine();
  autoBudget = AUTO_HIGHLIGHT_BUDGET;
  const html = prefilterHtml(marked.parse(String(text)));
  const srcs = [];
  purify.removeAllHooks();
  purify.addHook('afterSanitizeAttributes', (node) => {
    if (node.tagName === 'IMG') {
      srcs.push(node.getAttribute('src') || '');
      node.removeAttribute('src');
      node.setAttribute('class', `doc-img doc-img-${srcs.length - 1}`);
    } else if ((node.tagName === 'A' || node.tagName === 'AREA') && node.hasAttribute('href')) {
      const r = resolveDocLink(rel, node.getAttribute('href'));
      node.classList.add(`doc-link-${r.type}`);
      node.setAttribute('rel', 'noopener noreferrer');
      if (r.type === 'external') node.setAttribute('title', `${r.url} (브라우저로 열기)`);
    }
  });
  let box;
  try {
    box = document.createElement('div');
    box.appendChild(purify.sanitize(html, { ...SANITIZE_CONFIG, RETURN_DOM_FRAGMENT: true }));
  } finally {
    purify.removeAllHooks();
  }
  try {
    renderMath(box, KATEX_OPTIONS);
  } catch (err) {
    console.warn('[agentboard] KaTeX', err);
  }
  const frag = purify.sanitize(box.innerHTML, { ...FINAL_SANITIZE_CONFIG, RETURN_DOM_FRAGMENT: true });
  const hs = [...frag.querySelectorAll('h1, h2, h3, h4, h5, h6')];
  const toc = extractToc(hs.map((h) => ({ level: Number(h.tagName[1]), text: h.textContent.trim() })));
  const anchors = new Map();
  hs.forEach((h, i) => {
    h.id = headingId(paneId, toc[i].id); // namespaced: never an app id or a named global
    anchors.set(toc[i].id, h);
  });
  const images = [];
  for (const img of [...frag.querySelectorAll('img')]) {
    const m = /\bdoc-img-(\d+)\b/.exec(img.getAttribute('class') || '');
    const src = m ? srcs[Number(m[1])] : null;
    img.removeAttribute('src');
    if (src != null && resolveAssetSrc(rel, src).type === 'local') {
      img.setAttribute('class', 'doc-img loading');
      images.push({ img, src });
    } else {
      img.replaceWith(altSpan(img));
    }
  }
  return { frag, toc, anchors, images };
}

/** Load the document's images through `doc_image` (data: URLs); failures become alt text. */
function loadImages(rel, images, alive) {
  for (const { img, src } of images) {
    api.docImage(rel, src).then(
      (url) => {
        if (!alive() || typeof url !== 'string' || !url.startsWith('data:image/')) return;
        img.src = url;
        img.classList.remove('loading');
      },
      () => {
        if (alive() && img.isConnected) img.replaceWith(altSpan(img));
      },
    );
  }
}

let mermaidSeq = 0;

/**
 * Mermaid SVG (string) of render `rid` → sanitized fragment: no images, links, scripts, foreign
 * URLs; every `id` / `name` outside the `rid` namespace is prefixed ({@link mermaidId}; some
 * renderers write user-chosen ids such as `<g id="status">`) and `url(#…)` / `#…` references to
 * renamed ids follow, so markers and gradients still work.
 */
export async function cleanMermaidSvg(svg, rid) {
  const { purify } = await engine();
  const renamed = new Map();
  purify.removeAllHooks();
  purify.addHook('uponSanitizeAttribute', (_node, data) => {
    if (!mermaidAttrAllowed(data.attrName, data.attrValue)) {
      data.keepAttr = false;
      return;
    }
    if (data.attrName === 'id' || data.attrName === 'name') {
      const v = mermaidId(rid, data.attrValue);
      if (v !== data.attrValue) renamed.set(data.attrValue, v);
      data.attrValue = v;
    } else if (data.attrName === 'style') {
      data.attrValue = stripCssUrls(data.attrValue);
    }
  });
  purify.addHook('uponSanitizeElement', (node) => {
    if (node.nodeName && node.nodeName.toLowerCase() === 'style') node.textContent = stripCssUrls(node.textContent);
  });
  let frag;
  try {
    frag = purify.sanitize(svg, { ...MERMAID_SANITIZE_CONFIG, RETURN_DOM_FRAGMENT: true });
  } finally {
    purify.removeAllHooks();
  }
  if (renamed.size) {
    // the fragment is not in the document yet: rewrite references before it can be seen
    for (const el of frag.querySelectorAll('*')) {
      for (const a of [...el.attributes]) {
        if (a.name === 'id' || a.name === 'name') continue;
        const v = rewriteMermaidRefs(a.value, renamed);
        if (v !== a.value) el.setAttribute(a.name, v);
      }
      if (el.nodeName.toLowerCase() === 'style') el.textContent = rewriteMermaidRefs(el.textContent, renamed);
    }
  }
  return frag;
}

/**
 * Replace `pre.mermaid-src` blocks under `el` with rendered diagrams: `mermaid.render` gives the
 * SVG as a string, which is sanitized ({@link cleanMermaidSvg}) before it reaches the document.
 * Errors are shown as text; `alive()` false stops early.
 */
async function renderMermaidIn(el, alive) {
  const blocks = [...el.querySelectorAll('pre.mermaid-src')];
  if (!blocks.length) return;
  let m;
  try {
    m = await loadMermaid();
  } catch (err) {
    for (const pre of blocks) pre.before(errorLine(err));
    return;
  }
  for (const pre of blocks) {
    if (!alive()) return;
    try {
      const rid = `mermaid-svg-${++mermaidSeq}`;
      const { svg } = await m.render(rid, pre.textContent);
      if (!alive()) return;
      const div = document.createElement('div');
      div.className = 'mermaid';
      div.appendChild(await cleanMermaidSvg(svg, rid));
      pre.replaceWith(div);
    } catch (err) {
      pre.before(errorLine(err));
    }
  }
}

function errorLine(err) {
  const msg = document.createElement('div');
  msg.className = 'doc-error';
  msg.textContent = `Mermaid 오류: ${(err && err.message) || err}`;
  return msg;
}

// ---------------------------------------------------------------- search highlights (shared)

const searchRanges = new Map(); // paneId → {all: Range[], cur: Range|null}

function paintHighlights() {
  const reg = globalThis.CSS && CSS.highlights;
  if (!reg || typeof Highlight !== 'function') return;
  const all = [];
  const cur = [];
  for (const r of searchRanges.values()) {
    all.push(...r.all);
    if (r.cur) cur.push(r.cur);
  }
  reg.set('doc-find', new Highlight(...all));
  reg.set('doc-find-cur', new Highlight(...cur));
}

/** Ranges of every match of `query` in the text nodes under `el` (per text node). */
function rangesFor(el, query) {
  const out = [];
  const q = String(query || '').trim();
  if (!q) return out;
  const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    if (n.parentElement && n.parentElement.closest('.mermaid, .katex-mathml')) continue;
    for (const at of findMatches(n.data, q)) {
      const r = document.createRange();
      r.setStart(n, at);
      r.setEnd(n, at + q.length);
      out.push(r);
    }
  }
  return out;
}

// ---------------------------------------------------------------- mounted view

const deps = {
  toast: (m) => console.error('[agentboard]', m),
  prompt: async () => null,
  choose: async () => null,
};

/** `{toast(msg, kind?), prompt(title, initial), choose(title, options)}` (app.js). */
export function configureDocView(d) {
  Object.assign(deps, d);
}

const views = new Map(); // paneId → view
let unlisten = null;

function listen() {
  if (unlisten) return;
  unlisten = api.on('doc-changed', (p) => {
    const v = p && views.get(p.paneId);
    if (v) v.changed();
  });
}

function unlistenIfIdle() {
  if (views.size || !unlisten) return;
  const u = unlisten;
  unlisten = null;
  Promise.resolve(u).then((f) => typeof f === 'function' && f(), () => {});
}

const editable = (t) => !!t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable);

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

/**
 * Mount a document view for pane `paneId` showing `model.docPath`; returns `{el, focus(),
 * dispose()}` (pane.js mounts it once per pane id and disposes it with the pane).
 */
export function mountDoc(paneId, model) {
  const root = el('div', 'docview');
  root.tabIndex = -1;
  const bar = el('div', 'doc-bar');
  const pathEl = el('span', 'doc-path', (model && model.name) || '');
  const stateEl = el('span', 'doc-state');
  const findEl = el('span', 'doc-find hidden');
  const hint = el('span', 'doc-hint', '/ 검색 · o 목차 · [ ] 뒤/앞 · r 새로고침');
  bar.append(pathEl, stateEl, findEl, hint);
  const body = el('div', 'doc-body');
  const content = el('div', 'doc-content markdown-body');
  body.appendChild(content);
  root.append(bar, body);

  const st = {
    disposed: false,
    gen: 0, // render generation: a slower older load never overwrites a newer one
    hist: null, // createHistory once the first document loaded
    rel: null,
    root: '',
    toc: [],
    anchors: new Map(), // toc id (slug) → heading element
    query: '',
    matchIdx: -1,
    busy: false, // a prompt / choose flow is open
    reloadQueued: false,
    loading: 0, // loads in flight
  };

  const showMessage = (title, detail) => {
    content.replaceChildren();
    const box = el('div', 'doc-missing');
    box.append(el('div', 'doc-missing-title', title));
    if (detail) box.append(el('div', 'doc-missing-detail', String(detail)));
    box.append(el('div', 'doc-missing-detail', 'r: 다시 읽기'));
    content.appendChild(box);
  };

  /**
   * Load `path` (absolute or root-relative) and show it. `mode`: 'init' | 'reload' (keep scroll)
   * | 'push' (link: history push) | 'history' (back/forward: `scroll` restored). Resolves true
   * when shown.
   */
  async function loadOnce(path, { mode, scroll = 0, hash = '' }) {
    const gen = ++st.gen;
    const keepTop = body.scrollTop;
    let doc;
    try {
      doc = await api.docRead(path);
    } catch (err) {
      if (gen !== st.gen || st.disposed) return false;
      if (mode === 'push' || mode === 'history') {
        deps.toast(err);
        return false;
      }
      stateEl.textContent = '';
      pathEl.textContent = st.rel || (model && model.docPath) || '';
      showMessage('파일을 찾을 수 없습니다', err);
      // keep watching (a missing file inside the root is watched too): it reloads once created
      if (mode === 'init') api.docWatch(paneId, path).catch(() => {});
      return false;
    }
    if (gen !== st.gen || st.disposed) return false;
    let rendered;
    try {
      rendered = await renderMarkdown(doc.text, doc.rel, paneId);
    } catch (err) {
      if (gen !== st.gen || st.disposed) return false;
      showMessage('문서를 표시할 수 없습니다', err);
      return false;
    }
    if (gen !== st.gen || st.disposed) return false;
    if (!st.hist) st.hist = createHistory({ rel: doc.rel, scroll: 0 });
    else if (mode === 'push') st.hist.push({ rel: doc.rel, scroll: 0 }, keepTop);
    const moved = st.rel !== doc.rel;
    st.rel = doc.rel;
    st.root = doc.root;
    st.toc = rendered.toc;
    st.anchors = rendered.anchors;
    pathEl.textContent = doc.rel;
    pathEl.title = doc.path;
    stateEl.textContent = '';
    if (moved || mode === 'init') api.docWatch(paneId, doc.rel).catch((e) => deps.toast(e));

    const hadFocus = root.contains(document.activeElement);
    content.replaceChildren(rendered.frag);
    const alive = () => gen === st.gen && !st.disposed;
    loadImages(doc.rel, rendered.images, alive);
    if (focusLost(hadFocus, document.activeElement, root, document.body)) root.focus({ preventScroll: true });
    const top = mode === 'reload' ? keepTop : mode === 'history' ? scroll : 0;
    body.scrollTop = top;
    if (hash) scrollToId(hash);
    const settled = body.scrollTop;
    refreshSearch(false);
    await renderMermaidIn(content, alive);
    // diagrams change heights: put the reader back where they were unless they scrolled since
    if (gen === st.gen && !st.disposed && body.scrollTop === settled && !hash) body.scrollTop = top;
    if (gen === st.gen) refreshSearch(false);
    return true;
  }

  /** `loadOnce` + a reload requested meanwhile (file change, `r`) runs once afterwards. */
  async function load(path, opts) {
    st.loading++;
    try {
      return await loadOnce(path, opts);
    } finally {
      st.loading--;
      if (!st.loading && st.reloadQueued && !st.disposed) {
        st.reloadQueued = false;
        reload();
      }
    }
  }

  function scrollToId(id) {
    const want = String(id);
    const target =
      st.anchors.get(want) ||
      st.anchors.get(slugify(want)) ||
      content.querySelector(`[id="user-content-${CSS.escape(want)}"]`) ||
      content.querySelector(`[name="user-content-${CSS.escape(want)}"]`);
    if (target) body.scrollTop += target.getBoundingClientRect().top - body.getBoundingClientRect().top - 8;
  }

  // ---- search

  function refreshSearch(jump) {
    if (!st.query) {
      searchRanges.delete(paneId);
      findEl.classList.add('hidden');
      paintHighlights();
      return;
    }
    const all = rangesFor(content, st.query);
    if (st.matchIdx >= all.length) st.matchIdx = all.length ? 0 : -1;
    if (jump && all.length) {
      // first match below the top of the view
      const viewTop = body.getBoundingClientRect().top;
      const i = all.findIndex((r) => r.getBoundingClientRect().top >= viewTop + 1);
      st.matchIdx = i >= 0 ? i : 0;
    }
    const cur = st.matchIdx >= 0 ? all[st.matchIdx] : null;
    searchRanges.set(paneId, { all, cur });
    paintHighlights();
    findEl.classList.remove('hidden');
    findEl.textContent = all.length ? `검색 "${st.query}" ${st.matchIdx + 1}/${all.length} · n/N` : `검색 "${st.query}" 없음`;
    if (jump && cur) revealRange(cur);
  }

  function revealRange(r) {
    const rect = r.getBoundingClientRect();
    const b = body.getBoundingClientRect();
    if (rect.top < b.top + 8 || rect.bottom > b.bottom - 8) body.scrollTop += rect.top - b.top - body.clientHeight / 3;
  }

  function step(delta) {
    const r = searchRanges.get(paneId);
    if (!st.query || !r || !r.all.length) return;
    st.matchIdx = (st.matchIdx + delta + r.all.length) % r.all.length;
    r.cur = r.all[st.matchIdx];
    paintHighlights();
    findEl.textContent = `검색 "${st.query}" ${st.matchIdx + 1}/${r.all.length} · n/N`;
    revealRange(r.cur);
  }

  const flow = async (fn) => {
    if (st.busy) return;
    st.busy = true;
    try {
      await fn();
    } catch (err) {
      deps.toast(err);
    } finally {
      st.busy = false;
    }
  };

  // ---- actions

  const page = () => Math.max(LINE_PX, body.clientHeight * 0.9);
  const actions = {
    lineDown: () => body.scrollBy(0, LINE_PX),
    lineUp: () => body.scrollBy(0, -LINE_PX),
    pageDown: () => body.scrollBy(0, page()),
    pageUp: () => body.scrollBy(0, -page()),
    top: () => {
      body.scrollTop = 0;
    },
    bottom: () => {
      body.scrollTop = body.scrollHeight;
    },
    search: () =>
      flow(async () => {
        const q = await deps.prompt('문서 검색 (n/N: 다음/이전, 빈 값: 검색 해제)', st.query);
        if (q == null) return;
        st.query = q.trim();
        st.matchIdx = -1;
        refreshSearch(true);
      }),
    next: () => step(1),
    prev: () => step(-1),
    toc: () =>
      flow(async () => {
        if (!st.toc.length) {
          deps.toast('제목이 없는 문서입니다', 'info');
          return;
        }
        const id = await deps.choose(`목차 — ${docTitle(st.rel)}`, st.toc.map((t) => ({ label: t.label, value: t.id })));
        if (id != null) scrollToId(id);
      }),
    back: () => {
      if (!st.hist) return;
      const e = st.hist.back(body.scrollTop);
      if (e) load(e.rel, { mode: 'history', scroll: e.scroll }).then((ok) => ok || st.hist.forward(0));
    },
    forward: () => {
      if (!st.hist) return;
      const e = st.hist.forward(body.scrollTop);
      if (e) load(e.rel, { mode: 'history', scroll: e.scroll }).then((ok) => ok || st.hist.back(0));
    },
    reload: () => reload(),
  };

  function reload() {
    if (st.loading) {
      st.reloadQueued = true;
      return;
    }
    const path = st.rel || (model && model.docPath);
    if (path) load(path, { mode: st.rel ? 'reload' : 'init' });
  }

  const onKey = (e) => {
    if (e.defaultPrevented || editable(e.target)) return; // the app key router / an input owns it
    if (isFocusTrapKey(e)) {
      e.preventDefault(); // focus never wanders into a neighbouring terminal
      return;
    }
    const a = docKeyAction(e);
    if (!a) return;
    e.preventDefault();
    e.stopPropagation();
    actions[a]();
  };

  const onClick = (e) => {
    const a = e.target && e.target.closest ? e.target.closest(LINK_SELECTOR) : null;
    if (!a || !root.contains(a)) return;
    e.preventDefault(); // the WebView itself never navigates (the backend refuses it as well)
    if (e.type !== 'click' || e.button !== 0) return;
    if (!content.contains(a)) return;
    const href = a.getAttribute('href') ?? a.getAttributeNS('http://www.w3.org/1999/xlink', 'href');
    const r = resolveDocLink(st.rel || '', href);
    if (r.type === 'anchor') scrollToId(r.id);
    else if (r.type === 'doc') load(r.rel, { mode: 'push', hash: r.hash });
    else if (r.type === 'external') api.docOpenUrl(r.url).catch((err) => deps.toast(err));
    else deps.toast('이 링크는 열지 않습니다 (작업 폴더 안의 .md 와 http/https 링크만 엽니다)', 'info');
  };
  const onDrag = (e) => e.preventDefault(); // no dragging links/images out of (or into) the view

  root.addEventListener('keydown', onKey);
  root.addEventListener('click', onClick);
  root.addEventListener('auxclick', onClick);
  root.addEventListener('dragstart', onDrag);

  const view = {
    changed() {
      if (!st.disposed) reload(); // also a document that was missing and has now appeared
    },
  };
  views.set(paneId, view);
  listen();

  if (model && model.docPath) load(model.docPath, { mode: 'init' });
  else showMessage('파일을 찾을 수 없습니다', '문서 경로가 없습니다');

  return {
    el: root,
    focus() {
      if (!root.contains(document.activeElement)) root.focus({ preventScroll: true });
    },
    dispose() {
      if (st.disposed) return;
      st.disposed = true;
      st.gen++;
      root.removeEventListener('keydown', onKey);
      root.removeEventListener('click', onClick);
      root.removeEventListener('auxclick', onClick);
      root.removeEventListener('dragstart', onDrag);
      if (views.get(paneId) === view) views.delete(paneId);
      searchRanges.delete(paneId);
      paintHighlights();
      api.docUnwatch(paneId).catch(() => {});
      unlistenIfIdle();
      root.remove();
    },
  };
}
