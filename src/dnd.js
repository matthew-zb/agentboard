// Mouse drag & drop of tabs: reorder / move between tab bars, drop on a group body
// (center = append, edge = split a new group). Pure planners first (Node-tested), DOM wiring below.
// No top-level DOM access.
import { dropZone } from './dropzone.js';

/**
 * Backend `index` (final position after the dragged tab is removed) for a drop on a tab bar.
 * `groupTabIds`: the target group's tab ids in order. `target`: tab id under the cursor (null =
 * empty bar area, i.e. the end). `after`: cursor is right of the target's center.
 * Returns null when the drop would not change anything.
 */
export function tabDropIndex(groupTabIds, dragTab, target, after) {
  const src = groupTabIds.indexOf(dragTab);
  let raw;
  if (target == null) raw = groupTabIds.length;
  else {
    const t = groupTabIds.indexOf(target);
    raw = t < 0 ? groupTabIds.length : after ? t + 1 : t;
  }
  if (src < 0) return raw; // moving in from another group
  const final = src < raw ? raw - 1 : raw; // removing yourself shifts everything right of you left
  return final === src ? null : final;
}

/** MoveTab for a tab-bar drop, or null for a no-op. */
export function tabBarDrop(groupTabIds, group, dragTab, target, after) {
  const index = tabDropIndex(groupTabIds, dragTab, target, after);
  return index == null ? null : { type: 'moveTab', tab: dragTab, group, index };
}

/** Edge zone to split direction (`before` = new group goes left/above). center gives null. */
export function zoneToSplit(zone) {
  switch (zone) {
    case 'left': return { dir: 'horizontal', before: true };
    case 'right': return { dir: 'horizontal', before: false };
    case 'top': return { dir: 'vertical', before: true };
    case 'bottom': return { dir: 'vertical', before: false };
    default: return null;
  }
}

/** Command for a drop on a group body in `zone`, or null for a no-op (own group, already last). */
export function bodyDrop(groupTabIds, group, dragTab, zone) {
  const split = zoneToSplit(zone);
  if (split) return { type: 'splitGroupWith', tab: dragTab, group, ...split };
  return tabBarDrop(groupTabIds, group, dragTab, null, false);
}

// ── DOM wiring ─────────────────────────────────────────────────────────────

const MARKS = ['drop-before', 'drop-after', 'drop-end'];

/**
 * Install drag & drop on `root` (#groups, which outlives re-renders; events are delegated).
 * deps: `getWs() -> Workspace|null`, `send(cmd) -> Promise` (errors are toasted by the caller),
 * `refocus()` (DOM focus back to the focused pane).
 * Tabs must be `draggable` (renderTabBar sets it).
 */
