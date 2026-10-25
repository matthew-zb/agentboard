import { test } from 'node:test';
import assert from 'node:assert/strict';
import { leafIds, containsLeaf, normalizeSizes, dragSizes, pxToPct, MIN_SIZE } from '../../src/layout.js';
import { tabHasAttention } from '../../src/tabs.js';
import { statusSymbol, warnTitle, isDead, attachAction, keepRestartMark, staleIds, sizeChanged } from '../../src/pane.js';
import {
  focusedPaneId,
  visibleGroupIds,
  visiblePaneIds,
  layoutSignature,
  tabGroups,
  statusParts,
  sidebarTree,
} from '../../src/groups.js';

const leaf = (id) => ({ leaf: id });
const split = (dir, sizes, children) => ({ split: dir, sizes, children });
const close = (a, b) => a.every((x, i) => Math.abs(x - b[i]) < 1e-9);

test('leafIds / containsLeaf walk in reading order', () => {
  const n = split('horizontal', [60, 40], [leaf('a'), split('vertical', [50, 50], [leaf('b'), leaf('c')])]);
  assert.deepEqual(leafIds(n), ['a', 'b', 'c']);
  assert.equal(containsLeaf(n, 'c'), true);
  assert.equal(containsLeaf(n, 'z'), false);
  assert.equal(containsLeaf(n, null), false);
  assert.deepEqual(leafIds(leaf('x')), ['x']);
});

test('normalizeSizes mirrors layout.rs: bad → equal, scaled to 100', () => {
  assert.ok(close(normalizeSizes([60, 40], 2), [60, 40]));
  assert.ok(close(normalizeSizes([3, 1], 2), [75, 25]));
  assert.ok(close(normalizeSizes([50], 2), [50, 50]));
  assert.ok(close(normalizeSizes([50, 0], 2), [50, 50]));
  assert.ok(close(normalizeSizes(undefined, 3), [100 / 3, 100 / 3, 100 / 3]));
  assert.deepEqual(normalizeSizes([], 0), []);
});

test('dragSizes moves one border and clamps to MIN_SIZE', () => {
  assert.deepEqual(dragSizes([50, 50], 0, 10), [60, 40]);
  assert.deepEqual(dragSizes([50, 50], 0, -10), [40, 60]);
  assert.deepEqual(dragSizes([50, 50], 0, 80), [100 - MIN_SIZE, MIN_SIZE]);
  assert.deepEqual(dragSizes([50, 50], 0, -80), [MIN_SIZE, 100 - MIN_SIZE]);
  assert.deepEqual(dragSizes([30, 30, 40], 1, 10), [30, 40, 30]); // others untouched
  assert.deepEqual(dragSizes([50, 50], 1, 10), [50, 50]); // no border after the last child
  const s = [50, 50];
  dragSizes(s, 0, 10);
  assert.deepEqual(s, [50, 50]); // input not mutated
});

test('pxToPct converts against the shared space', () => {
  assert.equal(pxToPct(50, 500), 10);
  assert.equal(pxToPct(10, 0), 0);
});

test('statusSymbol maps every state', () => {
  assert.equal(statusSymbol({ status: { state: 'running' } }).label, '●');
  assert.equal(statusSymbol({ status: { state: 'exited', code: 3 } }).label, '○ 3');
  assert.equal(statusSymbol({ status: { state: 'starting' } }).label, '◌');
  const f = statusSymbol({ status: { state: 'failed', message: '작업 폴더가 없습니다' } });
  assert.equal(f.symbol, '✕');
  assert.match(f.title, /작업 폴더가 없습니다/);
});

test('warnTitle explains attention and prompt warning', () => {
  assert.equal(warnTitle({ attention: false, promptWarning: false }), null);
  assert.match(warnTitle({ attention: true, promptWarning: false }), /알림/);
  assert.match(warnTitle({ attention: false, promptWarning: true }), /프롬프트/);
  assert.equal(warnTitle({ attention: true, promptWarning: true }).split('\n').length, 2);
});

test('isDead is exited or failed', () => {
  assert.equal(isDead({ status: { state: 'exited', code: 0 } }), true);
  assert.equal(isDead({ status: { state: 'failed', message: 'x' } }), true);
  assert.equal(isDead({ status: { state: 'running' } }), false);
  assert.equal(isDead(null), false);
});

