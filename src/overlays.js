// Keyboard-driven overlays: number picker, command palette, list picker, new-tab wizard, confirm,
// text prompt, error list, toast. One overlay at a time; while one is open it owns the keyboard
// through the key router's modal hook (no app shortcut fires underneath). Esc cancels.
// Pure logic lives in overlay-logic.js / fuzzy.js. No top-level DOM access (Node-importable).
import * as api from './api.js';
import { fuzzyRank } from './fuzzy.js';
import { PICK_TIMEOUT_MS, TOAST_MS, pickerKey, listMove, dirChoices, newTabFlow } from './overlay-logic.js';

const cfg = {
  /** router.pushModal(fn) -> token / router.popModal(token): a stack, copy mode may sit below. */
  pushModal() {
    return null;
  },
  popModal() {},
  /** Give DOM focus back to the focused pane after the last overlay closed. */
  restoreFocus() {},
  /** Send CreateTab; resolves true on success, false after the error was toasted. */
  createTab: async () => false,
};

/** `{pushModal(fn) → token, popModal(token), restoreFocus(), createTab(cmd) → Promise<bool>}` (set by app.js). */
export function configureOverlays(c) {
  Object.assign(cfg, c);
}

let active = null; // {cancel()} of the open overlay
let epoch = 0; // bumped by closeOverlays: multi-step flows stop when it changes

export function overlayOpen() {
  return active !== null;
}

/** Close the open overlay (its promise resolves as cancelled) and stop running flows. */
export function closeOverlays() {
  epoch++;
  if (active) active.cancel();
}

/** `alive()` for a multi-step flow: false once closeOverlays ran after this call. */
export function flowGuard() {
  const e0 = epoch;
  return () => epoch === e0;
}

function h(tag, cls, text) {
  const el = document.createElement(tag);
  if (cls) el.className = cls;
  if (text != null) el.textContent = text;
  return el;
}

/**
 * Open an overlay. `build(finish, onCleanup)` creates the DOM and returns
 * `{onKey(e) → consumed?, cancelValue}`. Opening closes the previous overlay.
 */
function open(build) {
  if (active) active.cancel();
  return new Promise((resolve) => {
    const cleanups = [];
    let done = false;
    let spec = null;
    let token = null;
    const entry = { cancel: () => finish(spec ? spec.cancelValue : null) };
    function finish(value) {
      if (done) return;
      done = true;
      if (token !== null) cfg.popModal(token);
      if (active === entry) active = null;
      for (const f of cleanups.splice(0)) {
        try {
          f();
        } catch (_) {}
      }
      resolve(value);
      // A follow-up overlay (next wizard step) opens in a microtask; only refocus the pane when
      // nothing replaced this one.
      setTimeout(() => {
        if (!active) cfg.restoreFocus();
      }, 0);
    }
    active = entry;
    try {
      spec = build(finish, (f) => cleanups.push(f));
    } catch (err) {
      console.error('[agentboard] overlay', err);
      finish(null);
      return;
    }
    if (!done) token = cfg.pushModal((e) => (done ? false : spec.onKey(e))); // build may have finished
  });
}

/** Backdrop + centred box. A mousedown on the backdrop cancels. */
function dialog(cls, onOutside) {
  const back = h('div', 'ov-backdrop');
  const box = h('div', `ov-box ${cls}`);
  box.tabIndex = -1;
  back.appendChild(box);
  back.addEventListener('mousedown', (e) => {
    if (e.target === back) {
      e.preventDefault();
      onOutside();
    }
  });
  document.body.appendChild(back);
  return { back, box };
}

const isEnter = (e) => e.code === 'Enter' || e.code === 'NumpadEnter';
const composing = (e) => e.isComposing || e.keyCode === 229;

// ---------------------------------------------------------------- number picker

/**
 * Big numbers 1..9 over each target; resolves `{id}` (number), `{dir}` (hjkl, when allowHjkl),
 * `{key}` ('\\' / '-', when in extraKeys) or null (Esc, a click, or PICK_TIMEOUT_MS without
 * input). Other keys are swallowed (they restart the timer).
 */
