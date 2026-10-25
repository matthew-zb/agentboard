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