test('attachAction: attach once, re-attach after restart, never while starting/failed', () => {
  assert.equal(attachAction(null, 'starting', false, false), 'none');
  assert.equal(attachAction('starting', 'running', false, false), 'attach');
  assert.equal(attachAction('running', 'running', true, false), 'none');
  assert.equal(attachAction('running', 'starting', true, false), 'none');
  assert.equal(attachAction('starting', 'running', true, false), 'reattach');
  assert.equal(attachAction('exited', 'running', true, false), 'reattach');
  assert.equal(attachAction('running', 'running', true, true), 'reattach'); // starting snapshot skipped
  assert.equal(attachAction(null, 'exited', false, false), 'attach'); // ring still readable
  assert.equal(attachAction('running', 'exited', true, false), 'none');
  assert.equal(attachAction(null, 'failed', false, false), 'none');
});

test('attachAction: restarted process that exits before running is re-attached', () => {
  assert.equal(attachAction('starting', 'exited', true, false), 'reattach'); // starting → exited
  assert.equal(attachAction('running', 'exited', true, true), 'reattach'); // starting snapshot skipped
  assert.equal(attachAction('exited', 'exited', true, false), 'none');
});

test('keepRestartMark: consumed by any attach, dropped on a terminal state', () => {
  // never-attached failed pane restarted: starting keeps the mark, first attach consumes it
  assert.equal(keepRestartMark('starting', attachAction('failed', 'starting', false, true), true), true);
  assert.equal(keepRestartMark('running', attachAction('starting', 'running', false, true), true), false);
  // reattach consumes it
  assert.equal(keepRestartMark('running', 'reattach', true), false);
  // restart ends failed / exited without attaching
  assert.equal(keepRestartMark('failed', attachAction('starting', 'failed', false, true), true), false);
  assert.equal(keepRestartMark('exited', 'none', true), false);
  assert.equal(keepRestartMark('starting', 'none', false), false);
  // after the mark is dropped, an unrelated running snapshot does not reset the terminal
  assert.equal(attachAction('running', 'running', true, false), 'none');
});

test('staleIds lists panes missing from the snapshot', () => {
  assert.deepEqual(staleIds(['p1', 'p2', 'p3'], { p1: {}, p3: {} }), ['p2']);
  assert.deepEqual(staleIds([], { p1: {} }), []);
});

test('sizeChanged only on a real change', () => {
  assert.equal(sizeChanged(null, 80, 24), true);
  assert.equal(sizeChanged({ cols: 80, rows: 24 }, 80, 24), false);
  assert.equal(sizeChanged({ cols: 80, rows: 24 }, 81, 24), true);
  assert.equal(sizeChanged({ cols: 80, rows: 24 }, 80, 25), true);
});

const pane = (id, tab, name, extra = {}) => ({
  id,
  name,
  tab,
  agent: 'claude',
  status: { state: 'running' },
  attention: false,
  promptWarning: false,
  ...extra,
});

function fixture() {
  return {
    groupLayout: split('horizontal', [30, 70], [leaf('g1'), leaf('g2')]),
    groups: {
      g1: { id: 'g1', name: 'control', tabIds: ['t1'], activeTabId: 't1' },
      g2: { id: 'g2', name: 'work', tabIds: ['t2', 't3'], activeTabId: 't2' },
    },
    tabs: {
      t1: { id: 't1', name: 'dispatcher', isDispatcher: true, layout: leaf('p1'), lastFocusedPaneId: 'p1', maximizedPaneId: null, syncInput: false },
      t2: {
        id: 't2',
        name: 'api',
        isDispatcher: false,
        layout: split('horizontal', [60, 40], [leaf('p2'), split('vertical', [50, 50], [leaf('p3'), leaf('p4')])]),
        lastFocusedPaneId: 'p3',
        maximizedPaneId: null,
        syncInput: true,
      },
      t3: { id: 't3', name: 'web', isDispatcher: false, layout: leaf('p5'), lastFocusedPaneId: 'p5', maximizedPaneId: null, syncInput: false },
    },
    panes: {
      p1: pane('p1', 't1', 'dispatcher'),
      p2: pane('p2', 't2', 'lead'),
      p3: pane('p3', 't2', 'coder'),
      p4: pane('p4', 't2', 'tests', { status: { state: 'exited', code: 1 } }),
      p5: pane('p5', 't3', 'web', { attention: true }),
    },
    focus: { groupId: 'g2' },
    groupMaximized: null,
  };
}

test('focusedPaneId follows group → active tab → lastFocusedPaneId', () => {
  const ws = fixture();
  assert.equal(focusedPaneId(ws), 'p3');
  ws.focus.groupId = 'g1';
  assert.equal(focusedPaneId(ws), 'p1');
  ws.focus.groupId = 'nope';
  assert.equal(focusedPaneId(ws), null);
});

