// SPDX-License-Identifier: Apache-2.0

//! Property tests: random sequences of operations on random output sizes keep the
//! monitor layout's invariants (`MonitorLayout::check_invariants`), and bulk toggles
//! are lossless.

use proptest::prelude::*;

use super::{Direction, Mode, MonitorLayout, NewWindow, Rect, SizeLimits, WindowId};

#[derive(Clone, Debug)]
enum Op {
    /// Opens a window; `forced` with an optional parent (index of an existing window).
    Open {
        forced: bool,
        parent: Option<usize>,
        fixed: Option<(i32, i32)>,
    },
    /// Gives an unplaced floating window its size (the client's first buffer).
    Place(usize, (i32, i32)),
    Close(usize),
    Toggle(usize),
    FloatAll,
    TileAll,
    MoveBy(usize, i32, i32),
    ResizeBy(usize, i32, i32),
    /// A drag or client-chosen rectangle.
    SetRect(usize, Rect),
    Focus(usize),
    FocusDirection(Direction),
    Swap(Direction),
    SetForced(usize, bool, Option<usize>),
    SetArea(Rect),
    SetDefault(Mode),
}

fn direction() -> impl Strategy<Value = Direction> {
    prop_oneof![
        Just(Direction::Left),
        Just(Direction::Right),
        Just(Direction::Up),
        Just(Direction::Down)
    ]
}

fn area() -> impl Strategy<Value = Rect> {
    prop_oneof![
        4 => (-2000..2000i32, -2000..2000i32, 1..8000i32, 1..5000i32),
        1 => (0..2i32, 0..2i32, 1..6i32, 1..6i32),
    ]
    .prop_map(|(x, y, w, h)| Rect::new(x, y, w, h))
}

fn size() -> impl Strategy<Value = (i32, i32)> {
    (1..3000i32, 1..3000i32)
}

fn op() -> impl Strategy<Value = Op> {
    let index = any::<usize>();
    prop_oneof![
        4 => (any::<bool>(), prop::option::of(any::<usize>()), prop::option::of(size()))
            .prop_map(|(forced, parent, fixed)| Op::Open { forced, parent, fixed }),
        3 => (index, size()).prop_map(|(i, s)| Op::Place(i, s)),
        2 => index.prop_map(Op::Close),
        2 => index.prop_map(Op::Toggle),
        1 => Just(Op::FloatAll),
        1 => Just(Op::TileAll),
        2 => (index, -3000..3000i32, -3000..3000i32).prop_map(|(i, x, y)| Op::MoveBy(i, x, y)),
        2 => (index, -2000..2000i32, -2000..2000i32).prop_map(|(i, w, h)| Op::ResizeBy(i, w, h)),
        2 => (index, -5000..5000i32, -5000..5000i32, -100..4000i32, -100..4000i32)
            .prop_map(|(i, x, y, w, h)| Op::SetRect(i, Rect::new(x, y, w.max(0), h.max(0)))),
        2 => index.prop_map(Op::Focus),
        2 => direction().prop_map(Op::FocusDirection),
        1 => direction().prop_map(Op::Swap),
        1 => (index, any::<bool>(), prop::option::of(any::<usize>()))
            .prop_map(|(i, f, p)| Op::SetForced(i, f, p)),
        1 => area().prop_map(Op::SetArea),
        1 => prop_oneof![Just(Mode::Tiled), Just(Mode::Floating)].prop_map(Op::SetDefault),
    ]
}

fn pick(layout: &MonitorLayout, index: usize) -> Option<WindowId> {
    let windows: Vec<WindowId> = layout.windows().collect();
    (!windows.is_empty()).then(|| windows[index % windows.len()])
}

