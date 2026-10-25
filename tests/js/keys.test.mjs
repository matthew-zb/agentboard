import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { parseChord, buildKeymap, matchEvent, isPassthrough, chordKey, isRepeatable } from '../../src/keys.js';

const ev = (code, mods = {}, extra = {}) => ({
  code,
  key: '?',
  ctrlKey: !!mods.ctrl,
  altKey: !!mods.alt,
  shiftKey: !!mods.shift,
  metaKey: !!mods.meta,
  isComposing: false,
  ...extra,
});

test('matches Alt+h while Korean IME composing', () => {
  const km = buildKeymap({ 'pane.focusLeft': 'Alt+h' });
  assert.equal(
    matchEvent(km, { code: 'KeyH', key: 'ㅗ', altKey: true, ctrlKey: false, shiftKey: false, isComposing: true }),
    'pane.focusLeft',
  );
  // keyCode 229 / key 'Process' during composition does not matter either
  assert.equal(matchEvent(km, ev('KeyH', { alt: true }, { key: 'Process', keyCode: 229, isComposing: true })), 'pane.focusLeft');
});

test('modifiers must match exactly', () => {
  const km = buildKeymap({ 'pane.focusLeft': 'Alt+h', 'pane.resizeLeft': 'Alt+Shift+H', 'tab.prev': 'Ctrl+Alt+h' });
  assert.equal(matchEvent(km, ev('KeyH', { alt: true })), 'pane.focusLeft');
  assert.equal(matchEvent(km, ev('KeyH', { alt: true, shift: true })), 'pane.resizeLeft');
  assert.equal(matchEvent(km, ev('KeyH', { alt: true, ctrl: true })), 'tab.prev');
  assert.equal(matchEvent(km, ev('KeyH', { alt: true, ctrl: true, shift: true })), null);
  assert.equal(matchEvent(km, ev('KeyH')), null);
  assert.equal(matchEvent(km, ev('KeyH', { ctrl: true })), null);
  // Win key never matches
  assert.equal(matchEvent(km, ev('KeyH', { alt: true, meta: true })), null);
  // e.key is ignored: a Dvorak/IME key value with code KeyJ is not Alt+h
  assert.equal(matchEvent(km, ev('KeyJ', { alt: true }, { key: 'h' })), null);
  assert.equal(matchEvent(km, null), null);
  assert.equal(matchEvent(null, ev('KeyH', { alt: true })), null);
});

test('parseChord symbols', () => {
  assert.deepEqual(parseChord('Alt+\\'), { ctrl: false, alt: true, shift: false, code: 'Backslash' });
  assert.deepEqual(parseChord('Ctrl+Alt+-'), { ctrl: true, alt: true, shift: false, code: 'Minus' });
  assert.deepEqual(parseChord('Alt+;'), { ctrl: false, alt: true, shift: false, code: 'Semicolon' });
  assert.equal(parseChord('Alt+[').code, 'BracketLeft');
  assert.equal(parseChord('Alt+]').code, 'BracketRight');
  assert.equal(parseChord('Shift+tab').code, 'Tab');
  assert.equal(parseChord('Alt+1').code, 'Digit1');
  assert.equal(parseChord('Ctrl+Alt+9').code, 'Digit9');
  assert.deepEqual(parseChord('ctrl+alt+shift+h'), { ctrl: true, alt: true, shift: true, code: 'KeyH' });
  assert.deepEqual(parseChord('Alt+H'), parseChord('alt+h'));
  assert.equal(parseChord('Alt+Left').code, 'ArrowLeft');
  assert.equal(parseChord('Alt+Esc').code, 'Escape');
  assert.equal(parseChord('Alt+Escape').code, 'Escape');
  assert.equal(parseChord('Ctrl+Enter').code, 'Enter');
  assert.equal(parseChord('Alt+F5').code, 'F5');
  assert.equal(parseChord('Alt+=').code, 'Equal');
  assert.equal(parseChord('Alt+,').code, 'Comma');
  assert.equal(parseChord('Alt+/').code, 'Slash');
  assert.equal(parseChord('Ctrl+PageUp').code, 'PageUp');
  // code names are accepted too (passthroughKeys like "Ctrl+KeyR")
  assert.deepEqual(parseChord('Ctrl+KeyR'), parseChord('Ctrl+r'));
  assert.equal(parseChord('Alt+Digit3').code, 'Digit3');
  // invalid
  for (const bad of ['', 'Alt+', 'Alt+Shift', 'Alt+Q+W', 'Ctrl+Ctrl+a', 'Alt+ㅗ', 'Alt+F13', null, 42]) {
    assert.equal(parseChord(bad), null, String(bad));
  }
});

test('buildKeymap skips null and invalid bindings', () => {
  const km = buildKeymap({ 'pane.focusLeft': 'Alt+h', 'pane.breakToTab': null, bogus: 'Alt+Q+W' });
  assert.equal(km.size, 1);
  assert.equal(km.get(chordKey(parseChord('Alt+h'))), 'pane.focusLeft');
  assert.equal(buildKeymap(null).size, 0);
});

test('passthrough skips app handling', () => {
  const e = ev('KeyH', { alt: true }, { key: 'ㅗ', isComposing: true });
  assert.equal(isPassthrough(['Alt+h'], e), true);
  assert.equal(isPassthrough(['alt+H'], e), true);
  assert.equal(isPassthrough(['Alt+Shift+H'], e), false);
  assert.equal(isPassthrough(['Ctrl+KeyR'], ev('KeyR', { ctrl: true })), true);
  assert.equal(isPassthrough([], e), false);
  assert.equal(isPassthrough(undefined, e), false);
  assert.equal(isPassthrough(['garbage+'], e), false);
});

test('claude keys are never in default keymap', () => {
  const file = new URL('../../src-tauri/defaults/keybindings.json', import.meta.url);
  const bindings = JSON.parse(readFileSync(file, 'utf8'));
  const km = buildKeymap(bindings);
  // every non-null default parses
  const nonNull = Object.values(bindings).filter((v) => v != null);
  assert.equal(km.size, nonNull.length, 'every default binding parses and is unique');

  const reserved = [];
  for (const l of 'BFDYVMPTO') reserved.push(ev(`Key${l}`, { alt: true }));
  reserved.push(ev('ArrowUp', { alt: true }), ev('ArrowDown', { alt: true }));
  for (let c = 65; c <= 90; c++) reserved.push(ev(`Key${String.fromCharCode(c)}`, { ctrl: true }));
  reserved.push(ev('Enter', { ctrl: true }), ev('Tab', { shift: true }), ev('KeyV', { ctrl: true }));
  // Windows-global keys
  reserved.push(ev('Tab', { alt: true }), ev('F4', { alt: true }), ev('Space', { alt: true }), ev('Escape', { alt: true }));
  reserved.push(ev('Escape', { ctrl: true, shift: true }));
  for (const e of reserved) assert.equal(matchEvent(km, e), null, `${e.code} ctrl=${e.ctrlKey} alt=${e.altKey} shift=${e.shiftKey}`);
});

test('only movement and resize chords auto-repeat', () => {
  for (const id of ['pane.focusLeft', 'pane.resizeDown', 'tab.next', 'tab.prev']) assert.equal(isRepeatable(id), true, id);
  for (const id of ['pane.close', 'pane.splitRight', 'group.close', 'pane.restart', 'tab.moveLeft', 'pane.maximize'])
    assert.equal(isRepeatable(id), false, id);
});
