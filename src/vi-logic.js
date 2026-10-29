// Memo vi editing (memo spec §8, decisions V1 a · V2 a): a pure state machine over the memo body
// (text with `\n` line breaks, as a textarea gives it) and a cursor offset. Normal mode runs
// here; insert mode is the textarea itself (typing, IME) — `viLeaveInsert` takes its text back.
// Keys are read by `KeyboardEvent.code` (Hangul layout safe); Alt / Meta chords are never vi
// keys, so app shortcuts keep working (V4: no collision with the default keymap).
// Out of scope (spec): counts (3dd), visual mode, registers other than the unnamed one.
// No DOM: unit-tested in Node. memo.js drives the textarea.

/** Every vi key, as `[token, KeyboardEvent.code, modifiers]` (spec V1 a + Ctrl+d/u/[ + Ctrl+Enter). */
export const VI_KEYS = [
  ['Esc', 'Escape', ''], ['i', 'KeyI', ''], ['a', 'KeyA', ''], ['A', 'KeyA', 'S'], ['o', 'KeyO', ''], ['O', 'KeyO', 'S'],
  ['h', 'KeyH', ''], ['j', 'KeyJ', ''], ['k', 'KeyK', ''], ['l', 'KeyL', ''], ['w', 'KeyW', ''], ['b', 'KeyB', ''],
  ['0', 'Digit0', ''], ['$', 'Digit4', 'S'], ['g', 'KeyG', ''], ['G', 'KeyG', 'S'],
  ['x', 'KeyX', ''], ['d', 'KeyD', ''], ['y', 'KeyY', ''], ['p', 'KeyP', ''], ['P', 'KeyP', 'S'], ['u', 'KeyU', ''], ['C-r', 'KeyR', 'C'],
  ['/', 'Slash', ''], ['n', 'KeyN', ''], ['N', 'KeyN', 'S'], [':', 'Semicolon', 'S'], ['Z', 'KeyZ', 'S'],
  ['C-d', 'KeyD', 'C'], ['C-u', 'KeyU', 'C'], ['C-[', 'BracketLeft', 'C'], ['C-Enter', 'Enter', 'C'],
];

const CTRL = { KeyR: 'C-r', KeyD: 'C-d', KeyU: 'C-u', BracketLeft: 'C-[', Enter: 'C-Enter', NumpadEnter: 'C-Enter' };

/**
 * Token for a keydown in `mode` ('normal' | 'insert'), or null. Normal mode: letters (Shift =
 * capital), `0`, `$`, `/`, `:`, Esc and Ctrl+r/d/u/[/Enter — by code, even while an IME is
 * composing. Insert mode: only Esc, Ctrl+[ and Ctrl+Enter, and none while composing (the IME
 * takes that Esc).
 */
export function viKeyFromEvent(e, mode) {
  if (!e || e.altKey || e.metaKey) return null;
  if (mode === 'insert') {
    if (e.isComposing) return null;
    if (e.ctrlKey) return CTRL[e.code] === 'C-[' || CTRL[e.code] === 'C-Enter' ? CTRL[e.code] : null;
    return e.code === 'Escape' && !e.shiftKey ? 'Esc' : null;
  }
  if (e.ctrlKey) return e.shiftKey ? null : CTRL[e.code] || null;
  if (e.code === 'Escape') return 'Esc';
  const m = /^Key([A-Z])$/.exec(e.code || '');
  if (m) return e.shiftKey ? m[1] : m[1].toLowerCase();
  if (e.code === 'Digit0' && !e.shiftKey) return '0';
  if (e.code === 'Digit4' && e.shiftKey) return '$';
  if (e.code === 'Slash' && !e.shiftKey) return '/';
  if (e.code === 'Semicolon' && e.shiftKey) return ':';
  return null;
}

// ---------------------------------------------------------------- text helpers

const lineStart = (t, i) => t.lastIndexOf('\n', i - 1) + 1;
const lineEnd = (t, i) => {
  const n = t.indexOf('\n', i);
  return n < 0 ? t.length : n;
};
const isLow = (t, i) => {
  const c = t.charCodeAt(i);
  return c >= 0xdc00 && c <= 0xdfff;
};
const next = (t, i) => (isLow(t, i + 1) ? i + 2 : i + 1);
const prev = (t, i) => (i > 0 && isLow(t, i - 1) ? i - 2 : i - 1);

