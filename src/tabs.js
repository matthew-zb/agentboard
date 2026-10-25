// Tab bar rendering (one per group). Drag-and-drop arrives in a later task.
// No top-level DOM access: the pure helpers are unit-tested in Node.
import { leafIds } from './layout.js';

/** True when any pane of the tab shows ⚠ (attention or prompt warning). */
export function tabHasAttention(tab, panes) {
  return leafIds(tab.layout).some((id) => {
    const p = panes && panes[id];
    return !!p && (p.attention || p.promptWarning);
  });
}

/**
 * Tab bar for `group`. `tabs` are the group's Tab objects in order. `focused` = this is the
 * focused group; otherwise the bar gets `.dim`.
 * opts: `panes` (Workspace.panes, for the attention badge), `onSelect(tabId)` (tab click).
 */
export function renderTabBar(group, tabs, focused, opts = {}) {
  const { panes = {}, onSelect } = opts;
  const bar = document.createElement('div');
  bar.className = focused ? 'tabbar focused' : 'tabbar dim';
  bar.dataset.group = group.id;

  const name = document.createElement('span');
  name.className = 'group-name';
  name.textContent = group.name;
  bar.appendChild(name);

  for (const tab of tabs) {
    const el = document.createElement('div');
    el.className = 'tab';
    el.dataset.tab = tab.id;
    if (tab.id === group.activeTabId) el.classList.add('active');
    if (tab.isDispatcher) {
      el.classList.add('dispatcher');
      const icon = document.createElement('span');
      icon.className = 'tab-icon';
      icon.textContent = '◆';
      icon.title = 'Dispatcher (고정 탭)';
      el.appendChild(icon);
    }
    const label = document.createElement('span');
    label.className = 'tab-name';
    label.textContent = tab.name;
    el.appendChild(label);
    if (tabHasAttention(tab, panes)) {
      const badge = document.createElement('span');
      badge.className = 'tab-badge';
      badge.textContent = '⚠';
      badge.title = '주의가 필요한 창이 있습니다';
      el.appendChild(badge);
    }
    el.title = tab.dir ? `${tab.name} — ${tab.dir}` : tab.name;
    el.addEventListener('mousedown', (e) => {
      if (e.button !== 0) return;
      e.preventDefault(); // keep DOM focus in the terminal; render re-applies it
      if (onSelect) onSelect(tab.id);
    });
    bar.appendChild(el);
  }
  return bar;
}
