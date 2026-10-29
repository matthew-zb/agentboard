import { test } from 'node:test';
import assert from 'node:assert/strict';
import { splitMemo, joinMemo, memoKeyAction, memoEol, normalizeEol } from '../../src/memo-logic.js';

// ---------------------------------------------------------------- MF1: split / join / keys

test('splitMemo: body starts after the first "## 메모" line', () => {
  const t = '# 메모\n작성 기준\n- a\n\n## 메모\n- 할 일 하나 — billing\n- 둘\n';
  const m = splitMemo(t);
  assert.equal(m.marker, true);
  assert.equal(m.header, '# 메모\n작성 기준\n- a\n\n## 메모\n');
  assert.equal(m.body, '- 할 일 하나 — billing\n- 둘\n');
  assert.equal(joinMemo(m.header, m.body, m.marker), t);
  // only the first marker line counts: a second one is body text
  const twice = splitMemo('## 메모\n- a\n## 메모\n- b\n');
  assert.equal(twice.body, '- a\n## 메모\n- b\n');
});

test('splitMemo: only an exact "## 메모" line (trimmed) is the marker', () => {
  assert.equal(splitMemo('## 메모장\n- x\n').marker, false);
  assert.equal(splitMemo('### 메모\n- x\n').marker, false);
  assert.equal(splitMemo('  ## 메모  \r\n- x\r\n').body, '- x\r\n');
  assert.equal(splitMemo('a\r\n## 메모\r\n- x\r\n').header, 'a\r\n## 메모\r\n');
  // marker on the last line without a line break: empty body; joining a new body adds the break
  const last = splitMemo('# h\r\n## 메모');
  assert.deepEqual([last.marker, last.header, last.body], [true, '# h\r\n## 메모', '']);
  assert.equal(joinMemo(last.header, '', last.marker), '# h\r\n## 메모');
  assert.equal(joinMemo(last.header, '- y\r\n', last.marker), '# h\r\n## 메모\r\n- y\r\n');
});

test('splitMemo: no marker → whole file is the body', () => {
  const m = splitMemo('- x\n');
  assert.deepEqual([m.marker, m.header, m.body], [false, '', '- x\n']);
  assert.equal(joinMemo(m.header, '- y\n', m.marker), '- y\n');
  assert.deepEqual(splitMemo(''), { header: '', body: '', marker: false });
  assert.deepEqual(splitMemo(null), { header: '', body: '', marker: false });
});

test('edited body keeps the file line endings (textarea values use LF)', () => {
  assert.equal(memoEol('a\r\nb\r\n'), '\r\n');
  assert.equal(memoEol('a\nb'), '\n');
  assert.equal(memoEol(''), '\n');
  assert.equal(normalizeEol('a\nb\r\nc\rd', '\r\n'), 'a\r\nb\r\nc\r\nd');
  assert.equal(normalizeEol('a\r\nb', '\n'), 'a\nb');
  const m = splitMemo('# h\r\n## 메모\r\n- x\r\n');
  const typed = '- x\n- y\n'; // what textarea.value returns
  assert.equal(joinMemo(m.header, normalizeEol(typed, memoEol(m.header + m.body)), m.marker), '# h\r\n## 메모\r\n- x\r\n- y\r\n');
});

const key = (code, extra = {}) => ({ code, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, isComposing: false, repeat: false, ...extra });

