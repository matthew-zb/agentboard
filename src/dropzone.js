// Drop-zone classification for tab drag & drop onto a group. Pure: no DOM access.

/** Fraction of each dimension (from every edge) that counts as an edge zone. */
export const EDGE = 0.25;

const clamp01 = (v) => Math.min(1, Math.max(0, v));

/**
 * Which zone of `rect` the client point (x, y) falls in.
 * - `rect` is `{left, top, width, height}` in client coordinates.
 * - A point strictly within EDGE (25%) of an edge selects that edge; otherwise 'center'.
 * - Points outside the rect are clamped into it first.
 * - Where two edge zones overlap (corners) the nearer edge (smaller normalized distance) wins;
 *   on an exact tie the horizontal edges (left/right) win over top/bottom.
 * @returns {'left'|'right'|'top'|'bottom'|'center'}
 */
export function dropZone(rect, x, y) {
  const fx = rect.width > 0 ? clamp01((x - rect.left) / rect.width) : 0.5;
  const fy = rect.height > 0 ? clamp01((y - rect.top) / rect.height) : 0.5;
  // Order matters: strict `<` below keeps the earlier (horizontal) candidate on ties.
  const candidates = [
    ['left', fx],
    ['right', 1 - fx],
    ['top', fy],
    ['bottom', 1 - fy],
  ];
  let best = 'center';
  let bestDist = EDGE;
  for (const [name, dist] of candidates) {
    if (dist < bestDist) {
      best = name;
      bestDist = dist;
    }
  }
  return best;
}
