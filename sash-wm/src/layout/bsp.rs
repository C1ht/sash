// SPDX-License-Identifier: Apache-2.0

//! Binary space partitioning: every split divides a rectangle in two, and every leaf is
//! one window. The tree only records structure; rectangles are derived from the area
//! on demand, so changing the output size never changes the structure.
//!
//! Every window of the monitor has a leaf. A floating window's leaf is a placeholder:
//! it takes no space (its sibling gets it) but keeps the window's place, so tiling it
//! again restores the tree exactly.

use super::{InsertHint, Layout, Mode, Rect, WindowId};

/// Share of a split's length given to the first child.
const SPLIT_RATIO: f64 = 0.5;

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
    ratio: f64,
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
fn divide(area: Rect, orientation: Orientation, ratio: f64) -> (Rect, Rect) {
    match orientation {
        Orientation::SideBySide => {
            let first = (area.w as f64 * ratio).floor() as i32;
            (
                Rect::new(area.x, area.y, first, area.h),
                Rect::new(area.x + first, area.y, area.w - first, area.h),
            )
        }
        Orientation::Stacked => {
            let first = (area.h as f64 * ratio).floor() as i32;
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

    #[cfg(test)]
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
            ratio: SPLIT_RATIO,
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
}
