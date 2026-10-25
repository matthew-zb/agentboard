// Copy mode DOM layer (spec 6.3): drives the pure state machine in copymode.js against a pane's
// xterm. Kept out of copymode.js so that file stays DOM-free and Node-testable.
//
// - The mode is per pane and survives tab/group switches (the pane element is only parked); it is
//   NOT closed by closeOverlays(). Its keyboard owner (a modal on the router stack) answers only
//   while the focused pane is the copy-mode pane, otherwise it passes keys on (null).
// - App chords (Alt+h, Alt+c, ...) pass through so you can leave the pane; every other key is
//   swallowed and never reaches a PTY (pane.js additionally blocks onData/onBinary: IME
//   composition could otherwise leak text on compositionend).
// - Cursor marker: the cursor LINE is highlighted with term.selectLines (selection is linewise
//   anyway); the column is not drawn, because wide Korean cells make col -> cell ambiguous.
import { createCopyState, copyKey, copyKeyFromEvent, makeCopyBuffer, refreshBuffer, setSearch, selectionRange, viewportFor } from './copymode.js';
import { getPane, hasPane, setInputBlocked } from './pane.js';
import * as api from './api.js';

const cfg = {
  pushModal: () => null,
  popModal() {},
  isAppKey: () => false,
  focusedPane: () => null,
  prompt: async () => null,
  toast() {},
};

/** `{pushModal, popModal, isAppKey(e), focusedPane(), prompt(title, initial), toast(msg)}`. */
export function configureCopyMode(c) {
  Object.assign(cfg, c);
}

const sessions = new Map(); // paneId -> {state, token, lastQuery, busy}

export function copyActive(paneId) {
  return sessions.has(paneId);
}

function paint(entry, s, { scroll = true } = {}) {
  const r = selectionRange(s);
  const a = r ? r.startRow : s.cursor.row;
  const b = r ? r.endRow : s.cursor.row;
  entry.term.selectLines(a, b);
  if (scroll) entry.term.scrollToLine(viewportFor(s));
}

/** Fresh buffer view (live output may have grown the buffer since the last key). */
function freshState(entry, sess) {
  const t = entry.term;
  sess.state = refreshBuffer(sess.state, makeCopyBuffer(t.buffer.active, t.rows));
  return sess.state;
}

export function enterCopyMode(paneId) {
  if (sessions.has(paneId) || !hasPane(paneId) || !getPane(paneId).term) return false; // applets have no buffer
  const entry = getPane(paneId);
  const sess = { state: null, token: null, lastQuery: '', busy: false };
  sess.state = createCopyState(makeCopyBuffer(entry.term.buffer.active, entry.term.rows));
  sessions.set(paneId, sess);
  setInputBlocked(paneId, true);
  entry.el.classList.add('copy');
  sess.token = cfg.pushModal((e) => onKey(paneId, e));
  paint(entry, sess.state);
  return true;
}

export function exitCopyMode(paneId) {
  const sess = sessions.get(paneId);
  if (!sess) return;
  sessions.delete(paneId);
  cfg.popModal(sess.token);
  if (hasPane(paneId)) {
    const entry = getPane(paneId);
    setInputBlocked(paneId, false);
    entry.term.clearSelection();
    entry.el.classList.remove('copy');
  }
}

export function toggleCopyMode(paneId) {
  if (copyActive(paneId)) exitCopyMode(paneId);
  else enterCopyMode(paneId);
}

/** Drop sessions whose pane no longer exists (closed). */
export function pruneCopySessions() {
  for (const id of [...sessions.keys()]) if (!hasPane(id)) exitCopyMode(id);
}

function onKey(paneId, e) {
  const sess = sessions.get(paneId);
  if (!sess) return null;
  if (cfg.focusedPane() !== paneId) return null; // hidden / unfocused: not ours
  if (cfg.isAppKey(e)) return null; // Alt+h, Alt+c, tab switches ... still work
  if (sess.busy) return true; // a search prompt is open (it sits above us anyway)
  const key = copyKeyFromEvent(e);
  // Held keys repeat only for movement; v / y / / / q / Esc act once per press.
  if (key && (!e.repeat || /^(Arrow|Page|[hjklgGnN]$)/.test(key))) handleKey(paneId, sess, key);
  return true;
}

function handleKey(paneId, sess, key) {
  const entry = getPane(paneId);
  const { state, action } = copyKey(freshState(entry, sess), key);
  sess.state = state;
  if (!action) return paint(entry, state);
  switch (action.type) {
    case 'exit':
      return exitCopyMode(paneId);
    case 'copy':
      api.writeClipboardText(action.text).then(
        () => cfg.toast(`복사했습니다 (${action.text.split('\n').length}줄)`),
        (err) => cfg.toast(err),
      );
      return exitCopyMode(paneId);
    case 'search':
      return startSearch(paneId, sess);
    case 'findNext':
    case 'findPrevious':
      return find(paneId, sess, action.type === 'findNext', action.query);
  }
}

async function startSearch(paneId, sess) {
  sess.busy = true;
  let q;
  try {
    q = await cfg.prompt('검색', sess.lastQuery);
  } finally {
    sess.busy = false;
  }
  if (q == null || !sessions.has(paneId) || sessions.get(paneId) !== sess) return;
  q = q.trim();
  if (!q) return;
  sess.lastQuery = q;
  sess.state = setSearch(sess.state, q);
  find(paneId, sess, true, q);
}

function find(paneId, sess, forward, query) {
  const entry = getPane(paneId);
  const t = entry.term;
  // The linewise cursor highlight is the search origin (addon-search starts from the selection).
  const ok = forward ? entry.search.findNext(query, { incremental: false }) : entry.search.findPrevious(query, { incremental: false });
  if (!ok) {
    cfg.toast('일치하는 항목이 없습니다');
    paint(entry, sess.state, { scroll: false });
    return;
  }
  const pos = t.getSelectionPosition();
  if (pos) sess.state = { ...sess.state, anchor: null, cursor: { row: pos.start.y, col: pos.start.x } };
  // Leave the match highlighted; the next key repaints the cursor line.
}
