import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createCopyState, copyKey, setSearch, selectionRange, viewportFor } from '../../src/copymode.js';

function makeBuffer(lines, over = {}) {
  return {
    lineCount: lines.length,
    lineText: (i) => lines[i] ?? '',
    viewportTop: 0,
    rows: 3,
    cursor: { row: lines.length - 1, col: 0 },
    ...over,
  };
}
const LINES = ['line1', 'line2', 'line3', 'line4', 'line5'];
const press = (s, ...keys) => {
  let action = null;
  for (const k of keys) ({ state: s, action } = copyKey(s, k));
  return { state: s, action };
};

test('copy mode moves and selects lines', () => {
  const s = createCopyState(makeBuffer(LINES));
  const { action } = press(s, 'k', 'k', 'v', 'j', 'y');
  assert.deepEqual(action, { type: 'copy', text: 'line3\nline4' });
});

test('PgUp moves by rows and clamps at 0', () => {
  const s = createCopyState(makeBuffer(LINES));
  let r = press(s, 'PageUp');
  assert.equal(r.state.cursor.row, 1);
  r = press(r.state, 'PageUp', 'PageUp');
  assert.equal(r.state.cursor.row, 0);
  r = press(r.state, 'PageDown', 'PageDown', 'PageDown');
  assert.equal(r.state.cursor.row, 4);
});

test('Esc exits without copy', () => {
  const s = createCopyState(makeBuffer(LINES));
  assert.deepEqual(copyKey(s, 'Escape').action, { type: 'exit' });
  assert.deepEqual(copyKey(s, 'q').action, { type: 'exit' });
});

test('g/G jump to first/last line', () => {
  const s = createCopyState(makeBuffer(LINES));
  assert.equal(press(s, 'g').state.cursor.row, 0);
  assert.equal(press(s, 'g', 'G').state.cursor.row, 4);
});

test('h/l clamp within line; j/k clamp col to new line', () => {
  const s = createCopyState(makeBuffer(['abc', 'x', ''], { cursor: { row: 0, col: 0 } }));
  assert.equal(press(s, 'h').state.cursor.col, 0);
  assert.equal(press(s, 'l', 'l', 'l', 'l').state.cursor.col, 2);
  assert.equal(press(s, 'l', 'l', 'j').state.cursor.col, 0);
  assert.deepEqual(press(s, 'j', 'j').state.cursor, { row: 2, col: 0 });
  assert.equal(press(s, 'ArrowRight').state.cursor.col, 1);
  assert.equal(press(s, 'ArrowDown').state.cursor.row, 1);
  assert.equal(press(s, 'k').state.cursor.row, 0);
});

test('v toggles selection', () => {
  const s = createCopyState(makeBuffer(LINES));
  const a = press(s, 'v').state;
  assert.deepEqual(a.anchor, { row: 4, col: 0 });
  assert.deepEqual(selectionRange(a), { startRow: 4, endRow: 4 });
  assert.equal(press(a, 'v').state.anchor, null);
  assert.equal(selectionRange(s), null);
  assert.deepEqual(selectionRange(press(s, 'v', 'k', 'k').state), { startRow: 2, endRow: 4 });
});

test('y without selection copies cursor line, trims trailing whitespace', () => {
  const s = createCopyState(makeBuffer(['a  ', 'b  \t', 'c'], { cursor: { row: 1, col: 0 } }));
  assert.deepEqual(copyKey(s, 'y').action, { type: 'copy', text: 'b' });
  assert.deepEqual(press(s, 'v', 'k', 'y').action, { type: 'copy', text: 'a\nb' });
});

test('search: / asks caller, n/N need a query', () => {
  const s = createCopyState(makeBuffer(LINES));
  assert.deepEqual(copyKey(s, '/').action, { type: 'search' });
  assert.equal(copyKey(s, 'n').action, null);
  assert.equal(copyKey(s, 'N').action, null);
  const s2 = setSearch(s, 'line');
  assert.deepEqual(copyKey(s2, 'n').action, { type: 'findNext', query: 'line' });
  assert.deepEqual(copyKey(s2, 'N').action, { type: 'findPrevious', query: 'line' });
  assert.equal(s.search, null);
});

