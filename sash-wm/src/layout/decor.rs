// SPDX-License-Identifier: Apache-2.0

//! Server-side decorations as geometry: the bar and border inside a window's outer
//! rectangle, the client's rectangle that is left, and what a point on them means
//! (a button, the bar, a resize zone). No drawing and no Smithay types here.
//!
//! The layout's rectangles are outer rectangles: tiling coverage, snap zones and the
//! visibility rules apply to them; the client gets the outer rectangle minus the bar
//! and the borders.

use super::{Edges, Rect, SizeLimits};

/// Height of the bar.
pub const BAR: i32 = 32;
/// Width of the border on every side.
pub const BORDER: i32 = 1;
/// A button's hit area is BUTTON x BUTTON, starting at the bar's corner.
pub const BUTTON: i32 = 32;
/// Size of the glyph drawn in a button.
pub const GLYPH: i32 = 12;
/// A resize zone reaches this far inside the border.
pub const ZONE: i32 = 6;
/// A corner zone is CORNER x CORNER from the outer corner.
pub const CORNER: i32 = 16;

/// Which end of the bar holds the buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the other value of the BUTTON_SIDE constant")
    )]
    Right,
}

pub const BUTTON_SIDE: Side = Side::Left;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Close,
    Maximize,
}

/// The buttons, starting from BUTTON_SIDE's end of the bar.
pub const BUTTONS: [Button; 2] = [Button::Close, Button::Maximize];

/// What a window is decorated with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Decoration {
    pub bar: bool,
    pub border: bool,
}

/// What a point on the decorations means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Button(Button),
    /// The bar outside the buttons: a press moves the window.
    Bar,
    /// A resize zone: the edges a press resizes.
    Resize(Edges),
    /// The border where no zone is (an edge on the output boundary of a tile).
    Border,
}

impl Decoration {
    pub const NONE: Decoration = Decoration {
        bar: false,
        border: false,
    };
    pub const BORDER: Decoration = Decoration {
        bar: false,
        border: true,
    };
    pub const FULL: Decoration = Decoration {
        bar: true,
        border: true,
    };

    /// Space taken on the left, top, right and bottom.
    pub fn insets(self) -> (i32, i32, i32, i32) {
        let b = if self.border { BORDER } else { 0 };
        let bar = if self.bar { BAR } else { 0 };
        (b, b + bar, b, b)
    }

    /// The client's rectangle inside `outer` (never negative; empty if the outer
    /// rectangle is too small for the decorations).
    pub fn inner(self, outer: Rect) -> Rect {
        let (l, t, r, b) = self.insets();
        let w = (outer.w - l - r).max(0);
        let h = (outer.h - t - b).max(0);
        Rect::new(outer.x + l, outer.y + t, w, h)
    }

    /// The outer size around a client size.
    pub fn outer_size(self, (w, h): (i32, i32)) -> (i32, i32) {
        let (l, t, r, b) = self.insets();
        (w + l + r, h + t + b)
    }

    /// The client's size inside an outer size, at least 1x1 (0 would let the client
    /// choose).
    pub fn client_size(self, (w, h): (i32, i32)) -> (i32, i32) {
        let (l, t, r, b) = self.insets();
        ((w - l - r).max(1), (h - t - b).max(1))
    }

    /// Outer limits for a client's limits: the client keeps at least 1x1, and a
    /// maximum (0 = none) grows by the decorations.
    pub fn outer_limits(self, limits: SizeLimits) -> SizeLimits {
        let (dw, dh) = self.outer_size((0, 0));
        let grow = |v: i32, d: i32| if v > 0 { v + d } else { 0 };
        SizeLimits {
            min: (limits.min.0.max(1) + dw, limits.min.1.max(1) + dh),
            max: (grow(limits.max.0, dw), grow(limits.max.1, dh)),
        }
    }

    /// The bar's rectangle (inside the borders).
    pub fn bar_rect(self, outer: Rect) -> Option<Rect> {
        if !self.bar {
            return None;
        }
        let b = if self.border { BORDER } else { 0 };
        Some(Rect::new(
            outer.x + b,
            outer.y + b,
            (outer.w - 2 * b).max(0),
            BAR.min((outer.h - b).max(0)),
        ))
    }