export function pickNumber(targets, { allowHjkl = false, extraKeys = [] } = {}) {
  const shown = targets.slice(0, 9);
  return open((finish, onCleanup) => {
    const layer = h('div', 'pick-layer');
    layer.tabIndex = -1; // DOM focus leaves the terminal: no IME text can reach the PTY
    shown.forEach((t, i) => {
      const r = t.el.getBoundingClientRect();
      const frame = h('div', 'pick-target');
      Object.assign(frame.style, { left: `${r.left}px`, top: `${r.top}px`, width: `${r.width}px`, height: `${r.height}px` });
      frame.appendChild(h('div', 'pick-label', String(i + 1)));
      layer.appendChild(frame);
    });
    const help = ['숫자: 선택'];
    if (allowHjkl) help.push('h/j/k/l: 방향');
    if (extraKeys.length) help.push(`${extraKeys.join(' ')}: 새 그룹`);
    help.push('Esc: 취소');
    layer.appendChild(h('div', 'pick-help', help.join(' · ')));
    document.body.appendChild(layer);
    layer.focus({ preventScroll: true });

    let timer = null;
    const arm = () => {
      clearTimeout(timer);
      timer = setTimeout(() => finish(null), PICK_TIMEOUT_MS);
    };
    arm();
    const onMouse = () => finish(null);
    document.addEventListener('mousedown', onMouse, true);
    onCleanup(() => {
      clearTimeout(timer);
      document.removeEventListener('mousedown', onMouse, true);
      layer.remove();
    });

    return {
      cancelValue: null,
      onKey(e) {
        if (document.activeElement !== layer) layer.focus({ preventScroll: true });
        const r = pickerKey(e, shown.length, { allowHjkl, extraKeys });
        if (!r) {
          if (!/^(Control|Alt|Shift|Meta)/.test(e.code || '')) arm();
          return true;
        }
        if (r.cancel) finish(null);
        else if (r.index != null) finish({ id: shown[r.index].id });
        else finish(r);
        return true;
      },
    };
  });
}

// ---------------------------------------------------------------- list picker / palette

/**
 * Text input + list. `compute(query) → [{label, hint?, cls?, value}]`. ↑↓ move (wrap), Enter
 * picks, Esc cancels. Resolves `{value, query}` or null.
 */
function listOverlay({ title, placeholder = '', initial = '', compute, emptyText = '일치하는 항목이 없습니다' }) {
  return open((finish, onCleanup) => {
    const { back, box } = dialog('ov-list-box', () => finish(null));
    onCleanup(() => back.remove());
    if (title) box.appendChild(h('div', 'ov-title', title));
    const input = h('input', 'ov-input');
    Object.assign(input, { type: 'text', placeholder, value: initial, spellcheck: false, autocomplete: 'off' });
    const list = h('ul', 'ov-list');
    box.append(input, list);
    // keep DOM focus in the input when clicking inside the box
    box.addEventListener('mousedown', (e) => {
      if (e.target !== input) e.preventDefault();
    });

    let items = [];
    let sel = 0;
    const paint = () => {
      [...list.children].forEach((li, i) => li.classList.toggle('sel', i === sel && items.length > 0));
      const cur = items.length ? list.children[sel] : null;
      if (cur && cur.scrollIntoView) cur.scrollIntoView({ block: 'nearest' });
    };
    const draw = () => {
      items = compute(input.value);
      if (sel >= items.length) sel = 0;
      list.replaceChildren();
      if (!items.length) list.appendChild(h('li', 'ov-empty', emptyText));
      items.forEach((it, i) => {
        const li = h('li', `ov-item ${it.cls || ''}`);
        li.appendChild(h('span', 'ov-label', it.label));
        if (it.hint) li.appendChild(h('span', 'ov-hint', it.hint));
        li.addEventListener('mousedown', (e) => {
          e.preventDefault();
          sel = i;
          finish({ value: it.value, query: input.value });
        });
        list.appendChild(li);
      });
      paint();
    };
    input.addEventListener('input', () => {
      sel = 0;
      draw();
    });
    draw();
    input.focus();
    input.select();

    return {
      cancelValue: null,
      onKey(e) {
        if (composing(e)) return false; // the IME owns Enter/arrows while composing Hangul
        if (e.code === 'Escape') {
          finish(null);
          return true;
        }
        if (e.target !== input) {
          input.focus();
          return true;
        }
        if (e.code === 'ArrowDown' || e.code === 'ArrowUp') {
          sel = listMove(sel, items.length, e.code === 'ArrowDown' ? 1 : -1);
          paint();
          return true;
        }
        if (isEnter(e)) {
          if (!e.repeat && items.length) finish({ value: items[sel].value, query: input.value });
          return true;
        }
        if (e.code === 'Tab') return true;
        return false; // typing, Backspace, Ctrl+V … go to the input
      },
    };
  });
}

/**
 * Command palette: input + fuzzy list over `entries` ({label, hint?, run}). Enter closes the
 * palette, then runs the entry.
 */
export async function palette(entries) {
  const labels = entries.map((e) => e.label);
  const r = await listOverlay({
    title: '커맨드 팔레트',
    placeholder: '명령, 탭, 창, 그룹 이름 검색',
    compute: (q) => fuzzyRank(labels, q).map((i) => ({ label: entries[i].label, hint: entries[i].hint, value: entries[i] })),
  });
  if (!r || !r.value) return;
  try {
    await r.value.run();
  } catch (err) {
    toast(err);
  }
}

