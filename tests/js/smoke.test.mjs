import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';

test('xterm vendor file exists', () => {
  assert.ok(existsSync(new URL('../../src/vendor/xterm.mjs', import.meta.url)));
});
