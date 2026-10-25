// Attention list of the status bar (spec §5-1): ⚠ tabs the user cannot see right now.
// Pure (no DOM): unit-tested in Node.
import { leafIds } from './layout.js';
import { visibleGroupIds } from './groups.js';

/** Characters of a reason shown in the status bar; the full text is in the tooltip. */
export const REASON_SHOW = 20;

/**
 * Tabs with an attention pane whose tab bar is off screen (their group is hidden, e.g. by a
 * group maximize), in reading order (groups → tabs). Accepts a Snapshot or a bare Workspace.
 * Returns `{group, tab, name, pane, reason?}[]`: `pane` = the tab's first attention pane (the
 * click / jump target), `reason` = the first attention reason in the tab, if any.
 * Prompt warnings are not "needs the user" and are not listed.
 */
export function hiddenAttentionTabs(snapshot) {
  const ws = snapshot && (snapshot.workspace || snapshot);
  if (!ws || !ws.groups || !ws.tabs || !ws.panes || !ws.groupLayout) return [];
  const shown = new Set(visibleGroupIds(ws));
  const out = [];
  for (const gid of leafIds(ws.groupLayout)) {
    const g = ws.groups[gid];
    if (!g || shown.has(gid)) continue;
    for (const tid of g.tabIds) {
      const t = ws.tabs[tid];
      if (!t) continue;
      const flagged = leafIds(t.layout)
        .map((id) => ws.panes[id])
        .filter((p) => p && p.attention);
      if (!flagged.length) continue;
      const entry = { group: gid, tab: tid, name: t.name, pane: flagged[0].id };
      const withReason = flagged.find((p) => p.attentionReason);
      if (withReason) entry.reason = withReason.attentionReason;
      out.push(entry);
    }
  }
  return out;
}

/** Status bar text of one entry: `name` or `name(reason…)` with the reason cut to REASON_SHOW. */
export function statusEntryText(entry) {
  if (!entry.reason) return entry.name;
  const chars = [...entry.reason];
  const r = chars.length > REASON_SHOW ? `${chars.slice(0, REASON_SHOW).join('')}…` : entry.reason;
  return `${entry.name}(${r})`;
}
