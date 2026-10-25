// xterm instances per PaneId: created once, reused (DOM moved, never recreated) until the pane
// disappears from the snapshot. Header, attach/re-attach, fit + pty_resize, restart on Enter.
// No top-level DOM access: the pure helpers are unit-tested in Node.
import { Terminal } from './vendor/xterm.mjs';
import { FitAddon } from './vendor/addon-fit.mjs';
import { Unicode11Addon } from './vendor/addon-unicode11.mjs';
import { SearchAddon } from './vendor/addon-search.mjs';
import { WebglAddon } from './vendor/addon-webgl.mjs';
import * as api from './api.js';

// ---------------------------------------------------------------- pure helpers

/** Header status: ● running, ○ exited(code), ◌ starting, ✕ failed. */
export function statusSymbol(pane) {
  const s = (pane && pane.status) || {};
  switch (s.state) {
    case 'running':
      return { symbol: '●', label: '●', title: '실행 중', cls: 'running' };
    case 'exited':
      return { symbol: '○', label: `○ ${s.code}`, title: `종료됨 (코드 ${s.code})`, cls: 'exited' };
    case 'starting':
      return { symbol: '◌', label: '◌', title: '시작 중', cls: 'starting' };
    case 'failed':
      return { symbol: '✕', label: '✕ 실패', title: s.message ? `실행 실패: ${s.message}` : '실행 실패', cls: 'failed' };
    default:
      return { symbol: '?', label: '?', title: '알 수 없는 상태', cls: 'unknown' };
  }
}

/** Tooltip for ⚠, or null when the pane needs no attention. */
export function warnTitle(pane) {
  const parts = [];
  if (pane.attention) parts.push(pane.attentionReason ? `주의: ${pane.attentionReason}` : '주의: 이 창에서 알림(벨)이 왔습니다');
  if (pane.promptWarning) parts.push('프롬프트 경고: 준비 신호를 감지하지 못해 시간 초과 후 프롬프트를 넣었습니다');
  return parts.length ? parts.join('\n') : null;
}

/** Exited or failed: shows "Enter: 재시작" and Enter restarts. */
export function isDead(pane) {
  const st = pane && pane.status && pane.status.state;
  return st === 'exited' || st === 'failed';
}

/**
 * What to do with the PTY subscription when a pane's status is seen.
 * - never attached + running/exited → 'attach' (exited still has its ring buffer)
 * - attached + running, coming from another state or after a local restart → 'reattach'
 *   (a restarted process has a fresh ring and the old channel is gone)
 * - attached + exited right after a restart (starting → exited, or marked) → 'reattach': the new
 *   process died before it was seen running; its output is only in the fresh ring
 * - otherwise 'none' (starting: no process yet; failed: nothing to show)
 */
export function attachAction(prevState, state, attached, restartMarked) {
  if (state === 'running') {
    if (!attached) return 'attach';
    if (restartMarked || prevState !== 'running') return 'reattach';
    return 'none';
  }
  if (state === 'exited') {
    if (!attached) return 'attach';
    if (restartMarked || prevState === 'starting') return 'reattach';
  }
  return 'none';
}

/**
 * Whether a local restart mark survives this status. It is consumed by any (re)attach and
 * dropped when the restart ends in a terminal state (exited/failed) without one.
 */
export function keepRestartMark(state, action, marked) {
  return !!marked && action === 'none' && state !== 'failed' && state !== 'exited';
}

/**
 * Applet pane (`kind: {type: 'applet', kind}`): an app screen, not a terminal — no xterm, no
 * process, no input. Panes without `kind` (phase-1 snapshots) are terminals.
 */
export function isApplet(pane) {
  return !!(pane && pane.kind && pane.kind.type === 'applet');
}

const APPLET_LABELS = { inbox: '인박스 (준비 중)' };

/** Placeholder text for an applet until its real view exists. */
export function appletLabel(pane) {
  const k = pane && pane.kind && pane.kind.kind;
  return APPLET_LABELS[k] || `애플릿 '${k}' (준비 중)`;
}

/** Header buttons of a pane: applets have nothing to restart. */
export function headerButtons(pane) {
  return isApplet(pane) ? ['close'] : ['restart', 'close'];
}

/** Known pane ids that are no longer in the snapshot's `panes` map. */
export function staleIds(knownIds, panes) {
  return [...knownIds].filter((id) => !Object.prototype.hasOwnProperty.call(panes, id));
}

