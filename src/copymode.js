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
  const { viewportTop, rows } = state.buffer;
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
      return step(moveTo(state, row - b.rows, col));
    case 'PageDown':
      return step(moveTo(state, row + b.rows, col));
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
