// SPDX-License-Identifier: Apache-2.0

//! Binary space partitioning: every split divides a rectangle in two, and every leaf is
//! one window. The tree only records structure; rectangles are derived from the area
//! on demand, so changing the output size never changes the structure.
//!
//! Every window of the monitor has a leaf. A floating window's leaf is a placeholder:
//! it takes no space (its sibling gets it) but keeps the window's place, so tiling it
//! again restores the tree exactly.

use super::{InsertHint, Layout, Mode, Rect, WindowId};

/// The smallest a tile may be made by resizing, per axis.
pub const MIN_TILE: i32 = 64;

/// A split's share for its first child, as an exact fraction `num / den` with
/// `0 <= num <= den`. Kept reduced, so equal ratios compare equal. The first child gets
/// floor(length * num / den) pixels; nothing drifts however often it is applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ratio {
    num: u32,
    den: u32,
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

impl Ratio {
    pub const HALF: Ratio = Ratio { num: 1, den: 2 };

    /// `num / den`, reduced; `den` must be positive and `num <= den`.
    pub fn new(num: u32, den: u32) -> Self {
        debug_assert!(den > 0 && num <= den, "invalid ratio {num}/{den}");
        let g = gcd(num, den).max(1);
        Self {
            num: num / g,
            den: den / g,
        }
    }

    #[cfg(any(test, feature = "debug-record"))]
    pub fn num(self) -> u32 {
        self.num
    }

    #[cfg(any(test, feature = "debug-record"))]
    pub fn den(self) -> u32 {
        self.den
    }

    /// The first child's share of `len` pixels.
    pub fn first_of(self, len: i32) -> i32 {
        (len as i64 * self.num as i64 / self.den as i64) as i32
    }

    /// The smallest length whose first part is at least `first` and whose second part
    /// is at least `second` pixels, with this ratio. None if no length works (a share
    /// of zero that must hold something).
    fn min_length_for(self, first: i32, second: i32) -> Option<i32> {
        let (num, den) = (self.num as i64, self.den as i64);
        let (first, second) = (first.max(0) as i64, second.max(0) as i64);
        // floor(L * num / den) >= first  <=>  L >= ceil(first * den / num)
        let for_first = match (first, num) {
            (0, _) => 0,
            (_, 0) => return None,
            _ => (first * den + num - 1) / num,
        };
        // L - floor(L * num / den) >= second  <=>  L > (second - 1) * den / (den - num)
        let for_second = match (second, den - num) {
            (0, _) => 0,
            (_, 0) => return None,
            (s, rest) => (s - 1) * den / rest + 1,
        };
        i32::try_from(for_first.max(for_second)).ok()
    }
}

/// An edge of a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    fn orientation(self) -> Orientation {
        match self {
            Side::Left | Side::Right => Orientation::SideBySide,
            Side::Top | Side::Bottom => Orientation::Stacked,
        }
    }

    /// Whether the window sits in the split's first child when this edge is the split.
    fn in_first(self) -> bool {
        matches!(self, Side::Right | Side::Bottom)
    }
}

/// A split boundary being moved: where the split is (the branch taken at each level,
/// true = second child) and which way it divides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitHandle {
    path: Vec<bool>,
    orientation: Orientation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Orientation {
    /// Children are left | right.
    SideBySide,
    /// Children are top / bottom.
    Stacked,
}

#[derive(Clone, Debug, PartialEq)]
struct Leaf {
    window: WindowId,
    mode: Mode,
}

#[derive(Clone, Debug, PartialEq)]
struct Split {
    orientation: Orientation,
    ratio: Ratio,
    first: Node,
    second: Node,
}

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Leaf(Leaf),
    Split(Box<Split>),
}

impl Node {
    fn leaf(window: WindowId, mode: Mode) -> Self {
        Node::Leaf(Leaf { window, mode })
    }

    fn at_path_mut(&mut self, path: &[bool]) -> Option<&mut Node> {
        let Some((&second, rest)) = path.split_first() else {
            return Some(self);
        };
        match self {
            Node::Leaf(_) => None,
            Node::Split(split) => {
                let child = if second {
                    &mut split.second
                } else {
                    &mut split.first
                };
                child.at_path_mut(rest)
            }
        }
    }

    fn find_mut(&mut self, window: WindowId) -> Option<&mut Node> {
        if matches!(self, Node::Leaf(leaf) if leaf.window == window) {
            return Some(self);
        }
        match self {
            Node::Leaf(_) => None,
            Node::Split(split) => {
                let Split { first, second, .. } = &mut **split;
                match first.find_mut(window) {
                    Some(found) => Some(found),
                    None => second.find_mut(window),
                }
            }
        }
    }

