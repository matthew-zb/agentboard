import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createModalStack } from '../../src/keys.js';

const log = [];
const mk = (name, ret) => (e) => (log.push(name), typeof ret === 'function' ? ret(e) : ret);

test('top modal gets the key; consumed result reported', () => {
  const s = createModalStack();
  log.length = 0;
  s.push(mk('a', true));
  s.push(mk('b', false));
  assert.deepEqual(s.handle({}), { handled: true, consumed: false });
  assert.deepEqual(log, ['b']);
});

test('null falls through to the next modal, then to the app', () => {
  const s = createModalStack();
  log.length = 0;
  s.push(mk('a', true));
  s.push(mk('b', null));
  assert.deepEqual(s.handle({}), { handled: true, consumed: true });
  assert.deepEqual(log, ['b', 'a']);
  const t = createModalStack();
  t.push(() => null);
  assert.deepEqual(t.handle({}), { handled: false, consumed: false });
  assert.deepEqual(createModalStack().handle({}), { handled: false, consumed: false });
});

test('push/pop order and popping a non-top token', () => {
  const s = createModalStack();
  log.length = 0;
  const a = s.push(mk('a', true));
  const b = s.push(mk('b', true));
  const c = s.push(mk('c', true));
  assert.equal(s.pop(b), true); // not the top
  s.handle({});
  assert.deepEqual(log, ['c']);
  assert.equal(s.pop(b), false); // already gone
  assert.equal(s.pop(c), true);
  log.length = 0;
  s.handle({});
  assert.deepEqual(log, ['a']);
  assert.equal(s.size, 1);
  assert.equal(s.pop(a), true);
  assert.equal(s.size, 0);
});

test('overlay over copy mode restores copy mode', () => {
  const s = createModalStack();
  log.length = 0;
  s.push(mk('copy', true));
  const ov = s.push(mk('overlay', true));
  s.handle({});
  s.pop(ov);
  s.handle({});
  assert.deepEqual(log, ['overlay', 'copy']);
});

test('legacy setModal slot replaces itself and does not disturb others', () => {
  const s = createModalStack();
  log.length = 0;
  s.push(mk('copy', true));
  s.setLegacy(mk('x', true));
  s.setLegacy(mk('y', true));
  s.handle({});
  s.setLegacy(null);
  s.handle({});
  assert.deepEqual(log, ['y', 'copy']);
  assert.equal(s.size, 1);
});
