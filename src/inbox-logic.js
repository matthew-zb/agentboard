// Inbox applet pure logic (spec §5, Task 10): ordering, filtering, Korean labels, key → action,
// which actions an item allows. No DOM, no Tauri: unit-tested in Node. inbox.js draws.

const LABELS = { new: '새 항목', in_progress: '진행 중', close_proposed: '종료 제안됨', done: '완료' };

/** Korean label of an item state (unknown states are shown as is). */
export function stateLabel(state) {
  return LABELS[state] || String(state);
}

/** Sort group: close_proposed → new & unread → in_progress → everything else. */
function group(it) {
  if (it.state === 'close_proposed') return 0;
  if (it.state === 'new' && !it.read) return 1;
  if (it.state === 'in_progress') return 2;
  return 3;
}

const cmpDesc = (a, b) => (a < b ? 1 : a > b ? -1 : 0);

/** New array: by group (see `group`), newest first inside a group (created, then id). */
export function sortItems(items) {
  return [...(items || [])].sort(
    (a, b) => group(a) - group(b) || cmpDesc(a.created || '', b.created || '') || cmpDesc(a.id || '', b.id || ''),
  );
}

/** Items the list shows: done ones dropped while `hideDone`, then sorted. */
export function visibleItems(items, { hideDone = true } = {}) {
  return sortItems((items || []).filter((it) => !(hideDone && it.state === 'done')));
}

const KEYS = {
  KeyJ: 'down',
  ArrowDown: 'down',
  KeyK: 'up',
  ArrowUp: 'up',
  Enter: 'open',
  NumpadEnter: 'open',
  Escape: 'back',
  KeyI: 'instruct',
  KeyS: 'save',
  KeyU: 'issue',
  KeyC: 'close',
  KeyH: 'toggleDone',
};
const REPEATS = new Set(['down', 'up']);

/**
 * Applet action for a keydown (by `KeyboardEvent.code`, so the Korean layout works), or null.
 * Any Ctrl/Alt/Shift/Meta chord is not an applet key (app shortcuts stay with the key router);
 * IME composition is ignored; a held key repeats only j/k/↑/↓.
 */
export function keyAction(e) {
  if (!e || e.ctrlKey || e.altKey || e.shiftKey || e.metaKey || e.isComposing) return null;
  const a = KEYS[e.code] || null;
  if (a && e.repeat && !REPEATS.has(a)) return null;
  return a;
}

/**
 * Whether `action` is available for `item`: `close` (종료 확인) only for close_proposed;
 * instruct / save / issue for any item that is not done (`issue` also needs no linked issue).
 */
export function canDo(action, item) {
  if (!item) return false;
  const open = item.state !== 'done';
  switch (action) {
    case 'close':
      return item.state === 'close_proposed';
    case 'instruct':
    case 'save':
      return open;
    case 'issue':
      return open && !item.issue;
    default:
      return false;
  }
}

/** Selection after j/k: the neighbour of `selId` (clamped, no wrap); first item if gone. */
export function moveSelection(list, selId, delta) {
  if (!list || !list.length) return null;
  const i = list.findIndex((it) => it.id === selId);
  if (i < 0) return list[0].id;
  return list[Math.max(0, Math.min(list.length - 1, i + delta))].id;
}

/**
 * Keep `selId` when it is still listed; otherwise the item now at `prevIndex` (its old
 * position, clamped), or the first item without one. Null for an empty list.
 */
export function keepSelection(list, selId, prevIndex = 0) {
  if (!list || !list.length) return null;
  if (list.some((it) => it.id === selId)) return selId;
  const i = Math.max(0, Math.min(list.length - 1, Number.isInteger(prevIndex) ? prevIndex : 0));
  return list[i].id;
}

/**
 * Plain Tab / Shift+Tab inside the applet: swallowed so the browser's focus traversal cannot
 * move DOM focus into a neighbouring terminal (whose prompt would then receive j/i/c…).
 * Chords with Ctrl/Alt/Meta are left to the app key router.
 */
export function isFocusTrapKey(e) {
  return !!e && e.code === 'Tab' && !e.ctrlKey && !e.altKey && !e.metaKey;
}

/**
 * After a re-render replaced the applet's nodes: give focus back to the root when it was inside
 * before (`hadFocus`) and is now on `body`/nowhere — not when the user moved it elsewhere.
 */
export function focusLost(hadFocus, active, root, body) {
  if (!hadFocus) return false;
  if (!active || active === body) return true;
  return false;
}

/** `s`: 'save' when the item has an issue folder, 'needIssue' when it must be requested first. */
export function saveStep(item) {
  if (!canDo('save', item)) return null;
  return item.issue ? 'save' : 'needIssue';
}

/** Paths typed/pasted one per line; surrounding quotes (Explorer "경로로 복사") removed. */
export function parsePaths(text) {
  const out = [];
  for (const raw of String(text ?? '').split(/\r?\n|\r/)) {
    let p = raw.trim();
    if (p.length >= 2 && ((p[0] === '"' && p.endsWith('"')) || (p[0] === "'" && p.endsWith("'")))) p = p.slice(1, -1).trim();
    if (p && !out.includes(p)) out.push(p);
  }
  return out;
}

const TIMELINE = {
  posted: '등록',
  note: '메모',
  instruction: '지시',
  files_added: '자료 저장',
  issue_linked: '이슈 연결',
  issue_requested: '이슈 등록 요청',
  close_proposed: '종료 제안',
  reopened: '다시 진행',
  done: '완료',
};

/** Korean label of a timeline entry kind (`{type, …}`). */
export function timelineLabel(kind) {
  const t = kind && kind.type;
  return TIMELINE[t] || String(t);
}

/** 경로: how the item reached its owner. */
export function routeText(item) {
  if (item.source === 'app') return `앱 → ${item.ownerName}`;
  return `handover → dispatcher → ${item.ownerName}`;
}

/** `2026-10-25T14:03:07+09:00` → `10-25 14:03` (local time as stored). */
export function shortTime(at) {
  const m = /^\d{4}-(\d\d-\d\d)T(\d\d:\d\d)/.exec(String(at || ''));
  return m ? `${m[1]} ${m[2]}` : '';
}
