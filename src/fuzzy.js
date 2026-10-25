// Fuzzy filter (pure): case-insensitive subsequence match, ranked by score.
// Score per matched char: 1, +5 when it directly follows the previous match (contiguous),
// +10 at the very start of the item, +3 at a word start (after space - _ / . : ›).
// The best placement is found (small DP), not the greedy leftmost one.

const CONTIGUOUS = 5;
const START = 10;
const WORD = 3;
const SEPARATORS = new Set([' ', '-', '_', '/', '\\', '.', ':', '›', '→', '(', '[']);

/** Best score of `q` (lowercased) as a subsequence of `s` (lowercased), or -1. */
export function fuzzyScore(s, q) {
  const n = s.length;
  const m = q.length;
  if (m === 0) return 0;
  if (m > n) return -1;
  const bonus = (i) => (i === 0 ? START : SEPARATORS.has(s[i - 1]) ? WORD : 0);
  // prev[i] = best score with q[k-1] matched at s[i] (-Infinity if impossible)
  let prev = new Array(n).fill(-Infinity);
  for (let i = 0; i < n; i++) if (s[i] === q[0]) prev[i] = 1 + bonus(i);
  for (let k = 1; k < m; k++) {
    const cur = new Array(n).fill(-Infinity);
    let bestBefore = -Infinity; // max prev[j] for j < i - 1
    for (let i = 1; i < n; i++) {
      if (i >= 2) bestBefore = Math.max(bestBefore, prev[i - 2]);
      if (s[i] !== q[k]) continue;
      const viaGap = bestBefore;
      const viaAdj = prev[i - 1] + CONTIGUOUS;
      const base = Math.max(viaGap, viaAdj);
      if (base > -Infinity) cur[i] = base + 1 + bonus(i);
    }
    prev = cur;
  }
  const best = Math.max(...prev);
  return best === -Infinity ? -1 : best;
}

/**
 * Indices of `items` matching `query`, best score first; ties keep the original order.
 * An empty (or blank) query returns every index in order.
 */
export function fuzzyRank(items, query) {
  const q = String(query ?? '').trim().toLowerCase();
  if (!q) return items.map((_, i) => i);
  const scored = [];
  items.forEach((item, i) => {
    const sc = fuzzyScore(String(item).toLowerCase(), q);
    if (sc >= 0) scored.push([sc, i]);
  });
  scored.sort((a, b) => b[0] - a[0] || a[1] - b[1]);
  return scored.map((x) => x[1]);
}

/** `items` matching `query` (see fuzzyRank). */
export function fuzzyFilter(items, query) {
  return fuzzyRank(items, query).map((i) => items[i]);
}
