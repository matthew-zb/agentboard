import { createTerminal } from './pane.js';
import * as api from './api.js';

// TEMP(Task 11): interim bootstrap — mounts only the focused tab's last focused pane once it
// is running. The workspace renderer (groups/tabs/layout) replaces all of this.
const root = document.getElementById('app');
let mounted = null;

function focusedPaneId(ws) {
  const g = ws.groups[ws.focus.groupId];
  const t = g && ws.tabs[g.activeTabId];
  return t ? t.lastFocusedPaneId : null;
}

function maybeMount(snap) {
  if (mounted) return;
  const id = focusedPaneId(snap.workspace);
  const pane = id && snap.workspace.panes[id];
  if (!pane) return;
  if (pane.status.state === 'failed') {
    root.textContent = `[${pane.name}] 실행 실패: ${pane.status.message}`;
    return;
  }
  if (pane.status.state !== 'running') return;
  mounted = id;
  root.textContent = '';
  const { term, el, fit } = createTerminal(id);
  root.appendChild(el);
  requestAnimationFrame(() => {
    fit();
    term.focus();
  });
  new ResizeObserver(() => fit()).observe(root);
  api.on('pty-exit', ({ id: exited, code }) => {
    if (exited === mounted) term.write(`\r\n[프로세스 종료: ${code}]\r\n`);
  });
}

api.onWorkspaceChanged(maybeMount);
api.snapshot().then((snap) => {
  console.log('[agentboard] snapshot', snap);
  maybeMount(snap);
});
api.configErrors().then((errs) => {
  if (errs.length) console.warn('[agentboard] 설정 오류', errs);
});

// TEMP(Task 11): spike S1 probe. The key router replaces this.
document.addEventListener(
  'keydown',
  (e) => {
    if (e.altKey && e.code === 'KeyH') e.preventDefault();
  },
  true,
);
