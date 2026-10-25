import { test } from 'node:test';
import assert from 'node:assert/strict';
import { tabDropIndex, tabBarDrop, bodyDrop, zoneToSplit, strayEnded, isTabDrag, DRAG_MIME, STRAY_QUIET_MS } from '../../src/dnd.js';

const ids = ['a', 'b', 'c', 'd'];

test('tabDropIndex: same group, moving right past yourself subtracts one', () => {
  // drag a, drop after c -> raw 3 -> final 2
  assert.equal(tabDropIndex(ids, 'a', 'c', true), 2);
  // drag a, drop before c -> raw 2 -> final 1
  assert.equal(tabDropIndex(ids, 'a', 'c', false), 1);
  // drag a, drop after d -> end
  assert.equal(tabDropIndex(ids, 'a', 'd', true), 3);
});

test('tabDropIndex: same group, moving left keeps the raw index', () => {
  assert.equal(tabDropIndex(ids, 'd', 'b', false), 1);
  assert.equal(tabDropIndex(ids, 'd', 'b', true), 2);
  assert.equal(tabDropIndex(ids, 'c', 'a', false), 0);
});

test('tabDropIndex: no-op positions return null', () => {
  assert.equal(tabDropIndex(ids, 'b', 'b', false), null); // on itself
  assert.equal(tabDropIndex(ids, 'b', 'b', true), null);
  assert.equal(tabDropIndex(ids, 'b', 'c', false), null); // just before the next = same place
  assert.equal(tabDropIndex(ids, 'b', 'a', true), null); // just after the previous
  assert.equal(tabDropIndex(ids, 'd', null, false), null); // empty area = end, already last
});

test('tabDropIndex: cross group uses the raw index; empty area appends', () => {
  assert.equal(tabDropIndex(ids, 'x', 'b', false), 1);
  assert.equal(tabDropIndex(ids, 'x', 'b', true), 2);
  assert.equal(tabDropIndex(ids, 'x', 'd', true), 4);
  assert.equal(tabDropIndex(ids, 'x', null, false), 4);
  assert.equal(tabDropIndex([], 'x', null, false), 0);
});

test('tabDropIndex: empty area in the same group moves to the end (final position)', () => {
  assert.equal(tabDropIndex(ids, 'a', null, false), 3);
});

test('tabBarDrop builds MoveTab or nothing', () => {
  assert.deepEqual(tabBarDrop(ids, 'g1', 'a', 'c', true), { type: 'moveTab', tab: 'a', group: 'g1', index: 2 });
  assert.equal(tabBarDrop(ids, 'g1', 'b', 'b', true), null);
});

test('bodyDrop: center appends, edges split', () => {
  assert.deepEqual(bodyDrop(ids, 'g1', 'x', 'center'), { type: 'moveTab', tab: 'x', group: 'g1', index: 4 });
  assert.equal(bodyDrop(ids, 'g1', 'd', 'center'), null); // own group, already last
  assert.deepEqual(bodyDrop(ids, 'g1', 'a', 'center'), { type: 'moveTab', tab: 'a', group: 'g1', index: 3 });
  assert.deepEqual(bodyDrop(ids, 'g1', 'x', 'left'), { type: 'splitGroupWith', tab: 'x', group: 'g1', dir: 'horizontal', before: true });
  assert.deepEqual(bodyDrop(ids, 'g1', 'x', 'right'), { type: 'splitGroupWith', tab: 'x', group: 'g1', dir: 'horizontal', before: false });
  assert.deepEqual(bodyDrop(ids, 'g1', 'x', 'top'), { type: 'splitGroupWith', tab: 'x', group: 'g1', dir: 'vertical', before: true });
  assert.deepEqual(bodyDrop(ids, 'g1', 'x', 'bottom'), { type: 'splitGroupWith', tab: 'x', group: 'g1', dir: 'vertical', before: false });
});

test('zoneToSplit', () => {
  assert.equal(zoneToSplit('center'), null);
  assert.deepEqual(zoneToSplit('top'), { dir: 'vertical', before: true });
});

test('strayEnded: stray events during a live drag do not cancel it', () => {
  assert.equal(strayEnded({ now: 1000, lastDragEvent: 900, buttons: 0 }), false); // recent dragover
  assert.equal(strayEnded({ now: 1000, lastDragEvent: 0, buttons: 1 }), false); // button held
});

test('strayEnded: a real release after a lost dragend does', () => {
  assert.equal(strayEnded({ now: 1000 + STRAY_QUIET_MS, lastDragEvent: 1000, buttons: 0 }), true);
});

test('isTabDrag recognises only our MIME type', () => {
  assert.equal(isTabDrag([DRAG_MIME]), true);
  assert.equal(isTabDrag(['text/plain']), false);
  assert.equal(isTabDrag(null), false);
});
