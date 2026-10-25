import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  stateLabel,
  sortItems,
  visibleItems,
  keyAction,
  canDo,
  moveSelection,
  keepSelection,
  isFocusTrapKey,
  focusLost,
  saveStep,
  parsePaths,
  timelineLabel,
  routeText,
  shortTime,
} from '../../src/inbox-logic.js';
import { buildList, buildDetail, flatFiles } from '../../src/inbox.js';

const item = (id, state, read, extra = {}) => ({
  id,
  ownerTab: 't1',
  ownerName: 'billing',
  title: `제목 ${id}`,
  check: [],
  risk: [],
  source: 'slack #ops',
  refs: [],
  state,
  read,
  issue: null,
  issuePath: null,
  created: `2026-10-25T${id}:00+09:00`,
  timeline: [],
  ...extra,
});

const key = (code, mods = {}) => ({ code, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, isComposing: false, repeat: false, ...mods });

test('stateLabel: Korean labels for every state', () => {
  assert.equal(stateLabel('new'), '새 항목');
  assert.equal(stateLabel('in_progress'), '진행 중');
  assert.equal(stateLabel('close_proposed'), '종료 제안됨');
  assert.equal(stateLabel('done'), '완료');
  assert.equal(stateLabel('weird'), 'weird');
});

test('sortItems: close_proposed → new unread → in_progress → others, newest first inside a group', () => {
  const items = [
    item('09:00', 'in_progress', true),
    item('10:00', 'new', true), // read new → "others"
    item('11:00', 'close_proposed', true),
    item('12:00', 'new', false),
    item('08:00', 'new', false),
    item('13:00', 'done', true),
    item('07:00', 'close_proposed', false),
    item('14:00', 'in_progress', false),
  ];
  const ids = sortItems(items).map((i) => i.id);
  assert.deepEqual(ids, ['11:00', '07:00', '12:00', '08:00', '14:00', '09:00', '13:00', '10:00']);
  // input untouched
  assert.equal(items[0].id, '09:00');
});

test('visibleItems hides done items only while hideDone is on', () => {
  const items = [item('09:00', 'done', true), item('10:00', 'new', false)];
  assert.deepEqual(visibleItems(items, { hideDone: true }).map((i) => i.id), ['10:00']);
  assert.deepEqual(visibleItems(items, { hideDone: false }).map((i) => i.id), ['10:00', '09:00']);
  assert.deepEqual(visibleItems(null, { hideDone: true }), []);
});

test('keyAction maps plain keys by code; modifiers, composing and repeats of one-shot actions are ignored', () => {
  assert.equal(keyAction(key('KeyJ')), 'down');
  assert.equal(keyAction(key('ArrowDown')), 'down');
  assert.equal(keyAction(key('KeyK')), 'up');
  assert.equal(keyAction(key('ArrowUp')), 'up');
  assert.equal(keyAction(key('Enter')), 'open');
  assert.equal(keyAction(key('NumpadEnter')), 'open');
  assert.equal(keyAction(key('Escape')), 'back');
  assert.equal(keyAction(key('KeyI')), 'instruct');
  assert.equal(keyAction(key('KeyS')), 'save');
  assert.equal(keyAction(key('KeyU')), 'issue');
  assert.equal(keyAction(key('KeyC')), 'close');
  assert.equal(keyAction(key('KeyH')), 'toggleDone');
  assert.equal(keyAction(key('KeyX')), null);
  // app chords (Alt/Ctrl+Alt…) are never applet keys
  assert.equal(keyAction(key('KeyJ', { altKey: true })), null);
  assert.equal(keyAction(key('KeyH', { ctrlKey: true, altKey: true })), null);
  assert.equal(keyAction(key('KeyC', { ctrlKey: true })), null);
  assert.equal(keyAction(key('KeyI', { shiftKey: true })), null);
  assert.equal(keyAction(key('KeyI', { metaKey: true })), null);
  assert.equal(keyAction(key('KeyJ', { isComposing: true })), null);
  // held j/k repeat; held i/c/s/u/Enter do not open a second dialog
  assert.equal(keyAction(key('KeyJ', { repeat: true })), 'down');
  assert.equal(keyAction(key('KeyC', { repeat: true })), null);
  assert.equal(keyAction(key('Enter', { repeat: true })), null);
  // the Korean layout still gives KeyJ for ㅓ
  assert.equal(keyAction({ ...key('KeyJ'), key: 'ㅓ' }), 'down');
});

