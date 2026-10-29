import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
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

// ---------------------------------------------------------------- the "작업 상태" applet (fake DOM)
import { buildDashboard, mountDashboard, configureDashboard, updateDashboard, setDashboardFocus } from '../../src/dashboard.js';

/** Minimal DOM: elements keep children/text/attributes; any innerHTML use throws. */
function fakeDocument() {
  const doc = { activeElement: null, parked: new Set() };
  const textNode = (t) => ({ tagName: '#text', children: [], textContent: String(t), contains: () => false });
  const mk = (tag) => {
    const classes = new Set();
    const el = {
      tagName: String(tag).toUpperCase(),
      children: [],
      parent: null,
      dataset: {},
      attrs: {},
      listeners: {},
      _text: '',
      title: '',
      isConnected: true,
      classList: {
        add: (...c) => c.forEach((x) => classes.add(x)),
        remove: (...c) => c.forEach((x) => classes.delete(x)),
        toggle: (c, on) => ((on === undefined ? !classes.has(c) : on) ? classes.add(c) : classes.delete(c)),
        contains: (c) => classes.has(c),
      },
      get className() {
        return [...classes].join(' ');
      },
      set className(v) {
        classes.clear();
        String(v).split(/\s+/).filter(Boolean).forEach((c) => classes.add(c));
      },
      set textContent(v) {
        this._text = String(v);
        this.children = [];
      },
      get textContent() {
        return this._text + this.children.map((c) => c.textContent).join('');
      },
      set innerHTML(_) {
        throw new Error('innerHTML used');
      },
      append(...c) {
        c.forEach((x) => this.appendChild(typeof x === 'string' ? textNode(x) : x));
      },
      appendChild(c) {
        c.parent = this;
        this.children.push(c);
        return c;
      },
      replaceChildren(...c) {
        this.children = [];
        this.append(...c);
      },
      setAttribute(k, v) {
        this.attrs[k] = String(v);
      },
      addEventListener(t, f) {
        (this.listeners[t] ||= []).push(f);
      },
      removeEventListener(t, f) {
        this.listeners[t] = (this.listeners[t] || []).filter((x) => x !== f);
      },
      contains(n) {
        return n === this || this.children.some((c) => c.contains && c.contains(n));
      },
      /** `#parking` ancestor: the test parks an element by adding it to doc.parked */
      closest(sel) {
        if (sel === '#parking') {
          for (let n = this; n; n = n.parent) if (doc.parked.has(n)) return n;
          return null;
        }
        if (sel === '.dash-row[data-pane]') {
          for (let n = this; n; n = n.parent) if (n.classList && n.classList.contains('dash-row') && n.dataset.pane) return n;
        }
        return null;
      },
      focus() {
        doc.activeElement = this;
      },
      remove() {
        if (this.parent) this.parent.children = this.parent.children.filter((c) => c !== this);
      },
      scrollIntoView() {},
    };
    return el;
  };
  doc.createElement = mk;
  doc.createTextNode = textNode;
  return doc;
}

const tree = (el) => [el, ...(el.children || []).flatMap(tree)];
const rowsOf = (root) => tree(root).filter((e) => e.classList && e.classList.contains('dash-row'));
const settle = async () => {
  for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0));
};

const BOARD = {
  panes: {
    m1: { idle: false, lastPrompt: '결제 배치 오류 확인해', lastReply: '확인 중입니다', contextTokens: 456789, updatedAt: 1 },
    bm1: { idle: true, lastPrompt: null, lastReply: null, contextTokens: 12000, updatedAt: 1 },
  },
  nightly: { lastRun: '2026-10-28', phase: 'idle', pending: [], held: [] },
};
const INBOX = { version: 3, items: [{ id: 'i1', ownerName: 'billing', state: 'new', read: false }, { id: 'i2', ownerName: 'billing', state: 'in_progress', read: true }], unknownOwners: [] };