/** A normal-mode position: on a character of its line (the line start on an empty line). */
function clampNormal(t, i) {
  const pos = Math.max(0, Math.min(i, t.length));
  const ls = lineStart(t, pos);
  const le = lineEnd(t, ls);
  if (le === ls) return ls;
  let c = Math.min(Math.max(pos, ls), le - 1);
  if (isLow(t, c)) c--;
  return c;
}

const firstNonBlank = (t, ls) => {
  const le = lineEnd(t, ls);
  let i = ls;
  while (i < le && (t[i] === ' ' || t[i] === '\t')) i++;
  return clampNormal(t, i);
};

/** Column (code units) of `i` in its line, and the position at `col` on the line starting at `ls`. */
const column = (t, i) => i - lineStart(t, i);
const atColumn = (t, ls, col) => {
  const le = lineEnd(t, ls);
  return clampNormal(t, le === ls ? ls : Math.min(ls + col, le - 1)); // stays on that line
};

const cls = (c) => (c === undefined ? -1 : /\s/.test(c) ? 0 : /[\p{L}\p{N}_]/u.test(c) ? 1 : 2);

function wordForward(t, i) {
  if (i >= t.length) return clampNormal(t, i);
  const start = cls(t[i]);
  let j = i;
  if (start > 0) while (j < t.length && cls(t[j]) === start) j++;
  while (j < t.length && cls(t[j]) === 0) {
    if (t[j] === '\n' && t[j + 1] === '\n') return j + 1; // an empty line is a word
    j++;
  }
  return clampNormal(t, j);
}

function wordBackward(t, i) {
  let j = i - 1;
  while (j > 0 && cls(t[j]) === 0) {
    if (t[j] === '\n' && t[j - 1] === '\n') return j; // empty line
    j--;
  }
  if (j <= 0) return 0;
  const c = cls(t[j]);
  while (j > 0 && cls(t[j - 1]) === c) j--;
  return j;
}

/** Index of the line holding `i` and the line starts of the text. */
function lines(t) {
  const starts = [0];
  for (let i = t.indexOf('\n'); i >= 0; i = t.indexOf('\n', i + 1)) starts.push(i + 1);
  return starts;
}
const lineIndex = (starts, i) => {
  let n = 0;
  while (n + 1 < starts.length && starts[n + 1] <= i) n++;
  return n;
};

// ---------------------------------------------------------------- state

/** Normal-mode state over `text` (cursor 0 unless given). `base` = the text dirtiness compares to. */
export function createVi(text, { cursor = 0 } = {}) {
  const t = String(text ?? '');
  return { text: t, cursor: clampNormal(t, cursor), mode: 'normal', pending: '', want: null, reg: null, undo: [], redo: [], search: null, base: t, message: '' };
}

/** Whether the text differs from what the edit started from. */
export const viDirty = (s) => s.text !== s.base;

const set = (s, patch) => ({ ...s, pending: '', message: '', ...patch });
const move = (s, cursor, keepWant = false) => set(s, { cursor: clampNormal(s.text, cursor), want: keepWant ? s.want : null });

/** Apply a text change with an undo step (redo cleared). */
function edit(s, text, cursor, extra = {}) {
  return set(s, { text, cursor: clampNormal(text, cursor), want: null, undo: [...s.undo, { text: s.text, cursor: s.cursor }], redo: [], ...extra });
}

function vertical(s, delta) {
  const starts = lines(s.text);
  const n = Math.max(0, Math.min(starts.length - 1, lineIndex(starts, s.cursor) + delta));
  const want = s.want ?? column(s.text, s.cursor);
  return set(s, { cursor: atColumn(s.text, starts[n], want), want });
}

function enterInsert(s, cursor, text = s.text) {
  return { state: { ...set(s, { mode: 'insert', cursor, text, want: null }), undo: [...s.undo, { text: s.text, cursor: s.cursor }], redo: [] }, action: { type: 'insert' } };
}

function lineRange(t, i) {
  const ls = lineStart(t, i);
  const le = lineEnd(t, ls);
  return [ls, le];
}

