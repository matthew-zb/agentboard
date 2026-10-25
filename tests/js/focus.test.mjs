import { test } from 'node:test';
import assert from 'node:assert/strict';
import { nextAttention, paneOrder, focusMatches, currentRef } from '../../src/focus.js';

const ref = (pane, tab = 't1', group = 'g1') => ({ group, tab, pane });

test('nextAttention cycles then returns to origin', () => {
  const order = [ref('a'), ref('b'), ref('c', 't2'), ref('d', 't3', 'g2')];
  // from a: first attention pane after a → b; origin remembered
  let s = nextAttention(order, order[0], new Set(['b', 'd']), null);
  assert.deepEqual(s, { target: order[1], origin: order[0] });
  // focusing b cleared its attention; next is d, origin kept
  s = nextAttention(order, order[1], new Set(['d']), s.origin);
  assert.deepEqual(s, { target: order[3], origin: order[0] });
  // nothing left → back to origin, origin cleared
  s = nextAttention(order, order[3], new Set(), s.origin);
  assert.deepEqual(s, { target: order[0], origin: null });
  // and again with no origin → nothing to do
  assert.deepEqual(nextAttention(order, order[0], new Set(), null), { target: null, origin: null });
});

test('nextAttention wraps around and skips the current pane', () => {
  const order = [ref('a'), ref('b'), ref('c')];
  assert.deepEqual(nextAttention(order, order[2], new Set(['a', 'c']), null), { target: order[0], origin: order[2] });
  // only the current pane needs attention → return to origin
  assert.deepEqual(nextAttention(order, order[1], new Set(['b']), order[0]), { target: order[0], origin: null });
  // current unknown in order → start from the beginning
  assert.deepEqual(nextAttention(order, ref('zz'), new Set(['c']), null), { target: order[2], origin: ref('zz') });
});

const ws = {
  groupLayout: { split: 'horizontal', sizes: [50, 50], children: [{ leaf: 'g1' }, { leaf: 'g2' }] },
  groups: {
    g1: { id: 'g1', name: 'control', tabIds: ['t1', 't2'], activeTabId: 't2' },
    g2: { id: 'g2', name: 'work', tabIds: ['t3'], activeTabId: 't3' },
  },
  tabs: {
    t1: { id: 't1', layout: { leaf: 'p1' }, lastFocusedPaneId: 'p1' },
    t2: { id: 't2', layout: { split: 'vertical', sizes: [50, 50], children: [{ leaf: 'p2' }, { leaf: 'p3' }] }, lastFocusedPaneId: 'p3' },
    t3: { id: 't3', layout: { leaf: 'p4' }, lastFocusedPaneId: 'p4' },
  },
  panes: { p1: {}, p2: {}, p3: {}, p4: {} },
  focus: { groupId: 'g1' },
};

test('paneOrder walks groups → tabs → panes in reading order', () => {
  assert.deepEqual(paneOrder(ws), [ref('p1', 't1'), ref('p2', 't2'), ref('p3', 't2'), ref('p4', 't3', 'g2')]);
  assert.deepEqual(currentRef(ws), ref('p3', 't2'));
});

test('focusMatches: a pending focus request is satisfied only by a matching snapshot', () => {
  assert.equal(focusMatches(ws, { group: 'g1', tab: 't2', pane: 'p3' }), true);
  assert.equal(focusMatches(ws, { group: 'g1', tab: null, pane: null }), true);
  assert.equal(focusMatches(ws, { group: 'g1', tab: 't2', pane: 'p2' }), false);
  assert.equal(focusMatches(ws, { group: 'g1', tab: 't1', pane: null }), false);
  assert.equal(focusMatches(ws, { group: 'g2', tab: null, pane: null }), false);
});
