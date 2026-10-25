// Focus: attention cycling (pure), DOM focus following the focus chain, the 300ms highlight.
// Rule (spec §6.4): only user commands change focus. Snapshots caused by status/attention
// events re-apply the *model's* focus; they never pick a new pane.
// No top-level DOM access: the pure helpers are unit-tested in Node.
import { leafIds } from './layout.js';
import { focusedPaneId } from './groups.js';
import { focusPane } from './pane.js';

/** PaneRef = {group, tab, pane}. */

/** Every pane in reading order: groups (layout order) → tabs (bar order) → panes (layout order). */
export function paneOrder(ws) {
  const out = [];
  if (!ws) return out;
  for (const gid of leafIds(ws.groupLayout)) {
    const g = ws.groups[gid];
    if (!g) continue;
    for (const tid of g.tabIds) {
      const t = ws.tabs[tid];
      if (!t) continue;
      for (const pid of leafIds(t.layout)) if (ws.panes[pid]) out.push({ group: gid, tab: tid, pane: pid });
    }
  }
  return out;
}

/** The focus chain's end as a PaneRef, or null. */
export function currentRef(ws) {
  const pane = focusedPaneId(ws);
  if (!pane) return null;
  const g = ws.groups[ws.focus.groupId];
  return { group: g.id, tab: g.activeTabId, pane };
}

/**
 * `Alt+a`: the next attention pane after `current` (cyclic, skipping `current`). The first jump
 * remembers where it started (`origin`); when no other attention pane remains, go back to
 * `origin` and forget it. Returns `{target, origin}` (target null = nothing to do).
 */
export function nextAttention(order, current, attention, origin) {
  const same = (a, b) => !!a && !!b && a.pane === b.pane;
  const cands = order.filter((r) => attention.has(r.pane) && !same(r, current));
  if (cands.length === 0) return { target: origin || null, origin: null };
  const i = order.findIndex((r) => same(r, current)); // -1: start from the beginning
  const after = cands.find((r) => order.indexOf(r) > i);
  return { target: after || cands[0], origin: origin || current || null };
}

/** Pane ids that currently show attention. */
export function attentionSet(ws) {
  return new Set(Object.values((ws && ws.panes) || {}).filter((p) => p.attention).map((p) => p.id));
}

/**
 * Should a terminal bell in `paneId` send SetAttention{on:true}? Not when the pane already shows
 * attention (no spam) or is the focused pane of a focused window (the user is looking at it).
 * A bell in the focused pane of an unfocused window still flags it (cleared by the SetFocus
 * sent when the window regains focus).
 */
export function shouldFlagBell(ws, paneId, windowFocused) {
  const p = ws && ws.panes && ws.panes[paneId];
  if (!p || p.attention) return false;
  if (windowFocused && focusedPaneId(ws) === paneId) return false;
  return true;
}

/** True when the workspace's focus chain already reflects the requested `{group, tab?, pane?}`. */
export function focusMatches(ws, req) {
  if (!ws || !req || !ws.focus || ws.focus.groupId !== req.group) return false;
  const g = ws.groups[req.group];
  if (!g) return false;
  if (req.tab != null && g.activeTabId !== req.tab) return false;
  if (req.pane != null && focusedPaneId(ws) !== req.pane) return false;
  return true;
}

// ---------------------------------------------------------------- DOM

let pending = null; // a user SetFocus in flight: {group, tab, pane}

function isTextInput(el) {
  if (!el || el === document.body) return false;
  if (el.closest && el.closest('.xterm')) return false; // xterm's helper textarea
  return el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable;
}

function paneEl(paneId) {
  return document.querySelector(`.pane[data-pane="${CSS.escape(paneId)}"]`);
}

function onScreen(paneId) {
  const el = paneEl(paneId);
  return !!el && el.isConnected && !el.closest('#parking');
}

/**
 * A user focus change is about to be sent. Until `settleFocus`, snapshots that do not yet show it
 * (e.g. a status update emitted before the command) must not pull DOM focus back. A visible target
 * pane gets DOM focus right away.
 */
export function requestFocus(ref) {
  pending = ref ? { group: ref.group, tab: ref.tab ?? null, pane: ref.pane ?? null } : null;
  if (pending && pending.pane && onScreen(pending.pane)) focusPane(pending.pane);
}

/** The focus command was answered (or failed): reconcile DOM focus with `snapshot`. */
export function settleFocus(snapshot) {
  pending = null;
  applyFocus(snapshot);
}

/**
 * Give DOM focus (`term.focus()`) to the end of the snapshot's focus chain: focused group →
 * active tab → lastFocusedPaneId. Skipped while a text input (overlay) has focus, or while a
 * user focus request is pending and this snapshot does not reflect it yet.
 */
export function applyFocus(snapshot) {
  const ws = snapshot && snapshot.workspace;
  if (!ws) return;
  if (isTextInput(document.activeElement)) return;
  if (pending) {
    if (!focusMatches(ws, pending)) return;
    pending = null;
  }
  const id = focusedPaneId(ws);
  if (id) focusPane(id);
}

const FLASH_MS = 300;
const flashTimers = new Map();

/** One 300ms highlight of the pane border (after tab/group switches). */
export function flash(paneId) {
  if (!paneId) return;
  const el = paneEl(paneId);
  if (!el) return;
  clearTimeout(flashTimers.get(paneId));
  el.classList.remove('flash');
  void el.offsetWidth; // restart the transition when flashed twice in a row
  el.classList.add('flash');
  flashTimers.set(
    paneId,
    setTimeout(() => {
      el.classList.remove('flash');
      flashTimers.delete(paneId);
    }, FLASH_MS),
  );
}

/** `{group, tab}` of the focus chain (for "did the tab/group change?" checks). */
export function focusLocation(ws) {
  const r = ws && currentRef(ws);
  return r ? `${r.group}/${r.tab}` : null;
}
