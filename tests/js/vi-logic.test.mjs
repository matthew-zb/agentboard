// Memo vi editing (memo spec §8, decisions V1 a · V2 a): the pure state machine.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createVi, viKey, viKeyFromEvent, viLeaveInsert, viCommand, viSearch, viDirty, VI_KEYS } from '../../src/vi-logic.js';
import { buildKeymap, matchEvent } from '../../src/keys.js';

/** Run tokens from `state` in normal mode; returns the last result ({state, action}). */
function run(state, ...tokens) {
  let r = { state };
  for (const t of tokens) r = viKey(r.state, t);
  return r;
}
const at = (s) => s.text.slice(s.cursor, s.cursor + 1);

const ev = (code, mods = '', extra = {}) => ({
  code,
  key: '?',
  ctrlKey: mods.includes('C'),
  altKey: mods.includes('A'),
  shiftKey: mods.includes('S'),
  metaKey: false,
  isComposing: false,
  repeat: false,
  ...extra,
});

test('viKeyFromEvent maps KeyboardEvent.code (Hangul layout safe); app chords are not vi keys', () => {
  assert.equal(viKeyFromEvent(ev('KeyH'), 'normal'), 'h');
  assert.equal(viKeyFromEvent(ev('KeyH', '', { key: 'ㅗ', isComposing: true }), 'normal'), 'h', 'normal mode reads the code even while composing');
  assert.equal(viKeyFromEvent(ev('KeyA', 'S'), 'normal'), 'A');
  assert.equal(viKeyFromEvent(ev('Digit0'), 'normal'), '0');
  assert.equal(viKeyFromEvent(ev('Digit4', 'S'), 'normal'), '$');
  assert.equal(viKeyFromEvent(ev('Slash'), 'normal'), '/');
  assert.equal(viKeyFromEvent(ev('Semicolon', 'S'), 'normal'), ':');
  assert.equal(viKeyFromEvent(ev('Escape'), 'normal'), 'Esc');
  assert.equal(viKeyFromEvent(ev('BracketLeft', 'C'), 'normal'), 'C-[');
  assert.equal(viKeyFromEvent(ev('KeyR', 'C'), 'normal'), 'C-r');
  assert.equal(viKeyFromEvent(ev('KeyD', 'C'), 'normal'), 'C-d');
  assert.equal(viKeyFromEvent(ev('Enter', 'C'), 'normal'), 'C-Enter');
  // arrows / Home / End move like h j k l 0 $ (the read-only textarea's own caret never moves)
  assert.deepEqual(['ArrowLeft', 'ArrowDown', 'ArrowUp', 'ArrowRight', 'Home', 'End'].map((c) => viKeyFromEvent(ev(c), 'normal')), ['h', 'j', 'k', 'l', '0', '$']);
  assert.equal(viKeyFromEvent(ev('ArrowLeft'), 'insert'), null, 'insert mode: the textarea moves its caret');
  assert.equal(viKeyFromEvent(ev('KeyH', 'A'), 'normal'), null, 'Alt+h is an app chord');
  assert.equal(viKeyFromEvent(ev('KeyX', 'C'), 'normal'), null);
  // insert mode: only leaving and saving; typing (and an IME composition) is the textarea's
  assert.equal(viKeyFromEvent(ev('Escape'), 'insert'), 'Esc');
  assert.equal(viKeyFromEvent(ev('BracketLeft', 'C'), 'insert'), 'C-[');
  assert.equal(viKeyFromEvent(ev('Enter', 'C'), 'insert'), 'C-Enter');
  assert.equal(viKeyFromEvent(ev('KeyH'), 'insert'), null);
  assert.equal(viKeyFromEvent(ev('Escape', '', { isComposing: true }), 'insert'), null, 'Esc ends the composition first');
  assert.equal(viKeyFromEvent(null, 'normal'), null);
});

test('vi keys never collide with app shortcuts (memo spec V4)', () => {
  const km = buildKeymap(JSON.parse(readFileSync(new URL('../../src-tauri/defaults/keybindings.json', import.meta.url), 'utf8')));
  assert.ok(VI_KEYS.length >= 32, `${VI_KEYS.length} vi keys`);
  for (const [token, code, mods] of VI_KEYS) {
    const e = ev(code, mods);
    assert.equal(viKeyFromEvent(e, 'normal'), token, token);
    assert.equal(matchEvent(km, e), null, `${token} (${mods}+${code}) is an app shortcut`);
  }
  // and the app's pane moves stay app chords
  for (const c of ['KeyH', 'KeyJ', 'KeyK', 'KeyL']) assert.ok(matchEvent(km, ev(c, 'A')), `Alt+${c}`);
});