test('visible panes: active tabs only; maximize narrows', () => {
  const ws = fixture();
  assert.deepEqual(visibleGroupIds(ws), ['g1', 'g2']);
  assert.deepEqual(visiblePaneIds(ws), ['p1', 'p2', 'p3', 'p4']);
  ws.tabs.t2.maximizedPaneId = 'p4';
  assert.deepEqual(visiblePaneIds(ws), ['p1', 'p4']);
  ws.groupMaximized = 'g2';
  assert.deepEqual(visibleGroupIds(ws), ['g2']);
  assert.deepEqual(visiblePaneIds(ws), ['p4']);
  ws.groupMaximized = 'gone'; // not in the layout → ignored
  assert.deepEqual(visibleGroupIds(ws), ['g1', 'g2']);
});

test('layoutSignature ignores status/focus/names, tracks layout and active tabs', () => {
  const ws = fixture();
  const base = layoutSignature(ws);
  ws.panes.p2.status = { state: 'exited', code: 0 };
  ws.focus.groupId = 'g1';
  ws.tabs.t2.lastFocusedPaneId = 'p2';
  ws.groups.g2.name = 'renamed';
  assert.equal(layoutSignature(ws), base);
  ws.groups.g2.activeTabId = 't3';
  const switched = layoutSignature(ws);
  assert.notEqual(switched, base);
  ws.groups.g2.activeTabId = 't2';
  ws.tabs.t2.layout.sizes = [70, 30];
  assert.notEqual(layoutSignature(ws), base);
  ws.tabs.t2.layout.sizes = [60, 40];
  ws.tabs.t2.maximizedPaneId = 'p2';
  assert.notEqual(layoutSignature(ws), base);
});

test('tabGroups maps tabs to their group', () => {
  assert.deepEqual(tabGroups(fixture()), { t1: 'g1', t2: 'g2', t3: 'g2' });
});

test('statusParts: path, sync flag, error count', () => {
  const ws = fixture();
  assert.deepEqual(statusParts({ workspace: ws, errors: 2 }), { path: 'work › api › coder', sync: true, errors: 2 });
  ws.focus.groupId = 'g1';
  assert.deepEqual(statusParts({ workspace: ws }), { path: 'control › dispatcher › dispatcher', sync: false, errors: 0 });
});

test('tabHasAttention: any pane with attention or prompt warning', () => {
  const ws = fixture();
  assert.equal(tabHasAttention(ws.tabs.t3, ws.panes), true);
  assert.equal(tabHasAttention(ws.tabs.t2, ws.panes), false);
  ws.panes.p4.promptWarning = true;
  assert.equal(tabHasAttention(ws.tabs.t2, ws.panes), true);
});

test('sidebarTree: groups > tabs > panes with marks and symbols', () => {
  const tree = sidebarTree(fixture());
  assert.deepEqual(
    tree.map((g) => [g.name, g.focused]),
    [
      ['control', false],
      ['work', true],
    ],
  );
  const work = tree[1];
  assert.deepEqual(
    work.tabs.map((t) => [t.name, t.active]),
    [
      ['api', true],
      ['web', false],
    ],
  );
  assert.deepEqual(
    work.tabs[0].panes.map((p) => `${p.symbol} ${p.name}`),
    ['● lead', '● coder', '○ tests'],
  );
});

// ---- Task 14: sync input fan-out
import { syncTargets } from '../../src/pane.js';

const wsSync = (on) => ({
  tabs: { t1: { id: 't1', syncInput: on }, t2: { id: 't2', syncInput: true } },
  panes: {
    a: { id: 'a', tab: 't1', status: { state: 'running' } },
    b: { id: 'b', tab: 't1', status: { state: 'running' } },
    c: { id: 'c', tab: 't1', status: { state: 'exited', code: 0 } },
    d: { id: 'd', tab: 't1', status: { state: 'starting' } },
    e: { id: 'e', tab: 't2', status: { state: 'running' } },
  },
});

test('syncTargets: null when sync is off or unknown pane', () => {
  assert.equal(syncTargets(wsSync(false), 'a'), null);
  assert.equal(syncTargets(wsSync(true), 'zz'), null);
  assert.equal(syncTargets(null, 'a'), null);
});

test('syncTargets: every running pane of the tab exactly once, no other tabs', () => {
  const t = syncTargets(wsSync(true), 'a');
  assert.deepEqual([...t].sort(), ['a', 'b']);
  assert.equal(new Set(t).size, t.length);
});
