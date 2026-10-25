// Pure copy-mode state machine (vim-like keys over a terminal buffer). No DOM, no xterm access:
// the DOM layer (enterCopyMode) feeds keys in and applies the returned actions.

/**
 * Buffer snapshot. All rows are absolute, 0-based buffer line indices.
 * @typedef {{lineCount:number, lineText:(i:number)=>string, viewportTop:number, rows:number,
 *            cursor:{row:number,col:number}}} CopyBuffer
 * Immutable state. `anchor` is the selection start (null = no selection); `search` is set by
 * {@link setSearch}.
 * @typedef {{buffer:CopyBuffer, cursor:{row:number,col:number}, anchor:{row:number,col:number}|null,
 *            search:{query:string,dir:'forward'}|null}} CopyState
 * @typedef {{type:'copy',text:string}|{type:'search'}|{type:'findNext',query:string}
 *          |{type:'findPrevious',query:string}|{type:'exit'}|null} CopyAction
 */

const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));
const lastRow = (b) => Math.max(0, b.lineCount - 1);
const lineLen = (b, row) => (b.lineText(row) ?? '').length;
/** Window height, guarded against undefined/0/NaN. */
const rowsOf = (b) => Math.max(1, b.rows | 0);
const maxCol = (b, row) => Math.max(0, lineLen(b, row) - 1);

/** New copy state from a buffer snapshot; cursor starts at `buffer.cursor` (clamped), no selection. */
export function createCopyState(buffer) {
  const row = clamp(buffer.cursor?.row ?? 0, 0, lastRow(buffer));
  const col = clamp(buffer.cursor?.col ?? 0, 0, maxCol(buffer, row));
  return { buffer, cursor: { row, col }, anchor: null, search: null };
}

/** Returns a copy of `state` remembering `query` for n/N. Empty/blank query clears the search. */
export function setSearch(state, query) {
  return { ...state, search: query ? { query, dir: 'forward' } : null };
}

/** Selected line range (linewise, inclusive) or null when no selection is active. */
export function selectionRange(state) {
  if (!state.anchor) return null;
  return {
    startRow: Math.min(state.anchor.row, state.cursor.row),
    endRow: Math.max(state.anchor.row, state.cursor.row),
  };
}

/**
 * Top buffer row of a `rows`-high window that keeps the cursor visible with the smallest scroll
 * change relative to `buffer.viewportTop`.
 */
export function viewportFor(state) {
  const { viewportTop } = state.buffer;
  const rows = rowsOf(state.buffer);
  const row = state.cursor.row;
  if (row < viewportTop) return row;
  if (row > viewportTop + rows - 1) return row - rows + 1;
  return viewportTop;
}

function moveTo(state, row, col) {
  const b = state.buffer;
  const r = clamp(row, 0, lastRow(b));
  return { ...state, cursor: { row: r, col: clamp(col, 0, maxCol(b, r)) } };
}

function copyText(state) {
  const range = selectionRange(state) ?? { startRow: state.cursor.row, endRow: state.cursor.row };
  const out = [];
  for (let i = range.startRow; i <= range.endRow; i++) {
    out.push((state.buffer.lineText(i) ?? '').replace(/\s+$/, ''));
  }
  return out.join('\n');
}

/**
 * Applies one key (`KeyboardEvent.key`) to the state. Never mutates `state`.
 * Selection is linewise; `y` copies the selected lines (or the cursor line without selection),
 * trailing whitespace trimmed per line, joined with '\n'. Unknown keys return `action: null` and the
 * same state object (the caller swallows them).
 * @param {CopyState} state
 * @param {string} key
 * @returns {{state: CopyState, action: CopyAction}}
 */
export function copyKey(state, key) {
  const { row, col } = state.cursor;
  const b = state.buffer;
  const step = (s) => ({ state: s, action: null });
  switch (key) {
    case 'h':
    case 'ArrowLeft':
      return step(moveTo(state, row, col - 1));
    case 'l':
    case 'ArrowRight':
      return step(moveTo(state, row, col + 1));
    case 'j':
    case 'ArrowDown':
      return step(moveTo(state, row + 1, col));
    case 'k':
    case 'ArrowUp':
      return step(moveTo(state, row - 1, col));
    case 'PageUp':
      return step(moveTo(state, row - rowsOf(b), col));
    case 'PageDown':
      return step(moveTo(state, row + rowsOf(b), col));
    case 'g':
      return step(moveTo(state, 0, col));
    case 'G':
      return step(moveTo(state, lastRow(b), col));
    case 'v':
      return step({ ...state, anchor: state.anchor ? null : { row, col } });
    case 'y':
      return { state, action: { type: 'copy', text: copyText(state) } };
    case '/':
      return { state, action: { type: 'search' } };
    case 'n':
    case 'N':
      if (!state.search) return step(state);
      return {
        state,
        action: { type: key === 'n' ? 'findNext' : 'findPrevious', query: state.search.query },
      };
    case 'Escape':
    case 'q':
      return { state, action: { type: 'exit' } };
    default:
      return step(state);
  }
}

const LETTER_KEYS = { KeyH: 'h', KeyJ: 'j', KeyK: 'k', KeyL: 'l', KeyG: 'g', KeyV: 'v', KeyY: 'y', KeyN: 'n', KeyQ: 'q' };
const NAMED_KEYS = ['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Escape'];

/**
 * KeyboardEvent -> key string for {@link copyKey}, from `e.code` (IME-safe: Hangul composition
 * changes `e.key` but not `e.code`). Shift+letter gives the upper-case key (G, N). Ctrl/Alt/Meta
 * chords and unmapped keys give null.
 */
export function copyKeyFromEvent(e) {
  if (!e || e.ctrlKey || e.altKey || e.metaKey) return null;
  const c = e.code;
  if (LETTER_KEYS[c]) return e.shiftKey ? LETTER_KEYS[c].toUpperCase() : LETTER_KEYS[c];
  if (c === 'Slash') return e.shiftKey ? null : '/';
  return NAMED_KEYS.includes(c) ? c : null;
}

/**
 * CopyBuffer over xterm's `term.buffer.active` (rows are absolute buffer lines). The cursor
 * starts at the terminal's cursor row. Column is a cell index; wide (Korean) characters make it
 * differ from the string index, which only matters for the (line-level) cursor marker.
 */
export function makeCopyBuffer(active, rows) {
  return {
    lineCount: active.length,
    lineText: (i) => {
      const line = active.getLine(i);
      return line ? line.translateToString(true) : '';
    },
    viewportTop: active.viewportY,
    rows,
    cursor: { row: active.baseY + active.cursorY, col: active.cursorX },
  };
}

/** Same state over a fresh buffer snapshot (live output may have grown it); cursor/anchor clamped. */
export function refreshBuffer(state, buffer) {
  const fix = (p) => {
    const row = clamp(p.row, 0, lastRow(buffer));
    return { row, col: clamp(p.col, 0, maxCol(buffer, row)) };
  };
  return { ...state, buffer, cursor: fix(state.cursor), anchor: state.anchor ? fix(state.anchor) : null };
}
