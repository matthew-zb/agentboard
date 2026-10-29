// 집중근무 / 전달 멈춤 (focus-mode spec, decisions F1~F5 a): status flag, palette entries and the
// inbox view while a mode is on and after it is released.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { focusFlag, focusPaletteItems, inboxFocusView, FOCUS_HELD_LINE } from '../../src/focus-logic.js';
import { paletteItems } from '../../src/overlay-logic.js';
import { buildList } from '../../src/inbox.js';

const item = (id, created, state = 'new', read = false) => ({ id, created, state, read, title: `제목 ${id}`, ownerName: 'billing', source: 'handover' });
const ITEMS = [
  item('a', '2026-10-29T09:00:00+09:00', 'in_progress', true), // before the mode
  item('b', '2026-10-29T09:30:00+09:00', 'new'), // before
  item('c', '2026-10-29T10:10:00+09:00', 'new'), // during
  item('d', '2026-10-29T10:20:00+09:00', 'close_proposed'), // during, unread
  item('e', '2026-10-29T10:30:00+09:00', 'in_progress'), // during, unread
  item('f', '2026-10-29T10:40:00+09:00', 'done', true), // during, done
];
const SINCE = '2026-10-29T10:00:00+09:00';
const UNTIL = '2026-10-29T11:00:00+09:00';

test('status bar flag: [집중근무] / [전달 멈춤], nothing in normal mode', () => {
  assert.equal(focusFlag(null), null);
  assert.equal(focusFlag({ mode: 'normal' }), null);
  const f = focusFlag({ mode: 'focus', since: SINCE });
  assert.equal(f.text, '[집중근무]');
  assert.ok(f.title.includes('직접 물은 것만'));
  assert.equal(focusFlag({ mode: 'hold', since: SINCE }).text, '[전달 멈춤]');
});

test('palette: 집중근무 시작 · 전달 멈춤 · 집중근무 해제 offered from the current mode', () => {
  const labels = (mode) => focusPaletteItems({ mode }).map((i) => [i.label, i.action.mode]);
  assert.deepEqual(labels('normal'), [['집중근무 시작', 'focus'], ['전달 멈춤', 'hold']]);
  assert.deepEqual(labels('focus'), [['전달 멈춤', 'hold'], ['집중근무 해제', 'normal']]);
  assert.deepEqual(labels('hold'), [['집중근무 시작', 'focus'], ['집중근무 해제', 'normal']]);
  assert.deepEqual(focusPaletteItems(null), [], 'state unknown: no entries');
  for (const i of focusPaletteItems({ mode: 'normal' })) assert.equal(i.action.type, 'focus');
  // the palette lists them when the state is known
  const ws = { groupLayout: { leaf: 'g' }, groups: { g: { id: 'g', name: 'g', tabIds: ['t'], activeTabId: 't' } }, tabs: { t: { id: 't', name: 'x', isDispatcher: true, layout: { leaf: 'p' }, lastFocusedPaneId: 'p' } }, panes: { p: { id: 'p', tab: 't', name: 'p' } }, focus: { groupId: 'g' } };
  const got = paletteItems(ws, { focus: { mode: 'hold' } }).filter((i) => i.action.type === 'focus').map((i) => i.label);
  assert.deepEqual(got, ['집중근무 시작', '집중근무 해제']);
  assert.equal(paletteItems(ws, {}).filter((i) => i.action.type === 'focus').length, 0);
});

test('inbox while 집중근무: items that came in during the mode are hidden — no line at all', () => {
  const v = inboxFocusView(ITEMS, { mode: 'focus', since: SINCE }, { hideDone: true });
  assert.deepEqual(v.rest.map((i) => i.id), ['b', 'a']);
  assert.deepEqual([v.held, v.released], [0, []], 'complete silence: no (보류) line');
  assert.equal(v.hidden, 3, 'kept (counted) for later, open ones only');
});