    fn leaves<'a>(&'a self, out: &mut Vec<&'a Leaf>) {
        match self {
            Node::Leaf(leaf) => out.push(leaf),
            Node::Split(split) => {
                split.first.leaves(out);
                split.second.leaves(out);
            }
        }
    }

    fn leaves_mut(&mut self, f: &mut impl FnMut(&mut Leaf)) {
        match self {
            Node::Leaf(leaf) => f(leaf),
            Node::Split(split) => {
                split.first.leaves_mut(f);
                split.second.leaves_mut(f);
            }
        }
    }

    /// Whether the subtree takes space: it has a tiled leaf, or the leaf of `also`
    /// (treated as tiled, to ask where a placeholder would go).
    /// The smallest length along `orientation` this subtree can have with every tiled
    /// leaf at least MIN_TILE, given its current inner ratios. None if impossible.
    fn min_length(&self, orientation: Orientation) -> Option<i32> {
        match self {
            Node::Leaf(leaf) => Some(if leaf.mode == Mode::Tiled {
                MIN_TILE
            } else {
                0
            }),
            Node::Split(split) => {
                let (first, second) = (
                    split.first.min_length(orientation)?,
                    split.second.min_length(orientation)?,
                );
                let both = split.first.takes_space(None) && split.second.takes_space(None);
                if split.orientation == orientation && both {
                    split.ratio.min_length_for(first, second)
                } else {
                    Some(first.max(second))
                }
            }
        }
    }

    /// The path (branches, true = second) to `window`'s leaf.
    fn path_to(&self, window: WindowId) -> Option<Vec<bool>> {
        match self {
            Node::Leaf(leaf) => (leaf.window == window).then(Vec::new),
            Node::Split(split) => [(&split.first, false), (&split.second, true)]
                .into_iter()
                .find_map(|(child, branch)| {
                    child.path_to(window).map(|mut rest| {
                        rest.insert(0, branch);
                        rest
                    })
                }),
        }
    }

    fn takes_space(&self, also: Option<WindowId>) -> bool {
        match self {
            Node::Leaf(leaf) => leaf.mode == Mode::Tiled || Some(leaf.window) == also,
            Node::Split(split) => split.first.takes_space(also) || split.second.takes_space(also),
        }
    }

    /// Appends the rectangle of every leaf that takes space under this node, which
    /// covers `area`. A subtree that takes no space gives all of it to its sibling.
    fn place(&self, area: Rect, also: Option<WindowId>, out: &mut Vec<(WindowId, Rect)>) {
        match self {
            Node::Leaf(leaf) => {
                if leaf.mode == Mode::Tiled || Some(leaf.window) == also {
                    out.push((leaf.window, area));
                }
            }
            Node::Split(split) => match (
                split.first.takes_space(also),
                split.second.takes_space(also),
            ) {
                (true, true) => {
                    let (a, b) = divide(area, split.orientation, split.ratio);
                    split.first.place(a, also, out);
                    split.second.place(b, also, out);
                }
                (true, false) => split.first.place(area, also, out),
                (false, true) => split.second.place(area, also, out),
                (false, false) => {}
            },
        }
    }
}

/// Splits `area` in two. The first part gets the floor of `ratio` times the length,
/// the second part the remainder, so the two cover `area` exactly.
fn divide(area: Rect, orientation: Orientation, ratio: Ratio) -> (Rect, Rect) {
    match orientation {
        Orientation::SideBySide => {
            let first = ratio.first_of(area.w);
            (
                Rect::new(area.x, area.y, first, area.h),
                Rect::new(area.x + first, area.y, area.w - first, area.h),
            )
        }
        Orientation::Stacked => {
            let first = ratio.first_of(area.h);
            (
                Rect::new(area.x, area.y, area.w, first),
                Rect::new(area.x, area.y + first, area.w, area.h - first),
            )
        }
    }
}

/// Result of removing a leaf from a subtree.
enum Removal {
    /// The leaf is not in this subtree, returned unchanged.
    NotFound(Node),
    /// This node was the leaf itself; nothing remains of it.
    WasThisLeaf,
    /// The leaf was below; this is what replaces the subtree.
    Replaced(Node),
}

