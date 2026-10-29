// Command ids (spec §6.2) → WsCommand lists (pure planning) + the command registry that the key
// router, the palette and later tasks dispatch through.
// No top-level DOM access: the pure helpers are unit-tested in Node.
import { isLeaf, leafIds, containsLeaf } from './layout.js';
import { tabGroups } from './groups.js';
import { findNeighbor } from './geometry.js';

/** Resize step in percent for `pane.resize*`. */
export const RESIZE_STEP = 5;

/**
 * Ids wired by later tasks (overlays, copy mode, sync input, palette, attention, paste,
 * break-to-tab). Until a handler is registered they are swallowed as no-ops so the key never
 * reaches the terminal.
 */
export const PENDING_IDS = [
  'pane.pick',
  'pane.nextAttention',
  'pane.copyMode',
  'pane.paste',
  'pane.breakToTab',
  'tab.syncInput',
  'tab.new',
  'tab.moveTo',
  'group.mergeInto',
  'app.palette',
  'app.dashboard',
];

const nine = (prefix) => Array.from({ length: 9 }, (_, i) => `${prefix}${i + 1}`);

/** Ids planned by `planCommand` (turned into WsCommands here). */
export const CORE_IDS = [
  'pane.focusLeft',
  'pane.focusDown',
  'pane.focusUp',
  'pane.focusRight',
  'pane.resizeLeft',
  'pane.resizeDown',
  'pane.resizeUp',
  'pane.resizeRight',
  'pane.splitRight',
  'pane.splitDown',
  'pane.maximize',
  'pane.close',
  'pane.restart',
  'tab.prev',
  'tab.next',
  ...nine('tab.goto'),
  'tab.last',
  'tab.dispatcher',
  'tab.moveLeft',
  'tab.moveRight',
  'tab.moveDown',
  'tab.moveUp',
  ...nine('group.goto'),
  'group.splitRight',
  'group.splitDown',
  'group.close',
  'group.maximize',
];

const OPPOSITE = { left: 'right', right: 'left', up: 'down', down: 'up' };
const DIRS = { Left: 'left', Right: 'right', Up: 'up', Down: 'down' };

// ---------------------------------------------------------------- pure planning

/** Focus chain of the workspace: `{g, tab, pane}` (objects / id), or null. */
function chain(ws) {
  const g = ws && ws.focus && ws.groups[ws.focus.groupId];
  const tab = g && ws.tabs[g.activeTabId];
  if (!tab) return null;
  return { g, tab, pane: tab.lastFocusedPaneId };
}

/** Mirrors layout.rs `resize`: is there a split border on `side` of leaf `id`? */
function hasBorder(node, id, side) {
  if (!node || isLeaf(node)) return false;
  const kids = node.children || [];
  const i = kids.findIndex((c) => containsLeaf(c, id));
  if (i < 0) return false;
  if (hasBorder(kids[i], id, side)) return true;
  const axisOk = side === 'left' || side === 'right' ? node.split === 'horizontal' : node.split === 'vertical';
  if (!axisOk) return false;
  return side === 'left' || side === 'up' ? i > 0 : i + 1 < kids.length;
}

/**
 * ResizePane arguments that move a border in direction `dir` (tmux-like): grow toward `dir`
 * when that border exists, else shrink from the opposite border. The tab layout is tried first,
 * then the group layout (the backend falls back to the group border only when the tab has no
 * border on the requested side, which this choice respects).
 */
export function resizeArgs(ws, paneId, dir) {
  const opp = OPPOSITE[dir];
  const p = ws.panes[paneId];
  const tab = p && ws.tabs[p.tab];
  const gid = tab && tabGroups(ws)[tab.id];
  const layers = [
    [tab && tab.layout, paneId],
    [ws.groupLayout, gid],
  ];
  for (const [layout, id] of layers) {
    if (!layout || id == null) continue;
    if (hasBorder(layout, id, dir)) return { side: dir, delta: RESIZE_STEP };
    if (hasBorder(layout, id, opp)) return { side: opp, delta: -RESIZE_STEP };
  }
  return { side: dir, delta: RESIZE_STEP };
}

const setFocus = (group, tab = null, pane = null) => ({ type: 'setFocus', group, tab, pane });

/**
 * WsCommands for a core command id given the workspace and the on-screen geometry
 * `{paneRects, groupRects}` (`{id,left,top,right,bottom}` of visible panes / groups).
 * Returns `[]` when there is nothing to do (edge, already there, pending id) and `null` for ids
 * that are not core commands.
 */