    /// A button's hit area, clipped to the bar.
    pub fn button_rect(self, outer: Rect, button: Button) -> Option<Rect> {
        self.button_rect_on(outer, button, BUTTON_SIDE)
    }

    fn button_rect_on(self, outer: Rect, button: Button, side: Side) -> Option<Rect> {
        let bar = self.bar_rect(outer)?;
        let index = BUTTONS.iter().position(|b| *b == button)? as i32;
        let x = match side {
            Side::Left => bar.x + index * BUTTON,
            Side::Right => bar.right() - (index + 1) * BUTTON,
        };
        let left = x.max(bar.x);
        let right = (x + BUTTON).min(bar.right());
        (right > left).then(|| Rect::new(left, bar.y, right - left, bar.h.min(BUTTON)))
    }

    /// What (x, y) is on the decorations of a window at `outer`; None in the client's
    /// rectangle or outside the window. `zones` are the edges that have resize zones (a
    /// tile has none on the output boundary). Points outside `area` (the output) hit
    /// nothing, so zones are clamped to the output. Buttons win over zones.
    pub fn hit(self, outer: Rect, zones: Edges, area: Rect, x: i32, y: i32) -> Option<Part> {
        if self == Decoration::NONE || !outer.contains(x, y) || !area.contains(x, y) {
            return None;
        }
        for button in BUTTONS {
            if self
                .button_rect(outer, button)
                .is_some_and(|r| r.contains(x, y))
            {
                return Some(Part::Button(button));
            }
        }
        let reach = if self.border { BORDER + ZONE } else { ZONE };
        let near = |pos: i32, start: i32, end: i32, size: i32| {
            (
                pos - start < size.min(end - start),
                end - 1 - pos < size.min(end - start),
            )
        };
        let (left, right) = near(x, outer.x, outer.right(), reach);
        let (top, bottom) = near(y, outer.y, outer.bottom(), reach);
        let (corner_l, corner_r) = near(x, outer.x, outer.right(), CORNER);
        let (corner_t, corner_b) = near(y, outer.y, outer.bottom(), CORNER);
        // In a corner square the zone is the corner's two edges (those that have
        // zones); elsewhere the strip along one edge.
        let in_corner = (corner_l || corner_r) && (corner_t || corner_b);
        let edges = if in_corner {
            Edges {
                left: corner_l && zones.left,
                right: corner_r && !corner_l && zones.right,
                top: corner_t && zones.top,
                bottom: corner_b && !corner_t && zones.bottom,
            }
        } else {
            Edges {
                left: left && zones.left,
                right: right && !left && zones.right,
                top: top && zones.top,
                bottom: bottom && !top && zones.bottom,
            }
        };
        // A corner whose edges have no zones falls back to the strip it is in.
        let edges = if edges == Edges::default() && in_corner {
            Edges {
                left: left && zones.left,
                right: right && !left && zones.right,
                top: top && zones.top,
                bottom: bottom && !top && zones.bottom,
            }
        } else {
            edges
        };
        if edges != Edges::default() {
            return Some(Part::Resize(edges));
        }
        if self.bar_rect(outer).is_some_and(|r| r.contains(x, y)) {
            return Some(Part::Bar);
        }
        (!self.inner(outer).contains(x, y)).then_some(Part::Border)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0, 0, 1000, 800);
    const OUTER: Rect = Rect::new(100, 100, 400, 300);

    fn edges(l: bool, t: bool, r: bool, b: bool) -> Edges {
        Edges {
            left: l,
            right: r,
            top: t,
            bottom: b,
        }
    }