function setupDash({ inbox = INBOX } = {}) {
  const t = { calls: [], listeners: {}, jumps: [], toasts: [] };
  globalThis.document = fakeDocument();
  globalThis.__TAURI__ = {
    core: {
      invoke: async (cmd, args) => {
        t.calls.push([cmd, args]);
        if (cmd === 'board_status') return BOARD;
        if (cmd === 'inbox_list') return inbox;
        return null;
      },
    },
    event: {
      listen: async (ev, cb) => {
        (t.listeners[ev] ||= []).push(cb);
        return () => {
          t.listeners[ev] = t.listeners[ev].filter((x) => x !== cb);
        };
      },
    },
  };
  t.emit = (ev) => (t.listeners[ev] || []).forEach((cb) => cb({ payload: null }));
  t.count = (cmd) => t.calls.filter((c) => c[0] === cmd).length;
  configureDashboard({ jump: (cmd) => t.jumps.push(cmd), toast: (m) => t.toasts.push(String(m)) });
  return t;
}

function teardownDash(view) {
  view.dispose();
  updateDashboard(null);
  setDashboardFocus(null);
  delete globalThis.document;
  delete globalThis.__TAURI__;
}

const press = (root, code, extra = {}) => {
  const ev = { code, target: root, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, isComposing: false, repeat: false, defaultPrevented: false, ...extra };
  ev.preventDefault = () => {
    ev.defaultPrevented = true;
  };
  ev.stopPropagation = () => {};
  for (const f of root.listeners.keydown || []) f(ev);
  return ev;
};

test('buildDashboard draws text only: agent-written reasons and prompts never become markup', () => {
  globalThis.document = fakeDocument();
  try {
    const evil = '<img src=x onerror="alert(1)"><script>alert(2)</script>';
    const snap = snapshot();
    snap.workspace.panes.m1.attentionReason = evil;
    const board = { panes: { m1: { idle: false, lastPrompt: evil, lastReply: evil, contextTokens: 1, updatedAt: 1 } } };
    const groups = dashboardRows(snap, { board, inbox: INBOX.items });
    const el = buildDashboard(document, groups, { selected: 'm1', nightly: BOARD.nightly });
    const tags = new Set(tree(el).map((e) => e.tagName));
    assert.ok(!tags.has('IMG') && !tags.has('SCRIPT'), [...tags].join(','));
    assert.ok(el.textContent.includes(evil.slice(0, 20)), 'shown literally (shortened)');
    // one header per role tab, one row per agent pane, the selected row marked
    const heads = tree(el).filter((e) => e.classList && e.classList.contains('dash-tab'));
    assert.deepEqual(heads.map((h) => h.dataset.tab), ['t0', 't3', 't4', 't1']);
    const rows = rowsOf(el);
    assert.deepEqual(rows.map((r) => r.dataset.pane), ['d', 'h', 'm2', 'm1', 'bl', 'bm2', 'bm1', 'cl']);
    assert.ok(rows.find((r) => r.dataset.pane === 'm1').classList.contains('sel'));
    // billing header shows its open inbox items; the nightly line is in the bar
    assert.ok(heads[3].textContent.includes('인박스 새 1') && heads[3].textContent.includes('진행 중 1'));
    assert.ok(el.textContent.includes('야간 정리: 대기 · 마지막 10-28'));
  } finally {
    delete globalThis.document;
  }
});

