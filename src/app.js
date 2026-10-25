import { createTerminal } from './pane.js';
import * as api from './api.js';

// TEMP(Task 9): single hardcoded pane "p-test"; replaced by the workspace renderer.
const PANE = 'p-test';
const root = document.getElementById('app');
const { term, el, fit } = createTerminal(PANE);
root.appendChild(el);
requestAnimationFrame(() => {
  fit();
  term.focus();
});
new ResizeObserver(() => fit()).observe(root);
api.on('pty-exit', ({ id, code }) => {
  if (id === PANE) term.write(`\r\n[프로세스 종료: ${code}]\r\n`);
});

// TEMP(Task 9): spike S1 probe. Task 9's key router replaces this.
document.addEventListener(
  'keydown',
  (e) => {
    if (e.altKey && e.code === 'KeyH') e.preventDefault();
  },
  true,
);
