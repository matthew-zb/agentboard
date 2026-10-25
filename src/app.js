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
  restartPane,
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
import { buildKeymap, installKeyRouter } from './keys.js';
import { planCommand, registerCommand, runCommand, CORE_IDS } from './commands.js';
import { applyFocus, requestFocus, settleFocus, flash, focusLocation } from './focus.js';

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
  quiet(execute([{ type: 'setFocus', group, tab, pane }]));
}

/**
 * Run WsCommands in order (stops at the first error). A SetFocus is tracked as a pending user
 * focus request so a background snapshot cannot pull DOM focus back meanwhile. RestartPane goes
 * through pane.js (re-attach marking). Flashes the focused pane when the tab/group changed.
 */
async function execute(cmds) {
  const before = current && focusLocation(current.workspace);
  for (const cmd of cmds) {
    if (cmd.type === 'restartPane') {
      await restartPane(cmd.pane);
      continue;
    }
    const isFocus = cmd.type === 'setFocus';
    if (isFocus) requestFocus(cmd);
    try {
      await sendCmd(cmd);
    } catch (_) {
      if (isFocus) settleFocus(current);
      return; // sendCmd already showed the error
    }
    if (isFocus) settleFocus(current);
  }
  const ws = current && current.workspace;
  if (ws && focusLocation(ws) !== before) {
    requestAnimationFrame(() => flash(focusedPaneId(current.workspace)));
  }
}

// Key commands run one after another, each planned against the snapshot the previous one
// produced (fast repeated Alt+l moves twice, not to the same target twice).
let queue = Promise.resolve();
let queued = 0;
const MAX_QUEUED = 8;
function enqueue(fn) {
  if (queued >= MAX_QUEUED) return; // a held key outran the backend: drop repeats
  queued++;
  queue = queue.then(fn).catch(() => {}).finally(() => queued--);
}

/** On-screen rects of visible panes and groups (DOM order irrelevant). */
function geometry() {
  const rects = (sel, key) =>
    [...document.querySelectorAll(sel)]
      .filter((el) => !el.closest('#parking'))
      .map((el) => {
        const b = el.getBoundingClientRect();
        return { id: el.dataset[key], left: b.left, top: b.top, right: b.right, bottom: b.bottom };
      })
      .filter((r) => r.right > r.left && r.bottom > r.top);
  return { paneRects: rects('#groups .pane[data-pane]', 'pane'), groupRects: rects('#groups .group[data-group]', 'group') };
}

for (const id of CORE_IDS) {
  registerCommand(id, () =>
    enqueue(() => {
      const cmds = current && planCommand(id, current.workspace, geometry());
      return cmds && cmds.length ? execute(cmds) : undefined;
    }),
  );
}

// passthroughKeys per pane (profile-defined, fixed for the pane's life): fetched once.
const passthroughCache = new Map();
function getPassthrough(paneId) {
  if (!passthroughCache.has(paneId)) {
    passthroughCache.set(paneId, []);
    api.passthrough(paneId).then(
      (keys) => passthroughCache.has(paneId) && passthroughCache.set(paneId, Array.isArray(keys) ? keys : []),
      () => {},
    );
  }
  return passthroughCache.get(paneId);
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

  for (const id of staleIds(knownPaneIds(), ws.panes)) {
    disposePane(id);
    passthroughCache.delete(id);
  }
  for (const id of Object.keys(ws.panes)) getPassthrough(id);

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
    applyFocus(current);
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

const router = installKeyRouter({
  keymap: new Map(),
  getPassthrough,
  focusedPane: () => (current ? focusedPaneId(current.workspace) : null),
  // Unregistered ids (overlays, palette, … until Tasks 13–15) are swallowed as no-ops.
  run: (id, event) => runCommand(id, { id, event, snapshot: current }),
});

configurePanes({
  send: sendCmd,
  onFocus: setFocus,
  report: showError,
  keyFilter: (paneId, ev) => router.termKeyFilter(paneId, ev),
});
api.onWorkspaceChanged(render);
api.snapshot().then(render, showError);
api.keybindings().then((b) => router.setKeymap(buildKeymap(b)), showError);
refreshConfigErrors();
// Re-activating the app window returns DOM focus to the current pane (spec §6.4).
window.addEventListener('focus', () => applyFocus(current));