test('inbox while 전달 멈춤: one "(보류 — N건)" line instead of the new items', () => {
  const v = inboxFocusView(ITEMS, { mode: 'hold', since: SINCE }, { hideDone: true });
  assert.deepEqual(v.rest.map((i) => i.id), ['b', 'a']);
  assert.equal(v.held, 3);
  assert.equal(FOCUS_HELD_LINE(3), '(보류 — 3건)');
});

test('after release: the "집중근무 중 들어온 항목" group on top — decisions, new, in progress', () => {
  const v = inboxFocusView(ITEMS, { mode: 'normal', released: { from: SINCE, to: UNTIL } }, { hideDone: true });
  assert.deepEqual(v.released.map((i) => i.id), ['d', 'c', 'e']);
  assert.deepEqual(v.rest.map((i) => i.id), ['b', 'a'], 'the rest as before; done items hidden');
  assert.equal(v.held, 0);
  // the group is what was not seen yet: a read item of the window goes back to the usual list,
  // and once every one is read or done the group is gone
  const rel = { mode: 'normal', released: { from: SINCE, to: UNTIL } };
  const g = item('g', '2026-10-29T10:50:00+09:00', 'new', true);
  const withRead = inboxFocusView([...ITEMS, g], rel, { hideDone: true });
  assert.deepEqual(withRead.released.map((i) => i.id), ['d', 'c', 'e']);
  assert.ok(withRead.rest.some((i) => i.id === 'g'));
  const seen = ITEMS.map((it) => (it.id === 'c' || it.id === 'e' ? { ...it, read: true } : it.id === 'd' ? { ...it, state: 'done' } : it));
  const after = inboxFocusView(seen, rel, { hideDone: true });
  assert.deepEqual(after.released, []);
  assert.deepEqual(after.rest.map((i) => i.id), ['b', 'e', 'a', 'c'], 'back in the usual order (a read new item sorts after in progress)');
  const doc = fakeDoc();
  assert.ok(!tree(buildList(doc, after.rest, { total: 6, released: after.released })).some((e) => e.classList.contains('inbox-group')), 'no group line');
  // no mode, nothing released: the usual list
  const plain = inboxFocusView(ITEMS, { mode: 'normal' }, { hideDone: false });
  assert.deepEqual(plain.released, []);
  assert.equal(plain.rest.length, 6);
  assert.deepEqual(inboxFocusView(ITEMS, null, { hideDone: true }).rest.map((i) => i.id), ['d', 'c', 'b', 'e', 'a'], 'the usual order (sortItems)');
});

// ---- the inbox list draws the line and the group (text only)
function fakeDoc() {
  const mk = (tag) => {
    const classes = new Set();
    return {
      tagName: String(tag).toUpperCase(),
      children: [],
      dataset: {},
      _text: '',
      classList: { add: (c) => classes.add(c), remove: (c) => classes.delete(c), toggle: (c, on) => ((on === undefined ? !classes.has(c) : on) ? classes.add(c) : classes.delete(c)), contains: (c) => classes.has(c) },
      set className(v) {
        classes.clear();
        String(v).split(/\s+/).filter(Boolean).forEach((c) => classes.add(c));
      },
      get className() {
        return [...classes].join(' ');
      },
      set textContent(v) {
        this._text = String(v);
        this.children = [];
      },
      get textContent() {
        return this._text + this.children.map((c) => c.textContent).join('');
      },
      append(...c) {
        this.children.push(...c);
      },
      appendChild(c) {
        this.children.push(c);
        return c;
      },
    };
  };
  return { createElement: mk };
}
const tree = (e) => [e, ...(e.children || []).flatMap(tree)];

