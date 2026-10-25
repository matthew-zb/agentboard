//! Generic split-tree used for both the group layout and each tab's pane layout.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

const MIN_SIZE: f32 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    /// Children laid out left | right.
    Horizontal,
    /// Children laid out top / bottom.
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LayoutNode<T> {
    Split { dir: Dir, sizes: Vec<f32>, children: Vec<LayoutNode<T>>, slot: Option<String> },
    Leaf(T),
}

// Wire format: {"split":"horizontal","sizes":[..],"children":[..],"slot"?:".."} | {"leaf":..}
#[derive(Serialize)]
#[serde(untagged)]
enum ReprRef<'a, T> {
    Split {
        split: Dir,
        sizes: &'a Vec<f32>,
        children: &'a Vec<LayoutNode<T>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        slot: &'a Option<String>,
    },
    Leaf {
        leaf: &'a T,
    },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Repr<T> {
    Split {
        split: Dir,
        sizes: Vec<f32>,
        children: Vec<LayoutNode<T>>,
        #[serde(default)]
        slot: Option<String>,
    },
    Leaf {
        leaf: T,
    },
}

impl<T: Serialize> Serialize for LayoutNode<T> {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        match self {
            LayoutNode::Split { dir, sizes, children, slot } => {
                ReprRef::Split { split: *dir, sizes, children, slot }.serialize(ser)
            }
            LayoutNode::Leaf(t) => ReprRef::Leaf { leaf: t }.serialize(ser),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for LayoutNode<T> {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        Ok(match Repr::<T>::deserialize(de)? {
            Repr::Split { split, sizes, children, slot } => LayoutNode::Split { dir: split, sizes, children, slot },
            Repr::Leaf { leaf } => LayoutNode::Leaf(leaf),
        })
    }
}

fn normalize_sizes(sizes: &mut Vec<f32>, n: usize) {
    let bad = sizes.len() != n || sizes.iter().any(|s| !s.is_finite() || *s <= 0.0);
    if bad || n == 0 {
        *sizes = if n == 0 { vec![] } else { vec![100.0 / n as f32; n] };
        return;
    }
    let sum: f32 = sizes.iter().sum();
    for s in sizes.iter_mut() {
        *s = *s * 100.0 / sum;
    }
}

impl<T: Clone + PartialEq> LayoutNode<T> {
    /// Leaves in reading order (left to right, top to bottom).
    pub fn leaves(&self) -> Vec<T> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<T>) {
        match self {
            LayoutNode::Leaf(t) => out.push(t.clone()),
            LayoutNode::Split { children, .. } => children.iter().for_each(|c| c.collect(out)),
        }
    }

    /// Visit every leaf mutably, in reading order.
    pub fn for_each_leaf_mut(&mut self, mut f: impl FnMut(&mut T)) {
        fn walk<T>(n: &mut LayoutNode<T>, f: &mut impl FnMut(&mut T)) {
            match n {
                LayoutNode::Leaf(t) => f(t),
                LayoutNode::Split { children, .. } => children.iter_mut().for_each(|c| walk(c, f)),
            }
        }
        walk(self, &mut f)
    }

    pub fn leaf_count(&self) -> usize {
        match self {
            LayoutNode::Leaf(_) => 1,
            LayoutNode::Split { children, .. } => children.iter().map(|c| c.leaf_count()).sum(),
        }
    }

    pub fn contains(&self, t: &T) -> bool {
        match self {
            LayoutNode::Leaf(x) => x == t,
            LayoutNode::Split { children, .. } => children.iter().any(|c| c.contains(t)),
        }
    }

    pub fn map<U>(&self, mut f: impl FnMut(&T) -> U) -> LayoutNode<U> {
        self.map_inner(&mut f)
    }

    fn map_inner<U>(&self, f: &mut impl FnMut(&T) -> U) -> LayoutNode<U> {
        match self {
            LayoutNode::Leaf(t) => LayoutNode::Leaf(f(t)),
            LayoutNode::Split { dir, sizes, children, slot } => LayoutNode::Split {
                dir: *dir,
                sizes: sizes.clone(),
                children: children.iter().map(|c| c.map_inner(f)).collect(),
                slot: slot.clone(),
            },
        }
    }

