import { test } from 'node:test';
import assert from 'node:assert/strict';
import { splitMemo, joinMemo, memoKeyAction, memoEol, normalizeEol } from '../../src/memo-logic.js';

// ---------------------------------------------------------------- MF1: split / join / keys

test('splitMemo: body starts after the first "## 메모" line', () => {
  const t = '# 메모\n작성 기준\n- a\n\n## 메모\n- 할 일 하나 — billing\n- 둘\n';
  const m = splitMemo(t);
  assert.equal(m.marker, true);
  assert.equal(m.header, '# 메모\n작성 기준\n- a\n\n## 메모\n');
  assert.equal(m.body, '- 할 일 하나 — billing\n- 둘\n');
  assert.equal(joinMemo(m.header, m.body, m.marker), t);
  // only the first marker line counts: a second one is body text
  const twice = splitMemo('## 메모\n- a\n## 메모\n- b\n');
  assert.equal(twice.body, '- a\n## 메모\n- b\n');
});

test('splitMemo: only an exact "## 메모" line (trimmed) is the marker', () => {
  assert.equal(splitMemo('## 메모장\n- x\n').marker, false);
  assert.equal(splitMemo('### 메모\n- x\n').marker, false);
  assert.equal(splitMemo('  ## 메모  \r\n- x\r\n').body, '- x\r\n');
  assert.equal(splitMemo('a\r\n## 메모\r\n- x\r\n').header, 'a\r\n## 메모\r\n');
  // marker on the last line without a line break: empty body; joining a new body adds the break
  const last = splitMemo('# h\r\n## 메모');
  assert.deepEqual([last.marker, last.header, last.body], [true, '# h\r\n## 메모', '']);
  assert.equal(joinMemo(last.header, '', last.marker), '# h\r\n## 메모');
  assert.equal(joinMemo(last.header, '- y\r\n', last.marker), '# h\r\n## 메모\r\n- y\r\n');
});

test('splitMemo: no marker → whole file is the body', () => {
  const m = splitMemo('- x\n');
  assert.deepEqual([m.marker, m.header, m.body], [false, '', '- x\n']);
  assert.equal(joinMemo(m.header, '- y\n', m.marker), '- y\n');
  assert.deepEqual(splitMemo(''), { header: '', body: '', marker: false });
  assert.deepEqual(splitMemo(null), { header: '', body: '', marker: false });
});

test('edited body keeps the file line endings (textarea values use LF)', () => {
  assert.equal(memoEol('a\r\nb\r\n'), '\r\n');
  assert.equal(memoEol('a\nb'), '\n');
  assert.equal(memoEol(''), '\n');
  assert.equal(normalizeEol('a\nb\r\nc\rd', '\r\n'), 'a\r\nb\r\nc\r\nd');
  assert.equal(normalizeEol('a\r\nb', '\n'), 'a\nb');
  const m = splitMemo('# h\r\n## 메모\r\n- x\r\n');
  const typed = '- x\n- y\n'; // what textarea.value returns
  assert.equal(joinMemo(m.header, normalizeEol(typed, memoEol(m.header + m.body)), m.marker), '# h\r\n## 메모\r\n- x\r\n- y\r\n');
});

const key = (code, extra = {}) => ({ code, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, isComposing: false, repeat: false, ...extra });

test('memo key map: e/r in read, Ctrl+Enter/Esc in edit, IME and modifiers ignored', () => {
  // read
  assert.equal(memoKeyAction(key('KeyE'), 'read'), 'edit');
  assert.equal(memoKeyAction(key('KeyR'), 'read'), 'reload');
  assert.equal(memoKeyAction(key('KeyJ'), 'read'), 'scrollDown');
  assert.equal(memoKeyAction(key('ArrowDown'), 'read'), 'scrollDown');
  assert.equal(memoKeyAction(key('KeyK'), 'read'), 'scrollUp');
  assert.equal(memoKeyAction(key('KeyG'), 'read'), 'top');
  assert.equal(memoKeyAction(key('KeyG', { shiftKey: true }), 'read'), 'bottom');
  assert.equal(memoKeyAction(key('KeyE', { shiftKey: true }), 'read'), null);
  assert.equal(memoKeyAction(key('KeyX'), 'read'), null);
  assert.equal(memoKeyAction(key('Enter', { ctrlKey: true }), 'read'), null);
  assert.equal(memoKeyAction(key('KeyJ', { repeat: true }), 'read'), 'scrollDown'); // scrolling repeats
  assert.equal(memoKeyAction(key('KeyE', { repeat: true }), 'read'), null); // a held e does not re-enter
  // edit: only save / cancel; every other key is typing
  assert.equal(memoKeyAction(key('Enter', { ctrlKey: true }), 'edit'), 'save');
  assert.equal(memoKeyAction(key('NumpadEnter', { ctrlKey: true }), 'edit'), 'save');
  assert.equal(memoKeyAction(key('Escape'), 'edit'), 'cancel');
  for (const c of ['KeyE', 'KeyR', 'KeyJ', 'Enter', 'Space', 'Backspace']) assert.equal(memoKeyAction(key(c), 'edit'), null, c);
  assert.equal(memoKeyAction(key('Enter', { ctrlKey: true, repeat: true }), 'edit'), null); // held Ctrl+Enter saves once
  // IME composition and app chords (Alt/Meta) are never memo keys
  assert.equal(memoKeyAction(key('KeyE', { isComposing: true }), 'read'), null);
  assert.equal(memoKeyAction(key('Escape', { isComposing: true }), 'edit'), null);
  assert.equal(memoKeyAction(key('Enter', { ctrlKey: true, isComposing: true }), 'edit'), null);
  assert.equal(memoKeyAction(key('KeyH', { altKey: true }), 'read'), null);
  assert.equal(memoKeyAction(key('Enter', { ctrlKey: true, altKey: true }), 'edit'), null);
  assert.equal(memoKeyAction(null, 'read'), null);
});
