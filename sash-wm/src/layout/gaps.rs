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
//! In physical pixels ([`visible_physical`], owner after M18a): the cell's edges are
//! rounded each on its own, and each gap is rounded to whole pixels once per scale
//! (halves up: 13 at 1.5 is 20 on all four sides, 8 is 12), then the edges are placed.
//! So the outer gap is the same on every side and every inner gap is the same, at any
//! scale and wherever the output is. Everything drawn inside a window goes through its
//! [`Frame`], so it lines up with those edges.

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

/// A gap in whole physical pixels at `scale`, rounded once (halves up).
pub fn physical_gap(gap: i32, scale: f64) -> i32 {
    physical_edge(gap, scale)
}

/// What a window in `cell` shows, in physical pixels at `scale`: the cell's edges
/// rounded each on its own, then inset by the gaps rounded once per scale (the outer
/// gap on boundary edges, the inner gap's floor half on leading interior edges and the
/// rest on trailing ones). Never less than 1 px wide or high.
pub fn visible_physical(
    cell: Rect,
    usable: Rect,
    gaps: Gaps,
    scale: f64,
) -> Rectangle<i32, Physical> {
    let edge = |v: i32| physical_edge(v, scale);
    let outer = physical_gap(gaps.outer, scale);
    let inner = physical_gap(gaps.inner, scale);
    let (head, tail) = (inner / 2, inner - inner / 2);
    let axis = |start: i32, end: i32, lead_on_boundary: bool, trail_on_boundary: bool| {
        let (p0, p1) = (edge(start), edge(end));
        let len = p1 - p0;
        let a = if lead_on_boundary { outer } else { head };
        let b = if trail_on_boundary { outer } else { tail };
        let shrunk = (len - a - b).max(len.min(1));
        (p0 + a.min(len - shrunk), shrunk)
    };
    let (x, w) = axis(
        cell.x,
        cell.right(),
        cell.x <= usable.x,
        cell.right() >= usable.right(),
    );
    let (y, h) = axis(
        cell.y,
        cell.bottom(),
        cell.y <= usable.y,
        cell.bottom() >= usable.bottom(),
    );
    Rectangle::new((x, y).into(), (w, h).into())
}

/// A cell with what its gaps come from (the snap preview keeps one: its frame depends
/// on the scale it is drawn at).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gapped {
    pub cell: Rect,
    pub usable: Rect,
    pub gaps: Gaps,
}

impl Gapped {
    pub fn logical(&self) -> Rect {
        visible(self.cell, self.usable, self.gaps)
    }

    pub fn frame(&self, scale: f64) -> Frame {
        Frame {
            logical: self.logical(),
            physical: visible_physical(self.cell, self.usable, self.gaps, scale),
            scale,
        }
    }
}

/// A window's rectangle in logical and physical pixels. Rectangles inside the window
/// (its bar, border, buttons, client area, dim) are placed by [`Frame::map`]: each
/// edge is measured from the nearer window edge and that distance rounded, so they
/// line up with the window's physical edges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub logical: Rect,
    pub physical: Rectangle<i32, Physical>,
    pub scale: f64,
}

impl Frame {
    /// A window with no gaps: each edge rounded on its own.
    pub fn plain(rect: Rect, scale: f64) -> Self {
        Self {
            logical: rect,
            physical: to_physical(rect, scale),
            scale,
        }
    }

    /// A gapped window: `gaps::visible` and `visible_physical` of its cell.
    pub fn gapped(cell: Rect, usable: Rect, gaps: Gaps, scale: f64) -> Self {
        Gapped { cell, usable, gaps }.frame(scale)
    }

    fn map_x(&self, v: i32) -> i32 {
        let (l0, l1) = (self.logical.x, self.logical.right());
        let (p0, p1) = (
            self.physical.loc.x,
            self.physical.loc.x + self.physical.size.w,
        );
        map_edge(v, l0, l1, p0, p1, self.scale)
    }

    fn map_y(&self, v: i32) -> i32 {
        let (l0, l1) = (self.logical.y, self.logical.bottom());
        let (p0, p1) = (
            self.physical.loc.y,
            self.physical.loc.y + self.physical.size.h,
        );
        map_edge(v, l0, l1, p0, p1, self.scale)
    }