/// Removes `window`'s leaf. Its sibling takes the parent's place; the sibling's
/// windows are added to `sibling_windows` so the caller can pick the next focus.
fn remove_leaf(node: Node, window: WindowId, sibling_windows: &mut Vec<WindowId>) -> Removal {
    let split = match node {
        Node::Leaf(leaf) if leaf.window == window => return Removal::WasThisLeaf,
        Node::Leaf(leaf) => return Removal::NotFound(Node::Leaf(leaf)),
        Node::Split(split) => *split,
    };
    let Split {
        orientation,
        ratio,
        first,
        second,
    } = split;
    let rebuild = |first, second| {
        Node::Split(Box::new(Split {
            orientation,
            ratio,
            first,
            second,
        }))
    };
    let promote = |sibling: Node, out: &mut Vec<WindowId>| {
        let mut leaves = Vec::new();
        sibling.leaves(&mut leaves);
        out.extend(leaves.iter().map(|leaf| leaf.window));
        Removal::Replaced(sibling)
    };
    match remove_leaf(first, window, sibling_windows) {
        Removal::WasThisLeaf => promote(second, sibling_windows),
        Removal::Replaced(first) => Removal::Replaced(rebuild(first, second)),
        Removal::NotFound(first) => match remove_leaf(second, window, sibling_windows) {
            Removal::WasThisLeaf => promote(first, sibling_windows),
            Removal::Replaced(second) => Removal::Replaced(rebuild(first, second)),
            Removal::NotFound(second) => Removal::NotFound(rebuild(first, second)),
        },
    }
}

#[cfg(any(test, feature = "debug-record"))]
/// One node of the tree, for diagnostics (debug recording, layout tables): the tree in
/// preorder, first child before second, with each node's depth (the root is 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlineNode {
    Split {
        depth: usize,
        /// Children left | right (true) or top / bottom (false).
        side_by_side: bool,
        ratio: Ratio,
    },
    Leaf {
        depth: usize,
        window: WindowId,
        mode: Mode,
    },
}

#[cfg(any(test, feature = "debug-record"))]
fn outline_node(node: &Node, depth: usize, out: &mut Vec<OutlineNode>) {
    match node {
        Node::Leaf(leaf) => out.push(OutlineNode::Leaf {
            depth,
            window: leaf.window,
            mode: leaf.mode,
        }),
        Node::Split(split) => {
            out.push(OutlineNode::Split {
                depth,
                side_by_side: split.orientation == Orientation::SideBySide,
                ratio: split.ratio,
            });
            outline_node(&split.first, depth + 1, out);
            outline_node(&split.second, depth + 1, out);
        }
    }
}

/// The BSP tiling layout of one output.
#[derive(Clone, Debug, PartialEq)]
pub struct BspTree {
    area: Rect,
    root: Option<Node>,
}

impl BspTree {
    pub fn new(area: Rect) -> Self {
        Self { area, root: None }
    }

    /// All windows in the tree, in leaf order (first child before second).
    pub fn windows(&self) -> Vec<WindowId> {
        let mut leaves = Vec::new();
        if let Some(root) = &self.root {
            root.leaves(&mut leaves);
        }
        leaves.iter().map(|leaf| leaf.window).collect()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// The tree in preorder (see `OutlineNode`).
    #[cfg(any(test, feature = "debug-record"))]
    pub fn outline(&self) -> Vec<OutlineNode> {
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            outline_node(root, 0, &mut out);
        }
        out
    }

    pub fn rect_of(&self, window: WindowId) -> Option<Rect> {
        self.rects()
            .into_iter()
            .find(|(w, _)| *w == window)
            .map(|(_, rect)| rect)
    }

    /// The rectangle `window`'s leaf would get if it were tiled (for a placeholder).
    fn rect_if_tiled(&self, window: WindowId) -> Option<Rect> {
        let mut out = Vec::new();
        self.root.as_ref()?.place(self.area, Some(window), &mut out);
        out.into_iter()
            .find(|(w, _)| *w == window)
            .map(|(_, rect)| rect)
    }

    pub fn mode_of(&self, window: WindowId) -> Option<Mode> {
        let mut leaves = Vec::new();
        self.root.as_ref()?.leaves(&mut leaves);
        leaves
            .iter()
            .find(|leaf| leaf.window == window)
            .map(|leaf| leaf.mode)
    }

    /// Marks a leaf as tiled or as a floating placeholder. Structure does not change.
    pub fn set_mode(&mut self, window: WindowId, mode: Mode) -> bool {
        match self.root.as_mut().and_then(|root| root.find_mut(window)) {
            Some(Node::Leaf(leaf)) => {
                leaf.mode = mode;
                true
            }
            _ => false,
        }
    }

