// Inbox applet (spec §5, design C): list on the left, detail on the right, keyboard driven.
// Mounted by pane.js into an applet pane (`kind: inbox`) instead of an xterm; one shared store
// (inbox_list + `inbox-changed`) feeds every mounted instance. All item data is rendered with
// textContent / DOM building — never innerHTML (items are written by agents).
// No top-level DOM access: the builders take a `doc` and are unit-tested in Node with a fake one.
import * as api from './api.js';
import {
  stateLabel,
  visibleItems,
  keyAction,
  canDo,
  moveSelection,
  keepSelection,
  saveStep,
  parsePaths,
  timelineLabel,
  routeText,
  shortTime,
} from './inbox-logic.js';

// ---------------------------------------------------------------- DOM builders (pure-ish)

function el(doc, tag, cls, text) {
  const e = doc.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

/**
 * The list column. `items` are already filtered and sorted (visibleItems). Rows carry
 * `data-id`; classes `unread` (bold), `close-proposed` (highlight), `done`, `sel`.
 */
export function buildList(doc, items, { selected = null, hideDone = true, total = 0 } = {}) {
  const root = el(doc, 'div', 'inbox-list');
  const head = el(doc, 'div', 'inbox-head');
  const hidden = total - items.length;
  head.append(
    el(doc, 'span', 'inbox-count', `인박스 ${items.length}건`),
    el(doc, 'span', 'inbox-hint', hideDone ? `h: 완료 보기${hidden > 0 ? ` (숨김 ${hidden})` : ''}` : 'h: 완료 숨기기'),
  );
  root.appendChild(head);
  const rows = el(doc, 'div', 'inbox-rows');
  if (!items.length) rows.appendChild(el(doc, 'div', 'inbox-empty', '처리할 항목이 없습니다'));
  for (const it of items) {
    const row = el(doc, 'div', 'inbox-row');
    row.dataset.id = it.id;
    row.classList.toggle('unread', !it.read);
    row.classList.toggle('close-proposed', it.state === 'close_proposed');
    row.classList.toggle('done', it.state === 'done');
    row.classList.toggle('sel', it.id === selected);
    const badge = el(doc, 'span', `inbox-badge st-${it.state}`, stateLabel(it.state));
    const title = el(doc, 'span', 'inbox-title', it.title);
    const meta = el(doc, 'span', 'inbox-meta', `${it.ownerName} · ${shortTime(it.created)}`);
    row.append(badge, title, meta);
    rows.appendChild(row);
  }
  root.appendChild(rows);
  return root;
}

const ACTIONS = [
  ['instruct', '지시 (i)'],
  ['save', '자료 저장 (s)'],
  ['issue', '이슈 등록 (u)'],
  ['close', '종료 확인 (c)'],
];

function section(doc, title, lines, cls = '') {
  const s = el(doc, 'div', `inbox-sec ${cls}`.trim());
  s.appendChild(el(doc, 'div', 'inbox-sec-title', title));
  const ul = el(doc, 'ul', 'inbox-ul');
  if (!lines.length) ul.appendChild(el(doc, 'li', 'inbox-none', '없음'));
  for (const l of lines) ul.appendChild(el(doc, 'li', '', l));
  s.appendChild(ul);
  return s;
}

function field(doc, label, value) {
  const r = el(doc, 'div', 'inbox-field');
  r.append(el(doc, 'span', 'inbox-label', label), el(doc, 'span', 'inbox-value', value));
  return r;
}

/**
 * The detail column for `item` (null → a hint): 제목·담당·출처·경로·확인할 것·리스크·타임라인·
 * 이슈 폴더, then the action buttons (`data-action`, disabled unless canDo).
 */
export function buildDetail(doc, item, { focused = false } = {}) {
  const root = el(doc, 'div', 'inbox-detail');
  root.classList.toggle('active', !!focused);
  if (!item) {
    root.appendChild(el(doc, 'div', 'inbox-empty', '항목을 선택하세요 (j/k: 이동, Enter: 상세)'));
    return root;
  }
  const head = el(doc, 'div', 'inbox-dhead');
  head.append(el(doc, 'span', `inbox-badge st-${item.state}`, stateLabel(item.state)), el(doc, 'span', 'inbox-dtitle', item.title));
  root.appendChild(head);
  root.append(
    field(doc, '담당', item.ownerName),
    field(doc, '출처', [item.source === 'app' ? '앱' : item.source, shortTime(item.created)].filter(Boolean).join(' · ')),
    field(doc, '경로', routeText(item)),
  );
  root.appendChild(section(doc, '확인할 것', item.check || []));
  root.appendChild(section(doc, '리스크 포인트', item.risk || [], 'risk'));
  if (item.refs && item.refs.length) root.appendChild(section(doc, '참고', item.refs));
  // Task 14 attaches here: the issue folder's input/output file list (Enter → document tab).
  const issue = el(doc, 'div', 'inbox-sec inbox-issue');
  issue.appendChild(el(doc, 'div', 'inbox-sec-title', '이슈 폴더'));
  issue.appendChild(
    el(doc, 'div', 'inbox-path', item.issue ? item.issuePath || item.issue : '없음 — u: 담당 Manager에게 이슈 등록 요청'),
  );
  root.appendChild(issue);

  const tl = el(doc, 'div', 'inbox-sec inbox-timeline');
  tl.appendChild(el(doc, 'div', 'inbox-sec-title', '타임라인'));
  const ul = el(doc, 'ul', 'inbox-ul');
  for (const en of item.timeline || []) {
    const li = el(doc, 'li', `tl-${(en.kind && en.kind.type) || 'x'}`);
    li.append(
      el(doc, 'span', 'tl-time', shortTime(en.at)),
      el(doc, 'span', 'tl-who', en.who === 'user' ? '나' : en.who === 'app' ? '앱' : en.who),
      el(doc, 'span', 'tl-kind', timelineLabel(en.kind)),
    );
    if (en.text) li.appendChild(el(doc, 'span', 'tl-text', en.text));
    ul.appendChild(li);
  }
  tl.appendChild(ul);
  root.appendChild(tl);

  const btns = el(doc, 'div', 'inbox-btns');
  for (const [action, label] of ACTIONS) {
    const b = el(doc, 'button', 'inbox-btn', label);
    b.type = 'button';
    b.tabIndex = -1;
    b.dataset.action = action;
    b.disabled = !canDo(action, item);
    btns.appendChild(b);
  }
  root.appendChild(btns);
  return root;
}

// ---------------------------------------------------------------- shared store

const store = { items: [], version: -1, views: new Set(), unlisten: null, seq: 0, error: null };

function refresh() {
  const seq = ++store.seq;
  return api.inboxList(true).then(
    (r) => {
      // versions only grow within a run: an older answer arriving late is dropped
      if (seq !== store.seq && r.version < store.version) return;
      store.version = r.version;
      store.items = Array.isArray(r.items) ? r.items : [];
      store.error = null;
      for (const v of store.views) v.render();
    },
    (err) => {
      store.error = String(err);
      for (const v of store.views) v.render();
    },
  );
}

function subscribe(view) {
  store.views.add(view);
  if (store.views.size === 1 && !store.unlisten) {
    store.unlisten = api.on('inbox-changed', () => refresh());
  }
  refresh();
}

function unsubscribe(view) {
  store.views.delete(view);
  if (store.views.size === 0 && store.unlisten) {
    const u = store.unlisten;
    store.unlisten = null;
    Promise.resolve(u).then((f) => typeof f === 'function' && f(), () => {});
  }
}

// ---------------------------------------------------------------- mounted applet

const deps = {
  toast: (m) => console.error('[agentboard]', m),
  confirm: async () => false,
  promptText: async () => null,
  choose: async () => null,
};

/** `{toast(msg, kind?), confirm(msg, opts), promptText(title, opts), choose(title, options)}` (app.js). */
export function configureInbox(d) {
  Object.assign(deps, d);
}

const HIDE_KEY = 'agentboard.inbox.hideDone';
function loadHideDone() {
  try {
    return globalThis.localStorage ? localStorage.getItem(HIDE_KEY) !== '0' : true;
  } catch (_) {
    return true;
  }
}
function saveHideDone(v) {
  try {
    if (globalThis.localStorage) localStorage.setItem(HIDE_KEY, v ? '1' : '0');
  } catch (_) {
    /* per-viewer convenience only */
  }
}

const editable = (t) => !!t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable);

