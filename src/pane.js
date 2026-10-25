import { Terminal } from './vendor/xterm.mjs';
import { FitAddon } from './vendor/addon-fit.mjs';
import { Unicode11Addon } from './vendor/addon-unicode11.mjs';
import { SearchAddon } from './vendor/addon-search.mjs';
import { WebglAddon } from './vendor/addon-webgl.mjs';
import * as api from './api.js';

/** Minimal terminal pane: xterm + addons, wired to the PTY of `paneId`. */
export function createTerminal(paneId) {
  const el = document.createElement('div');
  el.className = 'term-host';
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
  term.open(el);
  try {
    const gl = new WebglAddon();
    gl.onContextLoss(() => gl.dispose()); // falls back to the DOM renderer
    term.loadAddon(gl);
  } catch (e) {
    console.warn('WebGL unavailable, using DOM renderer', e);
  }

  // R5: replies to queries inside a replayed snapshot must not reach the live pty.
  let suppressInput = true;
  term.onData((d) => {
    if (!suppressInput) api.write(paneId, d);
  });
  term.onBinary((d) => {
    if (!suppressInput) api.write(paneId, d);
  });

  const fit = () => {
    try {
      fitAddon.fit();
    } catch (_) {
      return;
    }
    api.resize(paneId, term.cols, term.rows).catch(() => {});
  };

  // Live bytes can arrive before the snapshot call resolves: queue until it is written.
  let queue = [];
  let live = false;
  api
    .attach(paneId, (bytes) => {
      if (live) term.write(bytes);
      else queue.push(bytes);
    })
    .then((snap) => {
      const finish = () => {
        suppressInput = false;
        live = true;
        for (const b of queue) term.write(b);
        queue = [];
      };
      if (snap.length) term.write(snap, finish);
      else finish();
    })
    .catch((e) => term.write(`\r\n[attach 실패: ${e}]\r\n`));

  return { term, el, fit, search };
}