function deleteLine(s) {
  const t = s.text;
  const [ls, le] = lineRange(t, s.cursor);
  const reg = { text: `${t.slice(ls, le)}\n`, lines: true };
  let from = ls;
  let to = le + 1;
  if (le >= t.length) {
    to = le;
    if (ls > 0) from = ls - 1; // last line: take the line break before it
  }
  const text = t.slice(0, from) + t.slice(to);
  const at = from < ls ? lineStart(text, Math.max(0, from)) : from;
  return edit(s, text, firstNonBlank(text, lineStart(text, Math.min(at, text.length))), { reg });
}

function put(s, before) {
  const r = s.reg;
  if (!r) return set(s, { message: '붙여 넣을 내용이 없습니다' });
  const t = s.text;
  if (r.lines) {
    const [ls, le] = lineRange(t, s.cursor);
    const body = r.text.endsWith('\n') ? r.text.slice(0, -1) : r.text;
    if (before) return edit(s, `${t.slice(0, ls)}${body}\n${t.slice(ls)}`, ls);
    if (le >= t.length) return edit(s, `${t}\n${body}`, t.length + 1);
    return edit(s, `${t.slice(0, le + 1)}${body}\n${t.slice(le + 1)}`, le + 1);
  }
  const [ls, le] = lineRange(t, s.cursor);
  const at = before || le === ls ? s.cursor : next(t, s.cursor);
  const text = t.slice(0, at) + r.text + t.slice(at);
  return edit(s, text, at + r.text.length - 1);
}

function find(s, query, dir) {
  if (!query) return set(s, { message: '검색어가 없습니다' });
  const t = s.text;
  let i;
  if (dir === 'backward') {
    i = s.cursor > 0 ? t.lastIndexOf(query, s.cursor - 1) : -1;
    if (i < 0) i = t.lastIndexOf(query);
  } else {
    i = t.indexOf(query, s.cursor + 1);
    if (i < 0) i = t.indexOf(query);
  }
  if (i < 0) return set(s, { message: `패턴 없음: ${query}` });
  return move(s, i); // the only match under the cursor: stays there (vi wraps the same way)
}

/** `/pattern` submitted: search forward from the cursor (wrapping); remembered for n / N. */
export function viSearch(s, query, dir = 'forward') {
  const q = String(query ?? '');
  const st = find({ ...s, search: q ? { query: q, dir } : s.search }, q, dir);
  return { state: { ...st, search: q ? { query: q, dir } : s.search } };
}

/** `:command` submitted. `w` save, `wq` / `x` save and leave, `q!` leave (Y/N when changed is the
 * caller's), `q` leave only when nothing changed. */
export function viCommand(s, line) {
  const c = String(line ?? '').trim();
  if (c === 'w') return { state: set(s, {}), action: { type: 'save' } };
  if (c === 'wq' || c === 'x') return { state: set(s, {}), action: { type: 'saveQuit' } };
  if (c === 'q!') return { state: set(s, {}), action: { type: 'quit', force: true } };
  if (c === 'q') {
    if (viDirty(s)) return { state: set(s, { message: '바뀐 내용이 있습니다 — 저장은 :w, 버리기는 :q!' }) };
    return { state: set(s, {}), action: { type: 'quit', force: false } };
  }
  return { state: set(s, { message: c ? `알 수 없는 명령: ${c}` : '' }) };
}

/** Back from insert mode with the textarea's `text` and caret: normal mode, one to the left (never
 * before the line start). An insert session that changed nothing leaves no undo step. */
export function viLeaveInsert(s, text, caret) {
  const t = String(text ?? '');
  const undo = t === s.text && s.undo.length && s.undo[s.undo.length - 1].text === t ? s.undo.slice(0, -1) : s.undo;
  const c = Math.max(lineStart(t, Math.min(caret, t.length)), Math.min(caret, t.length) - 1);
  return { ...set(s, { mode: 'normal', text: t, want: null }), cursor: clampNormal(t, c), undo };
}

/**
 * One normal-mode key token → `{state, action?}`. Actions: `{type:'insert'}` (make the textarea
 * editable at `state.cursor`), `{type:'prompt', kind: ':' | '/'}`, `{type:'saveQuit'}` (ZZ).
 * `opts.rows`: visible lines (Ctrl+d / Ctrl+u move half of them; default 10).
 */
