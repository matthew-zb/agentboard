import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { keyTable } from '../../scripts/keytable.mjs';

const readme = readFileSync(new URL('../../README.md', import.meta.url), 'utf8').replace(/\r\n/g, '\n');
const defaults = JSON.parse(readFileSync(new URL('../../src-tauri/defaults/keybindings.json', import.meta.url), 'utf8'));

test('README key table matches defaults/keybindings.json (run scripts/keytable.mjs to regenerate)', () => {
  const m = /<!-- KEYTABLE:START -->\n([\s\S]*?)\n?<!-- KEYTABLE:END -->/.exec(readme);
  assert.ok(m, 'KEYTABLE markers missing');
  assert.equal(m[1].trim(), keyTable().trim());
});

test('generated table covers every default command id', () => {
  const t = keyTable();
  for (const id of Object.keys(defaults)) {
    const m = /^(tab|group)\.goto[1-9]$/.exec(id);
    if (m) assert.ok(t.includes(`${m[1]}.goto1..9`), id);
    else assert.ok(t.includes(`\`${id}\``), id);
  }
});
