// SPDX-License-Identifier: Apache-2.0

//! The floating layout: one rectangle per window, in stacking order.

use super::{
    InsertHint, Layout, Rect, WindowId,
    geometry::{clamp_visible, initial_size},
};

/// Offset between cascaded new windows, on both axes.
pub const CASCADE_STEP: i32 = 32;

#[derive(Clone, Debug, PartialEq)]
pub struct FloatingLayout {
    area: Rect,
    /// Bottom first.
    stack: Vec<(WindowId, Rect)>,
    /// Cascade steps of the last cascaded window.
    cascade: i32,
}

/// `size` centered over `over`.
fn centered(size: (i32, i32), over: Rect) -> Rect {
    Rect::new(
        over.x + (over.w - size.0).div_euclid(2),
        over.y + (over.h - size.1).div_euclid(2),
        size.0,
        size.1,
    )
}

impl FloatingLayout {
    pub fn new(area: Rect) -> Self {
        Self {
            area,
            stack: Vec::new(),
            cascade: 0,
        }
    }

    pub fn rect_of(&self, window: WindowId) -> Option<Rect> {
        self.stack
            .iter()
            .find(|(w, _)| *w == window)
            .map(|(_, r)| *r)
    }

    /// Adds `window` at `rect` (moved to satisfy the visibility rule) on top.
    pub fn insert_at(&mut self, window: WindowId, rect: Rect) {
        debug_assert!(!self.contains(window), "{window:?} inserted twice");
        self.stack.push((window, clamp_visible(rect, self.area)));
    }

    /// Moves or resizes a window; the visibility rule applies. Returns the result.
    pub fn set_rect(&mut self, window: WindowId, rect: Rect) -> Option<Rect> {
        let area = self.area;
        let entry = self.stack.iter_mut().find(|(w, _)| *w == window)?;
        entry.1 = clamp_visible(rect, area);
        Some(entry.1)
    }

    /// Puts `window` on top of the stack.
    pub fn raise(&mut self, window: WindowId) -> bool {
        let Some(index) = self.stack.iter().position(|(w, _)| *w == window) else {
            return false;
        };
        let entry = self.stack.remove(index);
        self.stack.push(entry);
        true
    }

    /// The topmost window containing the point.
    #[cfg(test)]
    pub fn window_at(&self, x: i32, y: i32) -> Option<WindowId> {
        self.stack
            .iter()
            .rev()
            .find(|(_, r)| r.contains(x, y))
            .map(|(w, _)| *w)
    }

    /// Where a new window of `size` goes: centered on the output for the first window,
    /// then cascading by CASCADE_STEP, back to the center when it would leave the output.
    fn cascade_position(&mut self, size: (i32, i32)) -> Rect {
        let center = centered(size, self.area);
        let steps = if self.stack.is_empty() {
            0
        } else {
            self.cascade + 1
        };
        let candidate = Rect::new(
            center.x + steps * CASCADE_STEP,
            center.y + steps * CASCADE_STEP,
            size.0,
            size.1,
        );
        let leaves =
            candidate.right() > self.area.right() || candidate.bottom() > self.area.bottom();
        if leaves || steps == 0 {
            self.cascade = 0;
            center
        } else {
            self.cascade = steps;
            candidate
        }
    }
}

impl Layout for FloatingLayout {
    /// Changing the area (an output resize) moves windows back by the visibility rule.
    fn set_area(&mut self, area: Rect) {
        self.area = area;
        for (_, rect) in &mut self.stack {
            *rect = clamp_visible(*rect, area);
        }
    }

    /// Places a new window on top: the client's size (clamped to its limits and the
    /// output), centered over `hint.center_over` or cascaded.
    fn insert(&mut self, window: WindowId, hint: &InsertHint) {
        let size = initial_size(hint.size, hint.limits, self.area);
        let rect = match hint.center_over {
            Some(over) => centered(size, over),
            None => self.cascade_position(size),
        };
        self.insert_at(window, rect);
    }

    fn remove(&mut self, window: WindowId) -> bool {
        let before = self.stack.len();
        self.stack.retain(|(w, _)| *w != window);
        self.stack.len() != before
    }

    fn contains(&self, window: WindowId) -> bool {
        self.stack.iter().any(|(w, _)| *w == window)
    }