test('memo key map: e/r in read, Ctrl+Enter/Esc in edit, IME and modifiers ignored', () => {
  // read
  assert.equal(memoKeyAction(key('KeyE'), 'read'), 'edit');
  assert.equal(memoKeyAction(key('KeyR'), 'read'), 'reload');
  assert.equal(memoKeyAction(key('KeyJ'), 'read'), 'scrollDown');
  assert.equal(memoKeyAction(key('ArrowDown'), 'read'), 'scrollDown');
  assert.equal(memoKeyAction(key('KeyK'), 'read'), 'scrollUp');
  assert.equal(memoKeyAction(key('KeyG'), 'read'), 'top');
  assert.equal(memoKeyAction(key('KeyG', { shiftKey: true }), 'read'), 'bottom');
  assert.equal(memoKeyAction(key('KeyE', { shiftKey: true }), 'read'), null);
  assert.equal(memoKeyAction(key('KeyX'), 'read'), null);
  assert.equal(memoKeyAction(key('Enter', { ctrlKey: true }), 'read'), null);
  assert.equal(memoKeyAction(key('KeyJ', { repeat: true }), 'read'), 'scrollDown'); // scrolling repeats
  assert.equal(memoKeyAction(key('KeyE', { repeat: true }), 'read'), null); // a held e does not re-enter
  // edit: only save / cancel; every other key is typing
  assert.equal(memoKeyAction(key('Enter', { ctrlKey: true }), 'edit'), 'save');
  assert.equal(memoKeyAction(key('NumpadEnter', { ctrlKey: true }), 'edit'), 'save');
  assert.equal(memoKeyAction(key('Escape'), 'edit'), 'cancel');
  for (const c of ['KeyE', 'KeyR', 'KeyJ', 'Enter', 'Space', 'Backspace']) assert.equal(memoKeyAction(key(c), 'edit'), null, c);
  assert.equal(memoKeyAction(key('Enter', { ctrlKey: true, repeat: true }), 'edit'), null); // held Ctrl+Enter saves once
  // IME composition and app chords (Alt/Meta) are never memo keys
  assert.equal(memoKeyAction(key('KeyE', { isComposing: true }), 'read'), null);
  assert.equal(memoKeyAction(key('Escape', { isComposing: true }), 'edit'), null);
  assert.equal(memoKeyAction(key('Enter', { ctrlKey: true, isComposing: true }), 'edit'), null);
  assert.equal(memoKeyAction(key('KeyH', { altKey: true }), 'read'), null);
  assert.equal(memoKeyAction(key('Enter', { ctrlKey: true, altKey: true }), 'edit'), null);
  assert.equal(memoKeyAction(null, 'read'), null);
});

// ---------------------------------------------------------------- MF2: the applet (fake DOM)
import { mountMemo, configureMemo } from '../../src/memo.js';

/** Minimal DOM for the applet: elements keep children/text/value; focus sets activeElement. */
function fakeDocument() {
  const doc = { activeElement: null, body: null };
  const textNode = (t) => ({ tagName: '#text', children: [], textContent: String(t), contains: () => false });
  const mk = (tag) => {
    const classes = new Set();
    const el = {
      tagName: String(tag).toUpperCase(),
      children: [],
      parent: null,
      dataset: {},
      attrs: {},
      listeners: {},
      _text: '',
      value: '',
      scrollTop: 0,
      scrollHeight: 1000,
      selection: null,
      classList: {
        add: (...c) => c.forEach((x) => classes.add(x)),
        remove: (...c) => c.forEach((x) => classes.delete(x)),
        toggle: (c, on) => ((on === undefined ? !classes.has(c) : on) ? classes.add(c) : classes.delete(c)),
        contains: (c) => classes.has(c),
      },
      get className() {
        return [...classes].join(' ');
      },
      set className(v) {
        classes.clear();
        String(v).split(/\s+/).filter(Boolean).forEach((c) => classes.add(c));
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
      append(...c) {
        c.forEach((x) => this.appendChild(typeof x === 'string' ? textNode(x) : x));
      },
      appendChild(c) {
        c.parent = this;
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
      getAttribute(k) {
        return this.attrs[k] ?? null;
      },
      addEventListener(t, f) {
        (this.listeners[t] ||= []).push(f);
      },
      removeEventListener(t, f) {
        this.listeners[t] = (this.listeners[t] || []).filter((x) => x !== f);
      },
      contains(n) {
        return n === this || this.children.some((c) => c.contains && c.contains(n));
      },
      closest() {
        return null;
      },
      focus() {
        doc.activeElement = this;
      },
      remove() {
        if (this.parent) this.parent.children = this.parent.children.filter((c) => c !== this);
      },
      scrollBy(_x, y) {
        this.scrollTop += y;
      },
      setSelectionRange(a, b) {
        this.selection = [a, b];
      },
    };
    return el;
  };
  doc.createElement = mk;
  doc.createTextNode = textNode;
  doc.body = mk('body');
  return doc;
}

const tree = (el) => [el, ...(el.children || []).flatMap(tree)];
const byClass = (root, c) => tree(root).find((e) => e.classList && e.classList.contains(c));
const settle = async () => {
  for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0));
};

const HEADER = '# 메모 — 사용자가 고른 오늘 할 일\n\n작성 기준 (이 부분은 메모 창에 보이지 않는다)\n- 한 줄에 하나\n\n## 메모\n';
const BODY = '- 결제 오류 회신 확인 — billing\n- 주간 보고 초안\n';

