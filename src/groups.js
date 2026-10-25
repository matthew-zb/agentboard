// Group layout rendering, sidebar, status bar text.
// No top-level DOM access: the pure helpers are unit-tested in Node.
import { renderLayout, leafIds, containsLeaf } from './layout.js';
import { renderTabBar } from './tabs.js';
import { statusSymbol } from './pane.js';

/** Focus chain: focused group → its active tab → that tab's lastFocusedPaneId. */
export function focusedPaneId(ws) {
  const g = ws && ws.focus && ws.groups[ws.focus.groupId];
  const t = g && ws.tabs[g.activeTabId];
  return t ? t.lastFocusedPaneId : null;
}

/** Groups drawn on screen: only the maximized one, else all in layout order. */
export function visibleGroupIds(ws) {
  if (ws.groupMaximized && containsLeaf(ws.groupLayout, ws.groupMaximized)) return [ws.groupMaximized];
  return leafIds(ws.groupLayout).filter((id) => ws.groups[id]);
}

/** Panes drawn on screen (active tab of each visible group; maximized pane only if set). */
export function visiblePaneIds(ws) {
  const out = [];
  for (const gid of visibleGroupIds(ws)) {
    const tab = ws.tabs[ws.groups[gid].activeTabId];
    if (!tab) continue;
    if (tab.maximizedPaneId && containsLeaf(tab.layout, tab.maximizedPaneId)) out.push(tab.maximizedPaneId);
    else out.push(...leafIds(tab.layout));
  }
  return out;
}

/** Changes exactly when the split DOM must be rebuilt (layouts, active tabs, maximize). */
export function layoutSignature(ws) {
  return JSON.stringify({
    layout: ws.groupLayout,
    max: ws.groupMaximized ?? null,
    groups: visibleGroupIds(ws).map((gid) => {
      const tab = ws.tabs[ws.groups[gid].activeTabId];
      return tab ? [gid, tab.id, tab.layout, tab.maximizedPaneId ?? null] : [gid, null];
    }),
  });
}

/** tabId → groupId. */
export function tabGroups(ws) {
  const m = {};
  for (const g of Object.values(ws.groups)) for (const t of g.tabIds) m[t] = g.id;
  return m;
}

/** Status bar contents: `그룹 › 탭 › 창`, sync flag of the focused tab, config error count. */
export function statusParts(snap) {
  const ws = snap.workspace;
  const g = ws.focus && ws.groups[ws.focus.groupId];
  const t = g && ws.tabs[g.activeTabId];
  const p = t && ws.panes[t.lastFocusedPaneId];
  return {
    path: [g, t, p].filter(Boolean).map((x) => x.name).join(' › '),
    sync: !!(t && t.syncInput),
    errors: snap.errors || 0,
  };
}

/**
 * Sidebar model: groups (layout order) > tabs > panes (layout order), with focus marks.
 */
export function sidebarTree(ws) {
  const fg = ws.focus && ws.focus.groupId;
  return leafIds(ws.groupLayout)
    .filter((gid) => ws.groups[gid])
    .map((gid) => {
      const g = ws.groups[gid];
      return {
        id: gid,
        name: g.name,
        focused: gid === fg,
        tabs: g.tabIds
          .filter((tid) => ws.tabs[tid])
          .map((tid) => {
            const t = ws.tabs[tid];
            return {
              id: tid,
              name: t.name,
              active: tid === g.activeTabId,
              panes: leafIds(t.layout)
                .filter((pid) => ws.panes[pid])
                .map((pid) => ({ id: pid, name: ws.panes[pid].name, symbol: statusSymbol(ws.panes[pid]).symbol })),
            };
          }),
      };
    });
}

/**
 * Build the group split DOM. Each group: `.group[data-group]` > `.tabbar-slot` + `.group-body`
 * (the active tab's pane layout). Tab bars are filled by renderGroupChrome.
 * ctx: `renderPane(paneId) -> HTMLElement`, `onSizes(tabId|null, path, sizes)`.
 */
export function renderGroups(ws, ctx) {
  const root = renderLayout(
    ws.groupLayout,
    (gid) => renderGroup(ws, gid, ctx),
    (path, sizes) => ctx.onSizes(null, path, sizes),
    ws.groupMaximized ?? null,
  );
  root.classList.add('groups-root');
  return root;
}

function renderGroup(ws, gid, ctx) {
  const g = ws.groups[gid];
  const el = document.createElement('div');
  el.className = 'group';
  el.dataset.group = gid;
  const slot = document.createElement('div');
  slot.className = 'tabbar-slot';
  const body = document.createElement('div');
  body.className = 'group-body';
  el.append(slot, body);
  const tab = g && ws.tabs[g.activeTabId];
  if (tab) {
    body.appendChild(
      renderLayout(tab.layout, ctx.renderPane, (path, sizes) => ctx.onSizes(tab.id, path, sizes), tab.maximizedPaneId ?? null),
    );
  }
  return el;
}

/** Refresh tab bars and focused/dim classes of every `.group` under `rootEl`. */
export function renderGroupChrome(rootEl, ws, ctx) {
  const fg = ws.focus && ws.focus.groupId;
  for (const el of rootEl.querySelectorAll('.group[data-group]')) {
    const g = ws.groups[el.dataset.group];
    if (!g) continue;
    const focused = g.id === fg;
    el.classList.toggle('focused', focused);
    el.classList.toggle('dim', !focused);
    const tabs = g.tabIds.map((t) => ws.tabs[t]).filter(Boolean);
    const bar = renderTabBar(g, tabs, focused, { panes: ws.panes, onSelect: (tid) => ctx.onSelectTab(g.id, tid), onRefocus: ctx.onRefocus });
    el.querySelector('.tabbar-slot').replaceChildren(bar);
  }
}

/** Fill `#sidebar` with the group > tab > pane tree. ctx: `onSelect(groupId, tabId, paneId|null)`. */
export function renderSidebar(el, ws, ctx) {
  const ul = document.createElement('ul');
  ul.className = 'tree';
  for (const g of sidebarTree(ws)) {
    const gl = item(ul, 'tree-group', g.name, g.focused);
    const tul = document.createElement('ul');
    gl.appendChild(tul);
    for (const t of g.tabs) {
      const tl = item(tul, 'tree-tab', t.name, t.active);
      tl.firstChild.addEventListener('mousedown', (e) => {
        e.preventDefault();
        ctx.onSelect(g.id, t.id, null);
      });
      const pul = document.createElement('ul');
      tl.appendChild(pul);
      for (const p of t.panes) {
        const pl = item(pul, 'tree-pane', `${p.symbol} ${p.name}`, false);
        pl.firstChild.addEventListener('mousedown', (e) => {
          e.preventDefault();
          ctx.onSelect(g.id, t.id, p.id);
        });
      }
    }
  }
  el.replaceChildren(ul);
}

function item(parent, cls, text, mark) {
  const li = document.createElement('li');
  const span = document.createElement('span');
  span.className = mark ? `${cls} mark` : cls;
  span.textContent = text;
  li.appendChild(span);
  parent.appendChild(li);
  return li;
}

/** Show/hide `#sidebar` (hidden by default). `force` true/false sets; omitted toggles. Returns visibility. */
export function toggleSidebar(force) {
  const el = document.getElementById('sidebar');
  if (!el) return false;
  const show = force === undefined ? el.classList.contains('hidden') : !!force;
  el.classList.toggle('hidden', !show);
  return show;
}
