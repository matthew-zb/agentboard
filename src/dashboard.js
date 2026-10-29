// "작업 상태" applet (spec 2026-10-26-agentboard-dashboard-design.md §2–§4): one row per agent pane
// of the role tabs — process state, ⚠ and its reason, busy/idle, context size, last prompt and
// reply — grouped by tab with the tab's open inbox items, plus the nightly refresh state.
//
// Data paths (§3): snapshots come from app.js render (`updateDashboard`) — never a second
// `api.onWorkspaceChanged` subscriber, whose shared version gate would hide snapshots from one of
// them; `inbox-changed` → inbox_list, `board-changed` → board_status. A view whose tab is not on
// screen (parked) skips drawing and draws on the next update once shown (a tab switch publishes a
// snapshot). Everything agents wrote (reasons, prompts, replies) is set as text, never markup.
// Keys come from the applet element (bubble phase; app shortcuts are taken by the key router
// first): j/k ↑↓ select, Enter focuses the pane (a user action), h hides idle panes, r re-reads.
// No top-level DOM access (Node-importable).
import * as api from './api.js';
import { dashboardRows, visibleGroups, flatRows, dashboardKeyAction, jumpCommand, nightlySummary } from './dashboard-logic.js';
import { statusSymbol, warnTitle } from './pane.js';
import { isFocusTrapKey, keepSelection, moveSelection } from './inbox-logic.js';

const ROLE_LABELS = { dispatcher: 'dispatcher', handover: 'handover', manager: 'Manager', lead: '리드', member: '팀원', plain: '일반 탭' };

// ---------------------------------------------------------------- shared store

const store = { snap: null, inbox: [], board: null, focus: null, views: new Set(), unlisten: [], inboxSeq: 0, boardSeq: 0 };

const redrawAll = () => {
  for (const v of store.views) v.changed();
};

/** app.js render → every mounted view (drawn now if on screen, else when next shown). */
export function updateDashboard(snap) {
  store.snap = snap;
  redrawAll();
}

/** 집중근무 / 전달 멈춤 state from app.js (focus-logic.js shape; null = unknown): the inbox counts
 * follow the inbox list's rule. Kept while no view is mounted. */
export function setDashboardFocus(focus) {
  store.focus = focus || null;
  redrawAll();
}

function refreshInbox() {
  const seq = ++store.inboxSeq;
  return api.inboxList(true).then(
    (r) => {
      if (seq !== store.inboxSeq) return;
      store.inbox = r && Array.isArray(r.items) ? r.items : [];
      redrawAll();
    },
    () => {}, // the inbox applet reports inbox errors; the counts just stay
  );
}

function refreshBoard() {
  const seq = ++store.boardSeq;
  return api.boardStatus().then(
    (b) => {
      if (seq !== store.boardSeq) return;
      store.board = b || null;
      redrawAll();
    },
    () => {},
  );
}

function subscribe(view) {
  store.views.add(view);
  if (store.views.size === 1) {
    store.unlisten = [api.on('inbox-changed', refreshInbox), api.on('board-changed', refreshBoard)];
  }
  refreshInbox();
  refreshBoard();
}

function unsubscribe(view) {
  store.views.delete(view);
  if (store.views.size === 0) {
    for (const u of store.unlisten.splice(0)) Promise.resolve(u).then((f) => typeof f === 'function' && f(), () => {});
  }
}

// ---------------------------------------------------------------- drawing (text only)

