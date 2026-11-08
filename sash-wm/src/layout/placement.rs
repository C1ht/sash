// SPDX-License-Identifier: Apache-2.0

//! Where a new tiled window goes in the tile it splits (owner, M19b): the zone
//! function, pure and in exact integer arithmetic.
//!
//! The point is a pixel centre, (x + 0.5, y + 0.5). A centred box 23.6% of the
//! rectangle's width by 23.6% of its height is the centre; elsewhere the nearest edge,
//! measured in units of the width and the height, picks the side, so the two diagonals
//! divide the rectangle. Exact ties on a diagonal go to top or bottom; a tie between
//! top and bottom (the middle row) goes to bottom and one between left and right (the
//! middle column) to right, which can only happen inside the centre box. A point
//! outside the rectangle (in a gap) gets the side it lies beyond.

use super::Rect;

/// Where in a tile the new window goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DropSide {
    Left,
    Right,
    Top,
    Bottom,
    /// The longer side is split, the new window second.
    Centre,
}

/// The centre box's share of each axis, in thousandths (owner: 23.6%).
const CENTRE_PER_MILLE: i64 = 236;

/// The zone of `rect` (a tile's visible rectangle) that pixel (x, y) is in.
pub fn drop_side(rect: Rect, x: i32, y: i32) -> DropSide {
    let (w, h) = (i64::from(rect.w.max(1)), i64::from(rect.h.max(1)));
    // Twice the pixel centre's offset from the rectangle's left and top edges.
    let px = 2 * (i64::from(x) - i64::from(rect.x)) + 1;
    let py = 2 * (i64::from(y) - i64::from(rect.y)) + 1;
    // The centre box: |p - centre| < 11.8% of the length on both axes.
    let inside = |p: i64, len: i64| (p - len).abs() * 1000 < CENTRE_PER_MILLE * len;
    if inside(px, w) && inside(py, h) {
        return DropSide::Centre;
    }
    // Distances to the edges as fractions of the length, all scaled by 2 * w * h.
    let left = px * h;
    let right = (2 * w - px) * h;
    let top = py * w;
    let bottom = (2 * h - py) * w;
    let horizontal = left.min(right);
    let vertical = top.min(bottom);
    if vertical <= horizontal {
        if top < bottom {
            DropSide::Top
        } else {
            DropSide::Bottom
        }
    } else if left < right {
        DropSide::Left
    } else {
        DropSide::Right
    }
}