/** Single-line text input. Enter → the text, Esc → null. */
export function prompt(title, initial = '') {
  return open((finish, onCleanup) => {
    const { back, box } = dialog('ov-prompt', () => finish(null));
    onCleanup(() => back.remove());
    box.appendChild(h('div', 'ov-title', title));
    const input = h('input', 'ov-input');
    Object.assign(input, { type: 'text', value: initial, spellcheck: false, autocomplete: 'off' });
    box.appendChild(input);
    input.focus();
    input.select();
    return {
      cancelValue: null,
      onKey(e) {
        if (composing(e)) return false;
        if (e.code === 'Escape') {
          finish(null);
          return true;
        }
        if (e.target !== input) {
          input.focus();
          return true;
        }
        if (isEnter(e)) {
          if (!e.repeat) finish(input.value);
          return true;
        }
        if (e.code === 'Tab') return true;
        return false;
      },
    };
  });
}

// ---------------------------------------------------------------- dialogs

/** Y / Enter = yes (true), N / Esc = no (false). Closing it any other way → false. */
export function confirm(message) {
  return open((finish, onCleanup) => {
    const { back, box } = dialog('ov-confirm', () => finish(false));
    onCleanup(() => back.remove());
    box.appendChild(h('div', 'ov-message', message));
    const btns = h('div', 'ov-buttons');
    const yes = h('button', 'ov-btn primary', '예 (Y)');
    const no = h('button', 'ov-btn', '아니오 (N)');
    yes.addEventListener('mousedown', (e) => (e.preventDefault(), finish(true)));
    no.addEventListener('mousedown', (e) => (e.preventDefault(), finish(false)));
    btns.append(yes, no);
    box.appendChild(btns);
    box.focus();
    return {
      cancelValue: false,
      onKey(e) {
        if (document.activeElement !== box) box.focus();
        if (e.repeat) return true; // a held Enter from the previous step must not answer
        if (e.code === 'KeyY' || isEnter(e)) finish(true);
        else if (e.code === 'KeyN' || e.code === 'Escape') finish(false);
        return true;
      },
    };
  });
}

/** Error lines (`파일:줄:열 — 원인`) in a panel; Esc / Enter closes. Text is selectable. */
export function showErrors(lines) {
  const list = Array.isArray(lines) ? lines : [];
  return open((finish, onCleanup) => {
    const { back, box } = dialog('ov-errors', () => finish(null));
    onCleanup(() => back.remove());
    box.appendChild(h('div', 'ov-title', list.length ? `설정 오류 ${list.length}개 — Esc: 닫기` : '설정 오류 — Esc: 닫기'));
    const ul = h('ul', 'ov-error-list');
    if (!list.length) ul.appendChild(h('li', 'ov-empty', '설정 오류가 없습니다'));
    for (const l of list) ul.appendChild(h('li', 'ov-error', l));
    box.appendChild(ul);
    box.focus();
    return {
      cancelValue: null,
      onKey(e) {
        if (e.code === 'Escape' || isEnter(e)) {
          finish(null);
          return true;
        }
        if (e.ctrlKey && (e.code === 'KeyC' || e.code === 'KeyA')) return false; // copy the text
        const step = { ArrowDown: 40, ArrowUp: -40, PageDown: ul.clientHeight, PageUp: -ul.clientHeight }[e.code];
        if (step) ul.scrollBy(0, step);
        return true;
      },
    };
  });
}

/** Message above the status bar for TOAST_MS (newest at the bottom, at most 4). */
export function toast(message) {
  const text = message && message.message ? message.message : String(message);
  let box = document.getElementById('toasts');
  if (!box) {
    box = h('div');
    box.id = 'toasts';
    document.body.appendChild(box);
  }
  const t = h('div', 'toast', text);
  box.appendChild(t);
  while (box.children.length > 4) box.firstChild.remove();
  setTimeout(() => t.remove(), TOAST_MS);
}

// ---------------------------------------------------------------- new-tab wizard

/** ① workspace dir (fuzzy; unknown name → create) → ② template → ③ CreateTab. Esc cancels. */
export function newTabWizard() {
  return newTabFlow({
    alive: flowGuard(),
    listDirs: () => api.listDirs(),
    listTemplates: () => api.listTemplates(),
    defaultTemplate: () => api.defaultTemplate(),
    pickDir: (dirs) =>
      listOverlay({
        title: '새 탭 ① 작업 폴더 — 목록에 없는 이름을 입력하면 새로 만듭니다',
        placeholder: '폴더 이름',
        emptyText: '폴더가 없습니다. 새 이름을 입력하세요',
        compute: (q) =>
          dirChoices(dirs, q).map((c) => ({
            label: c.create ? `새 폴더 만들기: ${c.dir}` : c.dir,
            cls: c.create ? 'create' : '',
            value: c,
          })),
      }).then((r) => (r ? r.value : null)),
    confirm,
    createDir: (name) => api.createDir(name),
    pickTemplate: (list, def, dir) =>
      listOverlay({
        title: `새 탭 ② 템플릿 — 폴더: ${dir}`,
        placeholder: '템플릿 이름',
        emptyText: '템플릿이 없습니다',
        compute: (q) => fuzzyRank(list, q).map((i) => ({ label: list[i], hint: list[i] === def ? '기본값' : '', value: list[i] })),
      }).then((r) => (r ? r.value : null)),
    createTab: (cmd) => cfg.createTab(cmd),
    toast,
  });
}