export function viKey(s, token, { rows = 10 } = {}) {
  const t = s.text;
  const p = s.pending;
  if (token === 'Esc' || token === 'C-[') return { state: set(s, {}) };
  if (p) {
    if (p === 'g' && token === 'g') return { state: move(s, firstNonBlank(t, 0)) };
    if (p === 'd' && token === 'd') return { state: deleteLine(s) };
    if (p === 'y' && token === 'y') {
      const [ls, le] = lineRange(t, s.cursor);
      return { state: set(s, { reg: { text: `${t.slice(ls, le)}\n`, lines: true } }) };
    }
    if (p === 'Z' && token === 'Z') return { state: set(s, {}), action: { type: 'saveQuit' } };
    return { state: set(s, {}) }; // an unfinished operator: both keys dropped
  }
  const [ls, le] = lineRange(t, s.cursor);
  switch (token) {
    case 'h':
      return { state: move(s, s.cursor > ls ? prev(t, s.cursor) : s.cursor) };
    case 'l':
      return { state: move(s, s.cursor + 1 < le ? next(t, s.cursor) : s.cursor) };
    case 'j':
      return { state: vertical(s, 1) };
    case 'k':
      return { state: vertical(s, -1) };
    case 'C-d':
      return { state: vertical(s, Math.max(1, Math.floor(rows / 2))) };
    case 'C-u':
      return { state: vertical(s, -Math.max(1, Math.floor(rows / 2))) };
    case '0':
      return { state: move(s, ls) };
    case '$':
      return { state: move(s, le) };
    case 'w':
      return { state: move(s, wordForward(t, s.cursor)) };
    case 'b':
      return { state: move(s, wordBackward(t, s.cursor)) };
    case 'G':
      return { state: move(s, firstNonBlank(t, lineStart(t, t.length))) };
    case 'g':
    case 'd':
    case 'y':
    case 'Z':
      return { state: { ...s, pending: token, message: '' } };
    case 'i':
      return enterInsert(s, s.cursor);
    case 'a':
      return enterInsert(s, le === ls ? s.cursor : next(t, s.cursor));
    case 'A':
      return enterInsert(s, le);
    case 'o':
      return enterInsert(s, le + 1, `${t.slice(0, le)}\n${t.slice(le)}`);
    case 'O':
      return enterInsert(s, ls, `${t.slice(0, ls)}\n${t.slice(ls)}`);
    case 'x': {
      if (le === ls) return { state: set(s, {}) };
      const end = next(t, s.cursor);
      return { state: edit(s, t.slice(0, s.cursor) + t.slice(end), s.cursor, { reg: { text: t.slice(s.cursor, end), lines: false } }) };
    }
    case 'p':
      return { state: put(s, false) };
    case 'P':
      return { state: put(s, true) };
    case 'u': {
      if (!s.undo.length) return { state: set(s, { message: '더 되돌릴 것이 없습니다' }) };
      const last = s.undo[s.undo.length - 1];
      return { state: set(s, { text: last.text, cursor: clampNormal(last.text, last.cursor), undo: s.undo.slice(0, -1), redo: [...s.redo, { text: t, cursor: s.cursor }] }) };
    }
    case 'C-r': {
      if (!s.redo.length) return { state: set(s, { message: '다시 할 것이 없습니다' }) };
      const r = s.redo[s.redo.length - 1];
      return { state: set(s, { text: r.text, cursor: clampNormal(r.text, r.cursor), redo: s.redo.slice(0, -1), undo: [...s.undo, { text: t, cursor: s.cursor }] }) };
    }
    case '/':
      return { state: set(s, {}), action: { type: 'prompt', kind: '/' } };
    case ':':
      return { state: set(s, {}), action: { type: 'prompt', kind: ':' } };
    case 'n':
    case 'N': {
      if (!s.search) return { state: set(s, { message: '검색어가 없습니다' }) };
      const dir = (s.search.dir === 'backward') === (token === 'n') ? 'backward' : 'forward';
      return { state: { ...find(s, s.search.query, dir), search: s.search } };
    }
    default:
      return { state: s }; // not a vi key: nothing happens
  }
}