    /// Adds `window` with the given leaf mode by splitting `next_to`'s leaf (or the
    /// last leaf) along the longer side of the rectangle that leaf has, or would have
    /// if it were tiled. The new window is the second child.
    pub fn insert_with_mode(&mut self, window: WindowId, next_to: Option<WindowId>, mode: Mode) {
        debug_assert!(!self.contains(window), "{window:?} inserted twice");
        let new = Node::leaf(window, mode);
        let target = next_to
            .filter(|w| self.contains(*w))
            .or_else(|| self.windows().last().copied());
        let Some(target) = target else {
            self.root = Some(new);
            return;
        };
        let rect = self.rect_if_tiled(target).unwrap_or(self.area);
        let orientation = if rect.w >= rect.h {
            Orientation::SideBySide
        } else {
            Orientation::Stacked
        };
        let slot = self
            .root
            .as_mut()
            .and_then(|root| root.find_mut(target))
            .expect("the target is in the tree");
        let old = std::mem::replace(slot, new.clone());
        *slot = Node::Split(Box::new(Split {
            orientation,
            ratio: Ratio::HALF,
            first: old,
            second: new,
        }));
    }

    /// Removes `window`'s leaf; its sibling takes the parent's place. Returns the
    /// windows of that sibling subtree (empty if `window` was the only leaf), or None
    /// if `window` is not in the tree.
    pub fn remove_returning_sibling(&mut self, window: WindowId) -> Option<Vec<WindowId>> {
        let root = self.root.take()?;
        let mut sibling_windows = Vec::new();
        match remove_leaf(root, window, &mut sibling_windows) {
            Removal::NotFound(root) => {
                self.root = Some(root);
                None
            }
            Removal::WasThisLeaf => Some(sibling_windows),
            Removal::Replaced(root) => {
                self.root = Some(root);
                Some(sibling_windows)
            }
        }
    }

    /// Exchanges the windows of two leaves (with their modes); structure stays.
    pub fn swap(&mut self, a: WindowId, b: WindowId) -> bool {
        if a == b || !self.contains(a) || !self.contains(b) {
            return false;
        }
        if let Some(root) = &mut self.root {
            root.leaves_mut(&mut |leaf| {
                if leaf.window == a {
                    leaf.window = b;
                } else if leaf.window == b {
                    leaf.window = a;
                }
            });
        }
        true
    }
}

impl BspTree {
    /// The nodes along `path` with the area each one covers (the same division as
    /// `rects`: a child that takes no space gives its sibling everything).
    fn walk(&self, path: &[bool]) -> Vec<(&Node, Rect)> {
        let mut out = Vec::new();
        let (Some(mut node), mut area) = (self.root.as_ref(), self.area) else {
            return out;
        };
        out.push((node, area));
        for &second in path {
            let Node::Split(split) = node else {
                break;
            };
            let (a, b) = match (
                split.first.takes_space(None),
                split.second.takes_space(None),
            ) {
                (true, true) => divide(area, split.orientation, split.ratio),
                _ => (area, area),
            };
            (node, area) = if second {
                (&split.second, b)
            } else {
                (&split.first, a)
            };
            out.push((node, area));
        }
        out
    }

    /// The split whose boundary is `window`'s `side` edge, and the current length of
    /// that split's first part. None if the window is not tiled or the edge lies on the
    /// output boundary.
    pub fn edge_split(&self, window: WindowId, side: Side) -> Option<(SplitHandle, i32)> {
        if self.mode_of(window) != Some(Mode::Tiled) {
            return None;
        }
        let path = self.root.as_ref()?.path_to(window)?;
        let nodes = self.walk(&path);
        // The nearest ancestor that divides along this axis with the window on the
        // matching side and space on both sides.
        (0..path.len()).rev().find_map(|depth| {
            let (Node::Split(split), area) = nodes[depth] else {
                return None;
            };
            let visible = split.first.takes_space(None) && split.second.takes_space(None);
            let fits = split.orientation == side.orientation() && path[depth] != side.in_first();
            (visible && fits).then(|| {
                let (first, _) = divide(area, split.orientation, split.ratio);
                let length = match split.orientation {
                    Orientation::SideBySide => first.w,
                    Orientation::Stacked => first.h,
                };
                let handle = SplitHandle {
                    path: path[..depth].to_vec(),
                    orientation: split.orientation,
                };
                (handle, length)
            })
        })
    }