test('motions h j k l 0 $ w b gg G stay on lines and remember the column', () => {
  const s = createVi('abc def\nx\nlonger line', { cursor: 0 });
  assert.equal(s.mode, 'normal');
  assert.equal(at(run(s, 'l', 'l').state), 'c');
  assert.equal(run(s, 'h').state.cursor, 0, 'h stops at the line start');
  assert.equal(at(run(s, '$').state), 'f');
  assert.equal(run(s, '$', 'l').state.cursor, 6, 'l stops on the last character');
  assert.equal(run(s, '$', '0').state.cursor, 0);
  // j/k keep the wanted column; a short line clamps without forgetting it
  let r = run(s, 'l', 'l', 'l', 'l', 'j');
  assert.equal(r.state.cursor, 8, "'x' (short line)");
  r = run(r.state, 'j');
  assert.equal(at(r.state), 'e', 'column 4 again on the long line');
  assert.equal(run(r.state, 'k', 'k').state.cursor, 4);
  // words: w to the next word start (across lines), b back
  assert.equal(at(run(s, 'w').state), 'd');
  assert.equal(at(run(s, 'w', 'w').state), 'x');
  assert.equal(at(run(s, 'w', 'w', 'w').state), 'l');
  assert.equal(at(run(s, 'w', 'w', 'w', 'b').state), 'x');
  assert.equal(run(s, 'w', 'b').state.cursor, 0);
  const k = createVi('한글 메모, 끝', { cursor: 0 });
  assert.equal(at(run(k, 'w').state), '메');
  assert.equal(at(run(k, 'w', 'w').state), ',');
  // gg / G: first / last line, first non-blank
  const g = createVi('  one\ntwo\n  three', { cursor: 8 });
  assert.equal(at(run(g, 'G').state), 't');
  assert.equal(run(g, 'G').state.cursor, 12);
  assert.equal(run(g, 'g', 'g').state.cursor, 2);
  assert.equal(run(g, 'g', 'x').state.text, g.text, "'g' then another key: nothing (no x)");
});

test('i a A o O enter insert mode at the right place; Esc leaves one to the left', () => {
  const s = createVi('ab\ncd', { cursor: 1 }); // on 'b'
  let r = run(s, 'i');
  assert.deepEqual([r.state.mode, r.state.cursor, r.action], ['insert', 1, { type: 'insert' }]);
  assert.equal(run(s, 'a').state.cursor, 2);
  assert.equal(run(s, 'A').state.cursor, 2);
  r = run(s, 'o');
  assert.deepEqual([r.state.text, r.state.cursor], ['ab\n\ncd', 3]);
  r = run(s, 'O');
  assert.deepEqual([r.state.text, r.state.cursor], ['\nab\ncd', 0]);
  // typing happens in the textarea; Esc takes its text and caret back (cursor one left)
  const i = run(s, 'A').state;
  const back = viLeaveInsert(i, 'abXY\ncd', 4);
  assert.deepEqual([back.mode, back.text, back.cursor], ['normal', 'abXY\ncd', 3]);
  assert.equal(viLeaveInsert(run(s, '0', 'i').state, 'ab\ncd', 0).cursor, 0, 'never left of the line start');
});

test('x dd yy p P edit through a register (characters / lines)', () => {
  const s = createVi('one\ntwo\nthree', { cursor: 4 }); // on 't' of two
  let r = run(s, 'x');
  assert.deepEqual([r.state.text, at(r.state)], ['one\nwo\nthree', 'w']);
  r = run(r.state, 'p'); // the deleted 't' after the cursor
  assert.equal(r.state.text, 'one\nwto\nthree');
  r = run(s, 'd', 'd');
  assert.deepEqual([r.state.text, at(r.state)], ['one\nthree', 't']);
  r = run(r.state, 'p'); // the line below
  assert.equal(r.state.text, 'one\nthree\ntwo');
  r = run(run(s, 'd', 'd').state, 'P'); // the line above
  assert.equal(r.state.text, 'one\ntwo\nthree');
  r = run(s, 'y', 'y', 'G', 'p');
  assert.deepEqual([r.state.text, r.state.cursor], ['one\ntwo\nthree\ntwo', 14]);
  // dd on the last line removes the line break before it; on a one-line text leaves it empty
  assert.equal(run(s, 'G', 'd', 'd').state.text, 'one\ntwo');
  assert.equal(run(createVi('only'), 'd', 'd').state.text, '');
  assert.equal(run(createVi(''), 'x').state.text, '', 'x on an empty text does nothing');
});

