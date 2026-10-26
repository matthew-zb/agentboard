import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  dashboardRows,
  inboxOpenCounts,
  contextLabel,
  CONTEXT_CAP,
  shortText,
  visibleGroups,
  flatRows,
  dashboardKeyAction,
  jumpCommand,
  nightlySummary,
} from '../../src/dashboard-logic.js';

// ---- fixture: a snapshot with every kind of tab and pane (field names as serialized by Rust)
const leaf = (id) => ({ leaf: id });
const split = (dir, sizes, children) => ({ split: dir, sizes, children });
const pane = (id, tab, name, extra = {}) => ({
  id,
  tab,
  name,
  agent: 'claude',
  status: { state: 'running' },
  attention: false,
  promptWarning: false,
  kind: { type: 'terminal' },
  role: null,
  team: null,
  sessionName: null,
  attentionReason: null,
  ...extra,
});

function snapshot() {
  const tabs = {
    t1: { id: 't1', name: 'billing', isDispatcher: false, role: 'manager', layout: split('horizontal', [40, 30, 30], [leaf('m1'), leaf('sh1'), split('vertical', [50, 50], [leaf('bl'), leaf('bm2'), leaf('bm1')]), leaf('cl')]) },
    t0: { id: 't0', name: 'dispatcher', isDispatcher: true, role: 'dispatcher', layout: split('horizontal', [60, 40], [leaf('d'), split('vertical', [65, 35], [leaf('inbox'), leaf('memo')])]) },
    t2: { id: 't2', name: 'notes', isDispatcher: false, role: null, layout: leaf('n1') },
    t3: { id: 't3', name: 'handover', isDispatcher: false, role: 'handover', layout: leaf('h') },
    t4: { id: 't4', name: 'api', isDispatcher: false, role: 'manager', layout: leaf('m2') },
  };
  const panes = {
    d: pane('d', 't0', 'dispatcher', { role: 'dispatcher', sessionName: 'dispatcher' }),
    inbox: pane('inbox', 't0', 'inbox', { kind: { type: 'applet', kind: 'inbox' }, agent: '' }),
    memo: pane('memo', 't0', 'memo', { kind: { type: 'applet', kind: 'memo' }, agent: '' }),
    m1: pane('m1', 't1', 'manager', { role: 'manager', sessionName: 'billing', attention: true, attentionReason: '배포 승인 필요' }),
    sh1: pane('sh1', 't1', 'shell', { agent: 'powershell' }),
    bl: pane('bl', 't1', 'billing-a-lead', { role: 'lead', team: 'a', sessionName: 'billing-a-lead' }),
    bm2: pane('bm2', 't1', 'billing-a-m2', { role: 'member', team: 'a', sessionName: 'billing-a-m2', status: { state: 'exited', code: 0 } }),
    bm1: pane('bm1', 't1', 'billing-a-m1', { role: 'member', team: 'a', sessionName: 'billing-a-m1' }),
    cl: pane('cl', 't1', 'billing-b-lead', { role: 'lead', team: 'b', sessionName: 'billing-b-lead' }),
    n1: pane('n1', 't2', 'pane-1'),
    h: pane('h', 't3', 'handover', { role: 'handover', sessionName: 'handover' }),
    m2: pane('m2', 't4', 'manager', { role: 'manager', sessionName: 'api' }),
  };
  return {
    version: 7,
    workspace: {
      groups: {
        g1: { id: 'g1', name: 'control', tabIds: ['t0', 't3'], activeTabId: 't0' },
        g2: { id: 'g2', name: 'work', tabIds: ['t4', 't2', 't1'], activeTabId: 't4' },
      },
      groupLayout: split('horizontal', [35, 65], [leaf('g1'), leaf('g2')]),
      tabs,
      panes,
    },
  };
}

test('dashboardRows groups panes by tab and role', () => {
  const gs = dashboardRows(snapshot());
  // role tabs only: dispatcher, handover, then Manager tabs in reading order (groups → tabs)
  assert.deepEqual(gs.map((g) => [g.name, g.role]), [['dispatcher', 'dispatcher'], ['handover', 'handover'], ['api', 'manager'], ['billing', 'manager']]);
  const billing = gs.find((g) => g.name === 'billing');
  assert.deepEqual([billing.tab, billing.group], ['t1', 'g2']);
  // standing pane → each team's lead → its members (layout order); shell and applets left out
  assert.deepEqual(billing.rows.map((r) => [r.pane, r.role, r.team, r.depth]), [
    ['m1', 'manager', null, 0],
    ['bl', 'lead', 'a', 1],
    ['bm2', 'member', 'a', 2],
    ['bm1', 'member', 'a', 2],
    ['cl', 'lead', 'b', 1],
  ]);
  const m1 = billing.rows[0];
  assert.deepEqual([m1.name, m1.sessionName, m1.attention, m1.reason, m1.status.state], ['manager', 'billing', true, '배포 승인 필요', 'running']);
  assert.deepEqual(gs[0].rows.map((r) => r.pane), ['d'], 'inbox / memo applets are not rows');
  assert.equal(billing.rows.find((r) => r.pane === 'bm2').status.code, 0);
  // a bare workspace works too; garbage gives nothing
  assert.equal(dashboardRows(snapshot().workspace).length, 4);
  assert.deepEqual(dashboardRows(null), []);
  assert.deepEqual(dashboardRows({ workspace: {} }), []);
});