    #[test]
    fn inner_rect_and_sizes() {
        assert_eq!(Decoration::FULL.inner(OUTER), Rect::new(101, 133, 398, 266));
        assert_eq!(
            Decoration::BORDER.inner(OUTER),
            Rect::new(101, 101, 398, 298)
        );
        assert_eq!(Decoration::NONE.inner(OUTER), OUTER);
        assert_eq!(Decoration::FULL.outer_size((398, 266)), (400, 300));
        assert_eq!(Decoration::FULL.client_size((400, 300)), (398, 266));
        // Barely larger than bar plus borders, and smaller: the client gets 1x1 at least.
        assert_eq!(Decoration::FULL.client_size((3, 35)), (1, 1));
        assert_eq!(Decoration::FULL.client_size((2, 34)), (1, 1));
        assert_eq!(Decoration::FULL.inner(Rect::new(0, 0, 2, 20)).w, 0);
        let limits = SizeLimits {
            min: (0, 100),
            max: (500, 0),
        };
        assert_eq!(
            Decoration::FULL.outer_limits(limits),
            SizeLimits {
                min: (3, 134),
                max: (502, 0)
            }
        );
        // A fixed size stays fixed.
        let fixed = SizeLimits {
            min: (300, 200),
            max: (300, 200),
        };
        assert!(Decoration::FULL.outer_limits(fixed).is_fixed());
    }

    #[test]
    fn buttons_on_the_left_in_order() {
        let d = Decoration::FULL;
        assert_eq!(
            d.button_rect(OUTER, Button::Close),
            Some(Rect::new(101, 101, 32, 32))
        );
        assert_eq!(
            d.button_rect(OUTER, Button::Maximize),
            Some(Rect::new(133, 101, 32, 32))
        );
        assert_eq!(Decoration::BORDER.button_rect(OUTER, Button::Close), None);
        // With the side constant set to Right: close in the right corner, then maximize.
        assert_eq!(
            d.button_rect_on(OUTER, Button::Close, Side::Right),
            Some(Rect::new(467, 101, 32, 32))
        );
        assert_eq!(
            d.button_rect_on(OUTER, Button::Maximize, Side::Right),
            Some(Rect::new(435, 101, 32, 32))
        );
        // A window narrower than two buttons clips the second.
        let narrow = Rect::new(0, 0, 50, 100);
        assert_eq!(
            d.button_rect(narrow, Button::Maximize),
            Some(Rect::new(33, 1, 16, 32))
        );
    }

    /// Every zone and button of a floating window, at its corners and edges.
    #[test]
    fn hit_every_zone_and_button() {
        let d = Decoration::FULL;
        let all = Edges::ALL;
        let hit = |x, y| d.hit(OUTER, all, AREA, x, y);
        // Buttons win over the top-left corner and the top zone.
        assert_eq!(hit(101, 101), Some(Part::Button(Button::Close)));
        assert_eq!(hit(110, 105), Some(Part::Button(Button::Close)));
        assert_eq!(hit(132, 132), Some(Part::Button(Button::Close)));
        assert_eq!(hit(133, 101), Some(Part::Button(Button::Maximize)));
        assert_eq!(hit(164, 132), Some(Part::Button(Button::Maximize)));
        // The outer border pixel at the top-left is not a button: it is the corner.
        assert_eq!(
            hit(100, 100),
            Some(Part::Resize(edges(true, true, false, false)))
        );
        assert_eq!(
            hit(100, 115),
            Some(Part::Resize(edges(true, true, false, false)))
        );
        // Left zone below the corner square: border plus 6 px.
        assert_eq!(
            hit(100, 200),
            Some(Part::Resize(edges(true, false, false, false)))
        );
        assert_eq!(
            hit(106, 200),
            Some(Part::Resize(edges(true, false, false, false)))
        );
        assert_eq!(hit(107, 200), None, "client area");
        // Top zone right of the buttons, then the bar below it.
        assert_eq!(
            hit(300, 100),
            Some(Part::Resize(edges(false, true, false, false)))
        );
        assert_eq!(
            hit(300, 106),
            Some(Part::Resize(edges(false, true, false, false)))
        );
        assert_eq!(hit(300, 107), Some(Part::Bar));
        assert_eq!(hit(300, 132), Some(Part::Bar));
        assert_eq!(hit(300, 133), None, "client area under the bar");
        // The other three corners and edges.
        assert_eq!(
            hit(499, 100),
            Some(Part::Resize(edges(false, true, true, false)))
        );
        assert_eq!(
            hit(484, 115),
            Some(Part::Resize(edges(false, true, true, false)))
        );
        assert_eq!(
            hit(483, 106),
            Some(Part::Resize(edges(false, true, false, false)))
        );
        assert_eq!(
            hit(499, 200),
            Some(Part::Resize(edges(false, false, true, false)))
        );
        assert_eq!(
            hit(493, 200),
            Some(Part::Resize(edges(false, false, true, false)))
        );
        assert_eq!(hit(492, 200), None);
        assert_eq!(
            hit(100, 399),
            Some(Part::Resize(edges(true, false, false, true)))
        );
        assert_eq!(
            hit(499, 399),
            Some(Part::Resize(edges(false, false, true, true)))
        );
        assert_eq!(
            hit(484, 384),
            Some(Part::Resize(edges(false, false, true, true)))
        );
        assert_eq!(
            hit(300, 393),
            Some(Part::Resize(edges(false, false, false, true)))
        );
        assert_eq!(hit(300, 392), None);
        // Outside the window.
        assert_eq!(hit(99, 200), None);
        assert_eq!(hit(500, 200), None);
        assert_eq!(hit(300, 400), None);
    }