/** Fake Tauri: memo_read / memo_save over an in-memory MEMO.md, doc-changed listeners. */
function setup(text, { answer = true, vi = false } = {}) {
  const t = { text, calls: [], listeners: {}, rendered: [], confirms: [], toasts: [], answer };
  globalThis.document = fakeDocument();
  globalThis.__TAURI__ = {
    core: {
      invoke: async (cmd, args) => {
        t.calls.push([cmd, args]);
        if (cmd === 'memo_read') return { path: 'MEMO.md', text: t.text };
        if (cmd === 'memo_settings') return { vi: !!t.settingsVi };
        if (cmd === 'memo_save') t.text = args.text;
        return null;
      },
    },
    event: {
      listen: async (ev, cb) => {
        (t.listeners[ev] ||= []).push(cb);
        return () => {
          t.listeners[ev] = t.listeners[ev].filter((x) => x !== cb);
        };
      },
    },
  };
  t.emit = (ev, payload) => (t.listeners[ev] || []).forEach((cb) => cb({ payload }));
  t.cmds = (name) => t.calls.filter((c) => c[0] === name);
  configureMemo({
    render: async (body) => {
      t.rendered.push(body);
      const d = document.createElement('div');
      d.textContent = body;
      return { frag: d, images: [] };
    },
    loadImages: () => {},
    confirm: async (msg, opts) => {
      t.confirms.push([msg, opts]);
      return t.answer;
    },
    toast: (m) => t.toasts.push(String(m)),
    // memo spec §8 V3 b: a fixed answer, or null = follow memo_settings (reset by every test)
    vi: vi === null ? null : () => vi,
  });
  return t;
}

function teardown() {
  delete globalThis.document;
  delete globalThis.__TAURI__;
}

/** keydown at `target`, delivered to the root's listeners (bubble phase, like the browser). */
const press = (target, code, mods = {}) => {
  const ev = { code, target, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, isComposing: false, repeat: false, defaultPrevented: false, ...mods };
  ev.preventDefault = () => {
    ev.defaultPrevented = true;
  };
  ev.stopPropagation = () => {};
  let root = target;
  while (root.parent) root = root.parent;
  for (const f of root.listeners.keydown || []) f(ev);
  return ev;
};

test('memo view shows only the body', async () => {
  const t = setup(HEADER + BODY);
  const m = mountMemo('p-memo');
  await settle();
  assert.equal(t.rendered.at(-1), BODY);
  assert.ok(!m.el.textContent.includes('작성 기준'), 'header text is not shown');
  assert.ok(m.el.textContent.includes('결제 오류 회신 확인'));
  assert.deepEqual(t.cmds('doc_watch'), [['doc_watch', { paneId: 'p-memo', path: 'MEMO.md' }]]);
  assert.equal(byClass(m.el, 'memo-warn').classList.contains('hidden'), true);
  m.dispose();
  teardown();
});

test('memo edit saves header + edited body', async () => {
  const t = setup(HEADER + BODY);
  const m = mountMemo('p-memo');
  await settle();
  press(m.el, 'KeyE');
  const ed = byClass(m.el, 'memo-edit');
  assert.ok(ed && ed.tagName === 'TEXTAREA', 'editor shown');
  assert.equal(ed.value, BODY);
  assert.equal(document.activeElement, ed);
  assert.deepEqual(ed.selection, [BODY.length, BODY.length]); // cursor at the end
  ed.value = `${BODY}- 새 일\n`;
  const ev = press(ed, 'Enter', { ctrlKey: true });
  assert.equal(ev.defaultPrevented, true);
  await settle();
  assert.deepEqual(t.cmds('memo_save'), [['memo_save', { text: `${HEADER}${BODY}- 새 일\n` }]]);
  assert.equal(t.text, `${HEADER}${BODY}- 새 일\n`);
  assert.equal(byClass(m.el, 'memo-edit'), undefined, 'back to the read view');
  assert.equal(t.rendered.at(-1), `${BODY}- 새 일\n`);
  // typing keys inside the editor are not swallowed
  press(m.el, 'KeyE');
  const typed = press(byClass(m.el, 'memo-edit'), 'KeyJ');
  assert.equal(typed.defaultPrevented, false);
  m.dispose();
  teardown();
});