export function installDnd(root, { getWs, send, refocus }) {
  let drag = null; // {tab, token}
  let token = 0;

  const groupTabIds = (g) => {
    const ws = getWs();
    const grp = ws && ws.groups[g];
    return grp ? grp.tabIds : null;
  };

  function clearMarks() {
    for (const m of MARKS) for (const el of root.querySelectorAll('.' + m)) el.classList.remove(m);
  }

  function cleanup() {
    const was = drag;
    drag = null;
    for (const el of document.querySelectorAll('.drop-overlay')) el.remove();
    clearMarks();
    window.removeEventListener('mousemove', cleanup, true);
    window.removeEventListener('mouseup', cleanup, true);
    if (was) refocus();
  }

  function addOverlays(myToken) {
    if (!drag || drag.token !== myToken) return;
    for (const g of root.querySelectorAll('.group[data-group]')) {
      if (g.closest('#parking') || g.querySelector(':scope > .drop-overlay')) continue;
      const ov = document.createElement('div');
      ov.className = 'drop-overlay';
      const pv = document.createElement('div');
      pv.className = 'drop-preview hidden';
      ov.appendChild(pv);
      g.appendChild(ov);
    }
  }

  const preview = (ov, zone) => {
    const pv = ov.querySelector('.drop-preview');
    if (pv) pv.className = zone ? `drop-preview zone-${zone}` : 'drop-preview hidden';
  };

  const closest = (e, sel) => (e.target && e.target.closest ? e.target.closest(sel) : null);

  /** {tabEl|null, after} for the pointer position on a tab bar. */
  function barTarget(bar, e) {
    const tabEl = closest(e, '.tab[data-tab]');
    if (!tabEl || !bar.contains(tabEl)) return { tabEl: null, after: false };
    const r = tabEl.getBoundingClientRect();
    return { tabEl, after: e.clientX > r.left + r.width / 2 };
  }

  root.addEventListener('dragstart', (e) => {
    const tabEl = closest(e, '.tab[data-tab]');
    if (!tabEl) return;
    drag = { tab: tabEl.dataset.tab, token: ++token };
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = 'move';
      try { e.dataTransfer.setData('text/plain', drag.tab); } catch (_) { /* ignore */ }
    }
    // Mouse events do not fire during a native drag: seeing one means the drag ended without a
    // dragend (e.g. the dragged tab was re-rendered away mid-drag), so clean up.
    window.addEventListener('mousemove', cleanup, true);
    window.addEventListener('mouseup', cleanup, true);
    // Mutating the DOM inside dragstart can cancel the drag: add the overlays a frame later.
    const my = drag.token;
    requestAnimationFrame(() => addOverlays(my));
  });

  root.addEventListener('dragover', (e) => {
    if (!drag) return;
    const ov = closest(e, '.drop-overlay');
    const bar = closest(e, '.tabbar[data-group]');
    if (ov) {
      e.preventDefault();
      e.dataTransfer.dropEffect = 'move';
      preview(ov, dropZone(ov.getBoundingClientRect(), e.clientX, e.clientY));
    } else if (bar) {
      e.preventDefault();
      e.dataTransfer.dropEffect = 'move';
      clearMarks();
      const { tabEl, after } = barTarget(bar, e);
      if (tabEl) tabEl.classList.add(after ? 'drop-after' : 'drop-before');
      else bar.classList.add('drop-end');
    }
  });

  root.addEventListener('dragleave', (e) => {
    if (!drag) return;
    const ov = closest(e, '.drop-overlay');
    if (ov && !(e.relatedTarget && ov.contains(e.relatedTarget))) preview(ov, null);
    const bar = closest(e, '.tabbar[data-group]');
    if (bar && !(e.relatedTarget && bar.contains(e.relatedTarget))) clearMarks();
  });

  root.addEventListener('drop', (e) => {
    if (!drag) return;
    e.preventDefault();
    const tab = drag.tab;
    const ov = closest(e, '.drop-overlay');
    const bar = closest(e, '.tabbar[data-group]');
    let cmd = null;
    const ws = getWs();
    if (ws && ws.tabs[tab]) {
      if (ov) {
        const g = ov.closest('.group[data-group]').dataset.group;
        const ids = groupTabIds(g);
        if (ids) cmd = bodyDrop(ids, g, tab, dropZone(ov.getBoundingClientRect(), e.clientX, e.clientY));
      } else if (bar) {
        const g = bar.dataset.group;
        const ids = groupTabIds(g);
        if (ids) {
          const { tabEl, after } = barTarget(bar, e);
          cmd = tabBarDrop(ids, g, tab, tabEl ? tabEl.dataset.tab : null, after);
        }
      }
    }
    cleanup(); // also refocuses; the command's snapshot re-applies focus to the moved tab's pane
    if (cmd) send(cmd);
  });

  root.addEventListener('dragend', cleanup);
  document.addEventListener('keydown', (e) => {
    if (drag && e.key === 'Escape') cleanup();
  }, true);
  window.addEventListener('blur', () => {
    if (drag) cleanup();
  });
}