test('canDo: c only for close_proposed; nothing but viewing on done items', () => {
  const np = item('10:00', 'new', false);
  const ip = item('10:00', 'in_progress', true);
  const cp = item('10:00', 'close_proposed', true);
  const dn = item('10:00', 'done', true);
  for (const it of [np, ip, dn]) assert.equal(canDo('close', it), false, it.state);
  assert.equal(canDo('close', cp), true);
  for (const a of ['instruct', 'save', 'issue']) {
    assert.equal(canDo(a, np), true, a);
    assert.equal(canDo(a, cp), true, a);
    assert.equal(canDo(a, dn), false, a);
  }
  assert.equal(canDo('issue', { ...np, issue: '20261025-x' }), false, 'already has an issue');
  assert.equal(canDo('instruct', null), false);
  assert.equal(canDo('close', undefined), false);
});

test('moveSelection / keepSelection', () => {
  const list = [item('12:00', 'new', false), item('11:00', 'new', false), item('10:00', 'new', false)];
  assert.equal(moveSelection(list, '12:00', 1), '11:00');
  assert.equal(moveSelection(list, '10:00', 1), '10:00', 'no wrap at the bottom');
  assert.equal(moveSelection(list, '12:00', -1), '12:00', 'no wrap at the top');
  assert.equal(moveSelection(list, 'gone', 1), '12:00');
  assert.equal(moveSelection([], null, 1), null);
  assert.equal(keepSelection(list, '11:00'), '11:00');
  assert.equal(keepSelection(list, 'gone'), '12:00');
  assert.equal(keepSelection([], 'gone'), null);
});

test('saveStep: no issue → ask for one first; done → nothing', () => {
  assert.equal(saveStep(item('10:00', 'new', false)), 'needIssue');
  assert.equal(saveStep(item('10:00', 'new', false, { issue: '20261025-x' })), 'save');
  assert.equal(saveStep(item('10:00', 'done', true, { issue: '20261025-x' })), null);
  assert.equal(saveStep(null), null);
});

test('parsePaths: one per line, quotes from "경로로 복사" stripped, blanks and duplicates dropped', () => {
  assert.deepEqual(parsePaths('  "C:\\a b\\x.md"  \r\n\nC:\\y.pdf\nC:\\y.pdf\n\'C:\\z.txt\'\n'), ['C:\\a b\\x.md', 'C:\\y.pdf', 'C:\\z.txt']);
  assert.deepEqual(parsePaths(''), []);
  assert.deepEqual(parsePaths(null), []);
});

test('timeline labels, route and time', () => {
  assert.equal(timelineLabel({ type: 'instruction' }), '지시');
  assert.equal(timelineLabel({ type: 'close_proposed', opinion: 'x' }), '종료 제안');
  assert.equal(timelineLabel({ type: 'issue_requested' }), '이슈 등록 요청');
  assert.equal(timelineLabel({ type: 'files_added' }), '자료 저장');
  assert.equal(timelineLabel({ type: 'zzz' }), 'zzz');
  assert.equal(routeText(item('10:00', 'new', false)), 'handover → dispatcher → billing');
  assert.equal(routeText(item('10:00', 'new', false, { source: 'app' })), '앱 → billing');
  assert.equal(shortTime('2026-10-25T14:03:07+09:00'), '10-25 14:03');
  assert.equal(shortTime(''), '');
});

// ---------------------------------------------------------------- rendering (fake DOM)

