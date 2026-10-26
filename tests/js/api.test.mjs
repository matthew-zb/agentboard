import { test } from 'node:test';
import assert from 'node:assert/strict';
import { makeVersionGate } from '../../src/api.js';

test('version gate delivers only newer snapshots', () => {
  const g = makeVersionGate();
  assert.equal(g.accept({ version: 1 }), true);
  assert.equal(g.accept({ version: 3 }), true);
  assert.equal(g.accept({ version: 2 }), false); // stale, arrived late
  assert.equal(g.accept({ version: 3 }), false); // duplicate
  assert.equal(g.accept({ version: 4 }), true);
  assert.equal(g.last, 4);
});

test('version gate rejects malformed snapshots', () => {
  const g = makeVersionGate();
  assert.equal(g.accept(null), false);
  assert.equal(g.accept({}), false);
  assert.equal(g.accept({ version: '5' }), false);
  assert.equal(g.last, 0);
});

test('separate gates are independent', () => {
  const a = makeVersionGate();
  const b = makeVersionGate();
  a.accept({ version: 9 });
  assert.equal(b.accept({ version: 1 }), true);
});

import { readClipboardText, writeClipboardText } from '../../src/api.js';

test('clipboard wrappers go through __TAURI__.clipboardManager', async () => {
  const calls = [];
  globalThis.__TAURI__ = {
    clipboardManager: {
      writeText: async (t) => calls.push(['w', t]),
      readText: async () => '붙여넣기',
    },
  };
  await writeClipboardText('복사');
  assert.equal(await readClipboardText(), '붙여넣기');
  assert.deepEqual(calls, [['w', '복사']]);
  globalThis.__TAURI__.clipboardManager.readText = async () => null;
  assert.equal(await readClipboardText(), '');
  delete globalThis.__TAURI__;
});

// ---- first-use fixes F1: pty_write (user input) / pty_write_raw (terminal replies, binary)
import { write, writeRaw } from '../../src/api.js';

function fakeInvoke() {
  const calls = [];
  globalThis.__TAURI__ = { core: { invoke: async (cmd, args) => calls.push([cmd, args]) } };
  return calls;
}

test('writeRaw sends pty_write_raw { pane, bytes } as a plain number array', async () => {
  const calls = fakeInvoke();
  // a Uint8Array inside the args object would be JSON-serialized as {"0":27,...}, not an array
  await writeRaw('p1', new Uint8Array([0x1b, 0x5b, 0x4d, 0xa0, 0xff]));
  await writeRaw('p2', [0x1b, 0x5b, 0x49]);
  assert.equal(calls.length, 2);
  assert.equal(calls[0][0], 'pty_write_raw');
  assert.equal(Array.isArray(calls[0][1].bytes), true);
  assert.deepEqual(calls[0][1], { pane: 'p1', bytes: [0x1b, 0x5b, 0x4d, 0xa0, 0xff] });
  assert.deepEqual(calls[1], ['pty_write_raw', { pane: 'p2', bytes: [0x1b, 0x5b, 0x49] }]);
  delete globalThis.__TAURI__;
});

test('write keeps sending pty_write { pane, data } (user input contract)', async () => {
  const calls = fakeInvoke();
  await write('p1', '한글 a\r');
  assert.deepEqual(calls, [['pty_write', { pane: 'p1', data: '한글 a\r' }]]);
  delete globalThis.__TAURI__;
});