/**
 * Mount an inbox applet; returns `{el, focus(), dispose()}`. The element is created once and
 * moved by the layout (parking ↔ split), so listeners are attached once; `dispose` (pane closed)
 * removes them and drops the store subscription.
 */
export function mountInbox() {
  const root = document.createElement('div');
  root.className = 'inbox view-list';
  root.tabIndex = -1; // keydown target while the applet pane is focused
  const st = { sel: null, view: 'list', hideDone: loadHideDone(), busy: false, disposed: false };

  const list = () => visibleItems(store.items, { hideDone: st.hideDone });
  const selected = () => store.items.find((it) => it.id === st.sel) || null;

  const view = {
    render() {
      if (st.disposed) return;
      const items = list();
      st.sel = keepSelection(items, st.sel);
      const item = selected();
      const oldDetail = root.querySelector('.inbox-detail');
      const keepScroll = oldDetail && oldDetail.dataset.id === (item && item.id) ? oldDetail.scrollTop : 0;
      const l = buildList(document, items, { selected: st.sel, hideDone: st.hideDone, total: store.items.length });
      const d = buildDetail(document, item, { focused: st.view === 'detail' });
      d.dataset.id = item ? item.id : '';
      const parts = [l, d];
      if (store.error) parts.unshift(el(document, 'div', 'inbox-error', `인박스를 읽을 수 없습니다: ${store.error}`));
      root.replaceChildren(...parts);
      d.scrollTop = keepScroll;
      root.classList.toggle('view-list', st.view === 'list');
      root.classList.toggle('view-detail', st.view === 'detail');
      const row = l.querySelector('.inbox-row.sel');
      if (row && row.scrollIntoView) row.scrollIntoView({ block: 'nearest' });
    },
  };

  const markRead = (item) => {
    if (item && !item.read) api.inboxMarkRead(item.id).catch((e) => deps.toast(e));
  };

  /** One dialog flow at a time; errors are toasted. */
  const flow = async (fn) => {
    if (st.busy) return;
    st.busy = true;
    try {
      await fn();
    } catch (err) {
      deps.toast(err);
    } finally {
      st.busy = false;
    }
  };

  const requestIssue = async (item) => {
    await api.inboxRequestIssue(item.id);
    deps.toast('이슈 등록 요청을 dispatcher에게 보냈습니다', 'info');
  };

  const actions = {
    down: () => (st.view === 'detail' ? scrollDetail(1) : select(moveSelection(list(), st.sel, 1))),
    up: () => (st.view === 'detail' ? scrollDetail(-1) : select(moveSelection(list(), st.sel, -1))),
    open: () => {
      const item = selected();
      if (!item) return;
      st.view = 'detail';
      view.render();
      markRead(item);
    },
    back: () => {
      if (st.view === 'list') return false; // Esc in the list: not ours
      st.view = 'list';
      view.render();
      return true;
    },
    toggleDone: () => {
      st.hideDone = !st.hideDone;
      saveHideDone(st.hideDone);
      view.render();
    },
    instruct: () =>
      withItem('instruct', '완료된 항목에는 지시할 수 없습니다', (item) =>
        flow(async () => {
          const text = await deps.promptText(`지시 — ${item.title} (${item.ownerName})`, {
            placeholder: 'dispatcher를 거쳐 담당 Manager에게 전달됩니다',
            hint: 'Enter: 줄 바꿈 · Ctrl+Enter: 보내기 · Esc: 취소',
          });
          if (text == null || !text.trim()) return;
          await api.inboxInstruct(item.id, text);
          deps.toast('지시를 dispatcher에게 보냈습니다', 'info');
        }),
      ),
    issue: () =>
      withItem('issue', selected() && selected().issue ? '이미 이슈가 연결된 항목입니다' : '완료된 항목입니다', (item) =>
        flow(async () => {
          if (!(await deps.confirm(`'${item.title}' 항목의 이슈 등록을 담당 Manager에게 요청할까요?`))) return;
          await requestIssue(item);
        }),
      ),
    save: () =>
      withItem('save', '완료된 항목에는 자료를 저장할 수 없습니다', (item) =>
        flow(async () => {
          if (saveStep(item) === 'needIssue') {
            const ok = await deps.confirm('이슈 폴더가 없습니다 — 이슈를 먼저 등록해야 합니다.\n담당 Manager에게 이슈 등록을 요청할까요?');
            if (ok) await requestIssue(item);
            return;
          }
          const text = await deps.promptText(`자료 저장 — ${item.title}: 파일 경로를 한 줄에 하나씩`, {
            placeholder: 'C:\\자료\\보고서.md',
            hint: '복사만 합니다(원본 유지) · Ctrl+Enter: 다음 · Esc: 취소',
          });
          const paths = parsePaths(text);
          if (!paths.length) return;
          const target = await deps.choose(`저장 위치 — 파일 ${paths.length}개`, [
            { label: 'input — 입력 자료', value: 'input' },
            { label: 'output — 산출물', value: 'output' },
          ]);
          if (!target) return;
          const r = await api.inboxAddFiles(item.id, paths, target);
          const n = r && Array.isArray(r.files) ? r.files.length : 0;
          deps.toast(`${n}개 파일을 ${target}에 저장했습니다`, 'info');
          if (r && r.warning) deps.toast(r.warning);
        }),
      ),
    close: () =>
      withItem('close', '종료 제안된 항목만 종료 확인할 수 있습니다', (item) =>
        flow(async () => {
          const ok = await deps.confirm(`'${item.title}' 항목을 완료로 바꿀까요?\n담당 Manager에게 알립니다.`, { enterYes: false });
          if (!ok) return;
          const warning = await api.inboxConfirmDone(item.id);
          deps.toast('완료로 바꿨습니다', 'info');
          if (warning) deps.toast(warning);
        }),
      ),
  };

  function withItem(action, why, fn) {
    const item = selected();
    if (!item) return;
    if (!canDo(action, item)) {
      deps.toast(why);
      return;
    }
    fn(item);
  }

  function select(id) {
    if (id == null || id === st.sel) return;
    st.sel = id;
    view.render();
  }

  function scrollDetail(dir) {
    const d = root.querySelector('.inbox-detail');
    if (d) d.scrollBy(0, dir * 40);
  }

  const onKey = (e) => {
    if (e.defaultPrevented || editable(e.target)) return; // the app key router / an input owns it
    const a = keyAction(e);
    if (!a) return;
    const r = actions[a]();
    if (r === false) return;
    e.preventDefault();
    e.stopPropagation();
  };

  const onMouse = (e) => {
    const t = e.target;
    const btn = t && t.closest ? t.closest('button[data-action]') : null;
    if (btn) {
      e.preventDefault(); // keep DOM focus on the applet
      if (!btn.disabled) actions[btn.dataset.action]();
      return;
    }
    const row = t && t.closest ? t.closest('.inbox-row[data-id]') : null;
    if (row) {
      st.sel = row.dataset.id;
      st.view = 'detail';
      view.render();
      markRead(selected());
    }
  };

  root.addEventListener('keydown', onKey);
  root.addEventListener('mousedown', onMouse);
  subscribe(view);
  view.render();

  return {
    el: root,
    focus() {
      if (!root.contains(document.activeElement)) root.focus({ preventScroll: true });
    },
    dispose() {
      if (st.disposed) return;
      st.disposed = true;
      root.removeEventListener('keydown', onKey);
      root.removeEventListener('mousedown', onMouse);
      unsubscribe(view);
      root.remove();
    },
  };
}
