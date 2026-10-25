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