test('u undoes a whole insert session or an edit; Ctrl+r redoes', () => {
  const s = createVi('ab', { cursor: 0 });
  let st = run(s, 'A').state;
  st = viLeaveInsert(st, 'abcd', 4);
  st = run(st, 'x').state; // 'abc'
  assert.equal(st.text, 'abc');
  st = run(st, 'u').state;
  assert.equal(st.text, 'abcd');
  st = run(st, 'u').state;
  assert.equal(st.text, 'ab', 'the whole insert session at once');
  assert.equal(run(st, 'u').state.message, '더 되돌릴 것이 없습니다');
  st = run(st, 'C-r').state;
  assert.equal(st.text, 'abcd');
  st = run(st, 'C-r').state;
  assert.equal(st.text, 'abc');
  // an insert session that changed nothing leaves no undo step
  const n = viLeaveInsert(run(s, 'i').state, 'ab', 0);
  assert.equal(run(n, 'u').state.message, '더 되돌릴 것이 없습니다');
});

test('search: / asks for the pattern, n / N go on (wrapping), a miss is a message', () => {
  const s = createVi('alpha beta\nbeta gamma\nalpha', { cursor: 0 });
  assert.deepEqual(run(s, '/').action, { type: 'prompt', kind: '/' });
  let st = viSearch(s, 'beta').state;
  assert.equal(st.cursor, 6);
  st = run(st, 'n').state;
  assert.equal(st.cursor, 11);
  st = run(st, 'n').state;
  assert.equal(st.cursor, 6, 'wraps to the top');
  st = run(st, 'N').state;
  assert.equal(st.cursor, 11, 'N goes the other way (wrapping)');
  const miss = viSearch(s, '없는말').state;
  assert.deepEqual([miss.cursor, miss.message], [0, '패턴 없음: 없는말']);
  assert.equal(run(s, 'n').state.message, '검색어가 없습니다');
  assert.equal(viSearch(createVi('한글 메모 한글'), '한글').state.cursor, 6, 'Korean patterns (from the cursor on)');
});

test(':w :wq :q! :q ZZ save / save and leave / leave; unknown commands are a message', () => {
  const s = createVi('memo');
  assert.deepEqual(run(s, ':').action, { type: 'prompt', kind: ':' });
  assert.deepEqual(viCommand(s, 'w').action, { type: 'save' });
  assert.deepEqual(viCommand(s, ' wq ').action, { type: 'saveQuit' });
  assert.deepEqual(viCommand(s, 'x').action, { type: 'saveQuit' });
  assert.deepEqual(run(s, 'Z', 'Z').action, { type: 'saveQuit' });
  assert.deepEqual(viCommand(s, 'q!').action, { type: 'quit', force: true });
  assert.deepEqual(viCommand(s, 'q').action, { type: 'quit', force: false }, 'nothing changed: :q leaves');
  const changed = run(s, 'x').state;
  assert.equal(viDirty(changed), true);
  const q = viCommand(changed, 'q');
  assert.equal(q.action, undefined);
  assert.equal(q.state.message, '바뀐 내용이 있습니다 — 저장은 :w, 버리기는 :q!');
  assert.equal(viCommand(s, 'set nu').state.message, '알 수 없는 명령: set nu');
  assert.equal(viDirty(s), false);
});

test('Ctrl+d / Ctrl+u move half a page; Esc and Ctrl+[ cancel a pending operator', () => {
  const text = Array.from({ length: 30 }, (_, i) => `line ${i}`).join('\n');
  const s = createVi(text, { cursor: 0 });
  const line = (st) => st.text.slice(0, st.cursor).split('\n').length - 1;
  assert.equal(line(viKey(s, 'C-d', { rows: 20 }).state), 10);
  assert.equal(line(viKey(viKey(s, 'G').state, 'C-u', { rows: 20 }).state), 19);
  assert.equal(line(viKey(s, 'C-d').state), 5, 'without a size: 5 lines');
  for (const esc of ['Esc', 'C-[']) {
    const p = run(s, 'd', esc, 'd');
    assert.equal(p.state.text, text, `${esc} dropped the pending d`);
    assert.equal(run(s, 'd', esc).state.pending, '');
  }
  // keys that are not vi keys do nothing (and are reported as unused)
  assert.equal(viKey(s, 'q').state, s);
});
