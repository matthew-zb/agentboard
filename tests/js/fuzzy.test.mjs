import { test } from 'node:test';
import assert from 'node:assert/strict';
import { fuzzyFilter, fuzzyRank } from '../../src/fuzzy.js';

test('fuzzy matches subsequence case-insensitively', () => {
  assert.deepEqual(fuzzyFilter(['mdviewer', 'hoohootv', 'ha'], 'mv'), ['mdviewer']);
  assert.deepEqual(fuzzyFilter(['MdViewer', 'hoohootv'], 'mV'), ['MdViewer']);
  assert.deepEqual(fuzzyFilter(['abc'], 'acb'), []); // order matters
});

test('fuzzy ranks prefix and contiguous higher', () => {
  // 'ha': prefix + contiguous > prefix only > contiguous only
  assert.deepEqual(fuzzyFilter(['xha', 'hxxa', 'ha'], 'ha'), ['ha', 'hxxa', 'xha']);
  // contiguous run beats a scattered match when neither is a prefix
  assert.deepEqual(fuzzyFilter(['xaxbxc', 'xxabc'], 'abc'), ['xxabc', 'xaxbxc']);
  // the best placement is found, not the greedy leftmost one ('xaxab' → contiguous 'ab' at the end)
  assert.deepEqual(fuzzyFilter(['xaxxb', 'xaxab'], 'ab'), ['xaxab', 'xaxxb']);
  // a word start (after space, -, _, /) counts too
  assert.deepEqual(fuzzyFilter(['xxtv', 'hoo-tv'], 't'), ['hoo-tv', 'xxtv']);
});

test('fuzzy keeps the original order on ties', () => {
  assert.deepEqual(fuzzyFilter(['hoohootv', 'ha', 'hb'], 'h'), ['hoohootv', 'ha', 'hb']);
  assert.deepEqual(fuzzyRank(['b', 'a', 'ab'], 'z'), []);
});

test('empty query returns all in order', () => {
  assert.deepEqual(fuzzyFilter(['b', 'a', 'c'], ''), ['b', 'a', 'c']);
  assert.deepEqual(fuzzyFilter(['b', 'a'], '   '), ['b', 'a']);
  assert.deepEqual(fuzzyRank(['b', 'a', 'c'], ''), [0, 1, 2]);
});

test('Korean labels match', () => {
  assert.deepEqual(fuzzyFilter(['탭 이름 바꾸기', '사이드바 토글'], '이름'), ['탭 이름 바꾸기']);
});
