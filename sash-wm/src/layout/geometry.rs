// SPDX-License-Identifier: Apache-2.0

//! Rectangle arithmetic shared by the layouts: size limits, the visibility rule,
//! resize edges and directional neighbours.

use super::{Direction, Rect, WindowId};

/// At least this many pixels of a floating window (or all of it, if smaller) stay on
/// the output on each axis. The top edge has a stricter rule: see `clamp_visible`.
pub const MIN_VISIBLE: i32 = 64;

/// A client's min/max size; 0 on an axis means unconstrained.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SizeLimits {
    pub min: (i32, i32),
    pub max: (i32, i32),
}

impl SizeLimits {
    /// Fixed size: min equals max and both are non-zero.
    pub fn is_fixed(&self) -> bool {
        self.min == self.max && self.min.0 > 0 && self.min.1 > 0
    }

    /// Clamps one axis to [max(min, 1), max] (max 0 = none).
    fn clamp_axis(value: i32, min: i32, max: i32) -> i32 {
        let value = value.max(min).max(1);
        if max > 0 {
            value.min(max.max(min).max(1))
        } else {
            value
        }
    }

    pub fn clamp(&self, (w, h): (i32, i32)) -> (i32, i32) {
        (
            Self::clamp_axis(w, self.min.0, self.max.0),
            Self::clamp_axis(h, self.min.1, self.max.1),
        )
    }
}

/// The size a new floating window gets: the client's choice, no larger than the area,
/// then clamped to its limits (which win if they do not fit the area).
pub fn initial_size(size: (i32, i32), limits: SizeLimits, area: Rect) -> (i32, i32) {
    limits.clamp((size.0.min(area.w), size.1.min(area.h)))
}

/// Moves `rect` just enough that min(MIN_VISIBLE, size) pixels of it stay inside
/// `area` on each axis, and its top edge is not above the top of `area`. The size is
/// kept.
pub fn clamp_visible(rect: Rect, area: Rect) -> Rect {
    let highest = |len: i32, start: i32, area_len: i32| {
        start + area_len - MIN_VISIBLE.min(len).min(area_len).max(0)
    };
    let x_highest = highest(rect.w, area.x, area.w);
    let x_lowest = area.x + MIN_VISIBLE.min(rect.w).min(area.w).max(0) - rect.w;
    let y_highest = highest(rect.h, area.y, area.h).max(area.y);
    Rect::new(
        rect.x.clamp(x_lowest.min(x_highest), x_highest),
        rect.y.clamp(area.y, y_highest),
        rect.w,
        rect.h,
    )
}

/// A resize that moves the top edge stops at the top of `area`: the window gets shorter
/// instead (down to its min height; `clamp_visible` then moves what is left).
pub fn stop_top_edge(rect: Rect, edges: Edges, area: Rect, limits: SizeLimits) -> Rect {
    if !edges.top || edges.bottom || rect.y >= area.y {
        return rect;
    }
    let (_, h) = limits.clamp((rect.w, rect.bottom() - area.y));
    Rect::new(rect.x, rect.bottom() - h, rect.w, h)
}

/// True if `rect` satisfies the visibility rule in `area`.
#[cfg(test)]
pub fn is_visible_enough(rect: Rect, area: Rect) -> bool {
    clamp_visible(rect, area) == rect
}

/// Which edges a resize moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Edges {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl Edges {
    pub const NONE: Edges = Edges {
        left: false,
        right: false,
        top: false,
        bottom: false,
    };

    /// All four edges: a move.
    pub const ALL: Edges = Edges {
        left: true,
        right: true,
        top: true,
        bottom: true,
    };

    pub const BOTTOM_RIGHT: Edges = Edges {
        left: false,
        right: true,
        top: false,
        bottom: true,
    };
}

/// The edges to resize when a resize starts at (x, y): the cell of a 3x3 grid over the
/// window. Corner cells give corners, side cells give that edge, the center cell gives
/// the bottom-right corner.
pub fn resize_zone(rect: Rect, x: i32, y: i32) -> Edges {
    // Cell index 0, 1, 2 along an axis; points outside count as the nearest cell.
    let cell = |pos: i32, start: i32, len: i32| {
        let offset = (pos - start) as i64 * 3;
        if offset < len as i64 {
            0
        } else if offset < 2 * len as i64 {
            1
        } else {
            2
        }
    };
    let (column, row) = (cell(x, rect.x, rect.w), cell(y, rect.y, rect.h));
    if (column, row) == (1, 1) {
        return Edges::BOTTOM_RIGHT;
    }
    Edges {
        left: column == 0,
        right: column == 2,
        top: row == 0,
        bottom: row == 2,
    }
}