impl DropSide {
    /// Where the new window goes for a split on the other axis: the pointer's half of
    /// that axis; for centre (or a fall-back) the second child.
    pub fn other_axis(self, rect: Rect, at: Option<(i32, i32)>) -> DropSide {
        let horizontal_now = matches!(self, DropSide::Left | DropSide::Right)
            || (self == DropSide::Centre && rect.w >= rect.h);
        match (horizontal_now, at) {
            (true, Some((_, y))) if 2 * (y - rect.y) + 1 < rect.h => DropSide::Top,
            (true, _) => DropSide::Bottom,
            (false, Some((x, _))) if 2 * (x - rect.x) + 1 < rect.w => DropSide::Left,
            (false, _) => DropSide::Right,
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    /// A square tile rotated a quarter turn clockwise about its centre: the point and
    /// the side turn with it.
    fn rotate(side: DropSide) -> DropSide {
        match side {
            DropSide::Top => DropSide::Right,
            DropSide::Right => DropSide::Bottom,
            DropSide::Bottom => DropSide::Left,
            DropSide::Left => DropSide::Top,
            DropSide::Centre => DropSide::Centre,
        }
    }

    fn mirror_x(side: DropSide) -> DropSide {
        match side {
            DropSide::Left => DropSide::Right,
            DropSide::Right => DropSide::Left,
            other => other,
        }
    }

    fn mirror_y(side: DropSide) -> DropSide {
        match side {
            DropSide::Top => DropSide::Bottom,
            DropSide::Bottom => DropSide::Top,
            other => other,
        }
    }

    /// A pixel exactly on a diagonal of a square tile (not in the centre box).
    fn on_a_diagonal(n: i32, x: i32, y: i32) -> bool {
        x == y || x + y == n - 1
    }

    #[test]
    fn the_sides_of_a_tile() {
        let r = Rect::new(100, 50, 400, 300);
        assert_eq!(drop_side(r, 110, 200), DropSide::Left);
        assert_eq!(drop_side(r, 490, 200), DropSide::Right);
        assert_eq!(drop_side(r, 300, 60), DropSide::Top);
        assert_eq!(drop_side(r, 300, 340), DropSide::Bottom);
        assert_eq!(drop_side(r, 300, 200), DropSide::Centre);
        // The centre box: 23.6% of 400 = 94.4 wide, 23.6% of 300 = 70.8 high, centred
        // on (300, 200): x 252.8..347.2, y 164.6..235.4 (pixel centres).
        assert_eq!(drop_side(r, 253, 200), DropSide::Centre);
        assert_eq!(drop_side(r, 252, 200), DropSide::Left);
        assert_eq!(drop_side(r, 346, 200), DropSide::Centre);
        assert_eq!(drop_side(r, 347, 200), DropSide::Right);
        assert_eq!(drop_side(r, 300, 165), DropSide::Centre);
        assert_eq!(drop_side(r, 300, 164), DropSide::Top);
        assert_eq!(drop_side(r, 300, 234), DropSide::Centre);
        assert_eq!(drop_side(r, 300, 235), DropSide::Bottom);
        // A wide tile: the diagonals run corner to corner, so near the middle of the
        // top edge a point far from the top is still "top".
        let wide = Rect::new(0, 0, 1000, 100);
        assert_eq!(drop_side(wide, 500, 30), DropSide::Top);
        assert_eq!(drop_side(wide, 40, 50), DropSide::Left);
        assert_eq!(
            drop_side(wide, 60, 95),
            DropSide::Bottom,
            "past the diagonal"
        );
    }

    #[test]
    fn diagonal_ties_go_to_top_or_bottom() {
        let r = Rect::new(0, 0, 100, 100);
        for i in 0..100 {
            let side = drop_side(r, i, i);
            if side != DropSide::Centre {
                assert!(
                    matches!(side, DropSide::Top | DropSide::Bottom),
                    "{i}: {side:?}"
                );
            }
            let side = drop_side(r, 99 - i, i);
            if side != DropSide::Centre {
                assert!(
                    matches!(side, DropSide::Top | DropSide::Bottom),
                    "{i}: {side:?}"
                );
            }
        }
        assert_eq!(drop_side(r, 0, 0), DropSide::Top);
        assert_eq!(drop_side(r, 99, 99), DropSide::Bottom);
    }

    #[test]
    fn a_point_in_a_gap_gets_the_side_it_lies_beyond() {
        let r = Rect::new(13, 13, 383, 574);
        assert_eq!(drop_side(r, 400, 300), DropSide::Right, "the inner gap");
        assert_eq!(drop_side(r, 5, 300), DropSide::Left, "the outer gap");
        assert_eq!(drop_side(r, 200, 3), DropSide::Top);
        assert_eq!(drop_side(r, 200, 595), DropSide::Bottom);
    }

    #[test]
    fn the_other_axis_takes_the_pointers_half() {
        let r = Rect::new(0, 0, 400, 300);
        assert_eq!(DropSide::Left.other_axis(r, Some((10, 50))), DropSide::Top);
        assert_eq!(
            DropSide::Right.other_axis(r, Some((390, 250))),
            DropSide::Bottom
        );
        assert_eq!(DropSide::Top.other_axis(r, Some((50, 10))), DropSide::Left);
        assert_eq!(
            DropSide::Bottom.other_axis(r, Some((350, 290))),
            DropSide::Right
        );
        // Centre on a wide tile split side by side; the other axis is stacked, second.
        assert_eq!(DropSide::Centre.other_axis(r, None), DropSide::Bottom);
        assert_eq!(
            DropSide::Centre.other_axis(Rect::new(0, 0, 300, 400), None),
            DropSide::Right
        );
    }

    proptest! {
        /// Square tiles: rotating the tile a quarter turn (and the point with it), or
        /// mirroring it, maps the result the same way; on an exact diagonal the tie
        /// rule decides, so those points are checked separately.
        #[test]
        fn square_tiles_rotate_and_mirror(
            n in 1..400i32,
            x0 in -500..500i32,
            y0 in -500..500i32,
            fx in 0.0..1.0f64,
            fy in 0.0..1.0f64,
        ) {
            let r = Rect::new(x0, y0, n, n);
            let (dx, dy) = (((fx * f64::from(n)) as i32).min(n - 1), ((fy * f64::from(n)) as i32).min(n - 1));
            let side = drop_side(r, x0 + dx, y0 + dy);
            // Mirrors: (dx, dy) -> (n-1-dx, dy) and (dx, n-1-dy).
            prop_assert_eq!(drop_side(r, x0 + n - 1 - dx, y0 + dy), mirror_x(side));
            prop_assert_eq!(drop_side(r, x0 + dx, y0 + n - 1 - dy), mirror_y(side));
            // A quarter turn clockwise: (dx, dy) -> (n-1-dy, dx).
            let turned = drop_side(r, x0 + n - 1 - dy, y0 + dx);
            if on_a_diagonal(n, dx, dy) {
                prop_assert!(matches!(side, DropSide::Top | DropSide::Bottom | DropSide::Centre));
                prop_assert!(matches!(turned, DropSide::Top | DropSide::Bottom | DropSide::Centre));
            } else {
                prop_assert_eq!(turned, rotate(side));
            }
        }

        /// The centre box is symmetric and has the owner's size: a point is centre
        /// exactly when its pixel centre is within 11.8% of the length from the middle
        /// on both axes; every point gets exactly one result, the same every time.
        #[test]
        fn the_centre_box_and_determinism(
            x0 in -500..500i32,
            y0 in -500..500i32,
            w in 1..2000i32,
            h in 1..2000i32,
            dx in -50..2050i32,
            dy in -50..2050i32,
        ) {
            let r = Rect::new(x0, y0, w, h);
            let side = drop_side(r, x0 + dx, y0 + dy);
            prop_assert_eq!(side, drop_side(r, x0 + dx, y0 + dy));
            let cx = (f64::from(dx) + 0.5 - f64::from(w) / 2.0).abs();
            let cy = (f64::from(dy) + 0.5 - f64::from(h) / 2.0).abs();
            let centre = cx < 0.118 * f64::from(w) && cy < 0.118 * f64::from(h);
            // Away from the box's exact edge, where float rounding could differ.
            let near = (cx - 0.118 * f64::from(w)).abs() < 1e-6 || (cy - 0.118 * f64::from(h)).abs() < 1e-6;
            if !near {
                prop_assert_eq!(side == DropSide::Centre, centre);
            }
            // Mirroring the point through the centre mirrors the side.
            let opposite = drop_side(r, x0 + w - 1 - dx, y0 + h - 1 - dy);
            let expect = mirror_x(mirror_y(side));
            if side != DropSide::Centre && !(matches!(side, DropSide::Top | DropSide::Bottom) && {
                // A diagonal tie: the mirrored point is on the other diagonal end and may
                // tie the other way only if it is a tie itself.
                let px = 2 * i64::from(dx) + 1;
                let py = 2 * i64::from(dy) + 1;
                let (w, h) = (i64::from(w), i64::from(h));
                let hz = (px * h).min((2 * w - px) * h);
                let vt = (py * w).min((2 * h - py) * w);
                hz == vt
            }) {
                prop_assert_eq!(opposite, expect);
            }
        }

        /// Inside the rectangle, the side is the nearest edge (fractions of the
        /// length), ties to top or bottom.
        #[test]
        fn the_nearest_edge_wins(
            w in 1..2000i32,
            h in 1..2000i32,
            fx in 0.0..1.0f64,
            fy in 0.0..1.0f64,
        ) {
            let r = Rect::new(0, 0, w, h);
            let (x, y) = (((fx * f64::from(w)) as i32).min(w - 1), ((fy * f64::from(h)) as i32).min(h - 1));
            let side = drop_side(r, x, y);
            if side != DropSide::Centre {
                let u = (f64::from(x) + 0.5) / f64::from(w);
                let v = (f64::from(y) + 0.5) / f64::from(h);
                let d = [u, 1.0 - u, v, 1.0 - v];
                let best = d.iter().cloned().fold(f64::INFINITY, f64::min);
                let got = match side {
                    DropSide::Left => d[0],
                    DropSide::Right => d[1],
                    DropSide::Top => d[2],
                    DropSide::Bottom => d[3],
                    DropSide::Centre => unreachable!(),
                };
                prop_assert!((got - best).abs() < 1e-9, "{side:?}: {got} vs {best}");
            }
        }
    }
}
