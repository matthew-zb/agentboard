import { test } from 'node:test';
import assert from 'node:assert/strict';
import { dropZone, EDGE } from '../../src/dropzone.js';

const rect = { left: 0, top: 0, width: 100, height: 100 };

test('dropZone edges and center', () => {
  assert.equal(EDGE, 0.25);
  assert.equal(dropZone(rect, 10, 50), 'left');
  assert.equal(dropZone(rect, 90, 50), 'right');
  assert.equal(dropZone(rect, 50, 10), 'top');
  assert.equal(dropZone(rect, 50, 90), 'bottom');
  assert.equal(dropZone(rect, 50, 50), 'center');
  assert.equal(dropZone(rect, 24.9, 50), 'left');
  assert.equal(dropZone(rect, 25.1, 50), 'center');
});

test('corner prefers nearer edge', () => {
  assert.equal(dropZone(rect, 5, 20), 'left');
  assert.equal(dropZone(rect, 20, 5), 'top');
  assert.equal(dropZone(rect, 95, 80), 'right');
  assert.equal(dropZone(rect, 80, 95), 'bottom');
  assert.equal(dropZone(rect, 10, 10), 'left'); // tie -> horizontal edge wins
});

test('offset rect and out-of-rect points are clamped', () => {
  const r = { left: 200, top: 100, width: 400, height: 200 };
  assert.equal(dropZone(r, 210, 200), 'left');
  assert.equal(dropZone(r, 400, 200), 'center');
  assert.equal(dropZone(r, -50, 200), 'left');
  assert.equal(dropZone(r, 400, 9999), 'bottom');
});
