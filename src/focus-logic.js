// 집중근무 / 전달 멈춤 (focus-mode spec, decisions F1~F5 a), the parts the app draws: the status
// bar flag, the palette entries and the inbox view. While 집중근무 is on, inbox items that came in
// during the mode are hidden — kept and counted, no line at all (complete silence); while 전달
// 멈춤 is on they collapse into one "(보류 — N건)" line; after release the items of that window
// come first as a group, decisions (close_proposed) → new → in progress.
// Focus state shape used here (the backend's focus_status maps onto it):
//   { mode: 'normal' | 'focus' | 'hold', since?: ISO time the current mode started,
//     released?: { from: ISO, to: ISO } — the window of the last mode, after its release }
// No DOM, no Tauri: unit-tested in Node.
import { sortItems, visibleItems } from './inbox-logic.js';

const FLAGS = {
  focus: { text: '[집중근무]', title: '집중근무 — 다른 에이전트의 보고·알림을 전하지 않습니다. 직접 물은 것만 답합니다. 들어온 인박스 항목은 해제할 때 보여 줍니다' },
  hold: { text: '[전달 멈춤]', title: '전달 멈춤 — 직접 요청한 일의 답만 전하고, 나머지는 "(보류 — N건)" 한 줄로만 알립니다' },
};

/** Status bar flag `{text, title}` for the mode, or null (normal / unknown). */
export function focusFlag(focus) {
  return (focus && FLAGS[focus.mode]) || null;
}

const ENTRY = {
  focus: { label: '집중근무 시작', hint: '다른 에이전트의 보고를 전하지 않음' },
  hold: { label: '전달 멈춤', hint: '보류 건수만 한 줄로' },
  normal: { label: '집중근무 해제', hint: '보류한 항목 정리' },
};
const OFFER = { normal: ['focus', 'hold'], focus: ['hold', 'normal'], hold: ['focus', 'normal'] };

/** Palette entries for switching from the current mode; none while the state is unknown. */
export function focusPaletteItems(focus) {
  const offer = focus && OFFER[focus.mode];
  return (offer || []).map((mode) => ({ label: ENTRY[mode].label, hint: ENTRY[mode].hint, action: { type: 'focus', mode } }));
}

/** The line shown on top of the inbox while 전달 멈춤 holds `n` items. */
export const FOCUS_HELD_LINE = (n) => `(보류 — ${n}건)`;

const time = (s) => {
  const t = Date.parse(s);
  return Number.isFinite(t) ? t : NaN;
};
const inWindow = (it, from, to) => {
  const t = time(it.created);
  return Number.isFinite(t) && t >= from && (to === undefined || t <= to);
};

const RELEASE_ORDER = { close_proposed: 0, new: 1, in_progress: 2 };

/**
 * The inbox under the focus state: `{rest, held, hidden, released}` — `rest` the usual list
 * (visibleItems), `held` the count for the "(보류 — N건)" line (전달 멈춤), `hidden` the open items
 * kept out of sight (집중근무), `released` the group shown first after a release.
 */
export function inboxFocusView(items, focus, { hideDone = true } = {}) {
  const all = Array.isArray(items) ? items : [];
  const mode = focus && focus.mode;
  if ((mode === 'focus' || mode === 'hold') && Number.isFinite(time(focus.since))) {
    const from = time(focus.since);
    const during = all.filter((it) => inWindow(it, from));
    const open = during.filter((it) => it.state !== 'done').length;
    const rest = visibleItems(all.filter((it) => !during.includes(it)), { hideDone });
    return { rest, held: mode === 'hold' ? open : 0, hidden: mode === 'focus' ? open : 0, released: [] };
  }
  const r = focus && focus.released;
  if (r && Number.isFinite(time(r.from)) && Number.isFinite(time(r.to))) {
    const group = all.filter((it) => it.state !== 'done' && inWindow(it, time(r.from), time(r.to)));
    const released = sortItems(group).sort((a, b) => (RELEASE_ORDER[a.state] ?? 3) - (RELEASE_ORDER[b.state] ?? 3)); // stable: newest first kept
    const rest = visibleItems(all.filter((it) => !group.includes(it)), { hideDone });
    return { rest, held: 0, hidden: 0, released };
  }
  return { rest: visibleItems(all, { hideDone }), held: 0, hidden: 0, released: [] };
}