/** pty_resize only when cols/rows actually change. */
export function sizeChanged(last, cols, rows) {
  return !last || last.cols !== cols || last.rows !== rows;
}

/**
 * Sync input (spec 6.3): panes that receive what is typed in `paneId`. null when the pane's tab
 * has sync off (normal single-pane path); otherwise every Running pane of the tab, each once.
 */
export function syncTargets(ws, paneId) {
  const p = ws && ws.panes && ws.panes[paneId];
  const tab = p && ws.tabs && ws.tabs[p.tab];
  if (!tab || !tab.syncInput) return null;
  return Object.values(ws.panes)
    .filter((q) => q.tab === p.tab && !isApplet(q) && q.status && q.status.state === 'running')
    .map((q) => q.id);
}

const REPLY_PATTERNS = [
  /^\x1b\[\??\d+;\d+R$/, // CPR
  /^\x1b\[[?>]\d*(;\d+)*c$/, // DA1 / DA2
  /^\x1b\[[IO]$/, // focus in/out (DECSET 1004)
  /^\x1b\[<\d+;\d+;\d+[Mm]$/, // SGR mouse
  /^\x1b\[M[\s\S]{3}$/, // X10 mouse
  /^\x1b[\]P][\s\S]*(\x07|\x1b\\)$/, // OSC / DCS responses
  /^\x1b\[\??\d*n$/, // DSR status
  /^\x1b\[\??\d+(;\d+)*\$y$/, // DECRPM
  /^\x1b\[\d+(;\d+)+t$/, // window-size reports
];

/** True when `data` (from term.onData) is xterm's own answer/report, not user input. */
export function isTerminalReply(data) {
  return typeof data === 'string' && data.startsWith('\x1b') && REPLY_PATTERNS.some((r) => r.test(data));
}

/**
 * Panes to write `data` typed/pasted in `sourceId` to. Terminal replies go to the source only;
 * user input also goes to the other `targets` (each once) unless they are not live
 * (`suppressInput`), in copy mode (`inputBlocked`) or missing. `entryOf(id) -> entry|undefined`.
 */
export function fanoutIds(sourceId, targets, data, entryOf) {
  if (!targets || !targets.length || isTerminalReply(data)) return [sourceId];
  const out = [sourceId];
  for (const t of new Set(targets)) {
    if (t === sourceId) continue;
    const te = entryOf(t);
    if (te && !te.suppressInput && !te.inputBlocked) out.push(t);
  }
  return out;
}

// ---------------------------------------------------------------- runtime

const entries = new Map();
let hooks = {
  send: (cmd) => api.send(cmd),
  onFocus: () => {},
  report: (e) => console.error('[agentboard]', e),
  keyFilter: () => true,
  syncTargets: () => null,
  onBell: () => {},
};

/**
 * Wire the module to the app: `send(cmd)` (renders its result), `onFocus(group, tab, pane)`
 * (pane clicked while not focused), `report(error)` (status bar message),
 * `keyFilter(paneId, KeyboardEvent) -> bool` (xterm attachCustomKeyEventHandler: false = an app
 * shortcut, xterm must not process it), `syncTargets(paneId) -> paneId[] | null` (sync input), `onBell(paneId)` (terminal bell in a live pane).
 */
export function configurePanes(h) {
  hooks = { ...hooks, ...h };
}

function parking() {
  return document.getElementById('parking') || document.body;
}

function div(cls) {
  const el = document.createElement('div');
  el.className = cls;
  return el;
}

/**
 * The pane's entry `{id, el, term, search, ...}`; created on first use (parked in `#parking`
 * until placed). Attach happens in syncPane once the status allows it (a `starting` pane has no
 * process to attach to yet).
 */
export function getPane(paneId, model) {
  let e = entries.get(paneId);
  if (!e) {
    e = isApplet(model) ? createAppletEntry(paneId, model) : createEntry(paneId);
    entries.set(paneId, e);
  }
  return e;
}

/**
 * Entry for an applet pane: same element/header shape as a terminal entry but `term: null`
 * (no xterm, no attach, no input). Placeholder content until the applet view lands (Task 10).
 */
function createAppletEntry(id, model) {
  const el = div('pane applet');
  el.dataset.pane = id;
  const header = div('pane-header');
  const host = div('applet-host');
  const body = div('applet-placeholder');
  body.textContent = appletLabel(model);
  host.appendChild(body);
  el.append(header, host);
  parking().appendChild(el);
  const e = {
    id,
    el,
    header,
    host,
    restartLine: null,
    term: null,
    fitAddon: null,
    search: null,
    model: null,
    ctx: null,
    prevState: null,
    attachState: 'none',
    gen: 0,
    restartMarked: false,
    suppressInput: true, // never a sync-input target
    live: false,
    queue: [],
    lastSize: null,
    visible: false,
    fitQueued: false,
    resizeTimer: null,
    writeFailed: false,
    inputBlocked: false,
    ro: null,
  };
  el.tabIndex = -1; // focusable, so pane focus has somewhere to go
  el.addEventListener('mousedown', () => {
    if (e.ctx && (!e.ctx.focused || e.ctx.attention)) hooks.onFocus(e.ctx.group, e.ctx.tab, id);
  });
  return e;
}

export function hasPane(paneId) {
  return entries.has(paneId);
}

export function knownPaneIds() {
  return [...entries.keys()];
}

function createEntry(id) {
  const el = div('pane');
  el.dataset.pane = id;
  const header = div('pane-header');
  // Padding lives on .term-host; xterm opens in .term-inner so FitAddon measures the content box.
  const host = div('term-host');
  const inner = div('term-inner');
  host.appendChild(inner);
  const restartLine = div('pane-restart hidden');
  restartLine.textContent = 'Enter: 재시작';
  el.append(header, host, restartLine);
  parking().appendChild(el); // xterm needs a laid-out element to measure the font

  const term = new Terminal({
    scrollback: 10000,
    fontFamily: 'Consolas, "Cascadia Mono", "Malgun Gothic", monospace',
    fontSize: 14,
    allowProposedApi: true,
    theme: { background: '#1e1e1e' },
  });
  const fitAddon = new FitAddon();
  const search = new SearchAddon();
  term.loadAddon(fitAddon);
  term.loadAddon(new Unicode11Addon());
  term.loadAddon(search);
  term.unicode.activeVersion = '11';
  term.open(inner);
  // App shortcuts never reach xterm (the document capture router already handles them).
  term.attachCustomKeyEventHandler((ev) => hooks.keyFilter(id, ev));
  try {
    const gl = new WebglAddon();
    gl.onContextLoss(() => gl.dispose()); // falls back to the DOM renderer
    term.loadAddon(gl);
  } catch (err) {
    console.warn('WebGL unavailable, using DOM renderer', err);
  }

  const e = {
    id,
    el,
    header,
    host,
    restartLine,
    term,
    fitAddon,
    search,
    model: null,
    ctx: null,
    prevState: null,
    attachState: 'none', // none | pending | ok | error
    gen: 0,
    restartMarked: false,
    // R5: replies to queries inside a replayed snapshot must not reach the live pty.
    suppressInput: true,
    live: false,
    queue: [],
    lastSize: null,
    visible: false,
    fitQueued: false,
    resizeTimer: null,
    writeFailed: false,
    inputBlocked: false, // copy mode: nothing typed/pasted/composed may reach any PTY
    ro: null,
  };

  const toPty = (target, d) => {
    api.write(target, d).catch((err) => {
      const t = entries.get(target);
      if (t && !t.writeFailed) {
        t.writeFailed = true;
        t.term.write(`\r\n\x1b[31m[입력 전달 실패: ${err}]\x1b[0m\r\n`);
      }
    });
  };
  const input = (d) => {
    if (e.inputBlocked) return;
    if (isDead(e.model)) {
      if (d === '\r') restartPane(id);
      return;
    }
    if (e.suppressInput) return;
    // Sync input: user input is written once per Running pane of the tab (source included);
    // terminal replies stay with the source pane. No echo loop: other panes' onData is not involved.
    for (const t of fanoutIds(id, hooks.syncTargets(id), d, (x) => entries.get(x))) toPty(t, d);
  };
  // BEL inside the replayed snapshot (not live yet) is history, not a new request.
  term.onBell(() => {
    if (e.live) hooks.onBell(id);
  });
  term.onData(input);
  term.onBinary((d) => {
    if (!e.inputBlocked && !isDead(e.model) && !e.suppressInput) api.write(id, d).catch(() => {});
  });

  el.addEventListener('mousedown', () => {
    if (e.ctx && (!e.ctx.focused || e.ctx.attention)) hooks.onFocus(e.ctx.group, e.ctx.tab, id);
  });

  e.ro = new ResizeObserver(() => scheduleFit(e));
  e.ro.observe(host);
  return e;
}

/** Block/unblock all input of a pane (copy mode); its terminal still renders output. */
export function setInputBlocked(paneId, blocked) {
  const e = entries.get(paneId);
  if (e) e.inputBlocked = !!blocked;
}

/**
 * Paste text into a pane through xterm (bracketed paste when the app enabled it). The resulting
 * onData goes through the normal input path, so sync input fans it out exactly once per pane.
 * Ignored in copy mode.
 */
export function pasteText(paneId, text) {
  const e = entries.get(paneId);
  if (!e || !e.term || e.inputBlocked || !text) return false;
  e.term.paste(text);
  return true;
}

/** Header element for a Pane model: name, profile, status symbol, ⚠, ⟳, ✕. */
export function paneHeader(pane) {
  const h = div('pane-header');
  const st = statusSymbol(pane);
  const status = document.createElement('span');
  status.className = `pane-status ${st.cls}`;
  status.textContent = st.label;
  status.title = st.title;
  const name = document.createElement('span');
  name.className = 'pane-name';
  name.textContent = pane.name;
  const profile = document.createElement('span');
  profile.className = 'pane-profile';
  profile.textContent = isApplet(pane) ? '애플릿' : pane.agent;
  h.append(status, name, profile);
  const warn = warnTitle(pane);
  if (warn) {
    const w = document.createElement('span');
    w.className = 'pane-warn';
    w.textContent = '⚠';
    w.title = warn;
    h.appendChild(w);
  }
  const spacer = document.createElement('span');
  spacer.className = 'spacer';
  h.appendChild(spacer);
  if (headerButtons(pane).includes('restart')) h.appendChild(button('restart', '⟳', '재시작', () => restartPane(pane.id)));
  h.appendChild(button('close', '✕', '창 닫기', () => run({ type: 'closePane', pane: pane.id })));
  return h;
}

function button(cls, text, title, onClick) {
  const b = document.createElement('button');
  b.className = `pane-btn ${cls}`;
  b.type = 'button';
  b.tabIndex = -1;
  b.textContent = text;
  b.title = title;
  b.addEventListener('mousedown', (ev) => {
    ev.preventDefault(); // keep DOM focus in the terminal
    ev.stopPropagation(); // a header button is not a focus click
  });
  b.addEventListener('click', (ev) => {
    ev.stopPropagation();
    onClick();
  });
  return b;
}

function run(cmd) {
  // hooks.send (app sendCmd) already reports a rejected command (toast); don't report it twice.
  return Promise.resolve(hooks.send(cmd)).catch(() => {});
}

/** Send RestartPane; the next `running` status re-attaches to the new process. */
export function restartPane(paneId) {
  const e = entries.get(paneId);
  if (e) e.restartMarked = true;
  // hooks.send (app sendCmd) already reports the error; reporting again would toast it twice.
  return Promise.resolve(hooks.send({ type: 'restartPane', pane: paneId })).catch(() => {
    if (e) e.restartMarked = false;
  });
}

/**
 * Apply a Pane model from a snapshot. ctx: `{group, tab, focused, attention}` (focused = the
 * workspace focus chain ends at this pane; a click on a focused pane that shows ⚠ re-sends
 * SetFocus, which clears it). Updates header/classes and attaches when appropriate.
 */
export function syncPane(pane, ctx) {
  const e = getPane(pane.id, pane);
  const prev = e.prevState;
  const state = pane.status && pane.status.state;
  e.model = pane;
  e.ctx = ctx;

  const h = paneHeader(pane);
  e.header.replaceWith(h);
  e.header = h;
  e.el.classList.toggle('focused', !!ctx.focused);
  if (!e.term) {
    // applet: no process to attach to, nothing to restart into
    e.prevState = state;
    return e;
  }
  e.el.classList.toggle('dead', isDead(pane));
  e.restartLine.classList.toggle('hidden', !isDead(pane));

  const action = attachAction(prev, state, e.attachState !== 'none', e.restartMarked);
  e.restartMarked = keepRestartMark(state, action, e.restartMarked);
  if (prev !== state && action === 'none') {
    if (state === 'exited' && e.attachState === 'ok') e.term.write(`\r\n\x1b[90m[프로세스 종료: ${pane.status.code}]\x1b[0m\r\n`);
    if (state === 'failed') e.term.write(`\r\n\x1b[31m[${pane.status.message || '실행 실패'}]\x1b[0m\r\n`);
  }
  if (action !== 'none') attach(e, action === 'reattach');
  e.prevState = state;
  return e;
}

function attach(e, reset) {
  const gen = ++e.gen;
  const { term, id } = e;
  if (reset) term.reset();
  e.restartMarked = false; // any attach consumes a pending restart
  e.attachState = 'pending';
  e.suppressInput = true;
  e.live = false;
  e.queue = [];
  e.lastSize = null; // a new process starts at 80x24: resend our size
  e.writeFailed = false;
  // Live bytes can arrive before the snapshot call resolves: queue until it is written.
  api
    .attach(id, (bytes) => {
      if (gen !== e.gen) return;
      if (e.live) term.write(bytes);
      else e.queue.push(bytes);
    })
    .then((snap) => {
      if (gen !== e.gen) return;
      const finish = () => {
        if (gen !== e.gen) return;
        goLive(e);
        e.attachState = 'ok';
        fitPane(id);
      };
      if (snap.length) term.write(snap, finish);
      else finish();
    })
    .catch((err) => {
      if (gen !== e.gen) return;
      // Never leave the pane silently ignoring input.
      goLive(e);
      e.attachState = 'error';
      term.write(`\r\n\x1b[31m[터미널 연결 실패: ${err}]\x1b[0m\r\n`);
    });
}

function goLive(e) {
  e.suppressInput = false;
  e.live = true;
  for (const b of e.queue) e.term.write(b);
  e.queue = [];
}

/** Mark whether the pane is on screen; hidden panes are moved into `#parking`. */
export function setPaneVisible(paneId, visible) {
  const e = entries.get(paneId);
  if (!e) return;
  e.visible = visible;
  if (!visible) {
    const p = parking();
    if (e.el.parentElement !== p) p.appendChild(e.el);
  }
}

function scheduleFit(e) {
  if (e.fitQueued) return;
  e.fitQueued = true;
  requestAnimationFrame(() => {
    e.fitQueued = false;
    fitPane(e.id);
  });
}

/** Fit the terminal to its container; pty_resize only when cols/rows change. */
export function fitPane(paneId) {
  const e = entries.get(paneId);
  if (!e || !e.term || !e.visible || !e.el.isConnected) return;
  if (e.host.clientWidth < 20 || e.host.clientHeight < 20) return;
  try {
    e.fitAddon.fit();
  } catch (_) {
    return;
  }
  // The xterm refits immediately; pty_resize is debounced so a border drag sends one resize.
  clearTimeout(e.resizeTimer);
  e.resizeTimer = setTimeout(() => sendResize(e), RESIZE_DEBOUNCE_MS);
}

const RESIZE_DEBOUNCE_MS = 100;

function sendResize(e) {
  if (entries.get(e.id) !== e) return; // disposed
  const { cols, rows } = e.term;
  const running = e.model && e.model.status && e.model.status.state === 'running';
  if (e.attachState !== 'ok' || !running || !sizeChanged(e.lastSize, cols, rows)) return;
  e.lastSize = { cols, rows };
  api.resize(e.id, cols, rows).catch(() => {
    e.lastSize = null; // retry on the next fit
  });
}

/** Give the pane's terminal DOM focus unless it already has it. */
export function focusPane(paneId) {
  const e = entries.get(paneId);
  if (!e || !e.el.isConnected) return;
  if (e.el.contains(document.activeElement)) return;
  if (e.term) e.term.focus();
  else e.el.focus({ preventScroll: true });
}

/** Called for panes missing from the snapshot. */
export function disposePane(paneId) {
  const e = entries.get(paneId);
  if (!e) return;
  e.gen++; // ignore late attach callbacks
  clearTimeout(e.resizeTimer);
  entries.delete(paneId);
  if (e.ro) e.ro.disconnect();
  try {
    if (e.term) e.term.dispose();
  } catch (_) {
    /* already gone */
  }
  e.el.remove();
}
