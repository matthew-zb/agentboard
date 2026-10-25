// LayoutNode → split DOM (flex) with border drag. Shared by the group layout and each tab's
// pane layout. Wire format (task 7): {"split":"horizontal"|"vertical","sizes":[..],"children":[..],
// "slot"?:".."} | {"leaf":id}. "horizontal" = children left | right.
// No top-level DOM access: the pure helpers are unit-tested in Node.

/** Smallest size (percent) a split child may be dragged to; mirrors layout.rs MIN_SIZE. */
export const MIN_SIZE = 5;
/** Gutter thickness in px (must match `.gutter` in style.css). */
export const GUTTER_PX = 4;

export function isLeaf(node) {
  return !!node && Object.prototype.hasOwnProperty.call(node, 'leaf');
}

/** Leaf ids in reading order. */
export function leafIds(node, out = []) {
  if (!node) return out;
  if (isLeaf(node)) out.push(node.leaf);
  else for (const c of node.children || []) leafIds(c, out);
  return out;
}

export function containsLeaf(node, id) {
  return id != null && leafIds(node).includes(id);
}

/** Same rules as layout.rs normalize_sizes: wrong length / non-positive → equal; scale to 100. */
export function normalizeSizes(sizes, n) {
  if (n <= 0) return [];
  const bad =
    !Array.isArray(sizes) ||
    sizes.length !== n ||
    sizes.some((s) => typeof s !== 'number' || !Number.isFinite(s) || s <= 0);
  if (bad) return Array(n).fill(100 / n);
  const sum = sizes.reduce((a, b) => a + b, 0);
  return sizes.map((s) => (s * 100) / sum);
}

/** Pixel movement → percent of the space shared by the split's children (gutters excluded). */
export function pxToPct(dx, totalPx) {
  return totalPx > 0 ? (dx / totalPx) * 100 : 0;
}

/**
 * Move the border between child `i` and `i + 1` by `deltaPct` (positive grows `i`), keeping
 * both at least `min`. Returns a new array; other sizes are untouched.
 */
export function dragSizes(sizes, i, deltaPct, min = MIN_SIZE) {
  const out = sizes.slice();
  if (i < 0 || i + 1 >= out.length || !Number.isFinite(deltaPct)) return out;
  const hi = Math.max(0, out[i + 1] - min);
  const lo = -Math.max(0, out[i] - min);
  const d = Math.min(hi, Math.max(lo, deltaPct));
  out[i] += d;
  out[i + 1] -= d;
  return out;
}

function div(cls) {
  const el = document.createElement('div');
  el.className = cls;
  return el;
}

/**
 * Render `node` as nested flex boxes. `renderLeaf(id)` returns the element for a leaf (it is
 * moved, not cloned, into the tree). When a border drag ends, `onSizes(path, sizes)` is called
 * with the child-index path of the split and its new sizes (percent, sum 100).
 * If `maximized` names a leaf of the tree, only that leaf is drawn.
 */
export function renderLayout(node, renderLeaf, onSizes, maximized = null) {
  if (maximized != null && containsLeaf(node, maximized)) {
    const cell = div('layout-cell layout-max');
    cell.appendChild(renderLeaf(maximized));
    return cell;
  }
  return build(node, [], renderLeaf, onSizes);
}

function build(node, path, renderLeaf, onSizes) {
  if (isLeaf(node)) {
    const cell = div('layout-cell');
    cell.appendChild(renderLeaf(node.leaf));
    return cell;
  }
  const horizontal = node.split === 'horizontal';
  const box = div(`split ${horizontal ? 'split-h' : 'split-v'}`);
  const children = node.children || [];
  const sizes = normalizeSizes(node.sizes, children.length);
  const cells = children.map((c, i) => {
    const cell = build(c, [...path, i], renderLeaf, onSizes);
    cell.style.flex = `${sizes[i]} 1 0px`;
    return cell;
  });
  const state = { sizes };
  cells.forEach((cell, i) => {
    if (i > 0) box.appendChild(gutter(box, cells, state, i - 1, horizontal, path, onSizes));
    box.appendChild(cell);
  });
  return box;
}

function gutter(box, cells, state, i, horizontal, path, onSizes) {
  const g = div(`gutter ${horizontal ? 'gutter-h' : 'gutter-v'}`);
  g.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    try {
      g.setPointerCapture(e.pointerId);
    } catch (_) {
      /* synthetic pointer: no capture */
    }
    const rect = box.getBoundingClientRect();
    const total = (horizontal ? rect.width : rect.height) - (cells.length - 1) * GUTTER_PX;
    const start = horizontal ? e.clientX : e.clientY;
    const base = state.sizes.slice();
    let cur = base;
    const dragCls = horizontal ? 'dragging-h' : 'dragging-v';
    document.body.classList.add(dragCls);
    g.classList.add('active');
    const move = (ev) => {
      const pos = horizontal ? ev.clientX : ev.clientY;
      cur = dragSizes(base, i, pxToPct(pos - start, total));
      cells[i].style.flexGrow = String(cur[i]);
      cells[i + 1].style.flexGrow = String(cur[i + 1]);
    };
    const up = () => {
      g.removeEventListener('pointermove', move);
      g.removeEventListener('pointerup', up);
      g.removeEventListener('pointercancel', up);
      document.body.classList.remove(dragCls);
      g.classList.remove('active');
      const changed = cur.some((s, k) => Math.abs(s - base[k]) > 0.01);
      if (changed) {
        state.sizes = cur;
        onSizes(path, cur);
      }
    };
    g.addEventListener('pointermove', move);
    g.addEventListener('pointerup', up);
    g.addEventListener('pointercancel', up);
  });
  return g;
}
