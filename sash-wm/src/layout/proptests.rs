// SPDX-License-Identifier: Apache-2.0

//! Property tests: random sequences of operations on random output sizes keep the
//! tree's invariants (see `bsp::tests::check_invariants`).

use proptest::prelude::*;

use super::{BspTree, Direction, Layout, Rect, WindowId, bsp::tests::check_invariants};

#[derive(Clone, Debug)]
enum Op {
    Open,
    /// Closes the window at this index (modulo the number of windows).
    Close(usize),
    /// Swaps the focused window with its neighbour, like mod+Shift+direction.
    Swap(Direction),
    /// Moves focus, like mod+direction.
    Focus(Direction),
}

fn direction() -> impl Strategy<Value = Direction> {
    prop_oneof![
        Just(Direction::Left),
        Just(Direction::Right),
        Just(Direction::Up),
        Just(Direction::Down)
    ]
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => Just(Op::Open),
        2 => any::<usize>().prop_map(Op::Close),
        2 => direction().prop_map(Op::Swap),
        2 => direction().prop_map(Op::Focus),
    ]
}

fn area() -> impl Strategy<Value = Rect> {
    // Mostly realistic sizes, sometimes tiny ones where windows get zero pixels.
    prop_oneof![
        (-2000..2000i32, -2000..2000i32, 1..8000i32, 1..5000i32),
        (0..2i32, 0..2i32, 1..6i32, 1..6i32),
    ]
    .prop_map(|(x, y, w, h)| Rect::new(x, y, w, h))
}

fn sorted_rects(tree: &BspTree) -> Vec<Rect> {
    let mut rects: Vec<Rect> = tree.rects().into_iter().map(|(_, r)| r).collect();
    rects.sort_by_key(|r| (r.x, r.y, r.w, r.h));
    rects
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn random_operations_keep_invariants(area in area(), ops in prop::collection::vec(op(), 1..80)) {
        let mut tree = BspTree::new(area);
        let mut next = 0u64;
        for op in ops {
            match op {
                Op::Open => {
                    next += 1;
                    tree.insert(WindowId(next));
                    prop_assert_eq!(tree.focused(), Some(WindowId(next)));
                }
                Op::Close(index) => {
                    let windows = tree.windows();
                    if !windows.is_empty() {
                        let window = windows[index % windows.len()];
                        prop_assert!(tree.remove(window));
                        prop_assert!(!tree.contains(window));
                    }
                }
                Op::Swap(direction) => {
                    if let Some(focused) = tree.focused()
                        && let Some(other) = tree.neighbour(focused, direction)
                    {
                        let before = sorted_rects(&tree);
                        prop_assert!(tree.swap(focused, other));
                        prop_assert_eq!(sorted_rects(&tree), before, "swap changed the rectangles");
                        prop_assert_eq!(tree.focused(), Some(focused), "focus follows the swapped window");
                    }
                }
                Op::Focus(direction) => {
                    if let Some(focused) = tree.focused()
                        && let Some(other) = tree.neighbour(focused, direction)
                    {
                        prop_assert!(other != focused);
                        prop_assert!(tree.focus(other));
                    }
                }
            }
            check_invariants(&tree);
        }
    }
}