    /// Moves a split boundary so its first part is `first` pixels long, clamped so that
    /// no tile on either side gets smaller than MIN_TILE. If the split is too small for
    /// that, nothing changes (the tiny-output rule). Returns whether the ratio changed.
    pub fn set_split_first(&mut self, handle: &SplitHandle, first: i32) -> bool {
        let nodes = self.walk(&handle.path);
        let Some(&(Node::Split(split), area)) = nodes.get(handle.path.len()) else {
            return false;
        };
        if split.orientation != handle.orientation {
            return false;
        }
        let length = match split.orientation {
            Orientation::SideBySide => area.w,
            Orientation::Stacked => area.h,
        };
        let (Some(low), Some(second_min)) = (
            split.first.min_length(split.orientation),
            split.second.min_length(split.orientation),
        ) else {
            return false;
        };
        let high = length - second_min;
        if length <= 0 || low > high {
            return false;
        }
        let ratio = Ratio::new(first.clamp(low, high) as u32, length as u32);
        let Some(Node::Split(split)) = self
            .root
            .as_mut()
            .and_then(|root| root.at_path_mut(&handle.path))
        else {
            return false;
        };
        let changed = split.ratio.first_of(length) != ratio.first_of(length);
        split.ratio = ratio;
        changed
    }

    /// Moves `window`'s `side` edge by `delta` pixels (keyboard resize). Returns whether
    /// anything changed.
    pub fn resize_edge(&mut self, window: WindowId, side: Side, delta: i32) -> bool {
        let Some((handle, first)) = self.edge_split(window, side) else {
            return false;
        };
        self.set_split_first(&handle, first + delta)
    }
}

impl Layout for BspTree {
    fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    fn insert(&mut self, window: WindowId, hint: &InsertHint) {
        self.insert_with_mode(window, hint.next_to, Mode::Tiled);
    }

    fn remove(&mut self, window: WindowId) -> bool {
        self.remove_returning_sibling(window).is_some()
    }

    fn contains(&self, window: WindowId) -> bool {
        self.windows().contains(&window)
    }