test('memo esc with changes asks Y/N, Enter ignored', async () => {
  const t = setup(HEADER + BODY, { answer: false });
  const m = mountMemo('p-memo');
  await settle();
  // no changes: Esc leaves without asking
  press(m.el, 'KeyE');
  press(byClass(m.el, 'memo-edit'), 'Escape');
  await settle();
  assert.equal(t.confirms.length, 0);
  assert.equal(byClass(m.el, 'memo-edit'), undefined);
  // changed draft: Y/N (Enter does not answer); N keeps editing with the draft
  press(m.el, 'KeyE');
  const ed = byClass(m.el, 'memo-edit');
  ed.value = '- 바뀐 초안\n';
  press(ed, 'Escape');
  await settle();
  assert.equal(t.confirms.length, 1);
  assert.deepEqual(t.confirms[0][1], { enterYes: false });
  assert.equal(byClass(m.el, 'memo-edit'), ed, 'still editing');
  assert.equal(ed.value, '- 바뀐 초안\n');
  // Y discards: read view of the file as it is, nothing saved
  t.answer = true;
  press(ed, 'Escape');
  await settle();
  assert.equal(byClass(m.el, 'memo-edit'), undefined);
  assert.equal(t.cmds('memo_save').length, 0);
  assert.equal(t.rendered.at(-1), BODY);
  m.dispose();
  teardown();
});

test('memo external change while editing shows a notice and keeps the draft', async () => {
  const t = setup(HEADER + BODY);
  const m = mountMemo('p-memo');
  await settle();
  // read view: an external change re-renders; another pane's event is ignored
  t.text = `${HEADER}- dispatcher가 고친 줄\n`;
  t.emit('doc-changed', { paneId: 'other' });
  await settle();
  assert.equal(t.rendered.at(-1), BODY);
  t.emit('doc-changed', { paneId: 'p-memo' });
  await settle();
  assert.equal(t.rendered.at(-1), '- dispatcher가 고친 줄\n');
  // edit view: notice only, the draft stays
  press(m.el, 'KeyE');
  const ed = byClass(m.el, 'memo-edit');
  ed.value = '- 내 초안\n';
  const notice = byClass(m.el, 'memo-notice');
  assert.equal(notice.classList.contains('hidden'), true);
  t.text = `${HEADER}- 또 바뀜\n`;
  t.emit('doc-changed', { paneId: 'p-memo' });
  await settle();
  assert.equal(notice.classList.contains('hidden'), false);
  assert.equal(notice.textContent, '외부에서 바뀜 — 저장하면 덮어씀(Esc: 취소하고 다시 읽기)');
  assert.equal(byClass(m.el, 'memo-edit'), ed);
  assert.equal(ed.value, '- 내 초안\n');
  // saving overwrites (last writer wins) with the header the user edited against
  press(ed, 'Enter', { ctrlKey: true });
  await settle();
  assert.equal(t.text, `${HEADER}- 내 초안\n`);
  assert.equal(notice.classList.contains('hidden'), true);
  m.dispose();
  teardown();
});

test('memo without marker shows a warning', async () => {
  const t = setup('- 헤딩 없는 메모\n');
  const m = mountMemo('p-memo');
  await settle();
  const warn = byClass(m.el, 'memo-warn');
  assert.equal(warn.classList.contains('hidden'), false);
  assert.ok(warn.textContent.includes('구분 헤딩(## 메모) 없음'));
  assert.equal(t.rendered.at(-1), '- 헤딩 없는 메모\n');
  // the whole file is edited and saved
  press(m.el, 'KeyE');
  const ed = byClass(m.el, 'memo-edit');
  assert.equal(ed.value, '- 헤딩 없는 메모\n');
  ed.value = '- 고침\n';
  press(ed, 'Enter', { ctrlKey: true });
  await settle();
  assert.equal(t.text, '- 고침\n');
  m.dispose();
  teardown();
});

test('memo keeps CRLF on save and unwatches on dispose', async () => {
  const t = setup('# h\r\n## 메모\r\n- a\r\n');
  const m = mountMemo('p-memo');
  await settle();
  press(m.el, 'KeyE');
  const ed = byClass(m.el, 'memo-edit');
  ed.value = '- a\n- b\n'; // textarea values are LF-only
  press(ed, 'Enter', { ctrlKey: true });
  await settle();
  assert.equal(t.text, '# h\r\n## 메모\r\n- a\r\n- b\r\n');
  m.dispose();
  assert.deepEqual(t.cmds('doc_unwatch'), [['doc_unwatch', { paneId: 'p-memo' }]]);
  const reads = t.cmds('memo_read').length;
  t.emit('doc-changed', { paneId: 'p-memo' });
  await settle();
  assert.equal(t.cmds('memo_read').length, reads, 'no reads after dispose');
  teardown();
});