test('inbox open counts per owner tab', () => {
  const it = (ownerName, state, read = true) => ({ id: `${ownerName}-${state}-${read}`, ownerName, state, read });
  const items = [it('billing', 'new', false), it('billing', 'new'), it('billing', 'in_progress'), it('billing', 'close_proposed'), it('billing', 'done'), it('api', 'done'), it('gone', 'in_progress')];
  const c = inboxOpenCounts(items);
  assert.deepEqual(c.billing, { new: 2, unread: 1, inProgress: 1, closeProposed: 1, open: 4 });
  assert.equal(c.api, undefined, 'only done items: nothing open');
  assert.deepEqual(c.gone, { new: 0, unread: 0, inProgress: 1, closeProposed: 0, open: 1 });
  assert.deepEqual(inboxOpenCounts(null), {});
  // rows carry their tab's counts (owner = tab name, R29); a tab without items gets zeros
  const gs = dashboardRows(snapshot(), { inbox: items });
  assert.deepEqual(gs.find((g) => g.name === 'billing').inbox, c.billing);
  assert.deepEqual(gs.find((g) => g.name === 'api').inbox, { new: 0, unread: 0, inProgress: 0, closeProposed: 0, open: 0 });
});

test('board fields merge by pane id and tolerate missing entries', () => {
  const board = {
    panes: {
      m1: { idle: false, lastPrompt: '결제 배치 오류 확인해', lastReply: '확인 중입니다', contextTokens: 123456, updatedAt: 1792974000000 },
      bl: { idle: true, lastPrompt: null, lastReply: null, contextTokens: null, updatedAt: 1792974000001 },
      zz: { idle: true, lastPrompt: 'x', lastReply: 'y', contextTokens: 1, updatedAt: 1 }, // pane no longer in the snapshot
    },
    nightly: { lastRun: '2026-10-26', phase: 'idle', pending: [], held: [] },
  };
  const rows = flatRows(dashboardRows(snapshot(), { board }));
  const by = Object.fromEntries(rows.map((r) => [r.pane, r]));
  assert.deepEqual([by.m1.idle, by.m1.lastPrompt, by.m1.lastReply, by.m1.updatedAt], [false, '결제 배치 오류 확인해', '확인 중입니다', 1792974000000]);
  assert.deepEqual(by.m1.context, { text: '123k', over: false });
  assert.deepEqual([by.bl.idle, by.bl.lastPrompt, by.bl.context], [true, null, null]);
  // no board entry (no hook yet) or no board at all: unknown, not idle
  assert.deepEqual([by.d.idle, by.d.lastPrompt, by.d.context, by.d.updatedAt], [null, null, null, null]);
  for (const b of [null, {}, { panes: null }, { panes: { m1: null } }]) {
    const r = flatRows(dashboardRows(snapshot(), { board: b })).find((x) => x.pane === 'm1');
    assert.equal(r.idle, null, JSON.stringify(b));
  }
  assert.ok(!rows.some((r) => r.pane === 'zz'), 'board entries without a pane are ignored');
});

test('context tokens format (k) and over-400k flag', () => {
  assert.equal(CONTEXT_CAP, 400000);
  assert.deepEqual(contextLabel(123456), { text: '123k', over: false });
  assert.deepEqual(contextLabel(999), { text: '1k', over: false });
  assert.deepEqual(contextLabel(0), { text: '0k', over: false });
  assert.deepEqual(contextLabel(400000), { text: '400k', over: false });
  assert.deepEqual(contextLabel(400001), { text: '400k', over: true });
  assert.deepEqual(contextLabel(812345), { text: '812k', over: true });
  for (const v of [null, undefined, NaN, -1, '123', Infinity]) assert.equal(contextLabel(v), null, String(v));
});

