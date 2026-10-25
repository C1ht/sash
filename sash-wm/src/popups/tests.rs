// SPDX-License-Identifier: Apache-2.0

//! Popup placement: the positioner rules and constraint adjustments, against the
//! output, as sash-wm uses them.

use proptest::prelude::*;
use smithay::{
    reexports::wayland_protocols::xdg::shell::server::xdg_positioner::{
        Anchor, ConstraintAdjustment as Adjust, Gravity,
    },
    utils::{Logical, Point, Rectangle},
    wayland::shell::xdg::PositionerState,
};

use super::place;
use crate::layout::{MonitorLayout, NewWindow, Rect, WindowId};

const BIG: Rect = Rect::new(0, 0, 1000, 1000);
const ORIGIN: Point<i32, Logical> = Point::new(0, 0);

const ANCHORS: [Anchor; 9] = [
    Anchor::None,
    Anchor::Top,
    Anchor::Bottom,
    Anchor::Left,
    Anchor::Right,
    Anchor::TopLeft,
    Anchor::BottomLeft,
    Anchor::TopRight,
    Anchor::BottomRight,
];
const GRAVITIES: [Gravity; 9] = [
    Gravity::None,
    Gravity::Top,
    Gravity::Bottom,
    Gravity::Left,
    Gravity::Right,
    Gravity::TopLeft,
    Gravity::BottomLeft,
    Gravity::TopRight,
    Gravity::BottomRight,
];

fn positioner(
    size: (i32, i32),
    anchor_rect: (i32, i32, i32, i32),
    anchor: Anchor,
    gravity: Gravity,
    adjust: Adjust,
) -> PositionerState {
    let (x, y, w, h) = anchor_rect;
    PositionerState {
        rect_size: size.into(),
        anchor_rect: Rectangle::new((x, y).into(), (w, h).into()),
        anchor_edges: anchor,
        gravity,
        constraint_adjustment: adjust,
        ..Default::default()
    }
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
    Rectangle::new((x, y).into(), (w, h).into())
}

/// Which side of the anchor rectangle or popup an edge set names, per axis:
/// -1 left/top, 1 right/bottom, 0 center.
fn sides(name: &str) -> (i32, i32) {
    let x = if name.contains("Left") {
        -1
    } else if name.contains("Right") {
        1
    } else {
        0
    };
    let y = if name.contains("Top") {
        -1
    } else if name.contains("Bottom") {
        1
    } else {
        0
    };
    (x, y)
}

#[test]
fn every_anchor_and_gravity() {
    // Anchor rectangle (100, 100, 40, 20), popup 60x30, nothing constrained.
    for anchor in ANCHORS {
        for gravity in GRAVITIES {
            let p = positioner(
                (60, 30),
                (100, 100, 40, 20),
                anchor,
                gravity,
                Adjust::empty(),
            );
            let (ax, ay) = sides(&format!("{anchor:?}"));
            let (gx, gy) = sides(&format!("{gravity:?}"));
            let point_x = 100 + 20 * (ax + 1);
            let point_y = 100 + 10 * (ay + 1);
            // Gravity left/top puts the popup before the point, right/bottom after it,
            // none centers it.
            let x = point_x - 30 * (1 - gx);
            let y = point_y - 15 * (1 - gy);
            assert_eq!(
                place(p, ORIGIN, BIG),
                rect(x, y, 60, 30),
                "anchor {anchor:?} gravity {gravity:?}"
            );
        }
    }
}

#[test]
fn offset_moves_the_popup() {
    let mut p = positioner(
        (60, 30),
        (100, 100, 40, 20),
        Anchor::BottomLeft,
        Gravity::BottomRight,
        Adjust::empty(),
    );
    p.offset = (7, -3).into();
    assert_eq!(place(p, ORIGIN, BIG), rect(107, 117, 60, 30));
}

#[test]
fn parent_origin_shifts_the_output_bounds() {
    // The parent sits at (900, 0) on a 1000 wide output; a 200 wide popup at its
    // x = 0 would end at 1100: sliding moves it back by 100.
    let p = positioner(
        (200, 30),
        (0, 0, 10, 10),
        Anchor::TopLeft,
        Gravity::BottomRight,
        Adjust::SlideX,
    );
    assert_eq!(place(p, (900, 0).into(), BIG), rect(-100, 0, 200, 30));
}

/// A popup below and right of an anchor near the bottom-right output corner:
/// unconstrained it would be at (950, 960) with size 100x80.
fn corner(adjust: Adjust) -> PositionerState {
    positioner(
        (100, 80),
        (940, 950, 10, 10),
        Anchor::BottomRight,
        Gravity::BottomRight,
        adjust,
    )
}

#[test]
fn unconstrained_popups_are_not_adjusted() {
    let p = positioner(
        (100, 80),
        (100, 100, 10, 10),
        Anchor::BottomRight,
        Gravity::BottomRight,
        Adjust::all(),
    );
    assert_eq!(place(p, ORIGIN, BIG), rect(110, 110, 100, 80));
    assert_eq!(
        place(corner(Adjust::empty()), ORIGIN, BIG),
        rect(950, 960, 100, 80)
    );
}