// ---------------------------------------------------------------- memo spec §8: vi editing (V3 b: only when configured)

const viBar = (m) => byClass(m.el, 'memo-vi');
const viMode = (m) => byClass(m.el, 'memo-vi-mode').textContent;
const cmdline = (m) => byClass(m.el, 'memo-cmdline');
const L2 = BODY.indexOf('- 주간');

test('vi memo: e enters normal mode (read-only, block cursor, -- NORMAL --); Esc keeps editing', async () => {
  const t = setup(HEADER + BODY, { vi: true });
  const m = mountMemo('p-memo');
  await settle();
  press(m.el, 'KeyE');
  const ed = byClass(m.el, 'memo-edit');
  assert.equal(ed.value, BODY);
  assert.equal(ed.readOnly, true, 'no typing in normal mode (an IME cannot start composing text)');
  assert.deepEqual(ed.selection, [0, 1], 'the character under the cursor is selected (block cursor)');
  assert.equal(viBar(m).classList.contains('hidden'), false);
  assert.equal(viMode(m), '-- NORMAL --');
  const j = press(ed, 'KeyJ');
  assert.ok(j.defaultPrevented);
  assert.deepEqual(ed.selection, [L2, L2 + 1]);
  press(ed, 'KeyD');
  assert.equal(viMode(m), '-- NORMAL -- d', 'a pending operator is shown');
  press(ed, 'Escape');
  assert.equal(viMode(m), '-- NORMAL --');
  assert.equal(byClass(m.el, 'memo-edit'), ed, 'Esc in normal mode does not leave the editor');
  assert.equal(t.confirms.length, 0);
  // a key that is not a vi key does nothing to the read-only text and is not typed
  assert.ok(press(ed, 'KeyQ').defaultPrevented);
  // app chords (Alt+…) are not taken: the key router runs them first
  assert.equal(press(ed, 'KeyH', { altKey: true }).defaultPrevented, false);
  m.dispose();
  teardown();
});

test('vi memo: i types in the textarea, Esc back to normal, :w saves and stays, :wq saves and leaves', async () => {
  const t = setup(HEADER + BODY, { vi: true });
  const m = mountMemo('p-memo');
  await settle();
  press(m.el, 'KeyE');
  const ed = byClass(m.el, 'memo-edit');
  press(ed, 'KeyI');
  assert.equal(ed.readOnly, false);
  assert.equal(viMode(m), '-- INSERT --');
  assert.deepEqual(ed.selection, [0, 0]);
  // typing is the textarea's own; plain keys are not taken in insert mode
  assert.equal(press(ed, 'KeyJ').defaultPrevented, false);
  ed.value = `오늘 할 일\n${BODY}`;
  ed.selectionStart = 6;
  press(ed, 'Escape');
  assert.equal(ed.readOnly, true);
  assert.equal(viMode(m), '-- NORMAL --');
  assert.deepEqual(ed.selection, [5, 6], 'one to the left, like vi');
  // :w — the command line, then Enter
  press(ed, 'Semicolon', { shiftKey: true });
  const cl = cmdline(m);
  assert.equal(cl.classList.contains('hidden'), false);
  assert.equal(document.activeElement, cl);
  assert.equal(byClass(m.el, 'memo-vi-prefix').textContent, ':');
  cl.value = 'w';
  press(cl, 'Enter');
  await settle();
  assert.deepEqual(t.cmds('memo_save'), [['memo_save', { text: `${HEADER}오늘 할 일\n${BODY}` }]]);
  assert.equal(byClass(m.el, 'memo-edit'), ed, ':w keeps editing');
  assert.equal(cl.classList.contains('hidden'), true);
  assert.ok(viMode(m).includes('저장했습니다'), viMode(m));
  // Ctrl+Enter (memo's own save) still works; :wq saves and leaves
  press(ed, 'KeyX');
  press(ed, 'Semicolon', { shiftKey: true });
  cmdline(m).value = 'wq';
  press(cmdline(m), 'Enter');
  await settle();
  assert.equal(t.cmds('memo_save').length, 2);
  assert.equal(t.text, `${HEADER}오늘 할 \n${BODY}`, 'x removed the character under the cursor');
  assert.equal(byClass(m.el, 'memo-edit'), undefined, ':wq left the editor');
  m.dispose();
  teardown();
});

