// Tab bar rendering (one per group). Drag-and-drop itself lives in dnd.js (delegated on #groups).
// No top-level DOM access: the pure helpers are unit-tested in Node.
import { leafIds } from './layout.js';

/** True when any pane of the tab shows ⚠ (attention or prompt warning). */
export function tabHasAttention(tab, panes) {
  return leafIds(tab.layout).some((id) => {
    const p = panes && panes[id];
    return !!p && (p.attention || p.promptWarning);
  });
}

/** Tooltip of a tab's ⚠: a fixed line plus `pane name: reason` for each pane with a reason. */
export function tabAttentionTitle(tab, panes) {
  const lines = ['주의가 필요한 창이 있습니다'];
  for (const id of leafIds(tab.layout)) {
    const p = panes && panes[id];
    if (p && p.attention && p.attentionReason) lines.push(`${p.name}: ${p.attentionReason}`);
  }
  return lines.join('\n');
}

/**
 * Tab bar for `group`. `tabs` are the group's Tab objects in order. `focused` = this is the
 * focused group; otherwise the bar gets `.dim`.
 * opts: `panes` (Workspace.panes, for the attention badge), `onSelect(tabId)` (tab click),
 * `onRefocus()` (after a click: DOM focus back to the pane).
 */
export function renderTabBar(group, tabs, focused, opts = {}) {
  const { panes = {}, onSelect, onRefocus } = opts;
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
      badge.title = tabAttentionTitle(tab, panes);
      el.appendChild(badge);
    }
    el.title = tab.dir ? `${tab.name} — ${tab.dir}` : tab.name;
    // draggable: a mousedown preventDefault would suppress the drag, and a command sent on
    // mousedown would re-render (and detach) the tab before dragstart. So select on click; the
    // terminal loses DOM focus on mousedown and gets it back right after.
    el.draggable = true;
    el.addEventListener('click', (e) => {
      if (e.button !== 0) return;
      if (onSelect) onSelect(tab.id);
      if (onRefocus) onRefocus();
    });
    bar.appendChild(el);
  }
  return bar;
}