    fn rects(&self) -> Vec<(WindowId, Rect)> {
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            root.place(self.area, None, &mut out);
        }
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0, 0, 800, 600);

    fn id(n: u64) -> WindowId {
        WindowId(n)
    }

    fn rect_of(tree: &BspTree, window: u64) -> Rect {
        tree.rect_of(id(window)).expect("window has a rectangle")
    }

    /// Structural invariants plus exact coverage of the area by the tiled leaves.
    pub(crate) fn check_invariants(tree: &BspTree) {
        let windows = tree.windows();
        let mut unique = windows.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            windows.len(),
            "a window is in more than one leaf: {windows:?}"
        );

        let rects = tree.rects();
        let tiled = windows
            .iter()
            .filter(|w| tree.mode_of(**w) == Some(Mode::Tiled))
            .count();
        assert_eq!(
            rects.len(),
            tiled,
            "every tiled window gets exactly one rectangle"
        );
        for (w, r) in &rects {
            assert!(r.w >= 0 && r.h >= 0, "{w:?} has a negative size: {r:?}");
            let inside = r.x >= tree.area.x
                && r.y >= tree.area.y
                && r.right() <= tree.area.right()
                && r.bottom() <= tree.area.bottom();
            assert!(inside, "{w:?} at {r:?} is outside {:?}", tree.area);
        }
        for (i, (wa, a)) in rects.iter().enumerate() {
            for (wb, b) in &rects[i + 1..] {
                assert!(!a.overlaps(b), "{wa:?} {a:?} overlaps {wb:?} {b:?}");
            }
        }
        if !rects.is_empty() {
            let covered: i64 = rects.iter().map(|(_, r)| r.area()).sum();
            assert_eq!(
                covered,
                tree.area.area(),
                "rectangles do not cover the area exactly"
            );
        }
    }

    /// Opens windows 1..=n, each next to the previous one (as if each new window had
    /// focus).
    fn tree_with(n: u64) -> BspTree {
        let mut tree = BspTree::new(AREA);
        for i in 1..=n {
            tree.insert_with_mode(id(i), (i > 1).then(|| id(i - 1)), Mode::Tiled);
        }
        tree
    }

    #[test]
    fn first_window_fills_the_area() {
        let tree = tree_with(1);
        assert_eq!(tree.rects(), vec![(id(1), AREA)]);
        check_invariants(&tree);
    }

    #[test]
    fn new_window_splits_target_leaf_along_longer_side() {
        let mut tree = tree_with(2);
        assert_eq!(rect_of(&tree, 1), Rect::new(0, 0, 400, 600));
        assert_eq!(rect_of(&tree, 2), Rect::new(400, 0, 400, 600));
        tree.insert_with_mode(id(3), Some(id(2)), Mode::Tiled);
        assert_eq!(rect_of(&tree, 2), Rect::new(400, 0, 400, 300));
        assert_eq!(rect_of(&tree, 3), Rect::new(400, 300, 400, 300));
        check_invariants(&tree);
    }

    #[test]
    fn square_leaf_splits_side_by_side() {
        let mut tree = BspTree::new(Rect::new(0, 0, 500, 500));
        tree.insert_with_mode(id(1), None, Mode::Tiled);
        tree.insert_with_mode(id(2), Some(id(1)), Mode::Tiled);
        assert_eq!(rect_of(&tree, 1), Rect::new(0, 0, 250, 500));
        assert_eq!(rect_of(&tree, 2), Rect::new(250, 0, 250, 500));
    }

    #[test]
    fn odd_length_gives_the_remainder_to_the_second_child() {
        let mut tree = BspTree::new(Rect::new(0, 0, 801, 601));
        tree.insert_with_mode(id(1), None, Mode::Tiled);
        tree.insert_with_mode(id(2), Some(id(1)), Mode::Tiled);
        assert_eq!(rect_of(&tree, 1), Rect::new(0, 0, 400, 601));
        assert_eq!(rect_of(&tree, 2), Rect::new(400, 0, 401, 601));
        tree.insert_with_mode(id(3), Some(id(2)), Mode::Tiled);
        assert_eq!(rect_of(&tree, 2), Rect::new(400, 0, 401, 300));
        assert_eq!(rect_of(&tree, 3), Rect::new(400, 300, 401, 301));
        check_invariants(&tree);
    }

    #[test]
    fn area_offset_is_respected() {
        let mut tree = BspTree::new(Rect::new(1920, 100, 1000, 500));
        tree.insert_with_mode(id(1), None, Mode::Tiled);
        tree.insert_with_mode(id(2), Some(id(1)), Mode::Tiled);
        assert_eq!(rect_of(&tree, 1), Rect::new(1920, 100, 500, 500));
        assert_eq!(rect_of(&tree, 2), Rect::new(2420, 100, 500, 500));
        check_invariants(&tree);
    }

    #[test]
    fn very_small_area_still_covers_exactly() {
        for (w, h) in [(1, 1), (2, 1), (1, 3), (3, 2)] {
            let mut tree = BspTree::new(Rect::new(0, 0, w, h));
            for i in 1..=6 {
                tree.insert_with_mode(id(i), (i > 1).then(|| id(i - 1)), Mode::Tiled);
                check_invariants(&tree);
            }
            for i in [3, 1, 6] {
                assert!(tree.remove(id(i)));
                check_invariants(&tree);
            }
        }
    }

    #[test]
    fn remove_promotes_the_sibling_and_reports_its_windows() {
        let mut tree = tree_with(3); // 1 | (2 / 3)
        assert_eq!(
            tree.remove_returning_sibling(id(1)),
            Some(vec![id(2), id(3)])
        );
        assert_eq!(rect_of(&tree, 2), Rect::new(0, 0, 800, 300));
        assert_eq!(rect_of(&tree, 3), Rect::new(0, 300, 800, 300));
        assert_eq!(tree.remove_returning_sibling(id(3)), Some(vec![id(2)]));
        assert_eq!(tree.remove_returning_sibling(id(2)), Some(vec![]));
        assert!(tree.is_empty());
        assert_eq!(tree.remove_returning_sibling(id(2)), None);
        check_invariants(&tree);
    }

    #[test]
    fn closing_middle_window_refills() {
        let mut tree = tree_with(3); // 1 | (2 / 3)
        assert!(tree.remove(id(2)));
        assert_eq!(rect_of(&tree, 1), Rect::new(0, 0, 400, 600));
        assert_eq!(rect_of(&tree, 3), Rect::new(400, 0, 400, 600));
        check_invariants(&tree);
    }

    #[test]
    fn swap_exchanges_windows_and_keeps_structure() {
        let mut tree = tree_with(3); // 1 | (2 / 3)
        let before = tree.root.clone();
        assert!(tree.swap(id(1), id(3)));
        assert_eq!(rect_of(&tree, 3), Rect::new(0, 0, 400, 600));
        assert_eq!(rect_of(&tree, 1), Rect::new(400, 300, 400, 300));
        assert!(tree.swap(id(1), id(3)));
        assert_eq!(tree.root, before);
        assert!(!tree.swap(id(1), id(1)));
        assert!(!tree.swap(id(1), id(99)));
        check_invariants(&tree);
    }

    #[test]
    fn resizing_the_area_keeps_structure_and_recomputes_rectangles() {
        let mut tree = tree_with(3);
        let before = tree.root.clone();
        tree.set_area(Rect::new(0, 0, 1000, 1001));
        assert_eq!(tree.root, before);
        assert_eq!(rect_of(&tree, 1), Rect::new(0, 0, 500, 1001));
        assert_eq!(rect_of(&tree, 2), Rect::new(500, 0, 500, 500));
        assert_eq!(rect_of(&tree, 3), Rect::new(500, 500, 500, 501));
        check_invariants(&tree);
    }

    #[test]
    fn placeholder_takes_no_space_and_restores_exactly() {
        let mut tree = tree_with(3); // 1 | (2 / 3)
        let before = tree.clone();
        assert!(tree.set_mode(id(2), Mode::Floating));
        assert_eq!(
            tree.rects(),
            vec![
                (id(1), Rect::new(0, 0, 400, 600)),
                (id(3), Rect::new(400, 0, 400, 600))
            ]
        );
        check_invariants(&tree);
        assert!(tree.set_mode(id(2), Mode::Tiled));
        assert_eq!(tree, before);
    }

    #[test]
    fn splitting_a_placeholder_uses_the_rectangle_it_would_have() {
        // 1 | 2 with 2 floating: 1 fills the area. A window opened next to 2 splits 2's
        // would-be rectangle (400x600, taller than wide): stacked.
        let mut tree = tree_with(2);
        tree.set_mode(id(2), Mode::Floating);
        tree.insert_with_mode(id(3), Some(id(2)), Mode::Floating);
        assert_eq!(
            tree.rects(),
            vec![(id(1), AREA)],
            "placeholders move nothing"
        );
        tree.set_mode(id(2), Mode::Tiled);
        tree.set_mode(id(3), Mode::Tiled);
        assert_eq!(rect_of(&tree, 2), Rect::new(400, 0, 400, 300));
        assert_eq!(rect_of(&tree, 3), Rect::new(400, 300, 400, 300));
    }

    #[test]
    fn closing_a_placeholder_promotes_its_sibling() {
        let mut tree = tree_with(3); // 1 | (2 / 3)
        tree.set_mode(id(2), Mode::Floating);
        assert!(tree.remove(id(2)));
        assert_eq!(rect_of(&tree, 3), Rect::new(400, 0, 400, 600));
        assert_eq!(tree.windows(), vec![id(1), id(3)]);
        check_invariants(&tree);
    }

    fn widths(tree: &BspTree) -> Vec<(u64, i32, i32)> {
        tree.rects().iter().map(|(w, r)| (w.0, r.w, r.h)).collect()
    }

    #[test]
    fn ratios_are_exact_and_reduced() {
        assert_eq!(Ratio::new(2, 4), Ratio::HALF);
        assert_eq!(Ratio::new(440, 800), Ratio::new(11, 20));
        assert_eq!(Ratio::HALF.first_of(801), 400);
        assert_eq!(Ratio::new(1, 3).first_of(100), 33);
        // Applying the same ratio again never moves a boundary.
        let r = Ratio::new(333, 1000);
        assert_eq!(r.first_of(1000), 333);
        assert_eq!(r.first_of(1000), r.first_of(1000));
        // Minimum length with floors: first >= 64 and second >= 64 at 1/2 needs 128.
        assert_eq!(Ratio::HALF.min_length_for(64, 64), Some(128));
        assert_eq!(Ratio::new(1, 3).min_length_for(64, 64), Some(192));
        assert_eq!(Ratio::new(0, 1).min_length_for(64, 0), None);
    }

    #[test]
    fn resizing_an_edge_moves_its_split_and_keeps_coverage() {
        let mut tree = tree_with(2); // 1 | 2
        assert!(tree.resize_edge(id(1), Side::Right, 40));
        assert_eq!(rect_of(&tree, 1), Rect::new(0, 0, 440, 600));
        assert_eq!(rect_of(&tree, 2), Rect::new(440, 0, 360, 600));
        // The same boundary from the other side.
        assert!(tree.resize_edge(id(2), Side::Left, -40));
        assert_eq!(rect_of(&tree, 1), Rect::new(0, 0, 400, 600));
        check_invariants(&tree);
    }

    #[test]
    fn edges_on_the_output_boundary_do_nothing() {
        let mut tree = tree_with(2);
        let before = tree.clone();
        assert!(tree.edge_split(id(2), Side::Right).is_none());
        assert!(!tree.resize_edge(id(2), Side::Right, 40));
        assert!(!tree.resize_edge(id(1), Side::Left, -40));
        assert!(!tree.resize_edge(id(1), Side::Top, -40));
        assert!(!tree.resize_edge(id(1), Side::Bottom, 40));
        assert_eq!(tree, before);
    }

    #[test]
    fn resizing_clamps_at_the_minimum_tile_size() {
        let mut tree = tree_with(2);
        tree.resize_edge(id(1), Side::Right, -10_000);
        assert_eq!(rect_of(&tree, 1).w, MIN_TILE);
        tree.resize_edge(id(1), Side::Right, 10_000);
        assert_eq!(rect_of(&tree, 2).w, MIN_TILE);
        check_invariants(&tree);
    }

    #[test]
    fn nested_splits() {
        let mut tree = tree_with(3); // 1 | (2 / 3)
        assert!(tree.resize_edge(id(2), Side::Bottom, 50));
        assert_eq!(
            widths(&tree),
            vec![(1, 400, 600), (2, 400, 350), (3, 400, 250)]
        );
        // 2's left edge is the outer split.
        assert!(tree.resize_edge(id(2), Side::Left, -100));
        assert_eq!(rect_of(&tree, 1), Rect::new(0, 0, 300, 600));
        assert_eq!(rect_of(&tree, 2), Rect::new(300, 0, 500, 350));
        assert_eq!(rect_of(&tree, 3), Rect::new(300, 350, 500, 250));
        check_invariants(&tree);
    }

    #[test]
    fn minimum_respects_inner_splits() {
        // 1 | (2 | 3) on a wide output: shrinking the right half keeps both 2 and 3 at
        // least 64 wide.
        let mut tree = BspTree::new(Rect::new(0, 0, 1600, 600));
        tree.insert_with_mode(id(1), None, Mode::Tiled);
        tree.insert_with_mode(id(2), Some(id(1)), Mode::Tiled);
        tree.insert_with_mode(id(3), Some(id(2)), Mode::Tiled); // 800x600 splits side by side
        assert_eq!(rect_of(&tree, 3), Rect::new(1200, 0, 400, 600));
        tree.resize_edge(id(1), Side::Right, 10_000);
        assert_eq!(rect_of(&tree, 2).w, MIN_TILE);
        assert_eq!(rect_of(&tree, 3).w, MIN_TILE);
        assert_eq!(rect_of(&tree, 1).w, 1600 - 2 * MIN_TILE);
        check_invariants(&tree);
    }

    #[test]
    fn odd_sizes_keep_exact_coverage_through_many_resizes() {
        let mut tree = BspTree::new(Rect::new(0, 0, 801, 599));
        tree.insert_with_mode(id(1), None, Mode::Tiled);
        for i in 2..=5 {
            tree.insert_with_mode(id(i), Some(id(i - 1)), Mode::Tiled);
        }
        for step in 0..200 {
            let window = id(1 + step % 5);
            let side = [Side::Left, Side::Right, Side::Top, Side::Bottom][step as usize % 4];
            tree.resize_edge(window, side, [37, -53, 11, -7][step as usize % 4]);
            check_invariants(&tree);
        }
        // Resizing the area afterwards keeps the ratios and the coverage.
        let before = tree.root.clone();
        tree.set_area(Rect::new(0, 0, 1366, 767));
        assert_eq!(tree.root, before);
        check_invariants(&tree);
    }

    #[test]
    fn tiny_output_resize_does_nothing() {
        let mut tree = BspTree::new(Rect::new(0, 0, 100, 50));
        tree.insert_with_mode(id(1), None, Mode::Tiled);
        tree.insert_with_mode(id(2), Some(id(1)), Mode::Tiled);
        let before = tree.clone();
        assert!(
            !tree.resize_edge(id(1), Side::Right, 10),
            "100 px cannot hold two 64 px tiles"
        );
        assert_eq!(tree, before);
        check_invariants(&tree);
    }

    #[test]
    fn a_split_next_to_a_placeholder_is_not_a_boundary() {
        let mut tree = tree_with(3); // 1 | (2 / 3)
        tree.set_mode(id(3), Mode::Floating); // 2 fills the right half alone
        assert!(tree.edge_split(id(2), Side::Bottom).is_none());
        assert!(
            tree.edge_split(id(3), Side::Top).is_none(),
            "floating windows have no tiled edges"
        );
        assert!(tree.resize_edge(id(2), Side::Left, -50));
        assert_eq!(rect_of(&tree, 2), Rect::new(350, 0, 450, 600));
        check_invariants(&tree);
    }
}
