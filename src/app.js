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
  syncTargets,
  pasteText,
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
  visibleGroupIds,
  toggleSidebar,
} from './groups.js';
import { buildKeymap, installKeyRouter } from './keys.js';
import { planCommand, registerCommand, runCommand, hasCommand, CORE_IDS } from './commands.js';
import {
  applyFocus,
  requestFocus,
  settleFocus,
  flash,
  focusLocation,
  stepAttention,
  currentRef,
  shouldFlagBell,
} from './focus.js';
import { leafIds } from './layout.js';
import { installDnd } from './dnd.js';
import {
  configureOverlays,
  overlayOpen,
  closeOverlays,
  pickNumber,
  palette,
  prompt,
  confirm,
  showErrors,
  toast,
  newTabWizard,
  promptText,
  choose,
  pickDoc,
} from './overlays.js';
import { mountInbox, configureInbox } from './inbox.js';
import { mountDoc, configureDocView, renderMarkdown, loadImages } from './docview.js';
import { mountMemo, configureMemo } from './memo.js';
import { configureCopyMode, toggleCopyMode, copyActive, pruneCopySessions } from './copy-ui.js';
import { pickCommands, paletteItems, missingDirsFlow, closeFlow, envStatusFlag, reloadFlow, closeTabInfo, closeTabFlow } from './overlay-logic.js';
import { hiddenAttentionTabs, statusEntryText } from './attention-logic.js';

const $ = (id) => document.getElementById(id);

let current = null; // last rendered Snapshot
let builtSig = null; // layoutSignature of the split DOM currently mounted
let configErrorLines = [];
let bindings = {}; // id → key string (palette hints)
let renderedLoc = null; // focusLocation of the last render (overlays close when it changes)

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

/** Every command / IPC error is shown as a 3s toast above the status bar. */
function showError(err) {
  console.error('[agentboard]', err);
  toast(err);
}

function setFocus(group, tab, pane) {
  quiet(execute([{ type: 'setFocus', group, tab, pane }]));
}

/**
 * Run WsCommands in order (stops at the first error). A SetFocus is tracked as a pending user
 * focus request so a background snapshot cannot pull DOM focus back meanwhile. RestartPane goes
 * through pane.js (re-attach marking). Flashes the focused pane when the tab/group changed.
 * Resolves true when every command succeeded (errors were already toasted).
 */
