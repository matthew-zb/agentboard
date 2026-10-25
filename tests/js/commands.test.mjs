import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { planCommand, resizeArgs, registerCommand, runCommand, hasCommand, PENDING_IDS, CORE_IDS } from '../../src/commands.js';

const leaf = (id) => ({ leaf: id });
const split = (dir, sizes, children) => ({ split: dir, sizes, children });

// g1 (left): t1 dispatcher [p1], t2 active [p2 | p3/p4]; g2 (right): t3 [p5]
function fixture() {
  return {
    groupLayout: split('horizontal', [50, 50], [leaf('g1'), leaf('g2')]),
    groups: {
      g1: { id: 'g1', name: 'control', tabIds: ['t1', 't2'], activeTabId: 't2' },
      g2: { id: 'g2', name: 'work', tabIds: ['t3'], activeTabId: 't3' },
    },
    tabs: {
      t1: { id: 't1', name: 'dispatcher', isDispatcher: true, layout: leaf('p1'), lastFocusedPaneId: 'p1', maximizedPaneId: null },
      t2: {
        id: 't2',
        name: 'api',
        isDispatcher: false,
        layout: split('horizontal', [60, 40], [leaf('p2'), split('vertical', [50, 50], [leaf('p3'), leaf('p4')])]),
        lastFocusedPaneId: 'p2',
        maximizedPaneId: null,
      },
      t3: { id: 't3', name: 'ha', isDispatcher: false, layout: leaf('p5'), lastFocusedPaneId: 'p5', maximizedPaneId: null },
    },
    panes: {
      p1: { id: 'p1', tab: 't1' },
      p2: { id: 'p2', tab: 't2' },
      p3: { id: 'p3', tab: 't2' },
      p4: { id: 'p4', tab: 't2' },
      p5: { id: 'p5', tab: 't3' },
    },
    focus: { groupId: 'g1' },
    lastLocation: { groupId: 'g2', tabId: 't3' },
    groupMaximized: null,
  };
}

const r = (id, left, top, right, bottom) => ({ id, left, top, right, bottom });
// visible: p2 | p3/p4 in g1 (0-600), p5 in g2 (604-1000)
const geo = {
  paneRects: [r('p2', 0, 30, 360, 700), r('p3', 364, 30, 600, 400), r('p4', 364, 404, 600, 700), r('p5', 604, 30, 1000, 700)],
  groupRects: [r('g1', 0, 0, 600, 700), r('g2', 604, 0, 1000, 700)],
};

test('pane.focus* uses rects and crosses group boundaries', () => {
  const ws = fixture();
  assert.deepEqual(planCommand('pane.focusRight', ws, geo), [{ type: 'setFocus', group: 'g1', tab: 't2', pane: 'p3' }]);
  ws.tabs.t2.lastFocusedPaneId = 'p4';
  assert.deepEqual(planCommand('pane.focusRight', ws, geo), [{ type: 'setFocus', group: 'g2', tab: 't3', pane: 'p5' }]);
  assert.deepEqual(planCommand('pane.focusUp', ws, geo), [{ type: 'setFocus', group: 'g1', tab: 't2', pane: 'p3' }]);
  assert.deepEqual(planCommand('pane.focusLeft', ws, geo), [{ type: 'setFocus', group: 'g1', tab: 't2', pane: 'p2' }]);
  assert.deepEqual(planCommand('pane.focusDown', ws, geo), []); // edge: nothing
});

test('tab.prev/next wrap inside the group; goto N; last; dispatcher', () => {
  const ws = fixture();
  assert.deepEqual(planCommand('tab.next', ws, geo), [{ type: 'setFocus', group: 'g1', tab: 't1', pane: null }]);
  assert.deepEqual(planCommand('tab.prev', ws, geo), [{ type: 'setFocus', group: 'g1', tab: 't1', pane: null }]);
  assert.deepEqual(planCommand('tab.goto1', ws, geo), [{ type: 'setFocus', group: 'g1', tab: 't1', pane: null }]);
  assert.deepEqual(planCommand('tab.goto2', ws, geo), []); // already there
  assert.deepEqual(planCommand('tab.goto9', ws, geo), []); // no such tab
  assert.deepEqual(planCommand('tab.last', ws, geo), [{ type: 'setFocus', group: 'g2', tab: 't3', pane: null }]);
  ws.lastLocation = null;
  assert.deepEqual(planCommand('tab.last', ws, geo), []);
  assert.deepEqual(planCommand('tab.dispatcher', ws, geo), [{ type: 'setFocus', group: 'g1', tab: 't1', pane: null }]);
});

test('group.gotoN focuses the Nth group in layout order; un-maximizes a hidden target', () => {
  const ws = fixture();
  assert.deepEqual(planCommand('group.goto2', ws, geo), [{ type: 'setFocus', group: 'g2', tab: null, pane: null }]);
  assert.deepEqual(planCommand('group.goto1', ws, geo), []);
  assert.deepEqual(planCommand('group.goto3', ws, geo), []);
  ws.groupMaximized = 'g1';
  assert.deepEqual(planCommand('group.goto2', ws, geo), [
    { type: 'setGroupMaximized', group: null },
    { type: 'setFocus', group: 'g2', tab: null, pane: null },
  ]);
});

test('split / close / maximize / restart target the focused pane', () => {
  const ws = fixture();
  assert.deepEqual(planCommand('pane.splitRight', ws, geo), [{ type: 'splitPane', pane: 'p2', dir: 'horizontal' }]);
  assert.deepEqual(planCommand('pane.splitDown', ws, geo), [{ type: 'splitPane', pane: 'p2', dir: 'vertical' }]);
  assert.deepEqual(planCommand('pane.close', ws, geo), [{ type: 'closePane', pane: 'p2' }]);
  assert.deepEqual(planCommand('pane.restart', ws, geo), [{ type: 'restartPane', pane: 'p2' }]);
  assert.deepEqual(planCommand('pane.maximize', ws, geo), [{ type: 'setMaximized', tab: 't2', pane: 'p2' }]);
  ws.tabs.t2.maximizedPaneId = 'p2';
  assert.deepEqual(planCommand('pane.maximize', ws, geo), [{ type: 'setMaximized', tab: 't2', pane: null }]);
});

