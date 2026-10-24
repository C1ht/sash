// SPDX-License-Identifier: Apache-2.0

//! Binary space partitioning: every split divides a rectangle in two, and every leaf is
//! one window. The tree only records structure; rectangles are derived from the area
//! on demand, so changing the output size never changes the structure.

use std::collections::HashMap;

use super::{Direction, Layout, Mode, Rect, WindowId};

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
    fn leaf(window: WindowId) -> Self {
        Node::Leaf(Leaf {
            window,
            mode: Mode::Tiled,
        })
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

    fn takes_space(&self) -> bool {
        match self {
            Node::Leaf(leaf) => leaf.mode == Mode::Tiled,
            Node::Split(split) => split.first.takes_space() || split.second.takes_space(),
        }
    }

    /// Appends the rectangle of every tiled leaf under this node, which covers `area`.
    /// A subtree without tiled leaves takes no space; its sibling gets all of it.
    fn place(&self, area: Rect, out: &mut Vec<(WindowId, Rect)>) {
        match self {
            Node::Leaf(leaf) => {
                if leaf.mode == Mode::Tiled {
                    out.push((leaf.window, area));
                }
            }
            Node::Split(split) => match (split.first.takes_space(), split.second.takes_space()) {
                (true, true) => {
                    let (a, b) = divide(area, split.orientation, split.ratio);
                    split.first.place(a, out);
                    split.second.place(b, out);
                }
                (true, false) => split.first.place(area, out),
                (false, true) => split.second.place(area, out),
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
#[derive(Clone, Debug)]
pub struct BspTree {
    area: Rect,
    root: Option<Node>,
    focused: Option<WindowId>,
    /// When each window was last focused; higher is more recent.
    focus_order: HashMap<WindowId, u64>,
    focus_clock: u64,
}

impl BspTree {
    pub fn new(area: Rect) -> Self {
        Self {
            area,
            root: None,
            focused: None,
            focus_order: HashMap::new(),
            focus_clock: 0,
        }
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

    fn rect_of(&self, window: WindowId) -> Option<Rect> {
        self.rects()
            .into_iter()
            .find(|(w, _)| *w == window)
            .map(|(_, rect)| rect)
    }

    fn last_focused_of(&self, windows: &[WindowId]) -> Option<WindowId> {
        windows
            .iter()
            .copied()
            .max_by_key(|w| self.focus_order.get(w).copied().unwrap_or(0))
    }

    #[cfg(test)]
    pub(crate) fn set_mode(&mut self, window: WindowId, mode: Mode) {
        if let Some(Node::Leaf(leaf)) = self.root.as_mut().and_then(|root| root.find_mut(window)) {
            leaf.mode = mode;
        }
    }
}

impl Layout for BspTree {
    fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    fn insert(&mut self, window: WindowId) {
        debug_assert!(!self.contains(window), "{window:?} inserted twice");
        let target = self
            .focused
            .or_else(|| self.windows().last().copied())
            .and_then(|focused| self.rect_of(focused).map(|rect| (focused, rect)));
        match (&mut self.root, target) {
            (Some(root), Some((target, rect))) => {
                let orientation = if rect.w >= rect.h {
                    Orientation::SideBySide
                } else {
                    Orientation::Stacked
                };
                let slot = root
                    .find_mut(target)
                    .expect("the focused window is in the tree");
                let old = std::mem::replace(slot, Node::leaf(window));
                *slot = Node::Split(Box::new(Split {
                    orientation,
                    ratio: SPLIT_RATIO,
                    first: old,
                    second: Node::leaf(window),
                }));
            }
            (Some(_), None) => {
                unreachable!("a non-empty tree always has a window with a rectangle")
            }
            (None, _) => self.root = Some(Node::leaf(window)),
        }
        self.focus(window);
    }

    fn remove(&mut self, window: WindowId) -> bool {
        let Some(root) = self.root.take() else {
            return false;
        };
        let mut sibling_windows = Vec::new();
        let found = match remove_leaf(root, window, &mut sibling_windows) {
            Removal::NotFound(root) => {
                self.root = Some(root);
                false
            }
            Removal::WasThisLeaf => true,
            Removal::Replaced(root) => {
                self.root = Some(root);
                true
            }
        };
        if found {
            self.focus_order.remove(&window);
            if self.focused == Some(window) {
                self.focused = None;
                if let Some(next) = self.last_focused_of(&sibling_windows) {
                    self.focus(next);
                }
            }
        }
        found
    }

    fn contains(&self, window: WindowId) -> bool {
        self.windows().contains(&window)
    }

    fn focused(&self) -> Option<WindowId> {
        self.focused
    }

    fn focus(&mut self, window: WindowId) -> bool {
        if !self.contains(window) {
            return false;
        }
        self.focus_clock += 1;
        self.focus_order.insert(window, self.focus_clock);
        self.focused = Some(window);
        true
    }

    fn neighbour(&self, window: WindowId, direction: Direction) -> Option<WindowId> {
        let rects = self.rects();
        let (_, from) = *rects.iter().find(|(w, _)| *w == window)?;
        let spans_overlap = |a0: i32, a1: i32, b0: i32, b1: i32| a0 < b1 && b0 < a1;
        rects
            .iter()
            .filter(|(w, _)| *w != window)
            .filter_map(|&(w, to)| {
                // Distance from `from`'s edge to `to`'s facing edge, if `to` lies in
                // `direction` and shares part of `from`'s span.
                let (gap, overlap) = match direction {
                    Direction::Right => (
                        to.x - from.right(),
                        spans_overlap(from.y, from.bottom(), to.y, to.bottom()),
                    ),
                    Direction::Left => (
                        from.x - to.right(),
                        spans_overlap(from.y, from.bottom(), to.y, to.bottom()),
                    ),
                    Direction::Down => (
                        to.y - from.bottom(),
                        spans_overlap(from.x, from.right(), to.x, to.right()),
                    ),
                    Direction::Up => (
                        from.y - to.bottom(),
                        spans_overlap(from.x, from.right(), to.x, to.right()),
                    ),
                };
                (gap >= 0 && overlap).then_some((w, to, gap))
            })
            .min_by_key(|&(w, to, gap)| {
                let recency = self.focus_order.get(&w).copied().unwrap_or(0);
                // Final tie-break by position: topmost for left/right, leftmost for up/down.
                let position = match direction {
                    Direction::Left | Direction::Right => (to.y, to.x),
                    Direction::Up | Direction::Down => (to.x, to.y),
                };
                (gap, std::cmp::Reverse(recency), position)
            })
            .map(|(w, _, _)| w)
    }

    fn swap(&mut self, a: WindowId, b: WindowId) -> bool {
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

    fn rects(&self) -> Vec<(WindowId, Rect)> {
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            root.place(self.area, &mut out);
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

    /// The invariants every tree must keep after any operation.
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
        assert_eq!(
            rects.len(),
            windows.len(),
            "every (tiled) window gets exactly one rectangle"
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

        match tree.focused() {
            Some(focused) => assert!(
                windows.contains(&focused),
                "focus on a window not in the tree"
            ),
            None => assert!(windows.is_empty(), "windows exist but none is focused"),
        }
    }

    fn tree_with(n: u64) -> BspTree {
        let mut tree = BspTree::new(AREA);
        for i in 1..=n {
            tree.insert(id(i));
        }
        tree
    }

    #[test]
    fn first_window_fills_the_area() {
        let tree = tree_with(1);
        assert_eq!(tree.rects(), vec![(id(1), AREA)]);
        assert_eq!(tree.focused(), Some(id(1)));
        check_invariants(&tree);
    }

    #[test]
    fn new_window_splits_focused_leaf_along_longer_side() {
        let mut tree = tree_with(2);
        // 800x600 is wider than tall: side by side, new window second and focused.
        assert_eq!(rect_of(&tree, 1), Rect::new(0, 0, 400, 600));
        assert_eq!(rect_of(&tree, 2), Rect::new(400, 0, 400, 600));
        assert_eq!(tree.focused(), Some(id(2)));
        // 400x600 is taller than wide: stacked.
        tree.insert(id(3));
        assert_eq!(rect_of(&tree, 2), Rect::new(400, 0, 400, 300));
        assert_eq!(rect_of(&tree, 3), Rect::new(400, 300, 400, 300));
        assert_eq!(tree.focused(), Some(id(3)));
        check_invariants(&tree);
    }

    #[test]
    fn square_leaf_splits_side_by_side() {
        let mut tree = BspTree::new(Rect::new(0, 0, 500, 500));
        tree.insert(id(1));
        tree.insert(id(2));
        assert_eq!(rect_of(&tree, 1), Rect::new(0, 0, 250, 500));
        assert_eq!(rect_of(&tree, 2), Rect::new(250, 0, 250, 500));
    }

    #[test]
    fn odd_length_gives_the_remainder_to_the_second_child() {
        let mut tree = BspTree::new(Rect::new(0, 0, 801, 601));
        tree.insert(id(1));
        tree.insert(id(2));
        assert_eq!(rect_of(&tree, 1), Rect::new(0, 0, 400, 601));
        assert_eq!(rect_of(&tree, 2), Rect::new(400, 0, 401, 601));
        tree.insert(id(3));
        assert_eq!(rect_of(&tree, 2), Rect::new(400, 0, 401, 300));
        assert_eq!(rect_of(&tree, 3), Rect::new(400, 300, 401, 301));
        check_invariants(&tree);
    }

    #[test]
    fn area_offset_is_respected() {
        let mut tree = BspTree::new(Rect::new(1920, 100, 1000, 500));
        tree.insert(id(1));
        tree.insert(id(2));
        assert_eq!(rect_of(&tree, 1), Rect::new(1920, 100, 500, 500));
        assert_eq!(rect_of(&tree, 2), Rect::new(2420, 100, 500, 500));
        check_invariants(&tree);
    }

    #[test]
    fn very_small_area_still_covers_exactly() {
        for (w, h) in [(1, 1), (2, 1), (1, 3), (3, 2)] {
            let mut tree = BspTree::new(Rect::new(0, 0, w, h));
            for i in 1..=6 {
                tree.insert(id(i));
                check_invariants(&tree);
            }
            for i in [3, 1, 6] {
                assert!(tree.remove(id(i)));
                check_invariants(&tree);
            }
        }
    }

    #[test]
    fn close_gives_parent_space_to_sibling_and_focus_to_its_last_focused_window() {
        let mut tree = tree_with(3); // 1 | (2 / 3), focus 3
        tree.focus(id(2));
        tree.focus(id(1));
        assert!(tree.remove(id(1)));
        // The sibling subtree (2 / 3) takes the whole area; 2 was focused more recently.
        assert_eq!(rect_of(&tree, 2), Rect::new(0, 0, 800, 300));
        assert_eq!(rect_of(&tree, 3), Rect::new(0, 300, 800, 300));
        assert_eq!(tree.focused(), Some(id(2)));
        check_invariants(&tree);
    }

    #[test]
    fn closing_middle_window_refills() {
        let mut tree = tree_with(3); // 1 | (2 / 3)
        assert!(tree.remove(id(2)));
        assert_eq!(rect_of(&tree, 1), Rect::new(0, 0, 400, 600));
        assert_eq!(rect_of(&tree, 3), Rect::new(400, 0, 400, 600));
        assert_eq!(
            tree.focused(),
            Some(id(3)),
            "focus was not on the closed window"
        );
        check_invariants(&tree);
    }

    #[test]
    fn closing_unfocused_window_keeps_focus() {
        let mut tree = tree_with(3);
        tree.focus(id(1));
        assert!(tree.remove(id(3)));
        assert_eq!(tree.focused(), Some(id(1)));
        check_invariants(&tree);
    }

    #[test]
    fn closing_the_last_window_empties_the_tree() {
        let mut tree = tree_with(1);
        assert!(tree.remove(id(1)));
        assert!(tree.is_empty());
        assert_eq!(tree.focused(), None);
        assert!(tree.rects().is_empty());
        check_invariants(&tree);
        tree.insert(id(2));
        assert_eq!(tree.rects(), vec![(id(2), AREA)]);
    }

    #[test]
    fn removing_unknown_window_changes_nothing() {
        let mut tree = tree_with(2);
        let before = tree.rects();
        assert!(!tree.remove(id(99)));
        assert_eq!(tree.rects(), before);
        assert!(!BspTree::new(AREA).remove(id(1)));
    }

    #[test]
    fn swap_exchanges_windows_and_keeps_structure() {
        let mut tree = tree_with(3); // 1 | (2 / 3)
        let before = tree.root.clone();
        assert!(tree.swap(id(1), id(3)));
        assert_eq!(rect_of(&tree, 3), Rect::new(0, 0, 400, 600));
        assert_eq!(rect_of(&tree, 1), Rect::new(400, 300, 400, 300));
        assert_eq!(tree.focused(), Some(id(3)), "swap does not change focus");
        // Same shape: swapping back restores the identical tree.
        assert!(tree.swap(id(1), id(3)));
        assert_eq!(tree.root, before);
        assert!(!tree.swap(id(1), id(1)));
        assert!(!tree.swap(id(1), id(99)));
        check_invariants(&tree);
    }

    #[test]
    fn focus_moves_by_direction_without_wrapping() {
        let tree = tree_with(3); // 1 | (2 / 3)
        assert_eq!(tree.neighbour(id(3), Direction::Up), Some(id(2)));
        assert_eq!(tree.neighbour(id(2), Direction::Down), Some(id(3)));
        assert_eq!(tree.neighbour(id(3), Direction::Left), Some(id(1)));
        assert_eq!(
            tree.neighbour(id(3), Direction::Right),
            None,
            "no wrap-around"
        );
        assert_eq!(tree.neighbour(id(1), Direction::Left), None);
        assert_eq!(tree.neighbour(id(2), Direction::Up), None);
    }

    #[test]
    fn focus_move_tie_goes_to_most_recently_focused_then_topmost() {
        let mut tree = tree_with(3); // 1 | (2 / 3): 2 and 3 both touch 1's right edge
        tree.focus(id(1));
        // 3 was focused more recently than 2.
        assert_eq!(tree.neighbour(id(1), Direction::Right), Some(id(3)));
        tree.focus(id(2));
        tree.focus(id(1));
        assert_eq!(tree.neighbour(id(1), Direction::Right), Some(id(2)));
        // Without any focus history, position decides: topmost.
        tree.focus_order.clear();
        assert_eq!(tree.neighbour(id(1), Direction::Right), Some(id(2)));
    }

    #[test]
    fn neighbour_must_share_part_of_the_span() {
        // 1 | (2 / (3 | 4)): 4 is right of 3; nothing is right of 4.
        let mut tree = tree_with(3);
        tree.insert(id(4)); // splits 3 (400x300) side by side
        assert_eq!(rect_of(&tree, 4), Rect::new(600, 300, 200, 300));
        assert_eq!(tree.neighbour(id(4), Direction::Up), Some(id(2)));
        assert_eq!(tree.neighbour(id(4), Direction::Left), Some(id(3)));
        assert_eq!(tree.neighbour(id(3), Direction::Left), Some(id(1)));
        // 2 and 3 both touch 1's right edge; 3 was focused (inserted) after 2. 4 is farther.
        assert_eq!(tree.neighbour(id(1), Direction::Right), Some(id(3)));
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
    fn floating_leaf_takes_no_space() {
        let mut tree = tree_with(3); // 1 | (2 / 3)
        tree.set_mode(id(2), Mode::Floating);
        assert_eq!(
            tree.rects(),
            vec![
                (id(1), Rect::new(0, 0, 400, 600)),
                (id(3), Rect::new(400, 0, 400, 600))
            ]
        );
        tree.set_mode(id(2), Mode::Tiled);
        assert_eq!(rect_of(&tree, 2), Rect::new(400, 0, 400, 300));
    }
}