test('unknown keys are swallowed', () => {
  const s = createCopyState(makeBuffer(LINES));
  const r = copyKey(s, 'z');
  assert.equal(r.action, null);
  assert.equal(r.state, s);
});

test('copyKey never mutates its input', () => {
  const s = createCopyState(makeBuffer(LINES));
  const snap = JSON.stringify({ c: s.cursor, a: s.anchor, s: s.search });
  Object.freeze(s);
  Object.freeze(s.cursor);
  press(s, 'k', 'v', 'h', 'G', 'y');
  assert.equal(JSON.stringify({ c: s.cursor, a: s.anchor, s: s.search }), snap);
});

test('viewportFor keeps cursor visible with minimal scroll', () => {
  const lines = Array.from({ length: 20 }, (_, i) => `l${i}`);
  const s = createCopyState(makeBuffer(lines, { viewportTop: 5, rows: 4, cursor: { row: 6, col: 0 } }));
  assert.equal(viewportFor(s), 5);
  assert.equal(viewportFor({ ...s, cursor: { row: 2, col: 0 } }), 2);
  assert.equal(viewportFor({ ...s, cursor: { row: 12, col: 0 } }), 9);
});

// ---- Task 14 additions
import { copyKeyFromEvent, makeCopyBuffer, refreshBuffer } from '../../src/copymode.js';

test('rows undefined/0 is guarded (PageUp moves one row, viewportFor is finite)', () => {
  for (const rows of [undefined, 0, NaN]) {
    const s = createCopyState(makeBuffer(LINES, { rows }));
    assert.equal(press(s, 'PageUp').state.cursor.row, 3);
    assert.ok(Number.isFinite(viewportFor(s)));
  }
});

test('copyKeyFromEvent maps KeyboardEvent.code (IME-safe), modifiers excluded', () => {
  const e = (code, o = {}) => ({ code, key: 'ㅓ', ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...o });
  assert.equal(copyKeyFromEvent(e('KeyJ')), 'j');
  assert.equal(copyKeyFromEvent(e('KeyG', { shiftKey: true })), 'G');
  assert.equal(copyKeyFromEvent(e('KeyN', { shiftKey: true })), 'N');
  assert.equal(copyKeyFromEvent(e('Slash')), '/');
  assert.equal(copyKeyFromEvent(e('Slash', { shiftKey: true })), null);
  assert.equal(copyKeyFromEvent(e('Escape')), 'Escape');
  assert.equal(copyKeyFromEvent(e('PageUp')), 'PageUp');
  assert.equal(copyKeyFromEvent(e('ArrowLeft')), 'ArrowLeft');
  assert.equal(copyKeyFromEvent(e('KeyJ', { ctrlKey: true })), null);
  assert.equal(copyKeyFromEvent(e('KeyZ')), null);
});

test('makeCopyBuffer adapts xterm active buffer (absolute rows, cursor row)', () => {
  const lines = ['한글 줄  ', 'abc', ''];
  const active = {
    length: 3,
    viewportY: 1,
    baseY: 1,
    cursorY: 1,
    cursorX: 2,
    getLine: (i) => (lines[i] === undefined ? undefined : { translateToString: (trim) => (trim ? lines[i].trimEnd() : lines[i]) }),
  };
  const b = makeCopyBuffer(active, 2);
  assert.equal(b.lineCount, 3);
  assert.equal(b.lineText(0), '한글 줄');
  assert.equal(b.lineText(9), '');
  assert.equal(b.viewportTop, 1);
  assert.equal(b.rows, 2);
  assert.deepEqual(b.cursor, { row: 2, col: 2 });
});

test('refreshBuffer keeps cursor/anchor/search, clamps to the new buffer', () => {
  let s = createCopyState(makeBuffer(LINES));
  s = setSearch(press(s, 'v', 'k').state, 'x');
  const smaller = makeBuffer(['a', 'b'], { cursor: { row: 0, col: 0 } });
  const r = refreshBuffer(s, smaller);
  assert.equal(r.buffer, smaller);
  assert.equal(r.cursor.row, 1);
  assert.equal(r.anchor.row, 1);
  assert.equal(r.search.query, 'x');
  assert.equal(refreshBuffer(s, makeBuffer(LINES)).cursor.row, 3);
});
