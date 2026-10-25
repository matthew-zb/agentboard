// Bootstrap: receive snapshots → render → apply DOM focus.
import * as api from './api.js';
import {
  configurePanes,
  getPane,
  syncPane,
  disposePane,
  knownPaneIds,
  staleIds,
  setPaneVisible,
  fitPane,
  focusPane,
} from './pane.js';
import {
  renderGroups,
  renderGroupChrome,
  renderSidebar,
  layoutSignature,
  visiblePaneIds,
  focusedPaneId,
  tabGroups,
  statusParts,
} from './groups.js';

const $ = (id) => document.getElementById(id);

let current = null; // last rendered Snapshot
let builtSig = null; // layoutSignature of the split DOM currently mounted
let configErrorLines = [];
let msgTimer = null;

/** Send a WsCommand; render its snapshot (null = a newer one was already rendered). */
function sendCmd(cmd) {
  return api.send(cmd).then(
    (snap) => {
      if (snap) render(snap);
      return snap;
    },
    (err) => {
      showError(err);
      throw err;
    },
  );
}

const quiet = (p) => p.catch(() => {});

function showError(err) {
  console.error('[agentboard]', err);
  const el = document.querySelector('#status .status-msg');
  if (!el) return;
  el.textContent = String(err);
  clearTimeout(msgTimer);
  msgTimer = setTimeout(() => (el.textContent = ''), 6000);
}

function setFocus(group, tab, pane) {
  quiet(sendCmd({ type: 'setFocus', group, tab, pane }));
}

/**
 * Render a Snapshot: group layout → per group a tab bar + the active tab's pane layout.
 * Inactive tabs' pane elements are kept in `#parking`. Visible panes are fitted afterwards.
 */
export function render(snap) {
  if (!snap || !snap.workspace) return;
  if (current && snap.version < current.version) return;
  current = snap;
  const ws = snap.workspace;

  for (const id of staleIds(knownPaneIds(), ws.panes)) disposePane(id);

  const focusedId = focusedPaneId(ws);
  const groupOf = tabGroups(ws);
  for (const pane of Object.values(ws.panes)) {
    syncPane(pane, { group: groupOf[pane.tab] ?? null, tab: pane.tab, focused: pane.id === focusedId });
  }

  const groupsEl = $('groups');
  const sig = layoutSignature(ws);
  if (sig !== builtSig) {
    const root = renderGroups(ws, {
      renderPane: (id) => getPane(id).el,
      onSizes: (tab, path, sizes) => quiet(sendCmd({ type: 'setSizes', tab, path, sizes })),
    });
    groupsEl.replaceChildren(root);
    builtSig = sig;
  }
  const visible = new Set(visiblePaneIds(ws));
  for (const id of knownPaneIds()) setPaneVisible(id, visible.has(id));

  renderGroupChrome(groupsEl, ws, { onSelectTab: (group, tab) => selectTab(ws, group, tab) });
  renderStatus(snap);
  renderSidebar($('sidebar'), ws, { onSelect: (g, t, p) => setFocus(g, t, p) });

  requestAnimationFrame(() => {
    for (const id of visible) fitPane(id);
    applyFocus();
  });
}

function selectTab(ws, group, tab) {
  const g = ws.groups[group];
  if (ws.focus.groupId === group && g && g.activeTabId === tab) return;
  setFocus(group, tab, null);
}

function renderStatus(snap) {
  const s = statusParts(snap);
  $('status').querySelector('.status-path').textContent = s.path;
  const flags = $('status').querySelector('.status-flags');
  flags.replaceChildren();
  if (s.sync) flags.appendChild(flag('[SYNC]', 'sync', '입력 동기화: 이 탭의 모든 실행 중 창에 입력이 전달됩니다'));
  if (s.errors > 0) {
    if (configErrorLines.length !== s.errors) refreshConfigErrors();
    flags.appendChild(flag(`[설정 오류 ${s.errors}]`, 'errors', configErrorLines.join('\n') || '설정 오류'));
  }
}

function flag(text, cls, title) {
  const el = document.createElement('span');
  el.className = `flag ${cls}`;
  el.textContent = text;
  el.title = title;
  return el;
}

let errorsPending = false;
function refreshConfigErrors() {
  if (errorsPending) return;
  errorsPending = true;
  api
    .configErrors()
    .then((lines) => {
      configErrorLines = lines;
      const el = document.querySelector('#status .flag.errors');
      if (el) el.title = lines.join('\n');
    })
    .catch(() => {})
    .finally(() => (errorsPending = false));
}

function isTextInput(el) {
  if (!el || el === document.body) return false;
  if (el.closest && el.closest('.xterm')) return false; // xterm's helper textarea
  return el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable;
}

/** DOM focus → focused group's active tab's last focused pane, unless an input has focus. */
function applyFocus() {
  if (!current) return;
  if (isTextInput(document.activeElement)) return;
  const id = focusedPaneId(current.workspace);
  if (id) focusPane(id);
}

configurePanes({ send: sendCmd, onFocus: setFocus, report: showError });
api.onWorkspaceChanged(render);
api.snapshot().then(render, showError);
refreshConfigErrors();
// Re-activating the app window returns DOM focus to the current pane (spec §6.4).
window.addEventListener('focus', () => applyFocus());
