// SPDX-License-Identifier: Apache-2.0

//! Window layouts as plain data: no Smithay or Wayland types, so they can be tested
//! without a compositor. The compositor maps `WindowId`s to its windows.
//!
//! - `BspTree`: the tiling layout. Holds every window of the monitor as a leaf; only
//!   tiled leaves take space.
//! - `FloatingLayout`: rectangles and stacking order of floating windows.
//! - `MonitorLayout`: combines the two with per-window mode, focus and history.
//! - `layers`: layer-shell surfaces: placement, the usable area, stacking.

mod bsp;
pub mod decor;
mod floating;
#[cfg(test)]
mod gap_cases;
pub mod gaps;
mod geometry;
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the layout from M13b step 2")
)]
pub mod layers;
mod monitor;
#[cfg(test)]
mod proptests;
pub mod shape;
mod snap;

#[cfg(test)]
pub use bsp::MIN_TILE;
#[cfg(any(test, feature = "debug-record"))]
pub use bsp::OutlineNode;
pub use bsp::{BspTree, Side, SplitHandle};
pub use floating::FloatingLayout;
pub use geometry::{Edges, SizeLimits, resize_rect, resize_zone, stop_top_edge};
pub use monitor::{Layer, MonitorLayout, NewWindow, Presentation, STEP, TiledResize};
pub use snap::{ZoneId, snap_to_edges};

/// Identifies a window in a layout. Assigned by the compositor, never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub u64);

/// An axis-aligned rectangle in logical pixels. Width and height are never negative;
/// they can be zero on outputs too small to give every window a pixel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub const fn right(&self) -> i32 {
        self.x + self.w
    }

    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }

    #[cfg(test)]
    pub const fn area(&self) -> i64 {
        self.w as i64 * self.h as i64
    }

    pub const fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// True if the two rectangles share at least one pixel.
    #[cfg(test)]
    pub const fn overlaps(&self, other: &Rect) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// How a window is placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Tiled,
    Floating,
}

/// What a layout may use when a window is inserted.
#[derive(Clone, Copy, Debug, Default)]
pub struct InsertHint {
    /// BSP: the leaf to split (usually the focused window).
    pub next_to: Option<WindowId>,
    /// Floating: the size the client chose.
    pub size: (i32, i32),
    /// Floating: center over this rectangle (a dialog's parent) instead of cascading.
    pub center_over: Option<Rect>,
    /// Floating: the client's min/max size.
    pub limits: SizeLimits,
}

/// The interface shared by the BSP tiling layout and the floating layout.
pub trait Layout {
    /// Sets the area windows are laid out in: the output, in logical pixels.
    fn set_area(&mut self, area: Rect);

    /// Adds a window.
    fn insert(&mut self, window: WindowId, hint: &InsertHint);

    /// Removes a window. Returns false if it is not in the layout.
    fn remove(&mut self, window: WindowId) -> bool;

    fn contains(&self, window: WindowId) -> bool;

    /// The rectangle of every window that currently takes space. For floating windows
    /// the order is the stacking order, bottom first.
    fn rects(&self) -> Vec<(WindowId, Rect)>;
}