    /// A tile: no bar; zones only on the edges that are not on the output boundary; the
    /// border there is still decoration (never the client's).
    #[test]
    fn hit_a_tile_with_edges_on_the_output_boundary() {
        let d = Decoration::BORDER;
        let tile = Rect::new(0, 0, 500, 400);
        let zones = edges(false, false, true, true);
        let hit = |x, y| d.hit(tile, zones, AREA, x, y);
        assert_eq!(
            hit(0, 200),
            Some(Part::Border),
            "left border on the output edge"
        );
        assert_eq!(hit(1, 200), None, "no zone there: the client's");
        assert_eq!(hit(250, 0), Some(Part::Border));
        assert_eq!(
            hit(499, 200),
            Some(Part::Resize(edges(false, false, true, false)))
        );
        assert_eq!(
            hit(250, 393),
            Some(Part::Resize(edges(false, false, false, true)))
        );
        assert_eq!(
            hit(499, 399),
            Some(Part::Resize(edges(false, false, true, true)))
        );
        // Corners where one edge is on the boundary give the other edge only.
        assert_eq!(
            hit(0, 399),
            Some(Part::Resize(edges(false, false, false, true)))
        );
        assert_eq!(
            hit(499, 0),
            Some(Part::Resize(edges(false, false, true, false)))
        );
        assert_eq!(hit(5, 5), None);
    }

    /// A window partly off the output: zones there are out of reach; points outside the
    /// output hit nothing.
    #[test]
    fn zones_are_clamped_to_the_output() {
        let d = Decoration::FULL;
        let outer = Rect::new(-200, 600, 400, 300);
        assert_eq!(d.hit(outer, Edges::ALL, AREA, -195, 700), None);
        assert_eq!(
            d.hit(outer, Edges::ALL, AREA, 199, 700),
            Some(Part::Resize(edges(false, false, true, false)))
        );
        assert_eq!(d.hit(outer, Edges::ALL, AREA, 0, 799), None);
        // At the output's right edge.
        let outer = Rect::new(600, 100, 400, 300);
        assert_eq!(
            d.hit(outer, Edges::ALL, AREA, 999, 200),
            Some(Part::Resize(edges(false, false, true, false)))
        );
    }

    /// A window barely larger than its decorations still has its buttons; zones and
    /// buttons never panic on tiny or empty rectangles.
    #[test]
    fn tiny_windows() {
        let d = Decoration::FULL;
        for (w, h) in [
            (0, 0),
            (1, 1),
            (2, 34),
            (3, 35),
            (10, 10),
            (40, 40),
            (70, 34),
        ] {
            let outer = Rect::new(10, 10, w, h);
            for x in 5..90 {
                for y in 5..60 {
                    let _ = d.hit(outer, Edges::ALL, AREA, x, y);
                }
            }
        }
        assert_eq!(
            d.hit(Rect::new(0, 0, 3, 35), Edges::ALL, AREA, 1, 20),
            Some(Part::Button(Button::Close))
        );
    }
}