test('peer-message prompt summary passes through', () => {
  // the backend already shortens peer / app messages to "메시지: <from-name>"; the view keeps it
  assert.equal(shortText('메시지: billing-a-lead'), '메시지: billing-a-lead');
  assert.equal(shortText('메시지: agentboard'), '메시지: agentboard');
  // display shortening: whitespace folded to one line, 60 characters (code points) + …
  assert.equal(shortText('  첫 줄\n\n둘째   줄\t끝 '), '첫 줄 둘째 줄 끝');
  const long = '가'.repeat(70);
  assert.equal(shortText(long), `${'가'.repeat(60)}…`);
  assert.equal(shortText('a'.repeat(60)), 'a'.repeat(60));
  assert.equal(shortText('abcdef', 3), 'abc…');
  for (const v of [null, undefined, '', '   ']) assert.equal(shortText(v), null, JSON.stringify(v));
  // rows show the shortened text
  const board = { panes: { m1: { idle: false, lastPrompt: long, lastReply: '메시지: billing-b-lead', contextTokens: null, updatedAt: 1 } } };
  const m1 = flatRows(dashboardRows(snapshot(), { board })).find((r) => r.pane === 'm1');
  assert.deepEqual([m1.lastPrompt, m1.lastReply], [`${'가'.repeat(60)}…`, '메시지: billing-b-lead']);
});

test('idle / ended panes can be hidden (h); tab headers stay', () => {
  const board = { panes: { m1: { idle: true, lastPrompt: null, lastReply: null, contextTokens: null, updatedAt: 1 }, bl: { idle: false, lastPrompt: null, lastReply: null, contextTokens: null, updatedAt: 1 } } };
  const gs = dashboardRows(snapshot(), { board });
  assert.equal(visibleGroups(gs, false), gs);
  const v = visibleGroups(gs, true);
  const billing = v.find((g) => g.name === 'billing');
  // idle m1 and exited bm2 hidden; unknown (no board entry) panes stay
  assert.deepEqual(billing.rows.map((r) => r.pane), ['bl', 'bm1', 'cl']);
  assert.equal(billing.hidden, 2);
  assert.equal(v.length, gs.length, 'every tab header stays');
  assert.deepEqual(flatRows(v).map((r) => r.pane), ['d', 'h', 'm2', 'bl', 'bm1', 'cl']);
});

const key = (code, extra = {}) => ({ code, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, isComposing: false, repeat: false, ...extra });

test('dashboard key map: j/k/Enter/r/h', () => {
  assert.equal(dashboardKeyAction(key('KeyJ')), 'down');
  assert.equal(dashboardKeyAction(key('ArrowDown')), 'down');
  assert.equal(dashboardKeyAction(key('KeyK')), 'up');
  assert.equal(dashboardKeyAction(key('ArrowUp')), 'up');
  assert.equal(dashboardKeyAction(key('Enter')), 'jump');
  assert.equal(dashboardKeyAction(key('NumpadEnter')), 'jump');
  assert.equal(dashboardKeyAction(key('KeyR')), 'reload');
  assert.equal(dashboardKeyAction(key('KeyH')), 'toggleIdle');
  // held keys repeat only the movement; chords and IME composition are never dashboard keys
  assert.equal(dashboardKeyAction(key('KeyJ', { repeat: true })), 'down');
  assert.equal(dashboardKeyAction(key('Enter', { repeat: true })), null);
  assert.equal(dashboardKeyAction(key('KeyH', { repeat: true })), null);
  for (const mods of [{ altKey: true }, { ctrlKey: true }, { shiftKey: true }, { metaKey: true }, { isComposing: true }]) {
    assert.equal(dashboardKeyAction(key('KeyJ', mods)), null, JSON.stringify(mods));
  }
  assert.equal(dashboardKeyAction(key('KeyX')), null);
  assert.equal(dashboardKeyAction(null), null);
  // Enter jumps to the row's pane (a user action: SetFocus)
  const row = flatRows(dashboardRows(snapshot())).find((r) => r.pane === 'bl');
  assert.deepEqual(jumpCommand(row), { type: 'setFocus', group: 'g2', tab: 't1', pane: 'bl' });
  assert.equal(jumpCommand(null), null);
});

test('nightly summary from board_status', () => {
  assert.equal(nightlySummary(null), null);
  assert.equal(nightlySummary({ lastRun: null, phase: 'idle', pending: [], held: [] }), '야간 정리: 대기 · 실행 기록 없음');
  assert.equal(nightlySummary({ lastRun: '2026-10-26', phase: 'idle', pending: [], held: [] }), '야간 정리: 대기 · 마지막 10-26');
  assert.equal(nightlySummary({ lastRun: '2026-10-26', phase: 'waiting', pending: ['a', 'b'], held: [] }), '야간 정리: 보고 기다림(2) · 마지막 10-26');
  assert.equal(nightlySummary({ lastRun: '2026-10-26', phase: 'restarting', pending: [], held: ['x'] }), '야간 정리: 재시작 · 보류 1 · 마지막 10-26');
  assert.equal(nightlySummary({ lastRun: '2026-10-26', phase: 'notifying' }), '야간 정리: 알리는 중 · 마지막 10-26');
});