/** Minimal document: elements record text; any innerHTML/outerHTML/insertAdjacentHTML use throws. */
function fakeDoc() {
  const created = [];
  const mk = (tag) => {
    const el = {
      tagName: String(tag).toUpperCase(),
      children: [],
      className: '',
      dataset: {},
      attrs: {},
      _text: '',
      listeners: {},
      classList: {
        set: new Set(),
        add(...c) {
          c.forEach((x) => this.set.add(x));
        },
        remove(...c) {
          c.forEach((x) => this.set.delete(x));
        },
        toggle(c, on) {
          if (on === undefined ? !this.set.has(c) : on) this.set.add(c);
          else this.set.delete(c);
        },
        contains(c) {
          return this.set.has(c);
        },
      },
      set textContent(v) {
        this._text = String(v);
        this.children = [];
      },
      get textContent() {
        return this._text + this.children.map((c) => c.textContent).join('');
      },
      set innerHTML(_) {
        throw new Error('innerHTML used');
      },
      set outerHTML(_) {
        throw new Error('outerHTML used');
      },
      insertAdjacentHTML() {
        throw new Error('insertAdjacentHTML used');
      },
      append(...c) {
        c.forEach((x) => this.children.push(typeof x === 'string' ? textNode(x) : x));
      },
      appendChild(c) {
        this.children.push(c);
        return c;
      },
      replaceChildren(...c) {
        this.children = [];
        this.append(...c);
      },
      setAttribute(k, v) {
        this.attrs[k] = String(v);
      },
      addEventListener(t, f) {
        (this.listeners[t] ||= []).push(f);
      },
      removeEventListener() {},
    };
    created.push(el);
    return el;
  };
  const textNode = (t) => ({ tagName: '#text', children: [], textContent: String(t) });
  return {
    created,
    createElement: mk,
    createTextNode: textNode,
  };
}

const all = (el) => [el, ...(el.children || []).flatMap(all)];

test('render is text only: item data never becomes markup', () => {
  const evil = '<img src=x onerror="alert(1)"><script>alert(2)</script>';
  const it = item('10:00', 'close_proposed', false, {
    title: evil,
    ownerName: `own${evil}`,
    source: evil,
    check: [evil],
    risk: [evil],
    refs: [evil],
    issue: '20261025-x',
    issuePath: `C:\\w\\issues\\${evil}`,
    timeline: [{ at: '2026-10-25T10:00:00+09:00', who: evil, kind: { type: 'close_proposed', opinion: evil }, text: evil }],
  });
  const doc = fakeDoc();
  const list = buildList(doc, [it], { selected: it.id, hideDone: true, total: 1 });
  const detail = buildDetail(doc, it, { focused: true });
  const tags = new Set(doc.created.map((e) => e.tagName));
  assert.ok(!tags.has('IMG') && !tags.has('SCRIPT'), [...tags].join(','));
  assert.ok(list.textContent.includes(evil), 'title shown literally in the list');
  const text = detail.textContent;
  for (const part of [evil, `own${evil}`, `C:\\w\\issues\\${evil}`]) assert.ok(text.includes(part), part);
  // list marks: unread bold, close_proposed highlighted, selected row
  const row = all(list).find((e) => e.dataset && e.dataset.id === it.id);
  assert.ok(row, 'row carries data-id');
  assert.ok(row.classList.contains('unread') && row.classList.contains('close-proposed') && row.classList.contains('sel'));
});

test('detail buttons follow canDo (c only when close_proposed) and show their keys', () => {
  const doc = fakeDoc();
  const d = buildDetail(doc, item('10:00', 'in_progress', true), { focused: false });
  const btns = all(d).filter((e) => e.tagName === 'BUTTON');
  const byAction = Object.fromEntries(btns.map((b) => [b.dataset.action, b]));
  assert.deepEqual(Object.keys(byAction).sort(), ['close', 'instruct', 'issue', 'save']);
  assert.equal(byAction.close.disabled, true);
  assert.equal(byAction.instruct.disabled, false);
  assert.ok(byAction.instruct.textContent.includes('(i)'));
  const empty = buildDetail(fakeDoc(), null, {});
  assert.ok(empty.textContent.includes('항목을 선택'));
});