fn apply(layout: &mut MonitorLayout, next: &mut u64, op: &Op) {
    match op.clone() {
        Op::Open {
            forced,
            parent,
            fixed,
        } => {
            *next += 1;
            let limits = fixed
                .map(|size| SizeLimits {
                    min: size,
                    max: size,
                })
                .unwrap_or_default();
            let new = NewWindow {
                forced: forced || limits.is_fixed(),
                parent: parent.and_then(|p| pick(layout, p)),
                limits,
            };
            layout.open(WindowId(*next), new);
            assert_eq!(
                layout.focused(),
                Some(WindowId(*next)),
                "a new window gets focus"
            );
        }
        Op::Place(i, size) => {
            if let Some(w) = pick(layout, i) {
                layout.place(w, size);
            }
        }
        Op::Close(i) => {
            if let Some(w) = pick(layout, i) {
                assert!(layout.close(w));
                assert!(!layout.contains(w));
            }
        }
        Op::Toggle(i) => {
            if let Some(w) = pick(layout, i) {
                let forced = layout.is_forced(w);
                assert_eq!(layout.toggle(w), !forced, "toggle works unless forced");
            }
        }
        Op::FloatAll => layout.float_all(),
        Op::TileAll => {
            layout.tile_all();
            for w in layout.windows() {
                let expected = if layout.is_forced(w) {
                    Mode::Floating
                } else {
                    Mode::Tiled
                };
                assert_eq!(layout.mode(w), Some(expected));
            }
        }
        Op::MoveBy(i, dx, dy) => {
            if let Some(w) = pick(layout, i) {
                layout.move_by(w, dx, dy);
            }
        }
        Op::ResizeBy(i, dw, dh) => {
            if let Some(w) = pick(layout, i)
                && let Some(r) = layout.resize_by(w, dw, dh)
            {
                let limits = layout.limits(w);
                assert_eq!(limits.clamp((r.w, r.h)), (r.w, r.h), "size within limits");
            }
        }
        Op::SetRect(i, rect) => {
            if let Some(w) = pick(layout, i)
                && layout.mode(w) == Some(Mode::Floating)
            {
                layout.set_floating_rect(w, rect);
            }
        }
        Op::Focus(i) => {
            if let Some(w) = pick(layout, i) {
                layout.focus(w);
                assert_eq!(layout.recent_first().first(), Some(&w));
                if layout.mode(w) == Some(Mode::Floating) && layout.is_placed(w) {
                    let top = layout.floating_rects().last().map(|(t, _)| *t);
                    assert_eq!(top, Some(w), "focusing raises");
                }
            }
        }
        Op::FocusDirection(d) => {
            if let Some(f) = layout.focused()
                && let Some(n) = layout.neighbour(f, d)
            {
                assert_ne!(n, f);
                layout.focus(n);
            }
        }
        Op::Swap(d) => {
            let before = layout.focused();
            layout.swap_focused(d);
            assert_eq!(layout.focused(), before, "focus follows the swapped window");
        }
        Op::SetForced(i, forced, parent) => {
            if let Some(w) = pick(layout, i) {
                let parent = parent.and_then(|p| pick(layout, p));
                let fixed = layout.limits(w).is_fixed();
                layout.set_forced(w, forced || fixed, parent);
            }
        }
        Op::SetArea(area) => layout.set_area(area),
        Op::SetDefault(mode) => layout.set_default_mode(mode),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1500))]

    #[test]
    fn random_operations_keep_invariants(area in area(), ops in prop::collection::vec(op(), 1..80)) {
        let mut layout = MonitorLayout::new(area);
        let mut next = 0;
        for op in &ops {
            apply(&mut layout, &mut next, op);
            layout.check_invariants();
        }
    }

    /// From any state: tile all, then float all, any moves, resizes and output resizes,
    /// then tile all again gives the identical tree (with the final area).
    #[test]
    fn float_all_then_tile_all_is_lossless(
        area in area(),
        setup in prop::collection::vec(op(), 0..40),
        between in prop::collection::vec(prop_oneof![
            (any::<usize>(), -3000..3000i32, -3000..3000i32).prop_map(|(i, x, y)| Op::MoveBy(i, x, y)),
            (any::<usize>(), -2000..2000i32, -2000..2000i32).prop_map(|(i, w, h)| Op::ResizeBy(i, w, h)),
            any::<usize>().prop_map(Op::Focus),
            area().prop_map(Op::SetArea),
        ], 0..20),
    ) {
        let mut layout = MonitorLayout::new(area);
        let mut next = 0;
        for op in &setup {
            apply(&mut layout, &mut next, op);
        }
        layout.tile_all();
        let mut expected = layout.tree().clone();
        layout.float_all();
        for op in &between {
            apply(&mut layout, &mut next, op);
            layout.check_invariants();
        }
        layout.tile_all();
        use crate::layout::Layout;
        expected.set_area(layout.area());
        prop_assert_eq!(layout.tree(), &expected);
        prop_assert_eq!(layout.tiled_rects(), expected.rects());
    }
}