test('vi memo: dd / u, and :q! with changes asks Y/N (Enter ignored)', async () => {
  const t = setup(HEADER + BODY, { vi: true, answer: false });
  const m = mountMemo('p-memo');
  await settle();
  press(m.el, 'KeyE');
  const ed = byClass(m.el, 'memo-edit');
  press(ed, 'KeyD');
  press(ed, 'KeyD');
  assert.equal(ed.value, BODY.slice(L2));
  press(ed, 'KeyU');
  assert.equal(ed.value, BODY);
  press(ed, 'KeyD');
  press(ed, 'KeyD');
  press(ed, 'Semicolon', { shiftKey: true });
  cmdline(m).value = 'q!';
  press(cmdline(m), 'Enter');
  await settle();
  assert.equal(t.confirms.length, 1);
  assert.deepEqual(t.confirms[0][1], { enterYes: false });
  assert.equal(byClass(m.el, 'memo-edit'), ed, 'N keeps editing');
  // :q with changes only explains; nothing is saved
  press(ed, 'Semicolon', { shiftKey: true });
  cmdline(m).value = 'q';
  press(cmdline(m), 'Enter');
  await settle();
  assert.ok(viMode(m).includes(':q!'), viMode(m));
  assert.equal(t.cmds('memo_save').length, 0);
  m.dispose();
  teardown();
});

test('vi memo: / searches from the command line (Korean input), Esc closes it', async () => {
  setup(HEADER + BODY, { vi: true });
  const m = mountMemo('p-memo');
  await settle();
  press(m.el, 'KeyE');
  const ed = byClass(m.el, 'memo-edit');
  press(ed, 'Slash');
  assert.equal(byClass(m.el, 'memo-vi-prefix').textContent, '/');
  // composing Hangul in the command line: Enter belongs to the IME
  cmdline(m).value = '주간';
  assert.equal(press(cmdline(m), 'Enter', { isComposing: true }).defaultPrevented, false);
  press(cmdline(m), 'Enter');
  const at = BODY.indexOf('주간');
  assert.deepEqual(ed.selection, [at, at + 1]);
  assert.equal(document.activeElement, ed, 'focus back in the editor');
  press(ed, 'Slash');
  cmdline(m).value = '없는말';
  press(cmdline(m), 'Escape');
  assert.equal(cmdline(m).classList.contains('hidden'), true);
  assert.deepEqual(ed.selection, [at, at + 1], 'Esc: no search');
  press(ed, 'KeyN');
  assert.deepEqual(ed.selection, [at, at + 1], 'n: the only match (wraps to itself)');
  m.dispose();
  teardown();
});

test('vi memo is off unless configured: the editor is plain and Esc cancels as before', async () => {
  setup(HEADER + BODY);
  const m = mountMemo('p-memo');
  await settle();
  press(m.el, 'KeyE');
  const ed = byClass(m.el, 'memo-edit');
  assert.ok(!ed.readOnly);
  assert.equal(viBar(m).classList.contains('hidden'), true);
  press(ed, 'Escape');
  await settle();
  assert.equal(byClass(m.el, 'memo-edit'), undefined);
  m.dispose();
  teardown();
});

test('vi follows memo_settings: read at mount, again on config-reloaded', async () => {
  const t = setup(HEADER + BODY, { vi: null });
  t.settingsVi = true;
  const m = mountMemo('p-memo');
  await settle();
  assert.equal(t.cmds('memo_settings').length, 1);
  press(m.el, 'KeyE');
  assert.equal(byClass(m.el, 'memo-edit').readOnly, true, 'config memo.vi = true: normal mode');
  press(byClass(m.el, 'memo-edit'), 'Semicolon', { shiftKey: true });
  cmdline(m).value = 'q';
  press(cmdline(m), 'Enter');
  await settle();
  // the settings are re-read after "설정 다시 읽기" (config-reloaded) and apply to the next edit
  t.settingsVi = false;
  t.emit('config-reloaded', null);
  await settle();
  assert.equal(t.cmds('memo_settings').length, 2);
  press(m.el, 'KeyE');
  const ed = byClass(m.el, 'memo-edit');
  assert.ok(!ed.readOnly);
  assert.equal(viBar(m).classList.contains('hidden'), true);
  m.dispose();
  t.emit('config-reloaded', null);
  await settle();
  assert.equal(t.cmds('memo_settings').length, 2, 'not after dispose');
  teardown();
});