test('group split / close / maximize', () => {
  const ws = fixture();
  assert.deepEqual(planCommand('group.splitRight', ws, geo), [
    { type: 'splitGroupWith', tab: 't2', group: 'g1', dir: 'horizontal', before: false },
  ]);
  assert.deepEqual(planCommand('group.splitDown', ws, geo), [
    { type: 'splitGroupWith', tab: 't2', group: 'g1', dir: 'vertical', before: false },
  ]);
  assert.deepEqual(planCommand('group.close', ws, geo), [{ type: 'closeGroup', group: 'g1' }]);
  assert.deepEqual(planCommand('group.maximize', ws, geo), [{ type: 'setGroupMaximized', group: 'g1' }]);
  ws.groupMaximized = 'g1';
  assert.deepEqual(planCommand('group.maximize', ws, geo), [{ type: 'setGroupMaximized', group: null }]);
});

test('resizeArgs moves the border in the key direction (tab border first, then group border)', () => {
  const ws = fixture();
  // p2 (left in tab): right border exists → H shrinks it (moves its right border left)
  assert.deepEqual(resizeArgs(ws, 'p2', 'left'), { side: 'right', delta: -5 });
  assert.deepEqual(resizeArgs(ws, 'p2', 'right'), { side: 'right', delta: 5 });
  // p3 (top right in tab): left border exists
  assert.deepEqual(resizeArgs(ws, 'p3', 'left'), { side: 'left', delta: 5 });
  assert.deepEqual(resizeArgs(ws, 'p3', 'down'), { side: 'down', delta: 5 });
  assert.deepEqual(resizeArgs(ws, 'p4', 'down'), { side: 'up', delta: -5 });
  // p5 is alone in its tab → group border (g2 has a left neighbour)
  assert.deepEqual(resizeArgs(ws, 'p5', 'left'), { side: 'left', delta: 5 });
  assert.deepEqual(resizeArgs(ws, 'p5', 'right'), { side: 'left', delta: -5 });
  // no border on that axis anywhere → plain (backend no-op)
  assert.deepEqual(resizeArgs(ws, 'p5', 'up'), { side: 'up', delta: 5 });
  assert.deepEqual(planCommand('pane.resizeLeft', ws, geo), [{ type: 'resizePane', pane: 'p2', side: 'right', delta: -5 }]);
});

test('tab.moveLeft/Right reorders, then crosses to the neighbour group at the bar end', () => {
  const ws = fixture();
  // t2 is index 1 in g1
  assert.deepEqual(planCommand('tab.moveLeft', ws, geo), [{ type: 'moveTab', tab: 't2', group: 'g1', index: 0 }]);
  // at the right end of g1's bar → g2, position 0
  assert.deepEqual(planCommand('tab.moveRight', ws, geo), [{ type: 'moveTab', tab: 't2', group: 'g2', index: 0 }]);
  ws.groups.g1.activeTabId = 't1';
  assert.deepEqual(planCommand('tab.moveRight', ws, geo), [{ type: 'moveTab', tab: 't1', group: 'g1', index: 1 }]);
  assert.deepEqual(planCommand('tab.moveLeft', ws, geo), []); // left end, no left group
  // from g2 (only tab) moving left → end of g1
  ws.focus.groupId = 'g2';
  assert.deepEqual(planCommand('tab.moveLeft', ws, geo), [{ type: 'moveTab', tab: 't3', group: 'g1', index: 2 }]);
  assert.deepEqual(planCommand('tab.moveRight', ws, geo), []);
});

test('tab.moveUp/Down sends the tab to the group above/below', () => {
  const ws = fixture();
  const stacked = { paneRects: geo.paneRects, groupRects: [r('g1', 0, 0, 1000, 340), r('g2', 0, 344, 1000, 700)] };
  assert.deepEqual(planCommand('tab.moveDown', ws, stacked), [{ type: 'moveTab', tab: 't2', group: 'g2', index: 1 }]);
  assert.deepEqual(planCommand('tab.moveUp', ws, stacked), []);
});

test('pending ids plan nothing; unknown ids are not core commands', () => {
  const ws = fixture();
  for (const id of PENDING_IDS) assert.deepEqual(planCommand(id, ws, geo), [], id);
  assert.equal(planCommand('no.such', ws, geo), null);
  assert.deepEqual(planCommand('pane.close', null, geo), []);
});

test('command registry: later tasks plug in by id', () => {
  let got = null;
  assert.equal(hasCommand('test.cmd'), false);
  registerCommand('test.cmd', (ctx) => (got = ctx));
  assert.equal(hasCommand('test.cmd'), true);
  assert.equal(runCommand('test.cmd', { a: 1 }), true);
  assert.deepEqual(got, { a: 1 });
  assert.equal(runCommand('test.none', {}), false); // unregistered → swallowed no-op
});

test('every spec id is either a core command or a pending one', () => {
  const file = new URL('../../src-tauri/defaults/keybindings.json', import.meta.url);
  const ids = Object.keys(JSON.parse(readFileSync(file, 'utf8'))).sort();
  assert.deepEqual([...CORE_IDS, ...PENDING_IDS].sort(), ids);
  const ws = fixture();
  for (const id of CORE_IDS) assert.ok(Array.isArray(planCommand(id, ws, geo)), id);
});
