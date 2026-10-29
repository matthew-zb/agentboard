// Memo applet (spec docs/superpowers/specs/2026-10-26-agentboard-memo-design.md): the body of
// workspaceRoot/MEMO.md — the user's own list of today's work — below the inbox, next to the
// dispatcher. Mounted by pane.js into an applet pane `{type:'applet', kind:'memo'}`.
//
// Read view: the body rendered like a document tab (`deps.render` = docview.js renderMarkdown,
// same sanitizing); the header (writing rules) is never shown. Edit view: a textarea with the
// body only; Ctrl+Enter saves header + edited body (joined by memo-logic.js, in the file's line
// break style), Esc cancels (Y/N when the draft changed; Enter does not answer). MEMO.md is
// polled by the backend (doc_watch, 1 s): the read view re-renders, the edit view only shows a
// notice and keeps the draft — saving then overwrites (last writer wins; the backend keeps one
// backup in .agentboard/memo.bak).
// Keys come from the applet element (bubble phase): app shortcuts (Alt+…) are taken by the key
// router first and keep working while editing (the draft stays in the textarea).
// vi editing (spec §8, V3 b — only when config.json "memo": {"vi": true}, read with memo_settings at
// mount and again on `config-reloaded`):
// e opens the editor in normal mode — the textarea read-only, the character under the cursor
// selected as a block cursor, keys run vi-logic.js; i a A o O hand the textarea back for typing
// (IME included) until Esc / Ctrl+[. ":" and "/" open a command line under the editor. Esc in
// normal mode never leaves; :w saves and stays, :wq / ZZ / Ctrl+Enter save and leave, :q!
// leaves (Y/N when changed, Enter ignored). The mode is shown (-- NORMAL -- / -- INSERT --).
// No top-level DOM access (Node-importable).
import * as api from './api.js';
import { splitMemo, joinMemo, memoEol, normalizeEol, memoKeyAction } from './memo-logic.js';
import { createVi, viKey, viKeyFromEvent, viLeaveInsert, viCommand, viSearch } from './vi-logic.js';
import { isFocusTrapKey } from './inbox-logic.js';
import { LINK_SELECTOR, resolveDocLink } from './docview-logic.js';

const LINE_PX = 40;
export const MEMO_NOTICE = '외부에서 바뀜 — 저장하면 덮어씀(Esc: 취소하고 다시 읽기)';
export const MEMO_NO_MARKER = '구분 헤딩(## 메모) 없음 — 파일 전체를 보여 주고 편집합니다';

const deps = {
  render: async () => null, // (body, rel, paneId) → {frag, images}: docview.js renderMarkdown
  loadImages: () => {}, // (rel, images, alive): docview.js loadImages
  confirm: async () => false,
  toast: (m) => console.error('[agentboard]', m),
  vi: null, // tests: a fixed `() => bool`; null = follow memo_settings (spec §8 V3 b)
};

/** `{render, loadImages, confirm(msg, opts), toast(msg, kind?)}` (app.js). */
export function configureMemo(d) {
  Object.assign(deps, d);
}

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

function button(action, label, title) {
  const b = el('button', 'memo-btn', label);
  b.type = 'button';
  b.tabIndex = -1;
  b.dataset.action = action;
  b.title = title;
  return b;
}

/**
 * Mount the memo view for pane `paneId`; returns `{el, focus(), dispose()}` (pane.js mounts it
 * once per pane id and disposes it with the pane).
 */