test('dashboard view: board fields, j/k selection, Enter jumps (SetFocus), h hides idle, r re-reads', async () => {
  const t = setupDash();
  updateDashboard(snapshot());
  const v = mountDashboard('dash');
  await settle();
  const root = v.el;
  const m1 = () => rowsOf(root).find((r) => r.dataset.pane === 'm1');
  assert.ok(m1().textContent.includes('작업 중') && m1().textContent.includes('457k'), m1().textContent);
  assert.ok(m1().classList.contains('ctx-over'), 'over 400k is marked');
  assert.ok(m1().textContent.includes('결제 배치 오류 확인해'));
  assert.ok(rowsOf(root).find((r) => r.dataset.pane === 'bm1').textContent.includes('쉬는 중'));
  // first row selected; j moves down, k up (clamped)
  const sel = () => rowsOf(root).find((r) => r.classList.contains('sel')).dataset.pane;
  assert.equal(sel(), 'd');
  press(root, 'KeyJ');
  press(root, 'ArrowDown');
  assert.equal(sel(), 'm2');
  press(root, 'KeyK');
  assert.equal(sel(), 'h');
  const e = press(root, 'Enter');
  assert.ok(e.defaultPrevented);
  assert.deepEqual(t.jumps, [{ type: 'setFocus', group: 'g1', tab: 't3', pane: 'h' }]);
  // h hides idle (bm1) and ended (bm2) rows; the selection stays when still listed
  press(root, 'KeyH');
  assert.deepEqual(rowsOf(root).map((r) => r.dataset.pane), ['d', 'h', 'm2', 'm1', 'bl', 'cl']);
  assert.ok(root.textContent.includes('쉬는 창 2개 숨김'));
  press(root, 'KeyH');
  assert.equal(rowsOf(root).length, 8);
  // r re-reads the board and the inbox
  const [b, i] = [t.count('board_status'), t.count('inbox_list')];
  press(root, 'KeyR');
  await settle();
  assert.deepEqual([t.count('board_status'), t.count('inbox_list')], [b + 1, i + 1]);
  // app chords are not dashboard keys (the key router owns them)
  assert.equal(press(root, 'KeyJ', { altKey: true }).defaultPrevented, false);
  teardownDash(v);
});

test('dashboard view: mouse click selects, double-click jumps', async () => {
  const t = setupDash();
  updateDashboard(snapshot());
  const v = mountDashboard('dash');
  await settle();
  const row = rowsOf(v.el).find((r) => r.dataset.pane === 'bl');
  const cell = row.children[0];
  const md = { target: cell, button: 0, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; } };
  for (const f of v.el.listeners.mousedown || []) f(md);
  assert.ok(rowsOf(v.el).find((r) => r.dataset.pane === 'bl').classList.contains('sel'));
  assert.ok(md.defaultPrevented, 'keeps DOM focus on the applet');
  for (const f of v.el.listeners.dblclick || []) f({ target: cell });
  assert.deepEqual(t.jumps, [{ type: 'setFocus', group: 'g2', tab: 't1', pane: 'bl' }]);
  teardownDash(v);
});

test('dashboard view: board-changed / inbox-changed / snapshots redraw; a hidden tab waits', async () => {
  const t = setupDash();
  updateDashboard(snapshot());
  const v = mountDashboard('dash');
  await settle();
  assert.equal(t.count('board_status'), 1);
  assert.equal(t.count('inbox_list'), 1);
  t.emit('board-changed');
  t.emit('inbox-changed');
  await settle();
  assert.deepEqual([t.count('board_status'), t.count('inbox_list')], [2, 2]);
  // parked (its tab is not shown): a new snapshot is not drawn until the view is on screen
  document.parked.add(v.el);
  const s = snapshot();
  s.workspace.panes.m2.attention = true;
  s.workspace.panes.m2.attentionReason = 'api 결정 대기';
  updateDashboard(s);
  assert.ok(!v.el.textContent.includes('api 결정 대기'), 'not drawn while hidden');
  document.parked.delete(v.el);
  updateDashboard(s); // the tab switch publishes a snapshot
  assert.ok(v.el.textContent.includes('api 결정 대기'));
  // dispose stops listening
  teardownDash(v);
  assert.deepEqual([t.count('board_status'), t.count('inbox_list')], [2, 2]);
});