    /// Split the leaf `target`, adding `new` before/after it. A parent split of the same
    /// direction gets a sibling (sizes redistributed equally); otherwise the leaf becomes a
    /// two-way split.
    pub fn split_leaf(&mut self, target: &T, new: T, dir: Dir, before: bool) -> bool {
        match self {
            LayoutNode::Leaf(t) => {
                if t != target {
                    return false;
                }
                let old = LayoutNode::Leaf(t.clone());
                let nw = LayoutNode::Leaf(new);
                let children = if before { vec![nw, old] } else { vec![old, nw] };
                *self = LayoutNode::Split { dir, sizes: vec![50.0, 50.0], children, slot: None };
                true
            }
            LayoutNode::Split { dir: d, sizes, children, .. } => {
                if *d == dir {
                    if let Some(i) = children.iter().position(|c| matches!(c, LayoutNode::Leaf(x) if x == target)) {
                        children.insert(if before { i } else { i + 1 }, LayoutNode::Leaf(new));
                        *sizes = vec![100.0 / children.len() as f32; children.len()];
                        return true;
                    }
                }
                match children.iter_mut().find(|c| c.contains(target)) {
                    Some(c) => c.split_leaf(target, new, dir, before),
                    None => false,
                }
            }
        }
    }

    /// None = not found; Some(Some(t)) = removed, `t` is the focus candidate (first leaf of the
    /// adjacent sibling subtree); Some(None) = the tree became empty (root was the leaf).
    pub fn remove_leaf(&mut self, target: &T) -> Option<Option<T>> {
        match self {
            LayoutNode::Leaf(t) => (t == target).then_some(None),
            LayoutNode::Split { sizes, children, .. } => {
                let focus = if let Some(i) = children.iter().position(|c| matches!(c, LayoutNode::Leaf(x) if x == target)) {
                    children.remove(i);
                    if i < sizes.len() {
                        sizes.remove(i);
                    }
                    let adj = if i < children.len() { i } else { i.saturating_sub(1) };
                    children.get(adj).and_then(|c| c.leaves().into_iter().next())
                } else {
                    children.iter_mut().find(|c| c.contains(target))?.remove_leaf(target)?
                };
                self.normalize();
                Some(focus)
            }
        }
    }

    /// Collapse single-child splits, make `sizes` match `children` and sum to 100.
    pub fn normalize(&mut self) {
        if let LayoutNode::Split { sizes, children, .. } = self {
            children.iter_mut().for_each(|c| c.normalize());
            if children.len() == 1 {
                let only = children.pop().unwrap();
                *self = only;
                return;
            }
            normalize_sizes(sizes, children.len());
        }
    }

    /// Move the border on `side` of `target` by `delta` percent (target grows, the neighbour
    /// shrinks), using the deepest enclosing split whose axis matches and that has a neighbour
    /// on that side. Each side is clamped to at least 5%.
    pub fn resize(&mut self, target: &T, side: Side, delta: f32) -> bool {
        let LayoutNode::Split { dir, sizes, children, .. } = self else { return false };
        let Some(i) = children.iter().position(|c| c.contains(target)) else { return false };
        if children[i].resize(target, side, delta) {
            return true;
        }
        let axis_ok = matches!(
            (side, *dir),
            (Side::Left | Side::Right, Dir::Horizontal) | (Side::Up | Side::Down, Dir::Vertical)
        );
        if !axis_ok || sizes.len() != children.len() {
            return false;
        }
        let j = match side {
            Side::Left | Side::Up => i.checked_sub(1),
            Side::Right | Side::Down => (i + 1 < children.len()).then_some(i + 1),
        };
        let Some(j) = j else { return false };
        let d = delta.min(sizes[j] - MIN_SIZE).max(-(sizes[i] - MIN_SIZE));
        sizes[i] += d;
        sizes[j] -= d;
        true
    }