export function mountMemo(paneId) {
  const root = el('div', 'memo');
  root.tabIndex = -1;
  const bar = el('div', 'memo-bar');
  const hint = el('span', 'memo-hint');
  const btns = el('span', 'memo-btns');
  bar.append(el('span', 'memo-title', '메모'), hint, btns);
  const warn = el('div', 'memo-warn hidden', MEMO_NO_MARKER);
  const notice = el('div', 'memo-notice hidden', MEMO_NOTICE);
  const view = el('div', 'memo-body');
  const content = el('div', 'memo-content markdown-body');
  const editor = el('textarea', 'memo-edit');
  editor.spellcheck = false;
  view.appendChild(content);
  // vi: mode / message line and the ":" "/" command line (shown only while vi editing)
  const viBar = el('div', 'memo-vi hidden');
  const viMode = el('span', 'memo-vi-mode');
  const viPrefix = el('span', 'memo-vi-prefix');
  const cmdline = el('input', 'memo-cmdline hidden');
  cmdline.type = 'text';
  cmdline.spellcheck = false;
  viBar.append(viMode, viPrefix, cmdline);
  root.append(bar, warn, notice, view, viBar);

  const st = {
    disposed: false,
    mode: 'read', // 'read' | 'edit'
    shown: 'read', // what `view` holds
    path: 'MEMO.md',
    text: null, // MEMO.md as last read or saved
    latest: null, // splitMemo(text)
    memo: null, // the split the read view shows
    base: null, // the split the current edit started from (its header is saved back)
    external: false, // MEMO.md changed while editing
    reading: false,
    readAgain: false, // a change / reload request during a read or save: read once more after
    force: false,
    saving: false,
    asking: false,
    watching: false,
    gen: 0, // render generation: an older render never overwrites a newer one
    vi: null, // vi-logic state while vi editing (null: plain editor)
    viCmd: null, // ':' | '/' while the command line is open
  };

  function show() {
    const editing = st.mode === 'edit';
    root.classList.toggle('editing', editing);
    warn.classList.toggle('hidden', !st.memo || st.memo.marker);
    notice.classList.toggle('hidden', !(editing && st.external));
    viBar.classList.toggle('hidden', !(editing && st.vi));
    hint.textContent = !editing ? 'e 편집 · r 다시 읽기' : st.vi ? ':w 저장 · :wq 저장하고 닫기 · :q! 버리기' : 'Ctrl+Enter 저장 · Esc 취소';
    btns.replaceChildren(
      ...(editing
        ? [button('save', '저장', 'Ctrl+Enter'), button('cancel', '취소', 'Esc')]
        : [button('edit', '편집', 'e')]),
    );
    if (st.shown !== st.mode) {
      view.replaceChildren(editing ? editor : content);
      st.shown = st.mode;
    }
  }

  async function renderRead() {
    const gen = ++st.gen;
    const body = st.memo ? st.memo.body : '';
    if (!body.trim()) {
      content.replaceChildren(el('div', 'memo-empty', '메모가 비어 있습니다 — e: 편집'));
      return;
    }
    let r;
    try {
      r = await deps.render(body, st.path, paneId);
    } catch (err) {
      if (gen === st.gen && !st.disposed) content.replaceChildren(el('div', 'memo-error', `메모를 표시할 수 없습니다: ${err}`));
      return;
    }
    if (gen !== st.gen || st.disposed || !r) return;
    content.replaceChildren(r.frag);
    deps.loadImages(st.path, r.images || [], () => gen === st.gen && !st.disposed);
  }

  /** memo_read (one at a time). A change re-renders the read view; while editing it only raises
   * the notice. `force` (r): re-render even when the text did not change. */
  async function read({ force = false } = {}) {
    if (st.disposed) return;
    if (force) st.force = true;
    if (st.reading || st.saving) {
      st.readAgain = true;
      return;
    }
    st.reading = true;
    let r;
    try {
      r = await api.memoRead();
    } catch (err) {
      st.reading = false;
      if (st.disposed) return;
      if (st.text == null) {
        content.replaceChildren(el('div', 'memo-error', `메모를 읽을 수 없습니다: ${err}`), el('div', 'memo-empty', 'r: 다시 읽기'));
      } else {
        deps.toast(err);
      }
      readMore();
      return;
    }
    st.reading = false;
    if (st.disposed) return;
    const first = st.text == null;
    const text = String((r && r.text) ?? '');
    if (r && r.path) st.path = r.path;
    if (!st.watching) {
      st.watching = true;
      api.docWatch(paneId, st.path).catch((e) => deps.toast(e));
    }
    const changed = text !== st.text;
    st.text = text;
    st.latest = splitMemo(text);
    const forced = st.force;
    st.force = false;
    if (st.mode === 'edit') {
      if (changed && !first) {
        st.external = true;
        show();
      }
    } else if (changed || first || forced) {
      st.memo = st.latest;
      show();
      renderRead();
    }
    readMore();
  }

  function readMore() {
    if (st.readAgain && !st.disposed && !st.reading && !st.saving) {
      st.readAgain = false;
      read();
    }
  }

  function edit() {
    if (!st.memo || st.mode === 'edit') return;
    st.mode = 'edit';
    st.base = st.memo;
    st.external = false;
    editor.value = st.base.body;
    st.vi = viOn() ? createVi(normalizeEol(st.base.body, '\n')) : null;
    show();
    editor.focus({ preventScroll: true });
    if (st.vi) {
      paintVi(); // normal mode at the top, read-only
      return;
    }
    const n = editor.value.length;
    editor.setSelectionRange(n, n); // cursor at the end
    editor.scrollTop = editor.scrollHeight;
  }

  // ---- vi (spec §8)

  /** Mirror the vi state into the textarea: text, read-only outside insert mode, the cursor (a
   * one-character selection in normal mode), and the mode / message line. */
  function paintVi() {
    const v = st.vi;
    if (!v) return;
    if (editor.value !== v.text) editor.value = v.text;
    editor.readOnly = v.mode !== 'insert';
    const c = v.cursor;
    if (v.mode === 'insert') editor.setSelectionRange(c, c);
    else {
      const cp = v.text.codePointAt(c);
      const w = cp === undefined || v.text[c] === '\n' ? 0 : cp > 0xffff ? 2 : 1;
      editor.setSelectionRange(c, c + w);
    }
    viMode.textContent = `${v.mode === 'insert' ? '-- INSERT --' : '-- NORMAL --'}${v.pending ? ` ${v.pending}` : ''}${v.message ? `  ${v.message}` : ''}`;
  }

  const viewRows = () => Math.max(2, Math.floor((editor.clientHeight || 0) / 20)) || 10;

  function openCmd(kind) {
    st.viCmd = kind;
    viPrefix.textContent = kind;
    cmdline.value = '';
    cmdline.classList.remove('hidden');
    cmdline.focus({ preventScroll: true });
  }

  function closeCmd() {
    st.viCmd = null;
    viPrefix.textContent = '';
    cmdline.classList.add('hidden');
    if (st.mode === 'edit') editor.focus({ preventScroll: true });
  }

  function viAction(a) {
    if (!a) return;
    if (a.type === 'prompt') openCmd(a.kind);
    else if (a.type === 'save') save({ stay: true });
    else if (a.type === 'saveQuit') save();
    else if (a.type === 'quit') a.force ? cancel() : leaveEdit();
  }

  function submitCmd() {
    const kind = st.viCmd;
    const line = cmdline.value;
    closeCmd();
    const r = kind === '/' ? viSearch(st.vi, line) : viCommand(st.vi, line);
    st.vi = r.state;
    paintVi();
    viAction(r.action);
  }

  /** A keydown while vi editing (after the app router; bubble phase on the applet). */
  function onViKey(e) {
    if (e.target === cmdline) {
      if (e.isComposing) return; // Enter / Esc finish a Hangul composition first
      if (e.code === 'Enter' || e.code === 'NumpadEnter') {
        e.preventDefault();
        e.stopPropagation();
        submitCmd();
      } else if (e.code === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        closeCmd();
      }
      return; // typing into the command line
    }
    const v = st.vi;
    const k = viKeyFromEvent(e, v.mode);
    if (!k) {
      // normal mode: nothing else may act on the (read-only) textarea; insert mode: typing
      if (v.mode === 'normal' && !e.ctrlKey && !e.altKey && !e.metaKey) e.preventDefault();
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    if (k === 'C-Enter') {
      if (v.mode === 'insert') st.vi = viLeaveInsert(v, editor.value, editor.selectionStart ?? editor.value.length);
      save();
      return;
    }
    if (v.mode === 'insert') {
      st.vi = viLeaveInsert(v, editor.value, editor.selectionStart ?? editor.value.length); // Esc / Ctrl+[
      paintVi();
      return;
    }
    const r = viKey(v, k, { rows: viewRows() });
    st.vi = r.state;
    paintVi();
    viAction(r.action);
  }

  const dirty = () => normalizeEol(editor.value, '\n') !== normalizeEol(st.base.body, '\n');

  function leaveEdit() {
    const hadFocus = root.contains(document.activeElement);
    st.mode = 'read';
    st.external = false;
    st.base = null;
    st.vi = null;
    st.viCmd = null;
    editor.readOnly = false;
    cmdline.classList.add('hidden');
    st.memo = st.latest || st.memo;
    show();
    renderRead();
    if (hadFocus) root.focus({ preventScroll: true }); // the editor left the document
  }

  /** Save header + edited body. `stay` (vi :w): keep editing — the saved text becomes the base. */
  async function save({ stay = false } = {}) {
    if (st.mode !== 'edit' || st.saving) return;
    const base = st.base;
    const text = joinMemo(base.header, normalizeEol(editor.value, memoEol(base.header + base.body)), base.marker);
    st.saving = true;
    try {
      await api.memoSave(text);
    } catch (err) {
      st.saving = false;
      deps.toast(err); // stay in the editor with the draft
      readMore();
      return;
    }
    st.saving = false;
    if (st.disposed) return;
    st.text = text;
    st.latest = splitMemo(text);
    if (stay && st.mode === 'edit') {
      st.base = st.latest;
      st.external = false;
      if (st.vi) st.vi = { ...st.vi, base: st.vi.text, message: '저장했습니다' };
      show();
      paintVi();
    } else {
      leaveEdit();
    }
    readMore();
  }

  async function cancel() {
    if (st.mode !== 'edit' || st.asking) return;
    if (dirty()) {
      st.asking = true;
      let ok = false;
      try {
        ok = await deps.confirm('편집한 메모를 버릴까요?\n저장하지 않은 내용은 사라집니다.', { enterYes: false });
      } finally {
        st.asking = false;
      }
      if (st.disposed || st.mode !== 'edit') return;
      if (!ok) {
        editor.focus({ preventScroll: true });
        return;
      }
    }
    const external = st.external;
    leaveEdit();
    if (external) read({ force: true }); // "Esc: 취소하고 다시 읽기"
  }

  const actions = {
    edit,
    reload: () => read({ force: true }),
    scrollDown: () => view.scrollBy(0, LINE_PX),
    scrollUp: () => view.scrollBy(0, -LINE_PX),
    top: () => {
      view.scrollTop = 0;
    },
    bottom: () => {
      view.scrollTop = view.scrollHeight;
    },
    save,
    cancel,
  };

  const onKey = (e) => {
    if (e.defaultPrevented) return; // the app key router / an overlay owns it
    if (isFocusTrapKey(e)) {
      e.preventDefault(); // focus never wanders into a neighbouring terminal
      return;
    }
    if (st.mode === 'edit' && st.vi) {
      onViKey(e);
      return;
    }
    const a = memoKeyAction(e, st.mode);
    if (!a) return; // in the editor: typing
    e.preventDefault();
    e.stopPropagation();
    actions[a]();
  };

  const onMouse = (e) => {
    const b = e.target && e.target.closest ? e.target.closest('button[data-action]') : null;
    if (!b || !root.contains(b)) return;
    e.preventDefault(); // keep DOM focus where it is (the editor keeps its draft and cursor)
    actions[b.dataset.action]();
  };

  // links in the rendered body never navigate the WebView: http(s) → system browser, else nothing
  const onClick = (e) => {
    const a = e.target && e.target.closest ? e.target.closest(LINK_SELECTOR) : null;
    if (!a || !content.contains(a)) return;
    e.preventDefault();
    if (e.type !== 'click' || e.button !== 0) return;
    const href = a.getAttribute('href') ?? (a.getAttributeNS ? a.getAttributeNS('http://www.w3.org/1999/xlink', 'href') : null);
    const r = resolveDocLink(st.path, href);
    if (r.type === 'external') api.docOpenUrl(r.url).catch((err) => deps.toast(err));
  };
  const onDbl = (e) => {
    if (st.mode === 'read' && content.contains(e.target)) edit();
  };
  const onDrag = (e) => e.preventDefault();

  root.addEventListener('keydown', onKey);
  root.addEventListener('mousedown', onMouse);
  root.addEventListener('click', onClick);
  root.addEventListener('auxclick', onClick);
  root.addEventListener('dblclick', onDbl);
  root.addEventListener('dragstart', onDrag);
  // memo.vi (config.json): read at mount and after every settings reload; applies to the next edit
  let viSetting = false;
  const loadSettings = () =>
    api.memoSettings().then(
      (r) => {
        if (!st.disposed) viSetting = !!(r && r.vi);
      },
      () => {}, // unknown: plain editing
    );
  const viOn = () => (typeof deps.vi === 'function' ? !!deps.vi() : viSetting);
  const unlistenCfg = api.on('config-reloaded', () => {
    if (!st.disposed) loadSettings();
  });
  loadSettings();
  const unlisten = api.on('doc-changed', (p) => {
    if (p && p.paneId === paneId && !st.disposed) read();
  });

  show();
  read();

  return {
    el: root,
    focus() {
      if (st.mode === 'edit') {
        const target = st.viCmd ? cmdline : editor;
        if (document.activeElement !== target) target.focus({ preventScroll: true });
        return;
      }
      if (!root.contains(document.activeElement)) root.focus({ preventScroll: true });
    },
    dispose() {
      if (st.disposed) return;
      st.disposed = true;
      st.gen++;
      root.removeEventListener('keydown', onKey);
      root.removeEventListener('mousedown', onMouse);
      root.removeEventListener('click', onClick);
      root.removeEventListener('auxclick', onClick);
      root.removeEventListener('dblclick', onDbl);
      root.removeEventListener('dragstart', onDrag);
      for (const u of [unlisten, unlistenCfg]) Promise.resolve(u).then((f) => typeof f === 'function' && f(), () => {});
      api.docUnwatch(paneId).catch(() => {});
      root.remove();
    },
  };
}