/// `start` resized by moving `edges` by (dx, dy), clamped to `limits`. The edges that
/// do not move stay where they were.
pub fn resize_rect(start: Rect, edges: Edges, dx: i32, dy: i32, limits: SizeLimits) -> Rect {
    let mut w = start.w;
    let mut h = start.h;
    if edges.left {
        w -= dx;
    } else if edges.right {
        w += dx;
    }
    if edges.top {
        h -= dy;
    } else if edges.bottom {
        h += dy;
    }
    let (w, h) = limits.clamp((w, h));
    let x = if edges.left {
        start.right() - w
    } else {
        start.x
    };
    let y = if edges.top {
        start.bottom() - h
    } else {
        start.y
    };
    Rect::new(x, y, w, h)
}

/// The window to move to from `from` in `direction`: candidates whose center lies past
/// `from`'s center in that direction and whose perpendicular span overlaps `from`'s.
/// Nearest by edge gap (overlap counts as 0), then most recently focused, then
/// position (topmost for left/right, leftmost for up/down).
pub fn pick_in_direction(
    from: Rect,
    candidates: impl IntoIterator<Item = (WindowId, Rect)>,
    direction: Direction,
    recency: impl Fn(WindowId) -> u64,
) -> Option<WindowId> {
    // Doubled centers avoid halves.
    let center_x = |r: &Rect| 2 * r.x + r.w;
    let center_y = |r: &Rect| 2 * r.y + r.h;
    let spans_overlap = |a0: i32, a1: i32, b0: i32, b1: i32| a0 < b1 && b0 < a1;
    candidates
        .into_iter()
        .filter_map(|(window, to)| {
            let (beyond, overlap, gap) = match direction {
                Direction::Right => (
                    center_x(&to) > center_x(&from),
                    spans_overlap(from.y, from.bottom(), to.y, to.bottom()),
                    to.x - from.right(),
                ),
                Direction::Left => (
                    center_x(&to) < center_x(&from),
                    spans_overlap(from.y, from.bottom(), to.y, to.bottom()),
                    from.x - to.right(),
                ),
                Direction::Down => (
                    center_y(&to) > center_y(&from),
                    spans_overlap(from.x, from.right(), to.x, to.right()),
                    to.y - from.bottom(),
                ),
                Direction::Up => (
                    center_y(&to) < center_y(&from),
                    spans_overlap(from.x, from.right(), to.x, to.right()),
                    from.y - to.bottom(),
                ),
            };
            (beyond && overlap).then_some((window, to, gap.max(0)))
        })
        .min_by_key(|&(window, to, gap)| {
            let position = match direction {
                Direction::Left | Direction::Right => (to.y, to.x),
                Direction::Up | Direction::Down => (to.x, to.y),
            };
            (gap, std::cmp::Reverse(recency(window)), position)
        })
        .map(|(window, _, _)| window)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0, 0, 800, 600);

    #[test]
    fn limits_clamp_each_axis() {
        let limits = SizeLimits {
            min: (100, 0),
            max: (300, 200),
        };
        assert_eq!(limits.clamp((50, 50)), (100, 50));
        assert_eq!(limits.clamp((500, 500)), (300, 200));
        assert_eq!(
            SizeLimits::default().clamp((0, -3)),
            (1, 1),
            "never below one pixel"
        );
        // A max below the min is a protocol error elsewhere; here the min wins.
        let crossed = SizeLimits {
            min: (200, 200),
            max: (100, 100),
        };
        assert_eq!(crossed.clamp((150, 150)), (200, 200));
        assert!(
            SizeLimits {
                min: (10, 20),
                max: (10, 20)
            }
            .is_fixed()
        );
        assert!(
            !SizeLimits {
                min: (0, 0),
                max: (0, 0)
            }
            .is_fixed()
        );
        assert!(
            !SizeLimits {
                min: (10, 20),
                max: (10, 30)
            }
            .is_fixed()
        );
    }

    #[test]
    fn initial_size_fits_the_area_unless_limits_forbid() {
        assert_eq!(
            initial_size((1000, 300), SizeLimits::default(), AREA),
            (800, 300)
        );
        let big_fixed = SizeLimits {
            min: (1000, 900),
            max: (1000, 900),
        };
        assert_eq!(initial_size((1000, 900), big_fixed, AREA), (1000, 900));
    }

    #[test]
    fn visibility_rule() {
        // Inside: unchanged.
        let r = Rect::new(100, 100, 200, 200);
        assert_eq!(clamp_visible(r, AREA), r);
        // Far right / bottom: pulled back so 64 px stay.
        assert_eq!(
            clamp_visible(Rect::new(5000, 5000, 200, 200), AREA),
            Rect::new(736, 536, 200, 200)
        );
        // Far left: 64 px of the right part stay. Above the top: the top edge stops
        // at the top of the area.
        assert_eq!(
            clamp_visible(Rect::new(-5000, -5000, 200, 200), AREA),
            Rect::new(-136, 0, 200, 200)
        );
        assert_eq!(
            clamp_visible(Rect::new(100, -1, 200, 200), AREA),
            Rect::new(100, 0, 200, 200)
        );
        // An area that does not start at 0 (a usable area below a panel).
        let below_panel = Rect::new(0, 30, 800, 570);
        assert_eq!(
            clamp_visible(Rect::new(10, 0, 200, 200), below_panel),
            Rect::new(10, 30, 200, 200)
        );
        // Smaller than 64 px: the whole window stays.
        assert_eq!(
            clamp_visible(Rect::new(-50, 590, 30, 20), AREA),
            Rect::new(0, 580, 30, 20)
        );
        // Larger than the area: still only 64 px are required, except at the top.
        assert!(is_visible_enough(Rect::new(-1900, 0, 2000, 2000), AREA));
        assert_eq!(
            clamp_visible(Rect::new(-1900, -1900, 2000, 2000), AREA),
            Rect::new(-1900, 0, 2000, 2000)
        );
        // Area smaller than 64 px: the window must cover what it can.
        let tiny = Rect::new(0, 0, 10, 10);
        assert_eq!(
            clamp_visible(Rect::new(300, 300, 100, 100), tiny),
            Rect::new(0, 0, 100, 100)
        );
        assert_eq!(
            clamp_visible(Rect::new(-300, -300, 100, 100), tiny),
            Rect::new(-90, 0, 100, 100)
        );
    }

    #[test]
    fn three_by_three_zones() {
        let r = Rect::new(0, 0, 300, 300);
        let at = |x, y| resize_zone(r, x, y);
        let e = |left, right, top, bottom| Edges {
            left,
            right,
            top,
            bottom,
        };
        assert_eq!(at(10, 10), e(true, false, true, false), "top-left");
        assert_eq!(at(150, 10), e(false, false, true, false), "top");
        assert_eq!(at(290, 10), e(false, true, true, false), "top-right");
        assert_eq!(at(10, 150), e(true, false, false, false), "left");
        assert_eq!(
            at(150, 150),
            Edges::BOTTOM_RIGHT,
            "center uses bottom-right"
        );
        assert_eq!(at(290, 150), e(false, true, false, false), "right");
        assert_eq!(at(10, 290), e(true, false, false, true), "bottom-left");
        assert_eq!(at(150, 290), e(false, false, false, true), "bottom");
        assert_eq!(at(290, 290), e(false, true, false, true), "bottom-right");
        // Cell borders: 100 is the start of the middle column, 200 of the right one.
        assert_eq!(at(99, 150), e(true, false, false, false));
        assert_eq!(at(100, 150), Edges::BOTTOM_RIGHT);
        assert_eq!(at(200, 150), e(false, true, false, false));
    }

    #[test]
    fn resizing_keeps_the_opposite_edges() {
        let start = Rect::new(100, 100, 300, 200);
        let none = SizeLimits::default();
        let tl = Edges {
            left: true,
            top: true,
            ..Default::default()
        };
        assert_eq!(
            resize_rect(start, tl, -50, -20, none),
            Rect::new(50, 80, 350, 220)
        );
        assert_eq!(
            resize_rect(start, Edges::BOTTOM_RIGHT, 50, 20, none),
            Rect::new(100, 100, 350, 220)
        );
        let limits = SizeLimits {
            min: (250, 150),
            max: (320, 0),
        };
        // Shrinking from the left stops at the min width; the right edge stays at 400.
        assert_eq!(
            resize_rect(start, tl, 200, 200, limits),
            Rect::new(150, 150, 250, 150)
        );
        // Growing to the right stops at the max width.
        assert_eq!(
            resize_rect(start, Edges::BOTTOM_RIGHT, 500, 0, limits),
            Rect::new(100, 100, 320, 200)
        );
        // Moving only the top edge leaves the width alone.
        let top = Edges {
            top: true,
            ..Default::default()
        };
        assert_eq!(
            resize_rect(start, top, 999, -30, none),
            Rect::new(100, 70, 300, 230)
        );
    }

    #[test]
    fn a_top_edge_resize_stops_at_the_top_of_the_area() {
        let none = SizeLimits::default();
        let top = Edges {
            top: true,
            ..Default::default()
        };
        let top_left = Edges {
            left: true,
            top: true,
            ..Default::default()
        };
        // Pulled 50 px above the area: cut at the top, the bottom edge stays.
        assert_eq!(
            stop_top_edge(Rect::new(100, -50, 300, 250), top, AREA, none),
            Rect::new(100, 0, 300, 200)
        );
        assert_eq!(
            stop_top_edge(Rect::new(80, -50, 320, 250), top_left, AREA, none),
            Rect::new(80, 0, 320, 200)
        );
        // Inside the area, a bottom resize, or a move: unchanged.
        let inside = Rect::new(100, 10, 300, 250);
        assert_eq!(stop_top_edge(inside, top, AREA, none), inside);
        let above = Rect::new(100, -50, 300, 250);
        assert_eq!(stop_top_edge(above, Edges::BOTTOM_RIGHT, AREA, none), above);
        assert_eq!(stop_top_edge(above, Edges::ALL, AREA, none), above);
        // A min height larger than what fits keeps the min height; clamp_visible then
        // moves the window down.
        let tall = SizeLimits {
            min: (0, 280),
            max: (0, 0),
        };
        let cut = stop_top_edge(Rect::new(100, -100, 300, 300), top, AREA, tall);
        assert_eq!(cut, Rect::new(100, -80, 300, 280));
        assert_eq!(clamp_visible(cut, AREA), Rect::new(100, 0, 300, 280));
    }

    #[test]
    fn direction_uses_centers_and_overlapping_spans() {
        let from = Rect::new(300, 200, 200, 200);
        let w = |n| WindowId(n);
        let candidates = [
            (w(1), Rect::new(600, 250, 100, 100)), // right, gap 100
            (w(2), Rect::new(450, 150, 200, 100)), // right, overlaps from (gap 0)
            (w(3), Rect::new(600, 500, 100, 100)), // right but no vertical overlap
            (w(4), Rect::new(0, 0, 250, 800)),     // left
        ];
        let no_history = |_| 0;
        assert_eq!(
            pick_in_direction(from, candidates, Direction::Right, no_history),
            Some(w(2))
        );
        assert_eq!(
            pick_in_direction(from, candidates, Direction::Left, no_history),
            Some(w(4))
        );
        assert_eq!(
            pick_in_direction(from, candidates, Direction::Down, no_history),
            None
        );
        // A window centered on `from` is in no direction.
        let same_center = [(w(5), Rect::new(350, 250, 100, 100))];
        for d in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            assert_eq!(pick_in_direction(from, same_center, d, no_history), None);
        }
        // Equal gaps: most recent wins, then the topmost.
        let tie = [
            (w(6), Rect::new(500, 300, 100, 100)),
            (w(7), Rect::new(500, 200, 100, 100)),
        ];
        assert_eq!(
            pick_in_direction(from, tie, Direction::Right, no_history),
            Some(w(7))
        );
        assert_eq!(
            pick_in_direction(from, tie, Direction::Right, |id| (id == w(6)) as u64),
            Some(w(6))
        );
    }
}