    /// Replace the sizes of the split at `path` (child indices from this node).
    pub fn set_sizes(&mut self, path: &[usize], new_sizes: Vec<f32>) -> bool {
        match (self, path.split_first()) {
            (LayoutNode::Split { children, .. }, Some((&i, rest))) => match children.get_mut(i) {
                Some(c) => c.set_sizes(rest, new_sizes),
                None => false,
            },
            (LayoutNode::Split { sizes, children, .. }, None) => {
                if new_sizes.len() != children.len() {
                    return false;
                }
                *sizes = new_sizes;
                normalize_sizes(sizes, children.len());
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use LayoutNode::{Leaf, Split};

    fn l(s: &str) -> LayoutNode<String> {
        Leaf(s.to_string())
    }
    fn sp(dir: Dir, sizes: &[f32], children: Vec<LayoutNode<String>>) -> LayoutNode<String> {
        Split { dir, sizes: sizes.to_vec(), children, slot: None }
    }
    fn s(x: &str) -> String {
        x.to_string()
    }
    fn near(a: &[f32], b: &[f32]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
    }
    fn sizes_of(n: &LayoutNode<String>) -> Vec<f32> {
        match n {
            Split { sizes, .. } => sizes.clone(),
            _ => panic!("not a split"),
        }
    }
    fn sample() -> LayoutNode<String> {
        // [a, [b, c]V]H with 50/50 at both levels
        sp(Dir::Horizontal, &[50.0, 50.0], vec![l("a"), sp(Dir::Vertical, &[50.0, 50.0], vec![l("b"), l("c")])])
    }

    #[test]
    fn split_leaf_into_new_split() {
        let mut t = l("a");
        assert!(t.split_leaf(&s("a"), s("b"), Dir::Horizontal, false));
        assert_eq!(t, sp(Dir::Horizontal, &[50.0, 50.0], vec![l("a"), l("b")]));
        assert!(!t.split_leaf(&s("zzz"), s("q"), Dir::Horizontal, false));
    }

    #[test]
    fn split_same_dir_inserts_sibling() {
        let mut t = sp(Dir::Horizontal, &[50.0, 50.0], vec![l("a"), l("b")]);
        assert!(t.split_leaf(&s("b"), s("c"), Dir::Horizontal, false));
        match &t {
            Split { children, sizes, .. } => {
                assert_eq!(children, &vec![l("a"), l("b"), l("c")]);
                assert!(near(sizes, &[100.0 / 3.0; 3]));
            }
            _ => panic!(),
        }
        // different direction nests instead
        assert!(t.split_leaf(&s("a"), s("d"), Dir::Vertical, false));
        assert_eq!(t.leaves(), vec![s("a"), s("d"), s("b"), s("c")]);
        assert_eq!(t.leaf_count(), 4);
    }

    #[test]
    fn split_before_places_new_first() {
        let mut t = l("a");
        assert!(t.split_leaf(&s("a"), s("b"), Dir::Horizontal, true));
        assert_eq!(t.leaves(), vec![s("b"), s("a")]);
    }

    #[test]
    fn remove_leaf_returns_adjacent_sibling() {
        let mut t = sample();
        assert_eq!(t.remove_leaf(&s("a")), Some(Some(s("b"))));
        assert_eq!(t, sp(Dir::Vertical, &[50.0, 50.0], vec![l("b"), l("c")]));
        assert_eq!(t.remove_leaf(&s("zzz")), None);
        // removing the last child picks the previous sibling
        assert_eq!(t.remove_leaf(&s("c")), Some(Some(s("b"))));
        assert_eq!(t, l("b"));
    }

    #[test]
    fn remove_last_leaf_reports_empty() {
        let mut t = l("a");
        assert_eq!(t.remove_leaf(&s("a")), Some(None));
    }

    #[test]
    fn normalize_fixes_bad_sizes() {
        let mut t = sp(Dir::Horizontal, &[0.0, -5.0, 300.0], vec![l("a"), l("b"), l("c")]);
        t.normalize();
        let z = sizes_of(&t);
        assert_eq!(z.len(), 3);
        assert!((z.iter().sum::<f32>() - 100.0).abs() < 1e-3);
        assert!(z.iter().all(|x| *x > 0.0));
        // wrong length -> equal
        let mut t = sp(Dir::Horizontal, &[100.0], vec![l("a"), l("b")]);
        t.normalize();
        assert!(near(&sizes_of(&t), &[50.0, 50.0]));
        // single-child split collapses
        let mut t = sp(Dir::Horizontal, &[100.0], vec![sp(Dir::Vertical, &[100.0], vec![l("a")])]);
        t.normalize();
        assert_eq!(t, l("a"));
    }

    #[test]
    fn resize_moves_nearest_matching_border() {
        let mut t = sample();
        assert!(t.resize(&s("b"), Side::Left, 10.0));
        assert!(near(&sizes_of(&t), &[40.0, 60.0]));
        assert!(!t.resize(&s("b"), Side::Up, 10.0));
        let mut t = sample();
        assert!(t.resize(&s("c"), Side::Up, 10.0));
        match &t {
            Split { children, .. } => assert!(near(&sizes_of(&children[1]), &[40.0, 60.0])),
            _ => panic!(),
        }
        assert!(near(&sizes_of(&t), &[50.0, 50.0]));
        assert!(!l("a").resize(&s("a"), Side::Left, 5.0));
    }

    #[test]
    fn resize_clamps_to_min_5() {
        let mut t = sp(Dir::Horizontal, &[50.0, 50.0], vec![l("a"), l("b")]);
        assert!(t.resize(&s("a"), Side::Right, 80.0));
        assert!(near(&sizes_of(&t), &[95.0, 5.0]));
        assert!(t.resize(&s("a"), Side::Right, 10.0));
        assert!(near(&sizes_of(&t), &[95.0, 5.0]));
        assert!(t.resize(&s("b"), Side::Left, 200.0));
        assert!(near(&sizes_of(&t), &[5.0, 95.0]));
    }

    #[test]
    fn leaves_order_is_reading_order() {
        let t = sp(
            Dir::Horizontal,
            &[50.0, 50.0],
            vec![sp(Dir::Vertical, &[50.0, 50.0], vec![l("a"), l("b")]), sp(Dir::Vertical, &[50.0, 50.0], vec![l("c"), l("d")])],
        );
        assert_eq!(t.leaves(), vec![s("a"), s("b"), s("c"), s("d")]);
        assert!(t.contains(&s("c")));
        assert!(!t.contains(&s("x")));
        let m = t.map(|x| x.to_uppercase());
        assert_eq!(m.leaves(), vec![s("A"), s("B"), s("C"), s("D")]);
    }

    #[test]
    fn set_sizes_by_path() {
        let mut t = sample();
        assert!(t.set_sizes(&[1], vec![30.0, 70.0]));
        assert!(!t.set_sizes(&[0], vec![30.0, 70.0]));
        assert!(!t.set_sizes(&[], vec![100.0]));
        assert!(!t.set_sizes(&[5], vec![50.0, 50.0]));
        assert!(t.set_sizes(&[], vec![25.0, 75.0]));
        assert!(near(&sizes_of(&t), &[25.0, 75.0]));
    }

    #[test]
    fn serde_roundtrip() {
        let mut t = sample();
        if let Split { slot, .. } = &mut t {
            *slot = Some("main".into());
        }
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["split"], "horizontal");
        assert_eq!(v["slot"], "main");
        assert_eq!(v["children"][0], serde_json::json!({"leaf": "a"}));
        assert!(v["children"][1].get("slot").is_none());
        let back: LayoutNode<String> = serde_json::from_value(v).unwrap();
        assert_eq!(back, t);
        let d: Dir = serde_json::from_str("\"vertical\"").unwrap();
        assert_eq!(d, Dir::Vertical);
        let sd: Side = serde_json::from_str("\"left\"").unwrap();
        assert_eq!(sd, Side::Left);
    }
}
