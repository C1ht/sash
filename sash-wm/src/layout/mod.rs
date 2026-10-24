// SPDX-License-Identifier: Apache-2.0

//! Window layouts as plain data: no Smithay or Wayland types, so they can be tested
//! without a compositor. The compositor maps `WindowId`s to its windows.

mod bsp;
#[cfg(test)]
mod proptests;

pub use bsp::BspTree;

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

/// How a window is placed. Only `Tiled` is used until M4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Tiled,
    #[cfg_attr(not(test), expect(dead_code, reason = "floating arrives in M4"))]
    Floating,
}

/// The interface shared by the BSP tiling layout and (from M4) the floating layout.
pub trait Layout {
    /// Sets the area windows are laid out in: the output, in logical pixels.
    fn set_area(&mut self, area: Rect);

    /// Adds a window, which becomes focused.
    fn insert(&mut self, window: WindowId);

    /// Removes a window. Returns false if it is not in the layout.
    fn remove(&mut self, window: WindowId) -> bool;

    fn contains(&self, window: WindowId) -> bool;

    fn focused(&self) -> Option<WindowId>;

    /// Focuses a window. Returns false if it is not in the layout.
    fn focus(&mut self, window: WindowId) -> bool;

    /// The window next to `window` in `direction`, if there is one.
    fn neighbour(&self, window: WindowId, direction: Direction) -> Option<WindowId>;

    /// Exchanges the places of two windows. Returns false if either is missing.
    fn swap(&mut self, a: WindowId, b: WindowId) -> bool;

    /// The rectangle of every window that currently takes space.
    fn rects(&self) -> Vec<(WindowId, Rect)>;
}
