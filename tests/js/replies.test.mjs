// first-use fixes F1: what the real ConPTY + Claude Code 2.1.310 startup stream makes xterm answer.
// Measured 2026-10-26 with AgentBoard's ConPTY flags (portable-pty 0.9: INHERIT_CURSOR |
// RESIZE_QUIRK | WIN32_INPUT_MODE, system conhost): ConPTY turns on focus reporting for every
// pane; Claude turns it on twice more and enables any-motion SGR mouse reports; Claude's
// XTVERSION / kitty / OSC 7501 probes get no answer. The first ESC[6n is answered by
// src-tauri/src/pty.rs (filter_dsr) and never reaches xterm. Every answer xterm produces must be
// routed as a terminal reply (pty_write_raw, not counted as typing — R4).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Terminal } from '../../src/vendor/xterm.mjs';
import { routeInput } from '../../src/pane.js';

// mode / query sequences of the captured stream, in order (text and cursor moves omitted)
const STARTUP =
  '\x1b[?9001h\x1b[?1004h' + // ConPTY, t=0.03 s
  '\x1b]0;claude\x07' +
  '\x1b[?2004h\x1b[?2031h\x1b[?1004h' + // Claude, t=0.63 s
  '\x1b[>0q\x1b[?u\x1b]7501;?\x07' + // Claude capability probes (no answer expected)
  '\x1b[>4m\x1b[?2031l\x1b[?2004l' +
  '\x1b[?2004h\x1b[?2031h\x1b[?1004h' + // Claude, t=1.28 s
  '\x1b[?1049h\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h' + // alt screen + any-motion SGR mouse
  '\x1b[c\x1b[6n'; // DA1 / CPR: answered by the system conhost today, by xterm with a newer conhost

test('xterm answers to the measured ConPTY + Claude 2.1.310 startup stream are all terminal replies', async () => {
  const term = new Terminal({ allowProposedApi: true, cols: 80, rows: 24 });
  const answers = [];
  term.onData((d) => answers.push(d));
  term.onBinary((d) => answers.push(d));
  await new Promise((r) => term.write(STARTUP, r));
  // one focus report per ESC[?1004h (no DOM: not focused → ESC[O), plus DA1 and CPR
  assert.deepEqual(answers, ['\x1b[O', '\x1b[O', '\x1b[O', '\x1b[?1;2c', '\x1b[1;1R']);
  const calls = [];
  const io = { write: (id, s) => calls.push(['write', id, s]), writeRaw: (id) => calls.push(['writeRaw', id]) };
  for (const d of answers) routeInput('p1', ['p1', 'p2'], d, () => ({}), io);
  assert.deepEqual(calls, answers.map(() => ['writeRaw', 'p1']));
});
