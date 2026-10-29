// Status view pure logic (spec 2026-10-26-agentboard-dashboard-design.md §2, §4): the snapshot's
// role tabs and panes, the inbox's open items per owner tab and board_status (session state
// from hooks, nightly refresh) as rows — whatever the layout decision (D1) turns out to be.
// No DOM, no Tauri: unit-tested in Node. The view (not built yet) draws these rows.
import { leafIds } from './layout.js';

/** Context size above this is shown as too large (the cap the psmux session board used). */
export const CONTEXT_CAP = 400000;
/** Characters of a last prompt / reply shown in a row. */
export const TEXT_SHOW = 60;

const STANDING = new Set(['dispatcher', 'handover', 'manager']);
const TAB_RANK = { dispatcher: 0, handover: 1, manager: 2, plain: 3 };
const zeroCounts = () => ({ new: 0, unread: 0, inProgress: 0, closeProposed: 0, open: 0 });

/** Role of a tab in the status view; 'plain' tabs are listed only when one of their panes has a
 * board entry (a claude session the backend matched, D3b). */
function tabRole(t) {
  if (t.isDispatcher) return 'dispatcher';
  return t.role === 'handover' || t.role === 'manager' ? t.role : 'plain';
}

const boardEntry = (board, paneId) => {
  const panes = board && typeof board === 'object' ? board.panes : null;
  const e = panes && typeof panes === 'object' ? panes[paneId] : null;
  return e && typeof e === 'object' ? e : null;
};

/** Open (state ≠ done) inbox items per owner tab name (R29: ownership is the tab name). */
export function inboxOpenCounts(items) {
  const by = new Map();
  for (const it of Array.isArray(items) ? items : []) {
    if (!it || it.state === 'done' || !it.ownerName) continue;
    if (!by.has(it.ownerName)) by.set(it.ownerName, zeroCounts());
    const c = by.get(it.ownerName);
    if (it.state === 'new') {
      c.new++;
      if (!it.read) c.unread++;
    } else if (it.state === 'in_progress') c.inProgress++;
    else if (it.state === 'close_proposed') c.closeProposed++;
    c.open++;
  }
  return Object.fromEntries(by);
}

/** `contextTokens` → `{text: '123k', over}` (over = above CONTEXT_CAP); null when unknown. */
export function contextLabel(tokens) {
  if (typeof tokens !== 'number' || !Number.isFinite(tokens) || tokens < 0) return null;
  return { text: `${Math.round(tokens / 1000)}k`, over: tokens > CONTEXT_CAP };
}

/** One display line: whitespace folded, at most `max` characters (code points) + `…`; null
 * when empty. The backend's "메시지: <보낸 이>" summary of peer messages passes unchanged. */
export function shortText(s, max = TEXT_SHOW) {
  if (typeof s !== 'string') return null;
  const t = s.replace(/\s+/g, ' ').trim();
  if (!t) return null;
  const cps = [...t];
  return cps.length > max ? `${cps.slice(0, max).join('')}…` : t;
}

/** The board_status fields of one pane; unknown (no hook yet, no board) = nulls. */
function boardFields(board, paneId) {
  const e = boardEntry(board, paneId);
  if (!e) return { idle: null, lastPrompt: null, lastReply: null, context: null, model: null, updatedAt: null };
  return {
    idle: typeof e.idle === 'boolean' ? e.idle : null,
    lastPrompt: shortText(e.lastPrompt),
    lastReply: shortText(e.lastReply),
    context: contextLabel(e.contextTokens),
    model: typeof e.model === 'string' && e.model ? e.model : null,
    updatedAt: Number.isFinite(e.updatedAt) ? e.updatedAt : null,
  };
}

const isApplet = (p) => !!(p.kind && p.kind.type === 'applet');

/** Panes of a tab in view order: standing pane(s), then per team (first seen in the layout) its
 * lead(s) and members, then panes without a role that have a board entry (claude started in a
 * shell, D3b) — all in layout order. Shells without a session and applets are out. */
function orderedPanes(ws, t, board) {
  const all = leafIds(t.layout).map((id) => ws.panes[id]).filter((p) => p && !isApplet(p));
  const panes = all.filter((p) => p.role);
  const out = panes.filter((p) => STANDING.has(p.role)).map((p) => [p, 0]);
  const team = panes.filter((p) => p.role === 'lead' || p.role === 'member');
  const teamIds = [...new Set(team.map((p) => p.team ?? ''))];
  for (const id of teamIds) {
    const mine = team.filter((p) => (p.team ?? '') === id);
    for (const p of mine.filter((q) => q.role === 'lead')) out.push([p, 1]);
    for (const p of mine.filter((q) => q.role === 'member')) out.push([p, 2]);
  }
  for (const p of all.filter((q) => !q.role && boardEntry(board, q.id))) out.push([p, 0]);
  return out;
}