test('app.js feeds snapshots to the dashboard from render and mounts kind "dashboard"', () => {
  const src = readFileSync(new URL('../../src/app.js', import.meta.url), 'utf8');
  const render = src.slice(src.indexOf('export function render(snap)'), src.indexOf('function selectTab('));
  assert.match(render, /updateDashboard\(snap\)/);
  assert.match(src, /kind === 'dashboard' \? mountDashboard\(paneId\)/);
  const dash = readFileSync(new URL('../../src/dashboard.js', import.meta.url), 'utf8');
  assert.ok(!/onWorkspaceChanged\s*\(/.test(dash), 'a second subscriber would miss snapshots (shared version gate, api.js)');
});

// ---------------------------------------------------------------- ab-core contract (C1): D3b, model, Ctrl+Alt+d
import { buildKeymap, matchEvent } from '../../src/keys.js';
import { paletteItems } from '../../src/overlay-logic.js';
import { PENDING_IDS } from '../../src/commands.js';

test('D3b: panes without a role that have a board entry (claude started by hand) are listed too', () => {
  const board = {
    panes: {
      n1: { idle: false, lastPrompt: '직접 띄운 claude', lastReply: null, contextTokens: 50000, model: 'claude-opus-5-5', updatedAt: 1 },
      sh1: { idle: true, lastPrompt: null, lastReply: null, contextTokens: 1000, model: null, updatedAt: 1 },
    },
  };
  const gs = dashboardRows(snapshot(), { board });
  // role tabs as before, then plain tabs that have such panes (reading order)
  assert.deepEqual(gs.map((g) => [g.name, g.role]), [['dispatcher', 'dispatcher'], ['handover', 'handover'], ['api', 'manager'], ['billing', 'manager'], ['notes', 'plain']]);
  const notes = gs.at(-1);
  assert.deepEqual(notes.rows.map((r) => [r.pane, r.role, r.depth]), [['n1', null, 0]]);
  assert.equal(notes.rows[0].model, 'claude-opus-5-5');
  // inside a role tab: after the standing pane and the teams
  assert.deepEqual(gs.find((g) => g.name === 'billing').rows.map((r) => r.pane), ['m1', 'bl', 'bm2', 'bm1', 'cl', 'sh1']);
  // without board entries plain tabs and shells stay out
  assert.equal(dashboardRows(snapshot()).length, 4);
  assert.equal(flatRows(dashboardRows(snapshot(), { board })).find((r) => r.pane === 'm1').model, null);
});

test('Ctrl+Alt+d opens the 작업 상태 tab: default key, label, palette entry, handler', () => {
  const defaults = JSON.parse(readFileSync(new URL('../../src-tauri/defaults/keybindings.json', import.meta.url), 'utf8'));
  assert.equal(defaults['app.dashboard'], 'Ctrl+Alt+d');
  const km = buildKeymap(defaults);
  assert.equal(matchEvent(km, { code: 'KeyD', ctrlKey: true, altKey: true, shiftKey: false, metaKey: false, key: 'ㅇ' }), 'app.dashboard');
  assert.ok(PENDING_IDS.includes('app.dashboard'), 'handled by app.js, not planned by commands.js');
  const ws = snapshot().workspace;
  ws.focus = { groupId: 'g1' };
  const it = paletteItems(ws, { bindings: defaults }).find((i) => i.action.id === 'app.dashboard');
  assert.deepEqual(it, { label: '작업 상태', hint: 'Ctrl+Alt+d', action: { type: 'command', id: 'app.dashboard' } });
  const app = readFileSync(new URL('../../src/app.js', import.meta.url), 'utf8');
  assert.match(app, /registerCommand\('app\.dashboard', \(\) => enqueue\(\(\) => execute\(\[\{ type: 'openDashboard' \}\]\)\)\)/);
});

// ---------------------------------------------------------------- 집중근무: the counts follow the inbox rule
import { inboxFocusView } from '../../src/focus-logic.js';

const SINCE = '2026-10-29T10:00:00+09:00';
const UNTIL = '2026-10-29T11:00:00+09:00';
const FOCUS_ITEMS = [
  { id: 'a', ownerName: 'billing', state: 'in_progress', read: true, created: '2026-10-29T09:00:00+09:00' }, // before the mode
  { id: 'b', ownerName: 'billing', state: 'new', read: false, created: '2026-10-29T10:10:00+09:00' }, // during
  { id: 'c', ownerName: 'billing', state: 'close_proposed', read: true, created: '2026-10-29T10:20:00+09:00' }, // during, read
  { id: 'd', ownerName: 'api', state: 'new', read: false, created: '2026-10-29T10:30:00+09:00' }, // during
  { id: 'e', ownerName: 'api', state: 'done', read: true, created: '2026-10-29T10:40:00+09:00' }, // during, done
];

test('집중근무 / 전달 멈춤: items the inbox hides or holds are not counted; after release they count again', () => {
  const zero = { new: 0, unread: 0, inProgress: 0, closeProposed: 0, open: 0 };
  const all = inboxOpenCounts(FOCUS_ITEMS);
  assert.deepEqual(all.billing, { new: 1, unread: 1, inProgress: 1, closeProposed: 1, open: 3 });
  assert.deepEqual(all.api, { ...zero, new: 1, unread: 1, open: 1 });
  for (const mode of ['focus', 'hold']) {
    const c = inboxOpenCounts(FOCUS_ITEMS, { mode, since: SINCE });
    assert.deepEqual(c.billing, { ...zero, inProgress: 1, open: 1 }, `${mode}: only what came before the mode`);
    assert.equal(c.api, undefined, `${mode}: api had only items of the mode`);
    // the same items the inbox list leaves out (one rule: inboxFocusView)
    const v = inboxFocusView(FOCUS_ITEMS, { mode, since: SINCE });
    assert.equal(c.billing.open + (c.api ? c.api.open : 0), v.rest.length + v.released.length);
  }
  // released: the group (unread) and the rest (read ones of the window too) are all counted
  const released = { mode: 'normal', since: null, released: { from: SINCE, to: UNTIL } };
  assert.deepEqual(inboxOpenCounts(FOCUS_ITEMS, released), all);
  assert.deepEqual(inboxOpenCounts(FOCUS_ITEMS, { mode: 'normal', since: null }), all);
  assert.deepEqual(inboxOpenCounts(FOCUS_ITEMS, null), all, 'state unknown: as before');
  // rows carry the focus-aware counts
  const gs = dashboardRows(snapshot(), { inbox: FOCUS_ITEMS, focus: { mode: 'focus', since: SINCE } });
  assert.deepEqual(gs.find((g) => g.name === 'billing').inbox, { ...zero, inProgress: 1, open: 1 });
  assert.deepEqual(gs.find((g) => g.name === 'api').inbox, zero);
});

test('dashboard view: a focus change (setDashboardFocus) redraws the counts', async () => {
  setupDash({ inbox: { version: 1, items: FOCUS_ITEMS, unknownOwners: [] } });
  updateDashboard(snapshot());
  const v = mountDashboard('dash');
  await settle();
  const head = (name) => tree(v.el).find((e) => e.classList && e.classList.contains('dash-tab') && e.textContent.includes(name)).textContent;
  assert.ok(head('billing').includes('인박스 새 1(안 읽음 1) · 진행 중 1 · 종료 제안 1'), head('billing'));
  assert.ok(head('api').includes('인박스 새 1'));
  setDashboardFocus({ mode: 'focus', since: SINCE });
  assert.ok(head('billing').includes('인박스 진행 중 1') && !head('billing').includes('새 1'), head('billing'));
  assert.ok(!head('api').includes('인박스'), head('api'));
  setDashboardFocus({ mode: 'normal', since: null, released: { from: SINCE, to: UNTIL } });
  assert.ok(head('billing').includes('인박스 새 1(안 읽음 1) · 진행 중 1 · 종료 제안 1'), head('billing'));
  teardownDash(v);
});

test('app.js hands every focus state to the dashboard too (focus-changed → focus_get → apply)', () => {
  const src = readFileSync(new URL('../../src/app.js', import.meta.url), 'utf8');
  const sync = src.slice(src.indexOf('const focusSync = makeFocusSync('), src.indexOf('// Environment check'));
  assert.match(sync, /setInboxFocus\(state\)/);
  assert.match(sync, /setDashboardFocus\(state\)/);
  assert.match(src, /api\.on\('focus-changed', \(\) => focusSync\.refresh\(\)\)/);
});