export function planCommand(id, ws, geo = {}) {
  if (PENDING_IDS.includes(id)) return [];
  const known = /^(pane|tab|group)\./.test(id);
  if (!known) return null;
  const c = chain(ws);
  if (!c) return [];
  const { g, tab, pane } = c;
  const paneRects = geo.paneRects || [];
  const groupRects = geo.groupRects || [];

  let m;
  if ((m = /^pane\.focus(Left|Right|Up|Down)$/.exec(id))) {
    const target = findNeighbor(paneRects, pane, DIRS[m[1]]);
    const tp = target && ws.panes[target];
    if (!tp) return [];
    const tg = tabGroups(ws)[tp.tab];
    return tg ? [setFocus(tg, tp.tab, target)] : [];
  }
  if ((m = /^pane\.resize(Left|Right|Up|Down)$/.exec(id))) {
    const { side, delta } = resizeArgs(ws, pane, DIRS[m[1]]);
    return [{ type: 'resizePane', pane, side, delta }];
  }
  if ((m = /^tab\.goto([1-9])$/.exec(id))) {
    const t = g.tabIds[Number(m[1]) - 1];
    return t && t !== tab.id ? [setFocus(g.id, t)] : [];
  }
  if ((m = /^group\.goto([1-9])$/.exec(id))) {
    const target = leafIds(ws.groupLayout).filter((x) => ws.groups[x])[Number(m[1]) - 1];
    if (!target || target === g.id) return [];
    const out = [];
    if (ws.groupMaximized && ws.groupMaximized !== target) out.push({ type: 'setGroupMaximized', group: null });
    out.push(setFocus(target));
    return out;
  }
  if ((m = /^tab\.move(Left|Right|Up|Down)$/.exec(id))) return planTabMove(ws, g, tab, DIRS[m[1]], groupRects);

  switch (id) {
    case 'pane.splitRight':
      return [{ type: 'splitPane', pane, dir: 'horizontal' }];
    case 'pane.splitDown':
      return [{ type: 'splitPane', pane, dir: 'vertical' }];
    case 'pane.close':
      return [{ type: 'closePane', pane }];
    case 'pane.restart':
      return [{ type: 'restartPane', pane }];
    case 'pane.maximize':
      return [{ type: 'setMaximized', tab: tab.id, pane: tab.maximizedPaneId ? null : pane }];
    case 'tab.prev':
    case 'tab.next': {
      const n = g.tabIds.length;
      if (n < 2) return [];
      const i = g.tabIds.indexOf(tab.id);
      const j = (i + (id === 'tab.next' ? 1 : -1) + n) % n;
      return [setFocus(g.id, g.tabIds[j])];
    }
    case 'tab.last': {
      const l = ws.lastLocation;
      if (!l || !ws.groups[l.groupId] || !ws.tabs[l.tabId]) return [];
      const out = [];
      if (ws.groupMaximized && ws.groupMaximized !== l.groupId) out.push({ type: 'setGroupMaximized', group: null });
      out.push(setFocus(l.groupId, l.tabId));
      return out;
    }
    case 'tab.dispatcher': {
      const d = Object.values(ws.tabs).find((t) => t.isDispatcher);
      const dg = d && tabGroups(ws)[d.id];
      if (!dg || d.id === tab.id) return [];
      const out = [];
      if (ws.groupMaximized && ws.groupMaximized !== dg) out.push({ type: 'setGroupMaximized', group: null });
      out.push(setFocus(dg, d.id));
      return out;
    }
    case 'group.splitRight':
      return [{ type: 'splitGroupWith', tab: tab.id, group: g.id, dir: 'horizontal', before: false }];
    case 'group.splitDown':
      return [{ type: 'splitGroupWith', tab: tab.id, group: g.id, dir: 'vertical', before: false }];
    case 'group.close':
      return [{ type: 'closeGroup', group: g.id }];
    case 'group.maximize':
      return [{ type: 'setGroupMaximized', group: ws.groupMaximized === g.id ? null : g.id }];
    default:
      return null;
  }
}

/**
 * `tab.moveLeft/Right`: reorder inside the bar; at the bar end, move to the neighbouring group
 * (left → its end, right → position 0). `tab.moveUp/Down`: append to the group above/below.
 * MoveTab.index is the final position after removal (out of range appends).
 */
function planTabMove(ws, g, tab, dir, groupRects) {
  const i = g.tabIds.indexOf(tab.id);
  if (dir === 'left' && i > 0) return [{ type: 'moveTab', tab: tab.id, group: g.id, index: i - 1 }];
  if (dir === 'right' && i >= 0 && i + 1 < g.tabIds.length) return [{ type: 'moveTab', tab: tab.id, group: g.id, index: i + 1 }];
  const ng = findNeighbor(groupRects, g.id, dir);
  const target = ng && ws.groups[ng];
  if (!target) return [];
  const index = dir === 'right' ? 0 : target.tabIds.length;
  return [{ type: 'moveTab', tab: tab.id, group: target.id, index }];
}

// ---------------------------------------------------------------- registry

const registry = new Map();

/**
 * Register (or replace) the handler for a command id. `fn(ctx)` receives whatever the caller
 * passes to runCommand (the app passes `{id, event, snapshot}`).
 */
export function registerCommand(id, fn) {
  registry.set(id, fn);
}

export function hasCommand(id) {
  return registry.has(id);
}

/** Run a registered handler; false when none is registered (the key is still swallowed). */
export function runCommand(id, ctx) {
  const fn = registry.get(id);
  if (!fn) return false;
  fn(ctx);
  return true;
}

/** Registered ids (for the palette). */
export function commandIds() {
  return [...registry.keys()];
}