/**
 * Status view model from a Snapshot (or bare Workspace), the inbox items (`inbox_list`) and
 * `board_status()`: one entry per role tab — dispatcher, handover, then Manager tabs in reading
 * order (groups → tabs) — `{tab, group, name, role, inbox: counts, rows, hidden: 0}`; a row per
 * agent pane `{pane, tab, group, name, role, team, sessionName, depth (0 standing · 1 lead ·
 * 2 member), status, attention, reason, idle, lastPrompt, lastReply, context, model, updatedAt}`.
 * Plain tabs (role 'plain') come last and only when a pane of theirs has a board entry (D3b).
 */
export function dashboardRows(snapshot, { inbox = [], board = null } = {}) {
  const ws = snapshot && (snapshot.workspace || snapshot);
  if (!ws || !ws.groups || !ws.groupLayout || !ws.tabs || !ws.panes) return [];
  const counts = inboxOpenCounts(inbox);
  const out = [];
  for (const gid of leafIds(ws.groupLayout)) {
    const g = ws.groups[gid];
    for (const tid of (g && g.tabIds) || []) {
      const t = ws.tabs[tid];
      const role = t && tabRole(t);
      if (!role) continue;
      const rows = orderedPanes(ws, t, board).map(([p, depth]) => ({
        pane: p.id,
        tab: tid,
        group: gid,
        name: p.name,
        role: p.role ?? null,
        team: p.team ?? null,
        sessionName: p.sessionName ?? null,
        depth,
        status: p.status || {},
        attention: !!p.attention,
        reason: p.attentionReason || null,
        ...boardFields(board, p.id),
      }));
      if (role === 'plain' && !rows.length) continue; // no claude session there
      out.push({ tab: tid, group: gid, name: t.name, role, inbox: counts[t.name] || zeroCounts(), rows, hidden: 0 });
    }
  }
  return out.sort((a, b) => TAB_RANK[a.role] - TAB_RANK[b.role]); // stable: reading order kept
}

const ended = (status) => !!status && (status.state === 'exited' || status.state === 'failed');

/** `h`: with `hideIdle`, rows of idle (Stop seen) or ended panes are left out (`hidden` = how
 * many); every tab entry stays so its header and inbox counts remain. */
export function visibleGroups(groups, hideIdle) {
  if (!hideIdle) return groups;
  return groups.map((g) => {
    const rows = g.rows.filter((r) => !(r.idle === true || ended(r.status)));
    return { ...g, rows, hidden: g.rows.length - rows.length };
  });
}

/** All rows in view order (selection moves over this list). */
export function flatRows(groups) {
  return (groups || []).flatMap((g) => g.rows);
}

const KEYS = {
  KeyJ: 'down',
  ArrowDown: 'down',
  KeyK: 'up',
  ArrowUp: 'up',
  Enter: 'jump',
  NumpadEnter: 'jump',
  KeyR: 'reload',
  KeyH: 'toggleIdle',
};
const REPEATS = new Set(['down', 'up']);

/** Status view action for a keydown (by `KeyboardEvent.code`), or null: j/k/↑↓ move, Enter
 * jump, r reload, h hide idle. Chords (app shortcuts) and IME composition never match; a held
 * key repeats only the movement. */
export function dashboardKeyAction(e) {
  if (!e || e.ctrlKey || e.altKey || e.shiftKey || e.metaKey || e.isComposing) return null;
  const a = KEYS[e.code] || null;
  if (a && e.repeat && !REPEATS.has(a)) return null;
  return a;
}

/** Enter on a row: focus its pane (a user action, so the no-auto-focus rule holds). */
export function jumpCommand(row) {
  return row ? { type: 'setFocus', group: row.group, tab: row.tab, pane: row.pane } : null;
}

const PHASES = { idle: '대기', notifying: '알리는 중', waiting: '보고 기다림', restarting: '재시작' };

/** One line for board_status().nightly, e.g. `야간 정리: 보고 기다림(2) · 마지막 10-26`. */
export function nightlySummary(n) {
  if (!n || typeof n !== 'object') return null;
  const pending = Array.isArray(n.pending) ? n.pending.length : 0;
  const held = Array.isArray(n.held) ? n.held.length : 0;
  let label = PHASES[n.phase] || String(n.phase ?? '');
  if (n.phase === 'waiting' && pending) label += `(${pending})`;
  const parts = [`야간 정리: ${label}`];
  if (held) parts.push(`보류 ${held}`);
  const day = typeof n.lastRun === 'string' && /^\d{4}-\d\d-\d\d$/.test(n.lastRun) ? n.lastRun.slice(5) : n.lastRun;
  parts.push(day ? `마지막 ${day}` : '실행 기록 없음');
  return parts.join(' · ');
}