test('buildList: the held line and the released group, rows in view order', () => {
  const doc = fakeDoc();
  const held = buildList(doc, [ITEMS[1]], { selected: 'b', total: 6, held: 3 });
  const line = tree(held).find((e) => e.classList.contains('inbox-held'));
  assert.equal(line.textContent, '(보류 — 3건)');
  const v = inboxFocusView(ITEMS, { mode: 'normal', released: { from: SINCE, to: UNTIL } }, { hideDone: true });
  const rel = buildList(doc, v.rest, { selected: 'c', total: 6, released: v.released });
  const group = tree(rel).find((e) => e.classList.contains('inbox-group'));
  assert.equal(group.textContent, '집중근무 중 들어온 항목 3건');
  assert.deepEqual(tree(rel).filter((e) => e.classList.contains('inbox-row')).map((r) => r.dataset.id), ['d', 'c', 'e', 'b', 'a']);
  assert.ok(tree(rel).find((r) => r.dataset.id === 'c').classList.contains('sel'));
  // without the options nothing changes
  const plain = buildList(doc, ITEMS.slice(0, 2), { total: 2 });
  assert.ok(!tree(plain).some((e) => e.classList.contains('inbox-held') || e.classList.contains('inbox-group')));
});

// ---- the backend contract (ab-core C3): focus_get / focus-changed → {mode, since}
import { nextFocus, makeFocusSync } from '../../src/focus-logic.js';

const NOW = '2026-10-29T11:00:00.000Z';

test('nextFocus: a mode keeps its since; leaving it keeps the window as released until the next mode', () => {
  assert.deepEqual(nextFocus(null, { mode: 'normal', since: null }, NOW), { mode: 'normal', since: null }, 'start in normal: nothing released');
  const on = nextFocus(null, { mode: 'focus', since: SINCE }, NOW);
  assert.deepEqual(on, { mode: 'focus', since: SINCE });
  const hold = nextFocus(on, { mode: 'hold', since: SINCE }, NOW);
  assert.deepEqual(hold, { mode: 'hold', since: SINCE }, 'focus ↔ hold: the same period');
  const off = nextFocus(hold, { mode: 'normal', since: null }, NOW);
  assert.deepEqual(off, { mode: 'normal', since: null, released: { from: SINCE, to: NOW } });
  assert.deepEqual(nextFocus(off, { mode: 'normal', since: null }, '2026-10-29T12:00:00Z'), off, 'normal again: the window stays');
  assert.deepEqual(nextFocus(off, { mode: 'hold', since: UNTIL }, NOW), { mode: 'hold', since: UNTIL }, 'the next mode replaces it');
  assert.equal(nextFocus(on, { mode: 'x' }, NOW), on, 'an unknown payload keeps the state');
  assert.equal(nextFocus(null, null, NOW), null);
  // the released window feeds the inbox view (the release time is the end of the window)
  const v = inboxFocusView(ITEMS, nextFocus({ mode: 'focus', since: SINCE }, { mode: 'normal', since: null }, UNTIL), { hideDone: true });
  assert.deepEqual(v.released.map((i) => i.id), ['d', 'c', 'e']);
});

test('makeFocusSync: reads focus_get on refresh, applies only the latest answer, keeps the state on failure', async () => {
  const answers = [];
  const get = () => new Promise((resolve, reject) => answers.push({ resolve, reject }));
  const applied = [];
  const sync = makeFocusSync({ get, apply: (s) => applied.push(s), now: () => NOW });
  assert.equal(sync.state, null, 'unknown until the first answer');
  const first = sync.refresh();
  const second = sync.refresh(); // focus-changed events may arrive out of order: re-read, latest wins
  await Promise.resolve();
  answers[1].resolve({ mode: 'hold', since: SINCE });
  answers[0].resolve({ mode: 'normal', since: null }); // older answer, late
  await Promise.all([first, second]);
  assert.deepEqual(applied, [{ mode: 'hold', since: SINCE }]);
  assert.deepEqual(sync.state, { mode: 'hold', since: SINCE });
  const third = sync.refresh();
  await Promise.resolve();
  answers[2].resolve({ mode: 'normal', since: null });
  await third;
  assert.deepEqual(sync.state, { mode: 'normal', since: null, released: { from: SINCE, to: NOW } });
  const fourth = sync.refresh();
  await Promise.resolve();
  answers[3].reject(new Error('gone'));
  await fourth;
  assert.equal(applied.length, 2, 'a failed read applies nothing');
  assert.deepEqual(sync.state.mode, 'normal');
});
