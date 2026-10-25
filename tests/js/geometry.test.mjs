import { test } from 'node:test';
import assert from 'node:assert/strict';
import { findNeighbor } from '../../src/geometry.js';

const r = (id, left, top, right, bottom) => ({ id, left, top, right, bottom });

test('neighbor crosses group boundary', () => {
  // left group: A (full height); right group: B (top 0-300), C (bottom 300-700), 4px gutter
  const rects = [r('A', 0, 30, 400, 700), r('B', 404, 30, 800, 300), r('C', 404, 304, 800, 700)];
  // A overlaps B by 270px and C by 396px vertically → C
  assert.equal(findNeighbor(rects, 'A', 'right'), 'C');
  assert.equal(findNeighbor(rects, 'B', 'left'), 'A');
  assert.equal(findNeighbor(rects, 'C', 'left'), 'A');
  assert.equal(findNeighbor(rects, 'B', 'down'), 'C');
  assert.equal(findNeighbor(rects, 'C', 'up'), 'B');
  // stacked groups with a tab bar between them (larger gap) still connect
  const stacked = [r('top', 0, 30, 800, 300), r('bot', 0, 330, 800, 700)];
  assert.equal(findNeighbor(stacked, 'top', 'down'), 'bot');
  assert.equal(findNeighbor(stacked, 'bot', 'up'), 'top');
});

test('no neighbor at edge returns null', () => {
  const rects = [r('A', 0, 0, 400, 700), r('B', 404, 0, 800, 700)];
  assert.equal(findNeighbor(rects, 'A', 'left'), null);
  assert.equal(findNeighbor(rects, 'A', 'up'), null);
  assert.equal(findNeighbor(rects, 'A', 'down'), null);
  assert.equal(findNeighbor(rects, 'B', 'right'), null);
  assert.equal(findNeighbor(rects, 'missing', 'left'), null);
  assert.equal(findNeighbor([], 'A', 'left'), null);
  assert.equal(findNeighbor(rects, 'A', 'diagonal'), null);
});

test('prefers overlap over closer non-overlapping', () => {
  // B is right next to A but entirely below it; C is far right but overlaps A vertically
  const rects = [r('A', 0, 0, 100, 100), r('B', 104, 150, 200, 250), r('C', 400, 0, 500, 100)];
  assert.equal(findNeighbor(rects, 'A', 'right'), 'C');
  // without an overlapping candidate the closest one wins
  assert.equal(findNeighbor([rects[0], rects[1]], 'A', 'right'), 'B');
});

test('closest boundary wins, then larger overlap', () => {
  const rects = [r('A', 0, 0, 100, 100), r('near', 104, 50, 200, 100), r('far', 300, 0, 400, 100)];
  assert.equal(findNeighbor(rects, 'A', 'right'), 'near');
  const tie = [r('A', 0, 0, 100, 100), r('small', 104, 80, 200, 300), r('big', 104, 10, 200, 79)];
  assert.equal(findNeighbor(tie, 'A', 'right'), 'big');
});

test('a 1px overlap of borders still counts as beyond', () => {
  const rects = [r('A', 0, 0, 100, 100), r('B', 99.5, 0, 200, 100)];
  assert.equal(findNeighbor(rects, 'A', 'right'), 'B');
  assert.equal(findNeighbor(rects, 'B', 'left'), 'A');
  // a rect that is not completely past is not a candidate
  const partial = [r('A', 0, 0, 100, 100), r('B', 50, 0, 200, 100)];
  assert.equal(findNeighbor(partial, 'A', 'right'), null);
});