    fn rects(&self) -> Vec<(WindowId, Rect)> {
        self.stack.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{SizeLimits, geometry::is_visible_enough};

    const AREA: Rect = Rect::new(0, 0, 800, 600);

    fn id(n: u64) -> WindowId {
        WindowId(n)
    }

    fn hint(size: (i32, i32)) -> InsertHint {
        InsertHint {
            size,
            ..Default::default()
        }
    }

    #[test]
    fn first_window_is_centered_then_cascades_and_wraps() {
        let mut layout = FloatingLayout::new(AREA);
        layout.insert(id(1), &hint((400, 300)));
        assert_eq!(layout.rect_of(id(1)), Some(Rect::new(200, 150, 400, 300)));
        layout.insert(id(2), &hint((400, 300)));
        assert_eq!(layout.rect_of(id(2)), Some(Rect::new(232, 182, 400, 300)));
        layout.insert(id(3), &hint((400, 300)));
        assert_eq!(layout.rect_of(id(3)), Some(Rect::new(264, 214, 400, 300)));
        // 200 + 32 * n + 400 <= 800 and 150 + 32 * n + 300 <= 600 hold up to n = 4.
        layout.insert(id(4), &hint((400, 300)));
        layout.insert(id(5), &hint((400, 300)));
        assert_eq!(layout.rect_of(id(5)), Some(Rect::new(328, 278, 400, 300)));
        // n = 5 would leave the output at the bottom: back to the center.
        layout.insert(id(6), &hint((400, 300)));
        assert_eq!(layout.rect_of(id(6)), Some(Rect::new(200, 150, 400, 300)));
        layout.insert(id(7), &hint((400, 300)));
        assert_eq!(layout.rect_of(id(7)), Some(Rect::new(232, 182, 400, 300)));
    }

    #[test]
    fn cascade_restarts_at_the_center_when_no_floating_window_is_left() {
        let mut layout = FloatingLayout::new(AREA);
        layout.insert(id(1), &hint((100, 100)));
        layout.insert(id(2), &hint((100, 100)));
        layout.remove(id(1));
        layout.remove(id(2));
        layout.insert(id(3), &hint((100, 100)));
        assert_eq!(layout.rect_of(id(3)), Some(Rect::new(350, 250, 100, 100)));
    }

    #[test]
    fn size_is_clamped_to_output_then_limits() {
        let mut layout = FloatingLayout::new(AREA);
        layout.insert(id(1), &hint((2000, 100)));
        assert_eq!(layout.rect_of(id(1)), Some(Rect::new(0, 250, 800, 100)));
        let fixed_big = InsertHint {
            size: (1000, 900),
            limits: SizeLimits {
                min: (1000, 900),
                max: (1000, 900),
            },
            ..Default::default()
        };
        layout.insert(id(2), &fixed_big);
        // Larger than the output: centered, overhanging on all sides.
        assert_eq!(
            layout.rect_of(id(2)),
            Some(Rect::new(-100, -150, 1000, 900))
        );
        let min_limited = InsertHint {
            size: (10, 10),
            limits: SizeLimits {
                min: (200, 150),
                max: (0, 0),
            },
            ..Default::default()
        };
        layout.insert(id(3), &min_limited);
        assert_eq!(layout.rect_of(id(3)).map(|r| (r.w, r.h)), Some((200, 150)));
    }

    #[test]
    fn dialog_is_centered_over_its_parent_and_kept_visible() {
        let mut layout = FloatingLayout::new(AREA);
        let parent = Rect::new(400, 0, 400, 600);
        let dialog = InsertHint {
            size: (200, 100),
            center_over: Some(parent),
            ..Default::default()
        };
        layout.insert(id(1), &dialog);
        assert_eq!(layout.rect_of(id(1)), Some(Rect::new(500, 250, 200, 100)));
        // A parent mostly off the output: the dialog still keeps 64 px visible.
        let off = InsertHint {
            size: (200, 100),
            center_over: Some(Rect::new(1000, 0, 400, 600)),
            ..Default::default()
        };
        layout.insert(id(2), &off);
        assert_eq!(layout.rect_of(id(2)), Some(Rect::new(736, 250, 200, 100)));
    }

    #[test]
    fn moves_keep_64_pixels_visible() {
        let mut layout = FloatingLayout::new(AREA);
        layout.insert_at(id(1), Rect::new(100, 100, 300, 200));
        assert_eq!(
            layout.set_rect(id(1), Rect::new(10_000, -10_000, 300, 200)),
            Some(Rect::new(736, -136, 300, 200))
        );
        assert_eq!(layout.set_rect(id(99), Rect::new(0, 0, 1, 1)), None);
    }

    #[test]
    fn output_resize_clamps_windows_back() {
        let mut layout = FloatingLayout::new(Rect::new(0, 0, 1600, 1200));
        layout.insert_at(id(1), Rect::new(1400, 1000, 150, 150));
        layout.set_area(AREA);
        let rect = layout.rect_of(id(1)).expect("still there");
        assert_eq!(
            rect,
            Rect::new(736, 536, 150, 150),
            "moved just enough, size kept"
        );
        assert!(is_visible_enough(rect, AREA));
    }

    #[test]
    fn stacking_and_hit_testing_use_the_topmost() {
        let mut layout = FloatingLayout::new(AREA);
        layout.insert_at(id(1), Rect::new(0, 0, 300, 300));
        layout.insert_at(id(2), Rect::new(100, 100, 300, 300));
        assert_eq!(layout.window_at(150, 150), Some(id(2)));
        assert!(layout.raise(id(1)));
        assert_eq!(layout.window_at(150, 150), Some(id(1)));
        assert_eq!(layout.window_at(350, 350), Some(id(2)));
        assert_eq!(layout.window_at(500, 500), None);
        assert_eq!(
            layout.rects().iter().map(|(w, _)| *w).collect::<Vec<_>>(),
            vec![id(2), id(1)]
        );
        assert!(!layout.raise(id(9)));
    }

    #[test]
    fn tiny_output() {
        let tiny = Rect::new(0, 0, 3, 2);
        let mut layout = FloatingLayout::new(tiny);
        layout.insert(id(1), &hint((400, 300)));
        let rect = layout.rect_of(id(1)).expect("placed");
        assert_eq!((rect.w, rect.h), (3, 2), "clamped to the output");
        assert!(is_visible_enough(rect, tiny));
    }
}
