// Directional neighbour search over screen rectangles (panes or groups). Pure: no DOM access.

const EPS = 1; // borders/gutters may overlap by up to 1px and still count as "beyond"

/**
 * The rectangle next to `fromId` in direction `dir` ('left'|'right'|'up'|'down'), or null.
 * Candidates lie completely past the source edge (e.g. left: `r.right <= from.left + 1`).
 * Ranking: (1) overlapping the source on the perpendicular axis, (2) smallest gap between the
 * facing edges, (3) longest perpendicular overlap, (4) closest perpendicular centre.
 */
export function findNeighbor(rects, fromId, dir) {
  if (!Array.isArray(rects)) return null;
  const from = rects.find((r) => r.id === fromId);
  if (!from) return null;
  const horizontal = dir === 'left' || dir === 'right';
  if (!horizontal && dir !== 'up' && dir !== 'down') return null;

  let best = null;
  let bestKey = null;
  for (const r of rects) {
    if (r.id === fromId) continue;
    let gap;
    switch (dir) {
      case 'left':
        if (r.right > from.left + EPS) continue;
        gap = from.left - r.right;
        break;
      case 'right':
        if (r.left < from.right - EPS) continue;
        gap = r.left - from.right;
        break;
      case 'up':
        if (r.bottom > from.top + EPS) continue;
        gap = from.top - r.bottom;
        break;
      default:
        if (r.top < from.bottom - EPS) continue;
        gap = r.top - from.bottom;
    }
    const [a0, a1, b0, b1] = horizontal ? [from.top, from.bottom, r.top, r.bottom] : [from.left, from.right, r.left, r.right];
    const overlap = Math.min(a1, b1) - Math.max(a0, b0);
    const centre = Math.abs((a0 + a1) / 2 - (b0 + b1) / 2);
    const key = [overlap > 0 ? 0 : 1, Math.max(0, gap), -Math.max(0, overlap), centre];
    if (!bestKey || less(key, bestKey)) {
      best = r;
      bestKey = key;
    }
  }
  return best ? best.id : null;
}

function less(a, b) {
  for (let i = 0; i < a.length; i++) {
    if (Math.abs(a[i] - b[i]) > 1e-6) return a[i] < b[i];
  }
  return false;
}