#[test]
fn slide_in_each_axis_and_both() {
    assert_eq!(
        place(corner(Adjust::SlideX), ORIGIN, BIG),
        rect(900, 960, 100, 80)
    );
    assert_eq!(
        place(corner(Adjust::SlideY), ORIGIN, BIG),
        rect(950, 920, 100, 80)
    );
    assert_eq!(
        place(corner(Adjust::SlideX | Adjust::SlideY), ORIGIN, BIG),
        rect(900, 920, 100, 80)
    );
}

#[test]
fn flip_in_each_axis_and_both() {
    // Flipped: anchored at the anchor rectangle's top-left, growing up and left.
    assert_eq!(
        place(corner(Adjust::FlipX), ORIGIN, BIG),
        rect(840, 960, 100, 80)
    );
    assert_eq!(
        place(corner(Adjust::FlipY), ORIGIN, BIG),
        rect(950, 870, 100, 80)
    );
    assert_eq!(
        place(corner(Adjust::FlipX | Adjust::FlipY), ORIGIN, BIG),
        rect(840, 870, 100, 80)
    );
}

#[test]
fn resize_in_each_axis_and_both() {
    assert_eq!(
        place(corner(Adjust::ResizeX), ORIGIN, BIG),
        rect(950, 960, 50, 80)
    );
    assert_eq!(
        place(corner(Adjust::ResizeY), ORIGIN, BIG),
        rect(950, 960, 100, 40)
    );
    assert_eq!(
        place(corner(Adjust::ResizeX | Adjust::ResizeY), ORIGIN, BIG),
        rect(950, 960, 50, 40)
    );
}

#[test]
fn adjustments_follow_the_protocol_order() {
    // Flip before slide: the flip fits, so nothing slides.
    assert_eq!(
        place(corner(Adjust::FlipX | Adjust::SlideX), ORIGIN, BIG),
        rect(840, 960, 100, 80)
    );
    // Slide before resize: the slide fits, so nothing shrinks.
    assert_eq!(
        place(corner(Adjust::SlideX | Adjust::ResizeX), ORIGIN, BIG),
        rect(900, 960, 100, 80)
    );
    // All of them on both axes: flipped, nothing else needed.
    assert_eq!(
        place(corner(Adjust::all()), ORIGIN, BIG),
        rect(840, 870, 100, 80)
    );
}

#[test]
fn a_flip_that_does_not_help_is_reverted() {
    // 300 wide, anchored at x = 200 of a 400 wide output: neither side fits.
    let bounds = Rect::new(0, 0, 400, 400);
    let p = |adjust| {
        positioner(
            (300, 50),
            (200, 100, 0, 0),
            Anchor::TopLeft,
            Gravity::BottomRight,
            adjust,
        )
    };
    assert_eq!(
        place(p(Adjust::FlipX), ORIGIN, bounds),
        rect(200, 100, 300, 50)
    );
    // The slide then works from the unflipped position.
    assert_eq!(
        place(p(Adjust::FlipX | Adjust::SlideX), ORIGIN, bounds),
        rect(100, 100, 300, 50)
    );
}

#[test]
fn a_popup_larger_than_the_output() {
    let bounds = Rect::new(0, 0, 400, 300);
    let p = |adjust| {
        positioner(
            (600, 500),
            (50, 50, 10, 10),
            Anchor::TopLeft,
            Gravity::BottomRight,
            adjust,
        )
    };
    // Sliding shows its top-left corner at the output's.
    assert_eq!(
        place(p(Adjust::SlideX | Adjust::SlideY), ORIGIN, bounds),
        rect(0, 0, 600, 500)
    );
    // Sliding then resizing makes it exactly the output.
    assert_eq!(
        place(
            p(Adjust::all() - Adjust::FlipX - Adjust::FlipY),
            ORIGIN,
            bounds
        ),
        rect(0, 0, 400, 300)
    );
    // Resize alone keeps the left/top edges and cuts the rest.
    assert_eq!(
        place(p(Adjust::ResizeX | Adjust::ResizeY), ORIGIN, bounds),
        rect(50, 50, 350, 250)
    );
}

#[test]
fn reposition_uses_the_new_positioner() {
    let first = corner(Adjust::empty());
    let mut second = first;
    second.anchor_rect = rect(100, 100, 10, 10);
    second.constraint_adjustment = Adjust::SlideX;
    assert_eq!(place(first, ORIGIN, BIG), rect(950, 960, 100, 80));
    assert_eq!(place(second, ORIGIN, BIG), rect(110, 110, 100, 80));
}

fn inside_x(r: Rectangle<i32, Logical>, b: Rect) -> bool {
    r.loc.x >= b.x && r.loc.x + r.size.w <= b.x + b.w
}