function el(doc, tag, cls, text) {
  const e = doc.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

function inboxText(c) {
  if (!c || !c.open) return '';
  const parts = [];
  if (c.new) parts.push(`새 ${c.new}${c.unread ? `(안 읽음 ${c.unread})` : ''}`);
  if (c.inProgress) parts.push(`진행 중 ${c.inProgress}`);
  if (c.closeProposed) parts.push(`종료 제안 ${c.closeProposed}`);
  return `인박스 ${parts.join(' · ')}`;
}

const idleText = (idle) => (idle === true ? '쉬는 중' : idle === false ? '작업 중' : '—');

function rowEl(doc, r, selected) {
  const row = el(doc, 'div', `dash-row depth-${r.depth}`);
  row.dataset.pane = r.pane;
  if (r.pane === selected) row.classList.add('sel');
  if (r.context && r.context.over) row.classList.add('ctx-over');
  const st = statusSymbol({ status: r.status });
  const name = el(doc, 'span', 'dash-name');
  const sym = el(doc, 'span', `dash-status ${st.cls}`, st.label);
  sym.title = st.title;
  name.append(sym, el(doc, 'span', 'dash-pname', r.sessionName || r.name), el(doc, 'span', 'dash-role', ROLE_LABELS[r.role] || r.role));
  if (r.attention) {
    const w = el(doc, 'span', 'dash-warn', r.reason ? `⚠ ${r.reason}` : '⚠');
    w.title = warnTitle({ attention: true, attentionReason: r.reason }) || '';
    name.appendChild(w);
  }
  const ctx = el(doc, 'span', 'dash-ctx', r.context ? r.context.text : '—');
  const tips = [];
  if (r.context && r.context.over) tips.push('문맥이 400k를 넘었습니다');
  if (r.model) tips.push(`모델: ${r.model}`);
  if (tips.length) ctx.title = tips.join('\n');
  row.append(
    name,
    el(doc, 'span', `dash-idle ${r.idle === true ? 'idle' : r.idle === false ? 'busy' : 'unknown'}`, idleText(r.idle)),
    ctx,
    el(doc, 'span', 'dash-prompt', r.lastPrompt ? `▶ ${r.lastPrompt}` : ''),
    el(doc, 'span', 'dash-reply', r.lastReply ? `◀ ${r.lastReply}` : ''),
  );
  return row;
}

/**
 * The view content for `groups` (dashboardRows → visibleGroups): the nightly line, then per tab a
 * header (name, role, open inbox items, hidden count) and its rows. `doc` = document (injectable).
 */
export function buildDashboard(doc, groups, { selected = null, nightly = null } = {}) {
  const box = el(doc, 'div', 'dash-list');
  const night = nightlySummary(nightly);
  if (night) box.appendChild(el(doc, 'div', 'dash-nightly', night));
  if (!groups.length) box.appendChild(el(doc, 'div', 'dash-empty', '상시업무 탭이 없습니다'));
  for (const g of groups) {
    const head = el(doc, 'div', `dash-tab role-${g.role}`);
    head.dataset.tab = g.tab;
    head.append(el(doc, 'span', 'dash-tname', g.name), el(doc, 'span', 'dash-trole', ROLE_LABELS[g.role] || g.role));
    const inbox = inboxText(g.inbox);
    if (inbox) head.appendChild(el(doc, 'span', 'dash-inbox', inbox));
    if (g.hidden) head.appendChild(el(doc, 'span', 'dash-hidden', `쉬는 창 ${g.hidden}개 숨김`));
    box.appendChild(head);
    for (const r of g.rows) box.appendChild(rowEl(doc, r, selected));
  }
  return box;
}

// ---------------------------------------------------------------- mounted applet

const deps = {
  jump: () => {}, // SetFocus command → app.js execute
  toast: (m) => console.error('[agentboard]', m),
};

/** `{jump(setFocusCommand), toast(msg, kind?)}` (app.js). */
export function configureDashboard(d) {
  Object.assign(deps, d);
}

const HIDE_KEY = 'agentboard.dashboard.hideIdle';
function loadHideIdle() {
  try {
    return globalThis.localStorage ? localStorage.getItem(HIDE_KEY) === '1' : false;
  } catch (_) {
    return false;
  }
}
function saveHideIdle(v) {
  try {
    if (globalThis.localStorage) localStorage.setItem(HIDE_KEY, v ? '1' : '0');
  } catch (_) {
    /* per-viewer convenience only */
  }
}

/** Mount a "작업 상태" view for pane `paneId`; returns `{el, focus(), dispose()}`. */
export function mountDashboard(paneId) {
  const doc = document;
  const root = el(doc, 'div', 'dashboard');
  root.tabIndex = -1;
  const bar = el(doc, 'div', 'dash-bar');
  bar.append(el(doc, 'span', 'dash-title', '작업 상태'), el(doc, 'span', 'dash-hint', 'j/k 이동 · Enter 창으로 · h 쉬는 창 숨기기 · r 다시 읽기'));
  const body = el(doc, 'div', 'dash-body');
  root.append(bar, body);
  const st = { sel: null, selIndex: 0, hideIdle: loadHideIdle(), disposed: false, rows: [] };

  const onScreen = () => root.isConnected && !(root.closest && root.closest('#parking'));

  function draw() {
    if (st.disposed || !onScreen()) return; // drawn on the next update once shown
    const groups = visibleGroups(dashboardRows(store.snap, { inbox: store.inbox, board: store.board, focus: store.focus }), st.hideIdle);
    const rows = flatRows(groups).map((r) => ({ ...r, id: r.pane }));
    st.sel = keepSelection(rows, st.sel, st.selIndex);
    st.selIndex = Math.max(0, rows.findIndex((r) => r.id === st.sel));
    st.rows = rows;
    body.replaceChildren(buildDashboard(doc, groups, { selected: st.sel, nightly: store.board && store.board.nightly }));
    const cur = rowsByPane(st.sel);
    if (cur && cur.scrollIntoView) cur.scrollIntoView({ block: 'nearest' });
  }

  function rowsByPane(pane) {
    const stack = [body];
    while (stack.length) {
      const n = stack.pop();
      if (n.dataset && n.dataset.pane === pane && n.classList && n.classList.contains('dash-row')) return n;
      for (const c of n.children || []) stack.push(c);
    }
    return null;
  }

  function select(pane) {
    if (pane == null || pane === st.sel) return;
    st.sel = pane;
    draw();
  }

  const selectedRow = () => st.rows.find((r) => r.id === st.sel) || null;
  const jump = () => {
    const cmd = jumpCommand(selectedRow());
    if (cmd) deps.jump(cmd);
  };

  const actions = {
    down: () => select(moveSelection(st.rows, st.sel, 1)),
    up: () => select(moveSelection(st.rows, st.sel, -1)),
    jump,
    reload: () => {
      refreshInbox();
      refreshBoard();
    },
    toggleIdle: () => {
      st.hideIdle = !st.hideIdle;
      saveHideIdle(st.hideIdle);
      draw();
    },
  };

  const onKey = (e) => {
    if (e.defaultPrevented) return; // the app key router / an overlay owns it
    if (isFocusTrapKey(e)) {
      e.preventDefault(); // focus never wanders into a neighbouring terminal
      return;
    }
    const a = dashboardKeyAction(e);
    if (!a) return;
    e.preventDefault();
    e.stopPropagation();
    actions[a]();
  };

  const rowOf = (t) => (t && t.closest ? t.closest('.dash-row[data-pane]') : null);
  const onMouse = (e) => {
    const row = rowOf(e.target);
    if (!row) return;
    e.preventDefault(); // keep DOM focus on the applet
    select(row.dataset.pane);
  };
  const onDbl = (e) => {
    const row = rowOf(e.target);
    if (!row) return;
    select(row.dataset.pane);
    jump();
  };

  root.addEventListener('keydown', onKey);
  root.addEventListener('mousedown', onMouse);
  root.addEventListener('dblclick', onDbl);
  const view = { changed: draw };
  subscribe(view);
  draw();

  return {
    el: root,
    focus() {
      if (!root.contains(doc.activeElement)) root.focus({ preventScroll: true });
    },
    dispose() {
      if (st.disposed) return;
      st.disposed = true;
      root.removeEventListener('keydown', onKey);
      root.removeEventListener('mousedown', onMouse);
      root.removeEventListener('dblclick', onDbl);
      unsubscribe(view);
      root.remove();
    },
  };
}