    /// A logical rectangle inside the window, in physical pixels.
    pub fn map(&self, rect: Rect) -> Rectangle<i32, Physical> {
        let (x0, y0) = (self.map_x(rect.x), self.map_y(rect.y));
        let (x1, y1) = (self.map_x(rect.right()), self.map_y(rect.bottom()));
        Rectangle::new((x0, y0).into(), (x1 - x0, y1 - y0).into())
    }

    /// A logical point inside the window, in physical pixels.
    pub fn point(&self, x: i32, y: i32) -> smithay::utils::Point<i32, Physical> {
        (self.map_x(x), self.map_y(y)).into()
    }
}

/// `v` between logical edges `l0` and `l1`, measured from the nearer one and placed
/// from the matching physical edge.
fn map_edge(v: i32, l0: i32, l1: i32, p0: i32, p1: i32, scale: f64) -> i32 {
    if v - l0 <= l1 - v {
        p0 + physical_edge(v - l0, scale)
    } else {
        p1 - physical_edge(l1 - v, scale)
    }
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
        // Edges 8 apart stay 12 apart at 1.5 and 16 at 2, wherever they are, left of
        // the origin too.
        for x in -50..50 {
            for scale in [1.0, 1.5, 2.0] {
                let a = to_physical(Rect::new(x - 20, 0, 20, 10), scale);
                let b = to_physical(Rect::new(x + 8, 0, 10, 10), scale);
                assert_eq!(b.loc.x - (a.loc.x + a.size.w), (8.0 * scale) as i32);
            }
        }
    }

    const SCALES: [f64; 5] = [1.0, 1.25, 1.5, 1.75, 2.0];

    /// The four outer gaps and the inner gaps of 1 | 2 / 3 in physical pixels.
    fn physical_gaps(area: Rect, scale: f64) -> ([i32; 4], [i32; 2]) {
        let half = area.w / 2;
        let left = Rect::new(area.x, area.y, half, area.h);
        let top = Rect::new(area.x + half, area.y, area.w - half, area.h / 2);
        let bottom = Rect::new(
            area.x + half,
            area.y + area.h / 2,
            area.w - half,
            area.h - area.h / 2,
        );
        let p = |cell| visible_physical(cell, area, Gaps::DEFAULT, scale);
        let (a, b, c) = (p(left), p(top), p(bottom));
        let edge = |v| physical_edge(v, scale);
        let outer = [
            a.loc.x - edge(area.x),
            a.loc.y - edge(area.y),
            edge(area.right()) - (b.loc.x + b.size.w),
            edge(area.bottom()) - (c.loc.y + c.size.h),
        ];
        let inner = [
            b.loc.x - (a.loc.x + a.size.w),
            c.loc.y - (b.loc.y + b.size.h),
        ];
        (outer, inner)
    }

    #[test]
    fn each_gap_is_rounded_once_per_scale() {
        // 13 at 1.5 is 19.5: 20 on every side; 8 at 1.5 is 12.
        assert_eq!(physical_gap(13, 1.5), 20);
        assert_eq!(physical_gap(8, 1.5), 12);
        for scale in SCALES {
            let (outer, inner) = (physical_gap(13, scale), physical_gap(8, scale));
            // Outputs at odd places, left of and above the origin too.
            for (x, y) in [
                (0, 0),
                (1, 3),
                (-1920, 0),
                (-1281, -7),
                (2561, 1441),
                (-3, 5),
            ] {
                for (w, h) in [(1920, 1080), (1001, 777), (5120, 1440), (3441, 1439)] {
                    let area = Rect::new(x, y, w, h);
                    let (o, i) = physical_gaps(area, scale);
                    assert_eq!(o, [outer; 4], "x{scale} {area:?}: outer");
                    assert_eq!(i, [inner; 2], "x{scale} {area:?}: inner");
                }
            }
        }
    }

    #[test]
    fn a_frame_maps_its_own_edges_and_keeps_inner_widths() {
        for scale in SCALES {
            let area = Rect::new(-1281, 3, 1001, 777);
            let frame = Frame::gapped(Rect::new(-1281, 3, 500, 777), area, Gaps::DEFAULT, scale);
            assert_eq!(frame.map(frame.logical), frame.physical, "x{scale}");
            // A 1 px border is the same width on the left and the right.
            let l = frame.logical;
            let left = frame.map(Rect::new(l.x, l.y, 1, l.h));
            let right = frame.map(Rect::new(l.right() - 1, l.y, 1, l.h));
            assert_eq!(left.size.w, right.size.w, "x{scale}");
            assert_eq!(left.size.w, physical_edge(1, scale), "x{scale}");
        }
    }
}