fn inside_y(r: Rectangle<i32, Logical>, b: Rect) -> bool {
    r.loc.y >= b.y && r.loc.y + r.size.h <= b.y + b.h
}

fn arb_positioner() -> impl Strategy<Value = PositionerState> {
    (
        (1..1500i32, 1..1500i32),
        (-200..1200i32, -200..1200i32, 0..300i32, 0..300i32),
        0..9usize,
        0..9usize,
        0..64u32,
        (-60..60i32, -60..60i32),
    )
        .prop_map(|(size, anchor_rect, a, g, bits, offset)| {
            let mut p = positioner(
                size,
                anchor_rect,
                ANCHORS[a],
                GRAVITIES[g],
                Adjust::from_bits_truncate(bits),
            );
            p.offset = offset.into();
            p
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn placement_properties(
        p in arb_positioner(),
        (ow, oh) in (1..2000i32, 1..2000i32),
        (px, py) in (-300..2000i32, -300..2000i32),
        windows in 1..6u64,
        toggles in proptest::collection::vec(0..6u64, 0..4),
    ) {
        let bounds = Rect::new(0, 0, ow, oh);
        let origin: Point<i32, Logical> = (px, py).into();
        let placed = place(p, origin, bounds);
        // Deterministic.
        prop_assert_eq!(placed, place(p, origin, bounds));
        // Never larger than asked, never empty.
        prop_assert!(placed.size.w >= 1 && placed.size.w <= p.rect_size.w);
        prop_assert!(placed.size.h >= 1 && placed.size.h <= p.rect_size.h);
        // In output coordinates.
        let global = Rectangle::new(placed.loc + origin, placed.size);
        let unadjusted = Rectangle::new(p.get_geometry().loc + origin, p.rect_size);
        let adjust = p.constraint_adjustment;
        // A popup that fits is left alone.
        if inside_x(unadjusted, bounds) && inside_y(unadjusted, bounds) {
            prop_assert_eq!(global, unadjusted);
        }
        // Sliding keeps a popup that fits on that axis inside the output.
        if adjust.contains(Adjust::SlideX) && p.rect_size.w <= ow {
            prop_assert!(inside_x(global, bounds), "x: {:?} in {:?}", global, bounds);
        }
        if adjust.contains(Adjust::SlideY) && p.rect_size.h <= oh {
            prop_assert!(inside_y(global, bounds), "y: {:?} in {:?}", global, bounds);
        }
        // Resizing alone trims a popup that overlaps the output on that axis.
        let overlaps_x = unadjusted.loc.x < ow && unadjusted.loc.x + unadjusted.size.w > 0;
        let overlaps_y = unadjusted.loc.y < oh && unadjusted.loc.y + unadjusted.size.h > 0;
        if adjust & (Adjust::FlipX | Adjust::SlideX | Adjust::ResizeX) == Adjust::ResizeX
            && overlaps_x
        {
            prop_assert!(inside_x(global, bounds), "x: {:?} in {:?}", global, bounds);
        }
        if adjust & (Adjust::FlipY | Adjust::SlideY | Adjust::ResizeY) == Adjust::ResizeY
            && overlaps_y
        {
            prop_assert!(inside_y(global, bounds), "y: {:?} in {:?}", global, bounds);
        }

        // Placing popups on the windows of a layout leaves the layout as it was.
        let mut layout = MonitorLayout::new(bounds);
        for n in 1..=windows {
            layout.open(WindowId(n), NewWindow::default());
        }
        for t in toggles {
            layout.toggle(WindowId(t));
        }
        let before = layout.tiled_rects();
        for n in 1..=windows {
            if let Some(r) = layout.shown_rect(WindowId(n)) {
                let _ = place(p, (r.x, r.y).into(), layout.area());
            }
        }
        prop_assert_eq!(layout.tiled_rects(), before);
        layout.check_invariants();
    }
}

/// Cost of placing a popup: unconstrained, and with every adjustment in play. Run in
/// release: `cargo test --release -- --ignored bench_`.
#[test]
#[ignore = "benchmark"]
fn bench_popup_placement() {
    use std::{hint::black_box, time::Instant};
    let n = 1_000_000;
    for (name, p) in [
        (
            "unconstrained",
            positioner(
                (100, 80),
                (100, 100, 10, 10),
                Anchor::BottomRight,
                Gravity::BottomRight,
                Adjust::all(),
            ),
        ),
        ("corner, all adjustments", corner(Adjust::all())),
        (
            "corner, flip reverted then slide+resize",
            positioner(
                (1200, 1200),
                (500, 500, 10, 10),
                Anchor::BottomRight,
                Gravity::BottomRight,
                Adjust::all(),
            ),
        ),
    ] {
        let start = Instant::now();
        for i in 0..n {
            black_box(place(black_box(p), (i % 7, 0).into(), BIG));
        }
        println!(
            "{name}: {:.1} ns/op",
            start.elapsed().as_nanos() as f64 / n as f64
        );
    }
}