async function execute(cmds) {
  const before = current && focusLocation(current.workspace);
  for (const cmd of cmds) {
    if (cmd.type === 'restartPane') {
      await restartPane(cmd.pane); // never rejects; errors are toasted by sendCmd
      continue;
    }
    const isFocus = cmd.type === 'setFocus';
    if (isFocus) requestFocus(cmd);
    try {
      await sendCmd(cmd);
    } catch (_) {
      if (isFocus) settleFocus(current);
      return false; // sendCmd already showed the error
    }
    if (isFocus) settleFocus(current);
  }
  const ws = current && current.workspace;
  if (ws && focusLocation(ws) !== before) {
    requestAnimationFrame(() => flash(focusedPaneId(current.workspace)));
  }
  return true;
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

// ---------------------------------------------------------------- overlays (Task 13)

/** Visible `.pane` / `.group` elements of `ids`, in that order, as picker targets. */
function domTargets(ids, kind) {
  return ids
    .map((id) => ({ id, el: document.querySelector(`#groups .${kind}[data-${kind}="${CSS.escape(id)}"]`) }))
    .filter((t) => t.el && !t.el.closest('#parking'));
}

/**
 * Number pickers: 'pane' (pane.pick → SetFocus), 'moveTo' (tab.moveTo: number/hjkl → MoveTab;
 * `\` / `-` → SplitGroupWith of the focused group, like group.splitRight/Down), 'merge'
 * (group.mergeInto → MergeGroup into the picked group).
 */
async function pick(kind) {
  if (!current || overlayOpen()) return;
  const ws = current.workspace;
  const targets = kind === 'pane' ? domTargets(visiblePaneIds(ws), 'pane') : domTargets(visibleGroupIds(ws), 'group');
  if (!targets.length) return;
  const res = await pickNumber(targets, { allowHjkl: true, extraKeys: kind === 'moveTo' ? ['\\', '-'] : [] });
  if (!res) return;
  enqueue(() => {
    const cmds = current && pickCommands(kind, current.workspace, res, geometry());
    return cmds && cmds.length ? execute(cmds) : undefined;
  });
}

async function openPalette() {
  if (!current || overlayOpen()) return;
  // login autostart state for its entry (unknown → no entry)
  const autostart = await api.autostartGet().catch(() => null);
  if (!current || overlayOpen()) return;
  const items = paletteItems(current.workspace, { bindings, has: hasCommand, autostart });
  return palette(items.map((it) => ({ label: it.label, hint: it.hint, run: () => runPaletteAction(it.action) })));
}

/** Palette "로그인 시 자동 실행 켜기/끄기" (HKCU Run value). */
function setAutostart(on) {
  return api.autostartSet(on).then(
    (now) => toast(now ? '로그인 시 자동 실행을 켰습니다' : '로그인 시 자동 실행을 껐습니다', 'info'),
    showError,
  );
}

function runPaletteAction(a) {
  switch (a.type) {
    case 'command':
      return runCommand(a.id, { id: a.id, event: null, snapshot: current });
    case 'ws':
      return enqueue(() => execute(a.cmds));
    case 'rename':
      return renameTab(a.tab);
    case 'errors':
      return openErrors();
    case 'sidebar':
      toggleSidebar();
      return undefined;
    case 'openDoc':
      return openDocPicker();
    case 'autostart':
      return setAutostart(a.on);
    case 'reloadConfig':
      return reloadSettings();
    case 'closeTab':
      return closeCurrentTab(a.tab);
    default:
      return undefined;
  }
}

// ---------------------------------------------------------------- closing a tab (④ RF1)

/** Palette "현재 탭 닫기": Y/N (Enter ignored), then CloseTab sent directly — not through
 * sendCmd, whose error toast is too short for a refusal: the reason (open inbox items, running
 * teams) goes to the error panel. Manager / handover tabs also leave config.json (backend). */
function closeCurrentTab(tabId) {
  const info = current && closeTabInfo(current.workspace, tabId);
  return closeTabFlow(info, {
    confirm,
    close: (tab) =>
      api.send({ type: 'closeTab', tab }).then((snap) => {
        if (snap) render(snap);
      }),
    showErrors,
  });
}

// ---------------------------------------------------------------- settings reload

/** Key settings (keybindings.json, defaults merged) → the router's keymap and palette hints. */
function applyKeybindings(b) {
  bindings = b || {};
  router.setKeymap(buildKeymap(bindings));
}

function loadKeybindings() {
  return api.keybindings().then(applyKeybindings, showError);
}

/** Palette "설정 다시 읽기": config_reload (no pane restarts) → report toast + fresh keymap;
 * a refused reload opens the error panel. */
function reloadSettings() {
  return reloadFlow({
    reload: () => api.configReload(),
    keybindings: () => api.keybindings(),
    setKeymap: applyKeybindings,
    toast,
    showErrors,
  }).finally(refreshConfigErrors);
}

// ---------------------------------------------------------------- document tabs (Task 14)

/** OpenDoc (user action: the new or existing document tab takes the focus). */
function openDoc(path) {
  enqueue(() => execute([{ type: 'openDoc', path, group: null }]));
}

/** Palette "문서 열기": workspace .md files → fuzzy pick → OpenDoc. */
async function openDocPicker() {
  let paths;
  try {
    paths = await api.docListMd();
  } catch (err) {
    showError(err);
    return;
  }
  const path = await pickDoc(Array.isArray(paths) ? paths : []);
  if (path) openDoc(path);
}

async function renameTab(tabId) {
  const tab = current && current.workspace.tabs[tabId];
  if (!tab) return;
  const name = await prompt(`탭 이름 바꾸기: ${tab.name}`, tab.name);
  if (name == null || name.trim() === tab.name) return;
  enqueue(() => execute([{ type: 'renameTab', tab: tabId, name: name.trim() }]));
}

function openErrors() {
  return api.configErrors().then((lines) => {
    configErrorLines = lines;
    return showErrors(lines);
  }, showError);
}

// ---------------------------------------------------------------- attention (Task 15)

let attentionState = { origin: null, landed: null }; // Alt+a round (see stepAttention)
const bellInFlight = new Set(); // panes with a setAttention not yet answered

/** Terminal bell: flag the pane. Never moves focus. */
function onBell(paneId) {
  const ws = current && current.workspace;
  if (bellInFlight.has(paneId) || !shouldFlagBell(ws, paneId, document.hasFocus())) return;
  bellInFlight.add(paneId);
  quiet(sendCmd({ type: 'setAttention', pane: paneId, on: true }).finally(() => bellInFlight.delete(paneId)));
}

registerCommand('pane.nextAttention', () =>
  enqueue(() => {
    const ws = current && current.workspace;
    if (!ws) return undefined;
    const r = stepAttention(ws, attentionState);
    attentionState = r.state;
    if (!r.target) return undefined;
    return execute([{ type: 'setFocus', group: r.target.group, tab: r.target.tab, pane: r.target.pane }]);
  }),
);

registerCommand('pane.pick', () => pick('pane'));
registerCommand('tab.moveTo', () => pick('moveTo'));
registerCommand('group.mergeInto', () => pick('merge'));
registerCommand('tab.new', () => {
  if (!overlayOpen()) newTabWizard().catch(showError);
});
registerCommand('app.palette', () => openPalette());
registerCommand('pane.breakToTab', () =>
  enqueue(() => {
    const pane = current && focusedPaneId(current.workspace);
    return pane ? execute([{ type: 'breakPaneToTab', pane }]) : undefined;
  }),
);

// ---------------------------------------------------------------- copy mode · sync · paste (Task 14)

registerCommand('pane.copyMode', () => {
  const pane = current && focusedPaneId(current.workspace);
  if (pane && !overlayOpen()) toggleCopyMode(pane);
});
registerCommand('tab.syncInput', () =>
  enqueue(() => {
    const ws = current && current.workspace;
    const g = ws && ws.focus && ws.groups[ws.focus.groupId];
    const tab = g && ws.tabs[g.activeTabId];
    return tab ? execute([{ type: 'setSyncInput', tab: tab.id, on: !tab.syncInput }]) : undefined;
  }),
);
registerCommand('pane.paste', async () => {
  const pane = current && focusedPaneId(current.workspace);
  if (!pane || copyActive(pane)) return;
  try {
    // term.paste -> onData -> normal input path (sync input fans it out once per pane).
    pasteText(pane, await api.readClipboardText());
  } catch (err) {
    showError(err);
  }
});

/** Spec §4.2: ask (one folder at a time) to create each missing workspace folder. */
function checkMissingDirs() {
  return missingDirsFlow({
    alive: () => true, // R19: a tab/group switch closes the open question (→ no), not the flow
    missingDirs: () => api.missingDirs(),
    confirm,
    createDir: (name) => api.createDir(name),
    panesOf: (tabId) => {
      const t = current && current.workspace.tabs[tabId];
      return t ? leafIds(t.layout) : [];
    },
    restart: (ids) => execute(ids.map((pane) => ({ type: 'restartPane', pane }))),
    toast,
  }).catch(showError);
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

  // Spec §6.4: a tab/group switch (key, mouse, palette) closes any open overlay.
  const loc = focusLocation(ws);
  if (renderedLoc !== null && loc !== renderedLoc) closeOverlays();
  renderedLoc = loc;

  for (const id of staleIds(knownPaneIds(), ws.panes)) {
    disposePane(id);
    bellInFlight.delete(id);
    pruneCopySessions();
    passthroughCache.delete(id);
  }
  for (const id of Object.keys(ws.panes)) getPassthrough(id);

  const focusedId = focusedPaneId(ws);
  const groupOf = tabGroups(ws);
  for (const pane of Object.values(ws.panes)) {
    syncPane(pane, { group: groupOf[pane.tab] ?? null, tab: pane.tab, focused: pane.id === focusedId, attention: !!pane.attention });
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

  renderGroupChrome(groupsEl, ws, {
    onSelectTab: (group, tab) => selectTab(ws, group, tab),
    onRefocus: () => {
      if (!overlayOpen()) applyFocus(current);
    },
  });
  renderStatus(snap);
  renderSidebar($('sidebar'), ws, { onSelect: (g, t, p) => setFocus(g, t, p) });

  requestAnimationFrame(() => {
    for (const id of visible) fitPane(id);
    if (!overlayOpen()) applyFocus(current); // an open overlay keeps DOM focus
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
  renderAttentionList($('status').querySelector('.status-attn'), snap);
  const flags = $('status').querySelector('.status-flags');
  flags.replaceChildren();
  if (s.sync) flags.appendChild(flag('[SYNC]', 'sync', '입력 동기화: 이 탭의 모든 실행 중 창에 입력이 전달됩니다'));
  if (s.errors > 0) {
    if (configErrorLines.length !== s.errors) refreshConfigErrors();
    const f = flag(`[설정 오류 ${s.errors}]`, 'errors', configErrorLines.join('\n') || '설정 오류');
    f.addEventListener('mousedown', (e) => {
      e.preventDefault();
      if (!overlayOpen()) openErrors();
    });
    flags.appendChild(f);
  }
  const env = envStatusFlag(envItems);
  if (env) flags.appendChild(flag(env.text, env.highlight ? 'env env-error' : 'env', env.title));
}

// Environment check (Task 12): cached items from the backend; re-read when the check finishes
// (`env-changed`) and after model changes (folder-trust items follow pane state).
let envItems = null;
let envPending = false;
let envAgain = false; // a request that arrived while one was in flight: run once more after
function refreshEnv() {
  if (envPending) {
    envAgain = true;
    return;
  }
  envPending = true;
  envAgain = false;
  api
    .envStatus()
    .then((items) => {
      const changed = JSON.stringify(items) !== JSON.stringify(envItems);
      envItems = items;
      if (changed && current) renderStatus(current);
    })
    .catch(() => {})
    .finally(() => {
      envPending = false;
      if (envAgain) refreshEnv();
    });
}

/**
 * `⚠ billing(사유…), api`: ⚠ tabs whose tab bar is off screen (spec §5-1). Text only via
 * textContent (reasons come from agents); a click focuses that tab's attention pane, which
 * clears its ⚠. Alt+a (pane.nextAttention) reaches the same panes.
 */
function renderAttentionList(el, snap) {
  if (!el) return;
  const entries = hiddenAttentionTabs(snap);
  el.replaceChildren();
  el.classList.toggle('hidden', entries.length === 0);
  if (!entries.length) return;
  el.appendChild(document.createTextNode('⚠ '));
  entries.forEach((en, i) => {
    if (i > 0) el.appendChild(document.createTextNode(', '));
    const a = document.createElement('span');
    a.className = 'attn-tab';
    a.textContent = statusEntryText(en);
    a.title = en.reason ? `${en.name}: ${en.reason}` : `${en.name}: 주의가 필요한 창이 있습니다`;
    a.addEventListener('mousedown', (e) => {
      e.preventDefault();
      if (overlayOpen()) return;
      enqueue(() => execute([{ type: 'setFocus', group: en.group, tab: en.tab, pane: en.pane }]));
    });
    el.appendChild(a);
  });
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

configureOverlays({
  pushModal: (fn) => router.pushModal(fn),
  popModal: (t) => router.popModal(t),
  restoreFocus: () => applyFocus(current),
  createTab: (cmd) => execute([cmd]),
  createManager: ({ name, dir }) =>
    api.createManagerTab(name, dir).then(
      () => true,
      (e) => {
        showError(e);
        return false;
      },
    ),
});
configureCopyMode({
  pushModal: (fn) => router.pushModal(fn),
  popModal: (t) => router.popModal(t),
  isAppKey: (e) => router.isAppKey(e),
  focusedPane: () => (current ? focusedPaneId(current.workspace) : null),
  prompt,
  toast,
});
configurePanes({
  syncTargets: (id) => (current ? syncTargets(current.workspace, id) : null),
  send: sendCmd,
  onFocus: setFocus,
  report: showError,
  keyFilter: (paneId, ev) => router.termKeyFilter(paneId, ev),
  onBell,
  // Applet panes (Task 10): the inbox view replaces the xterm. Its keys are handled on the
  // applet element (bubble phase), so the router's app chords always win.
  // Document tabs (Task 14) mount the markdown viewer the same way; the memo (below the inbox)
  // renders MEMO.md's body with the same pipeline.
  mountApplet: (kind, paneId, model) =>
    kind === 'inbox' ? mountInbox() : kind === 'doc' ? mountDoc(paneId, model) : kind === 'memo' ? mountMemo(paneId) : null,
});
configureInbox({ toast, confirm, promptText, choose, onOpenFile: openDoc });
configureDocView({ toast, prompt, choose });
configureMemo({ render: renderMarkdown, loadImages, confirm, toast });
// Tab drag & drop (spec 6.6). Errors from MoveTab/SplitGroupWith are toasted by sendCmd.
installDnd($('groups'), {
  getWs: () => (current ? current.workspace : null),
  send: (cmd) => quiet(execute([cmd])),
  refocus: () => {
    if (!overlayOpen()) applyFocus(current);
  },
});
api.onWorkspaceChanged((snap) => {
  render(snap);
  refreshEnv();
});
api.on('env-changed', refreshEnv);
refreshEnv();
api
  .snapshot()
  .then(render)
  .then(checkMissingDirs, showError);
loadKeybindings();
// settings re-read at runtime (palette, or the dispatcher's MCP config_reload): key settings
// and the config error count may have changed
api.on('config-reloaded', () => {
  loadKeybindings();
  refreshConfigErrors();
});
refreshConfigErrors();
// Re-activating the app window returns DOM focus to the current pane (spec §6.4); an open
// overlay keeps it.
window.addEventListener('focus', () => {
  if (!overlayOpen()) applyFocus(current);
  // A bell that arrived while the window was unfocused flagged the focused pane; seeing it
  // again clears it (SetFocus on the already focused pane; focus does not move).
  // Everything is re-evaluated when the queued job runs: earlier key commands may have moved focus.
  enqueue(() => {
    if (overlayOpen()) return undefined;
    const ws = current && current.workspace;
    const id = ws && focusedPaneId(ws);
    if (!id || !ws.panes[id] || !ws.panes[id].attention) return undefined;
    const r = currentRef(ws);
    return execute([{ type: 'setFocus', group: r.group, tab: r.tab, pane: r.pane }]);
  });
});

// App close (spec §8): the window never closes by itself; ask when agents are running, then
// app_exit (save → kill all panes → exit). A failed lookup falls back to asking (safe side).
const closeState = { busy: false };
const closeWin = globalThis.__TAURI__ && globalThis.__TAURI__.window;
if (closeWin) {
  closeWin.getCurrentWindow().onCloseRequested((e) => {
    e.preventDefault();
    if (closeState.busy) return undefined; // dialog already open: ignore repeats
    closeOverlays();
    return closeFlow(closeState, {
      runningCount: () => api.runningCount().catch(() => 1),
      confirm,
      exit: () => api.appExit(),
    }).catch(showError);
  });
}
