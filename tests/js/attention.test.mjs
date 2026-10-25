import { test } from 'node:test';
import assert from 'node:assert/strict';
import { hiddenAttentionTabs, statusEntryText, REASON_SHOW } from '../../src/attention-logic.js';
import { warnTitle } from '../../src/pane.js';
import { tabAttentionTitle } from '../../src/tabs.js';

const leaf = (id) => ({ leaf: id });
const split = (dir, sizes, children) => ({ split: dir, sizes, children });

/** Two groups side by side: g1 [dispatcher, billing], g2 [api, ops]. */
function snap(over = {}) {
  const pane = (id, tab, extra = {}) => ({ id, name: id, tab, attention: false, promptWarning: false, ...extra });
  const tab = (id, name, layout, last) => ({ id, name, layout, lastFocusedPaneId: last, maximizedPaneId: null });
  const ws = {
    groupLayout: split('horizontal', [50, 50], [leaf('g1'), leaf('g2')]),
    groups: {
      g1: { id: 'g1', name: 'main', tabIds: ['t1', 't2'], activeTabId: 't1' },
      g2: { id: 'g2', name: 'side', tabIds: ['t3', 't4'], activeTabId: 't3' },
    },
    tabs: {
      t1: tab('t1', 'dispatcher', leaf('p1'), 'p1'),
      t2: tab('t2', 'billing', split('vertical', [50, 50], [leaf('p2'), leaf('p5')]), 'p2'),
      t3: tab('t3', 'api', leaf('p3'), 'p3'),
      t4: tab('t4', 'ops', leaf('p4'), 'p4'),
    },
    panes: {
      p1: pane('p1', 't1'),
      p2: pane('p2', 't2'),
      p5: pane('p5', 't2'),
      p3: pane('p3', 't3'),
      p4: pane('p4', 't4'),
    },
    focus: { groupId: 'g1' },
    groupMaximized: null,
    ...over,
  };
  return { workspace: ws, errors: 0, version: 1 };
}

test('hidden_attention_tabs: nothing hidden while every group is on screen', () => {
  const s = snap();
  s.workspace.panes.p4.attention = true;
  s.workspace.panes.p2.attention = true;
  assert.deepEqual(hiddenAttentionTabs(s), []);
});

test('hidden_attention_tabs: a maximized group hides the other groups’ ⚠ tabs, in reading order', () => {
  const s = snap({ groupMaximized: 'g1' });
  const ps = s.workspace.panes;
  ps.p4.attention = true;
  ps.p4.attentionReason = '배포 승인 필요';
  ps.p3.attention = true; // bell: no reason
  ps.p2.attention = true; // g1 is on screen: its tab bar shows the ⚠ itself
  ps.p1.promptWarning = true; // prompt warnings are not "needs the user"
  assert.deepEqual(hiddenAttentionTabs(s), [
    { group: 'g2', tab: 't3', name: 'api', pane: 'p3' },
    { group: 'g2', tab: 't4', name: 'ops', pane: 'p4', reason: '배포 승인 필요' },
  ]);
  // also accepts a bare workspace; the reason comes from the first flagged pane that has one
  s.workspace.groupMaximized = 'g2';
  ps.p5.attention = true;
  ps.p5.attentionReason = '질문 있음';
  assert.deepEqual(hiddenAttentionTabs(s.workspace), [{ group: 'g1', tab: 't2', name: 'billing', pane: 'p2', reason: '질문 있음' }]);
  assert.deepEqual(hiddenAttentionTabs(null), []);
});

test('hidden_attention_tabs: a stale groupMaximized (no such group) hides nothing', () => {
  const s = snap({ groupMaximized: 'g9' });
  s.workspace.panes.p4.attention = true;
  assert.deepEqual(hiddenAttentionTabs(s), []);
});

test('statusEntryText truncates the reason; the full text stays for the tooltip', () => {
  assert.equal(statusEntryText({ name: 'api' }), 'api');
  assert.equal(statusEntryText({ name: 'billing', reason: '짧은 사유' }), 'billing(짧은 사유)');
  const long = '가'.repeat(REASON_SHOW + 5);
  assert.equal(statusEntryText({ name: 'b', reason: long }), `b(${'가'.repeat(REASON_SHOW)}…)`);
});

test('⚠ tooltips show the reason', () => {
  assert.equal(warnTitle({ attention: true, attentionReason: '배포 승인 필요' }), '주의: 배포 승인 필요');
  assert.equal(warnTitle({ attention: true }), '주의: 이 창에서 알림(벨)이 왔습니다');
  assert.equal(warnTitle({ attention: false, attentionReason: null }), null);
  const panes = { a: { id: 'a', name: 'pane-1', attention: true, attentionReason: '질문' }, b: { id: 'b', name: 'pane-2', attention: true } };
  const t = { layout: split('vertical', [50, 50], [leaf('a'), leaf('b')]) };
  assert.equal(tabAttentionTitle(t, panes), '주의가 필요한 창이 있습니다\npane-1: 질문');
});