test('isFocusTrapKey: plain Tab / Shift+Tab stay in the applet; chords and other keys do not', () => {
  assert.equal(isFocusTrapKey(key('Tab')), true);
  assert.equal(isFocusTrapKey(key('Tab', { shiftKey: true })), true);
  assert.equal(isFocusTrapKey(key('Tab', { ctrlKey: true })), false);
  assert.equal(isFocusTrapKey(key('Tab', { altKey: true })), false);
  assert.equal(isFocusTrapKey(key('Tab', { metaKey: true })), false);
  assert.equal(isFocusTrapKey(key('KeyJ')), false);
  assert.equal(isFocusTrapKey(null), false);
});

test('focusLost: refocus the root only when focus was inside it and is now nowhere useful', () => {
  const body = { id: 'body' };
  const root = { contains: (x) => x === root || x === inner };
  const inner = { id: 'inner' };
  const other = { id: 'xterm' };
  assert.equal(focusLost(true, body, root, body), true, 'replaced child → body');
  assert.equal(focusLost(true, null, root, body), true);
  assert.equal(focusLost(true, inner, root, body), false, 'still inside');
  assert.equal(focusLost(true, other, root, body), false, 'user moved elsewhere');
  assert.equal(focusLost(false, body, root, body), false, 'was not ours');
});

test('keepSelection: a vanished item selects its neighbour at the old position', () => {
  const list = [item('12:00', 'new', false), item('11:00', 'new', false), item('10:00', 'new', false)];
  assert.equal(keepSelection(list, 'gone', 1), '11:00');
  assert.equal(keepSelection(list, 'gone', 7), '10:00', 'clamped to the last');
  assert.equal(keepSelection(list, 'gone', -1), '12:00');
  assert.equal(keepSelection(list, '10:00', 0), '10:00', 'still listed → unchanged');
});

test('detail lists issue input/output files; .md rows are marked, selection only in file mode', () => {
  const evil = '<img src=x onerror=alert(1)>.md';
  const files = {
    input: [{ name: '메일.md', path: 'C:/w/issues/x/input/메일.md', isMd: true }, { name: '첨부.pdf', path: 'p', isMd: false }],
    output: [{ name: evil, path: 'q', isMd: true }],
  };
  assert.deepEqual(flatFiles(files).map((f) => [f.where, f.name]), [['input', '메일.md'], ['input', '첨부.pdf'], ['output', evil]]);
  assert.deepEqual(flatFiles(null), []);
  const it = item('10:00', 'in_progress', true, { issue: '20261025-x', issuePath: 'C:/w/issues/x' });
  const doc = fakeDoc();
  const d = buildDetail(doc, it, { focused: true, files, fileSel: 2, fileMode: true });
  const rows = all(d).filter((e) => e.className === 'inbox-file');
  assert.deepEqual(rows.map((r) => r.dataset.index), ['0', '1', '2']);
  assert.deepEqual(rows.map((r) => r.classList.contains('md')), [true, false, true]);
  assert.deepEqual(rows.map((r) => r.classList.contains('sel')), [false, false, true]);
  assert.ok(d.textContent.includes(evil), 'file names are text');
  assert.ok(!doc.created.some((e) => e.tagName === 'IMG'));
  assert.ok(d.textContent.includes('Enter: 문서 탭으로 열기'));
  // not in file mode: nothing selected, the hint says how to enter
  const d2 = buildDetail(fakeDoc(), it, { focused: true, files, fileSel: 0, fileMode: false });
  assert.ok(all(d2).filter((e) => e.className === 'inbox-file').every((r) => !r.classList.contains('sel')));
  assert.ok(d2.textContent.includes('Tab: 파일 목록'));
  // no issue → no list even when files are passed
  const d3 = buildDetail(fakeDoc(), item('10:00', 'new', false), { files });
  assert.ok(!all(d3).some((e) => e.className === 'inbox-file'));
});
