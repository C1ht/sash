// SPDX-License-Identifier: Apache-2.0

//! Gaps (owner, M18a): the one place a visible rectangle is computed.
//!
//! A cell (a tile, a snap zone, the maximize rectangle) is exact layout geometry; what
//! the window shows is the cell inset on every edge: by the outer gap where the edge
//! lies on the usable area's boundary (after layer-shell reservations, so a window
//! sits 13 px from a panel), by half the inner gap where it lies inside (the leading
//! edge the floor, the trailing edge the rest), so two neighbours are exactly the inner
//! gap apart. Fullscreen windows use no cell and no gap. A visible rectangle is never
//! less than 1 px wide or high.
//!
//! Logical rectangles become physical pixels with [`to_physical`], each edge rounded
//! on its own, so neighbours keep the exact gap at scales 1, 1.5 and 2.

use smithay::utils::{Physical, Rectangle};

use super::Rect;

/// Between a window and the usable area's edge (logical pixels).
pub const OUTER_GAP: i32 = 13;
/// Between two neighbours (logical pixels); each interior edge insets half.
pub const INNER_GAP: i32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gaps {
    pub outer: i32,
    pub inner: i32,
}

impl Gaps {
    pub const DEFAULT: Gaps = Gaps {
        outer: OUTER_GAP,
        inner: INNER_GAP,
    };
    pub const NONE: Gaps = Gaps { outer: 0, inner: 0 };

    /// Never negative. (Release builds use only the constants.)
    #[cfg_attr(not(any(debug_assertions, test)), allow(dead_code))]
    pub fn new(outer: i32, inner: i32) -> Self {
        Self {
            outer: outer.max(0),
            inner: inner.max(0),
        }
    }
}

/// What a window in `cell` shows, inside `usable`.
pub fn visible(cell: Rect, usable: Rect, gaps: Gaps) -> Rect {
    let (head, tail) = (gaps.inner / 2, gaps.inner - gaps.inner / 2);
    let lead = |on_boundary: bool| if on_boundary { gaps.outer } else { head };
    let trail = |on_boundary: bool| if on_boundary { gaps.outer } else { tail };
    let axis = |start: i32, len: i32, a: i32, b: i32| {
        let shrunk = (len - a - b).max(len.min(1));
        (start + a.min(len - shrunk), shrunk)
    };
    let (x, w) = axis(
        cell.x,
        cell.w,
        lead(cell.x <= usable.x),
        trail(cell.right() >= usable.right()),
    );
    let (y, h) = axis(
        cell.y,
        cell.h,
        lead(cell.y <= usable.y),
        trail(cell.bottom() >= usable.bottom()),
    );
    Rect::new(x, y, w, h)
}

/// A logical edge in physical pixels: halves round up, the same on both sides of
/// zero (`f64::round` rounds halves away from zero, which would make a gap one pixel
/// wider on an output left of or above the origin; found by the long property run).
pub fn physical_edge(v: i32, scale: f64) -> i32 {
    (f64::from(v) * scale + 0.5).floor() as i32
}

/// `rect` in physical pixels at `scale`, each edge rounded on its own (Smithay's
/// `to_physical_precise_round` rounds the position and the size separately, which
/// can move an edge by a pixel).
pub fn to_physical(rect: Rect, scale: f64) -> Rectangle<i32, Physical> {
    let edge = |v: i32| physical_edge(v, scale);
    let (x0, y0) = (edge(rect.x), edge(rect.y));
    let (x1, y1) = (edge(rect.right()), edge(rect.bottom()));
    Rectangle::new((x0, y0).into(), (x1 - x0, y1 - y0).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0, 0, 1000, 600);

    #[test]
    fn a_cell_alone_gets_the_outer_gap_on_every_side() {
        assert_eq!(
            visible(AREA, AREA, Gaps::DEFAULT),
            Rect::new(13, 13, 1000 - 26, 600 - 26)
        );
        assert_eq!(visible(AREA, AREA, Gaps::NONE), AREA);
    }

    #[test]
    fn neighbours_are_the_inner_gap_apart() {
        let left = visible(Rect::new(0, 0, 500, 600), AREA, Gaps::DEFAULT);
        let right = visible(Rect::new(500, 0, 500, 600), AREA, Gaps::DEFAULT);
        assert_eq!(left, Rect::new(13, 13, 500 - 13 - 4, 600 - 26));
        assert_eq!(right.x - left.right(), 8);
        assert_eq!(1000 - right.right(), 13);
    }

    #[test]
    fn an_odd_inner_gap_splits_floor_then_rest() {
        let gaps = Gaps::new(0, 7);
        let left = visible(Rect::new(0, 0, 500, 600), AREA, gaps);
        let right = visible(Rect::new(500, 0, 500, 600), AREA, gaps);
        assert_eq!((left.right(), right.x), (496, 503));
    }

    #[test]
    fn a_tiny_cell_keeps_one_pixel() {
        let tiny = visible(Rect::new(0, 0, 10, 10), AREA, Gaps::DEFAULT);
        assert_eq!((tiny.w, tiny.h), (1, 1));
        assert!(Rect::new(0, 0, 10, 10).contains(tiny.x, tiny.y));
    }

    #[test]
    fn physical_edges_round_on_their_own() {
        // Smithay would give x 2 (1.5 rounded) and width 5 (4.5 rounded): right edge 7.
        let r = to_physical(Rect::new(1, 0, 3, 1), 1.5);
        assert_eq!((r.loc.x, r.loc.x + r.size.w), (2, 6));
        // Neighbours 8 apart stay 12 apart at 1.5 and 16 at 2, wherever they are,
        // left of the origin too.
        for x in -50..50 {
            for scale in [1.0, 1.5, 2.0] {
                let a = to_physical(Rect::new(x - 20, 0, 20, 10), scale);
                let b = to_physical(Rect::new(x + 8, 0, 10, 10), scale);
                assert_eq!(b.loc.x - (a.loc.x + a.size.w), (8.0 * scale) as i32);
            }
        }
    }
}
