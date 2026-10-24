// SPDX-License-Identifier: Apache-2.0

//! One monitor's windows: the BSP tree (every window has a leaf), the floating layout,
//! each window's mode, and focus with its history.
//!
//! A window's effective mode is floating if it is forced floating (dialog, fixed
//! size) and otherwise its own mode. Mode changes only flip leaf modes in the tree,
//! so tiling again restores the tree exactly.

use std::collections::BTreeMap;

use super::{
    BspTree, Direction, FloatingLayout, InsertHint, Layout, Mode, Rect, SizeLimits, WindowId,
    geometry::pick_in_direction,
};

/// Keyboard move and resize step, in pixels.
pub const STEP: i32 = 40;

#[derive(Clone, Debug, PartialEq)]
struct WindowState {
    /// The mode the user (or the default) chose; ignored while forced.
    own_mode: Mode,
    forced: bool,
    limits: SizeLimits,
    parent: Option<WindowId>,
    /// Where the window was the last time it floated.
    last_floating: Option<Rect>,
}

impl WindowState {
    fn mode(&self) -> Mode {
        if self.forced {
            Mode::Floating
        } else {
            self.own_mode
        }
    }
}

/// What the compositor knows about a window when it opens.
#[derive(Clone, Copy, Debug, Default)]
pub struct NewWindow {
    pub forced: bool,
    pub parent: Option<WindowId>,
    pub limits: SizeLimits,
}

#[derive(Clone, Debug)]
pub struct MonitorLayout {
    area: Rect,
    default_mode: Mode,
    tree: BspTree,
    floating: FloatingLayout,
    windows: BTreeMap<WindowId, WindowState>,
    focused: Option<WindowId>,
    /// Focus history, most recent last.
    history: Vec<WindowId>,
}

impl MonitorLayout {
    pub fn new(area: Rect) -> Self {
        Self {
            area,
            default_mode: Mode::Tiled,
            tree: BspTree::new(area),
            floating: FloatingLayout::new(area),
            windows: BTreeMap::new(),
            focused: None,
            history: Vec::new(),
        }
    }

    #[cfg(test)]
    pub fn area(&self) -> Rect {
        self.area
    }

    /// Output resize: tiles are recomputed, floating windows clamped back.
    pub fn set_area(&mut self, area: Rect) {
        self.area = area;
        self.tree.set_area(area);
        self.floating.set_area(area);
    }

    pub fn default_mode(&self) -> Mode {
        self.default_mode
    }

    pub fn set_default_mode(&mut self, mode: Mode) {
        self.default_mode = mode;
    }

    #[cfg(test)]
    pub fn tree(&self) -> &BspTree {
        &self.tree
    }

    pub fn windows(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.windows.keys().copied()
    }

    pub fn contains(&self, window: WindowId) -> bool {
        self.windows.contains_key(&window)
    }

    /// The effective mode.
    pub fn mode(&self, window: WindowId) -> Option<Mode> {
        self.windows.get(&window).map(WindowState::mode)
    }

    #[cfg(test)]
    pub fn is_forced(&self, window: WindowId) -> bool {
        self.windows.get(&window).is_some_and(|s| s.forced)
    }

    pub fn limits(&self, window: WindowId) -> SizeLimits {
        self.windows
            .get(&window)
            .map(|s| s.limits)
            .unwrap_or_default()
    }

    pub fn set_limits(&mut self, window: WindowId, limits: SizeLimits) {
        if let Some(state) = self.windows.get_mut(&window) {
            state.limits = limits;
        }
    }

    /// Opens a window: its leaf splits the focused window's leaf; it starts in the
    /// default mode (or floating if forced) and gets focus. A floating window has no
    /// rectangle until `place` is called with the size the client chose.
    pub fn open(&mut self, window: WindowId, new: NewWindow) {
        debug_assert!(!self.contains(window), "{window:?} opened twice");
        let state = WindowState {
            own_mode: self.default_mode,
            forced: new.forced,
            limits: new.limits,
            parent: new.parent.filter(|p| self.contains(*p)),
            last_floating: None,
        };
        self.tree
            .insert_with_mode(window, self.focused, state.mode());
        self.windows.insert(window, state);
        self.focus(window);
    }

    /// Gives a floating window without a rectangle its first one: the client's size,
    /// clamped; forced windows centered over their parent (or the output), others
    /// cascaded. Returns the window's rectangle.
    pub fn place(&mut self, window: WindowId, size: (i32, i32)) -> Option<Rect> {
        let state = self.windows.get(&window)?;
        if state.mode() == Mode::Floating && !self.floating.contains(window) {
            let center_over = state.forced.then(|| {
                state
                    .parent
                    .and_then(|p| self.rect_of(p))
                    .unwrap_or(self.area)
            });
            let hint = InsertHint {
                next_to: None,
                size,
                center_over,
                limits: state.limits,
            };
            self.floating.insert(window, &hint);
        }
        self.rect_of(window)
    }

    /// False for a floating window still waiting for its first rectangle.
    #[cfg(test)]
    pub fn is_placed(&self, window: WindowId) -> bool {
        self.rect_of(window).is_some()
    }

    /// Closes a window. If it had focus: after a tiled window, the most recently
    /// focused window of the sibling subtree that took its place; after a floating
    /// window, the most recently focused window overall.
    pub fn close(&mut self, window: WindowId) -> bool {
        let Some(state) = self.windows.remove(&window) else {
            return false;
        };
        self.floating.remove(window);
        let siblings = self
            .tree
            .remove_returning_sibling(window)
            .unwrap_or_default();
        self.history.retain(|w| *w != window);
        if self.focused == Some(window) {
            self.focused = None;
            let next = match state.mode() {
                Mode::Tiled => self.most_recent_of(&siblings),
                Mode::Floating => None,
            }
            .or_else(|| self.history.last().copied());
            if let Some(next) = next {
                self.focus(next);
            }
        }
        for other in self.windows.values_mut() {
            if other.parent == Some(window) {
                other.parent = None;
            }
        }
        true
    }

    fn recency(&self, window: WindowId) -> u64 {
        self.history
            .iter()
            .position(|w| *w == window)
            .map_or(0, |i| i as u64 + 1)
    }

    fn most_recent_of(&self, candidates: &[WindowId]) -> Option<WindowId> {
        candidates.iter().copied().max_by_key(|w| self.recency(*w))
    }

    pub fn focused(&self) -> Option<WindowId> {
        self.focused
    }

    /// Focuses and raises a window and records it as the most recent.
    pub fn focus(&mut self, window: WindowId) -> bool {
        if !self.preview_focus(window) {
            return false;
        }
        self.commit_focus();
        true
    }

    /// Focuses and raises a window without recording it in the history (while
    /// cycling with mod+Tab).
    pub fn preview_focus(&mut self, window: WindowId) -> bool {
        if !self.contains(window) {
            return false;
        }
        self.focused = Some(window);
        self.floating.raise(window);
        true
    }

    /// Records the focused window as the most recent (the end of a mod+Tab cycle).
    pub fn commit_focus(&mut self) {
        if let Some(window) = self.focused {
            self.history.retain(|w| *w != window);
            self.history.push(window);
        }
    }

    /// All windows, most recently focused first.
    pub fn recent_first(&self) -> Vec<WindowId> {
        let mut order: Vec<WindowId> = self.history.iter().rev().copied().collect();
        order.extend(self.windows().filter(|w| !self.history.contains(w)));
        order
    }

    /// Where the window is: its tile, or its floating rectangle.
    pub fn rect_of(&self, window: WindowId) -> Option<Rect> {
        match self.mode(window)? {
            Mode::Tiled => self.tree.rect_of(window),
            Mode::Floating => self.floating.rect_of(window),
        }
    }

    pub fn tiled_rects(&self) -> Vec<(WindowId, Rect)> {
        self.tree.rects()
    }

    /// Floating windows in stacking order, bottom first.
    pub fn floating_rects(&self) -> Vec<(WindowId, Rect)> {
        self.floating.rects()
    }

    /// The topmost window at a point: floating windows are above tiled ones.
    pub fn window_at(&self, x: i32, y: i32) -> Option<WindowId> {
        self.floating.window_at(x, y).or_else(|| {
            self.tree
                .rects()
                .into_iter()
                .find(|(_, r)| r.contains(x, y))
                .map(|(w, _)| w)
        })
    }

    /// The window in `direction` from `window`, among all windows on the monitor.
    pub fn neighbour(&self, window: WindowId, direction: Direction) -> Option<WindowId> {
        let from = self.rect_of(window)?;
        let candidates = self
            .tiled_rects()
            .into_iter()
            .chain(self.floating_rects())
            .filter(|(w, _)| *w != window);
        pick_in_direction(from, candidates, direction, |w| self.recency(w))
    }

    /// Swaps the focused window with its tiled neighbour in `direction`. Tiled windows
    /// only: nothing happens if the focused window floats. Focus stays on the window.
    pub fn swap_focused(&mut self, direction: Direction) -> bool {
        let Some(focused) = self.focused.filter(|w| self.mode(*w) == Some(Mode::Tiled)) else {
            return false;
        };
        let Some(from) = self.tree.rect_of(focused) else {
            return false;
        };
        let candidates = self
            .tiled_rects()
            .into_iter()
            .filter(|(w, _)| *w != focused);
        match pick_in_direction(from, candidates, direction, |w| self.recency(w)) {
            Some(other) => self.tree.swap(focused, other),
            None => false,
        }
    }

    fn set_own_mode(&mut self, window: WindowId, mode: Mode) {
        if let Some(state) = self.windows.get_mut(&window) {
            state.own_mode = mode;
            let effective = state.mode();
            self.tree.set_mode(window, effective);
        }
    }

    /// mod+F. A tiled window floats at its last floating rectangle (or, the first time,
    /// where its tile is); a floating window returns to its leaf. Does nothing on a
    /// forced-floating window.
    pub fn toggle(&mut self, window: WindowId) -> bool {
        let Some(state) = self.windows.get(&window) else {
            return false;
        };
        if state.forced {
            return false;
        }
        match state.mode() {
            Mode::Tiled => {
                let rect = state.last_floating.or_else(|| self.tree.rect_of(window));
                self.set_own_mode(window, Mode::Floating);
                if let Some(rect) = rect {
                    self.floating.insert_at(window, rect);
                }
            }
            Mode::Floating => self.tile(window),
        }
        true
    }

    /// Returns a non-forced floating window to its leaf, remembering its rectangle.
    fn tile(&mut self, window: WindowId) {
        if let Some(rect) = self.floating.rect_of(window)
            && let Some(state) = self.windows.get_mut(&window)
        {
            state.last_floating = Some(rect);
        }
        self.floating.remove(window);
        self.set_own_mode(window, Mode::Tiled);
    }

    /// Floats a tiled window where its tile is (a drag or keyboard move on a tiled
    /// window). Returns false if it was not tiled.
    pub fn float_in_place(&mut self, window: WindowId) -> bool {
        if self.mode(window) != Some(Mode::Tiled) {
            return false;
        }
        let Some(rect) = self.tree.rect_of(window) else {
            return false;
        };
        self.set_own_mode(window, Mode::Floating);
        self.floating.insert_at(window, rect);
        true
    }

    /// mod+Shift+F: every tiled window floats at its current tile.
    pub fn float_all(&mut self) {
        let tiles = self.tree.rects();
        for (window, rect) in tiles {
            self.set_own_mode(window, Mode::Floating);
            self.floating.insert_at(window, rect);
        }
        if let Some(focused) = self.focused {
            self.floating.raise(focused);
        }
    }

    /// mod+Shift+T: every floating window that is not forced returns to its leaf.
    pub fn tile_all(&mut self) {
        let floating: Vec<WindowId> = self
            .windows
            .iter()
            .filter(|(_, s)| s.mode() == Mode::Floating && !s.forced)
            .map(|(w, _)| *w)
            .collect();
        for window in floating {
            self.tile(window);
        }
    }

    /// Updates whether a window is forced floating (checked on every commit). A
    /// window that becomes forced floats centered over its parent (or the output) at
    /// its current size; one that stops being forced follows its own mode again.
    /// Returns true if anything changed.
    pub fn set_forced(&mut self, window: WindowId, forced: bool, parent: Option<WindowId>) -> bool {
        let parent = parent.filter(|p| *p != window && self.contains(*p));
        let Some(state) = self.windows.get_mut(&window) else {
            return false;
        };
        let parent_changed = state.parent != parent;
        state.parent = parent;
        if state.forced == forced {
            return parent_changed;
        }
        let before = state.mode();
        state.forced = forced;
        let after = state.mode();
        let limits = state.limits;
        if before == after {
            return true;
        }
        match after {
            Mode::Floating => {
                let tile = self.tree.rect_of(window);
                self.tree.set_mode(window, Mode::Floating);
                if let Some(tile) = tile {
                    let hint = InsertHint {
                        next_to: None,
                        size: (tile.w, tile.h),
                        center_over: Some(
                            parent.and_then(|p| self.rect_of(p)).unwrap_or(self.area),
                        ),
                        limits,
                    };
                    self.floating.insert(window, &hint);
                }
            }
            Mode::Tiled => self.tile(window),
        }
        true
    }

    /// Sets a floating window's rectangle (drag, resize, client-chosen size): the size
    /// is clamped to the window's limits, the position by the visibility rule.
    pub fn set_floating_rect(&mut self, window: WindowId, rect: Rect) -> Option<Rect> {
        let limits = self.windows.get(&window)?.limits;
        let (w, h) = limits.clamp((rect.w, rect.h));
        self.floating
            .set_rect(window, Rect::new(rect.x, rect.y, w, h))
    }

    /// Keyboard move by (dx, dy). A tiled window is floated in place first.
    pub fn move_by(&mut self, window: WindowId, dx: i32, dy: i32) -> Option<Rect> {
        self.float_in_place(window);
        let r = self.floating.rect_of(window)?;
        self.set_floating_rect(window, Rect::new(r.x + dx, r.y + dy, r.w, r.h))
    }

    /// Keyboard resize by (dw, dh) with the top-left corner fixed. A tiled window is
    /// floated in place first.
    pub fn resize_by(&mut self, window: WindowId, dw: i32, dh: i32) -> Option<Rect> {
        self.float_in_place(window);
        let r = self.floating.rect_of(window)?;
        self.set_floating_rect(window, Rect::new(r.x, r.y, r.w + dw, r.h + dh))
    }

    /// Consistency checks used by tests: every window has exactly one leaf, leaf
    /// modes match effective modes, and exactly the placed floating windows are in the
    /// stacking order, once each.
    #[cfg(test)]
    pub fn check_invariants(&self) {
        use super::{bsp::tests::check_invariants as check_tree, geometry::is_visible_enough};
        check_tree(&self.tree);
        let mut tree_windows = self.tree.windows();
        tree_windows.sort();
        let known: Vec<WindowId> = self.windows().collect();
        assert_eq!(tree_windows, known, "every window has exactly one leaf");
        let stack: Vec<WindowId> = self.floating.rects().iter().map(|(w, _)| *w).collect();
        let mut unique = stack.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            stack.len(),
            "a window is in the stacking order twice"
        );
        for (window, state) in &self.windows {
            assert_eq!(
                self.tree.mode_of(*window),
                Some(state.mode()),
                "leaf mode of {window:?}"
            );
            if state.mode() == Mode::Tiled {
                assert!(
                    !stack.contains(window),
                    "tiled {window:?} is in the stacking order"
                );
            }
        }
        for (window, rect) in self.floating.rects() {
            assert_eq!(
                self.mode(window),
                Some(Mode::Floating),
                "{window:?} in stack but not floating"
            );
            assert!(
                is_visible_enough(rect, self.area),
                "{window:?} at {rect:?} breaks the visibility rule"
            );
        }
        match self.focused {
            Some(f) => assert!(self.contains(f), "focus on an unknown window"),
            None => assert!(self.windows.is_empty(), "windows exist but none is focused"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0, 0, 800, 600);

    fn id(n: u64) -> WindowId {
        WindowId(n)
    }

    /// Opens windows 1..=n with defaults (tiled).
    fn layout_with(n: u64) -> MonitorLayout {
        let mut layout = MonitorLayout::new(AREA);
        for i in 1..=n {
            layout.open(id(i), NewWindow::default());
        }
        layout
    }

    fn rect(layout: &MonitorLayout, n: u64) -> Rect {
        layout.rect_of(id(n)).expect("window has a rectangle")
    }

    #[test]
    fn floating_one_window_refills_the_others_and_toggle_restores() {
        let mut layout = layout_with(3); // 1 | (2 / 3)
        let tree = layout.tree().clone();
        assert!(layout.toggle(id(2)));
        assert_eq!(layout.mode(id(2)), Some(Mode::Floating));
        assert_eq!(
            rect(&layout, 2),
            Rect::new(400, 0, 400, 300),
            "floats where its tile was"
        );
        assert_eq!(
            rect(&layout, 3),
            Rect::new(400, 0, 400, 600),
            "its sibling takes the space"
        );
        layout.move_by(id(2), -100, 50);
        assert!(layout.toggle(id(2)));
        assert_eq!(layout.tree(), &tree, "identical tree after toggling back");
        assert_eq!(rect(&layout, 2), Rect::new(400, 0, 400, 300));
        // Floating again restores the last floating rectangle, not the tile.
        layout.toggle(id(2));
        assert_eq!(rect(&layout, 2), Rect::new(300, 50, 400, 300));
        layout.check_invariants();
    }

    #[test]
    fn float_all_then_tile_all_is_lossless() {
        let mut layout = layout_with(4);
        let tree = layout.tree().clone();
        let tiles = layout.tiled_rects();
        layout.float_all();
        assert!(layout.tiled_rects().is_empty());
        // Each window floats exactly where its tile was.
        for (window, tile) in &tiles {
            assert_eq!(layout.rect_of(*window), Some(*tile));
        }
        layout.check_invariants();
        layout.move_by(id(1), 37, 11);
        layout.resize_by(id(3), 80, -40);
        layout.tile_all();
        assert_eq!(layout.tree(), &tree, "identical tree");
        assert_eq!(layout.tiled_rects(), tiles, "identical rectangles");
        layout.check_invariants();
    }

    #[test]
    fn output_resize_between_float_all_and_tile_all_keeps_structure() {
        let mut layout = layout_with(3);
        let mut expected = layout.tree().clone();
        layout.float_all();
        layout.set_area(Rect::new(0, 0, 1000, 1001));
        layout.check_invariants();
        layout.tile_all();
        expected.set_area(Rect::new(0, 0, 1000, 1001));
        assert_eq!(layout.tree(), &expected, "same structure, new area");
        assert_eq!(rect(&layout, 1), Rect::new(0, 0, 500, 1001));
        assert_eq!(rect(&layout, 2), Rect::new(500, 0, 500, 500));
        assert_eq!(rect(&layout, 3), Rect::new(500, 500, 500, 501));
    }

    #[test]
    fn windows_opened_and_closed_while_floating_follow_the_tiled_rules() {
        // 1 | (2 / 3), float all, open 4 (focused: 3), close 2 (3's old sibling), tile all.
        let mut layout = layout_with(3);
        layout.float_all();
        layout.open(id(4), NewWindow::default()); // default tiled: takes the whole area
        assert_eq!(rect(&layout, 4), AREA, "placeholders take no space");
        layout.close(id(2));
        layout.tile_all();
        // Expected: the tree as if everything had stayed tiled. 4 split 3's 400x300 tile
        // side by side; closing 2 promotes (3 | 4) to the right half.
        let mut expected = layout_with(3);
        expected.open(id(4), NewWindow::default());
        expected.close(id(2));
        assert_eq!(layout.tree(), expected.tree());
        assert_eq!(layout.tiled_rects(), expected.tiled_rects());
        assert_eq!(rect(&layout, 1), Rect::new(0, 0, 400, 600));
        assert_eq!(rect(&layout, 3), Rect::new(400, 0, 200, 600));
        assert_eq!(rect(&layout, 4), Rect::new(600, 0, 200, 600));
        layout.check_invariants();
    }

    #[test]
    fn new_window_in_floating_default_gets_a_placeholder_and_moves_nothing() {
        let mut layout = layout_with(2);
        let tiles = layout.tiled_rects();
        layout.set_default_mode(Mode::Floating);
        layout.open(id(3), NewWindow::default());
        assert_eq!(layout.tiled_rects(), tiles, "no tiled rectangle changes");
        assert!(!layout.is_placed(id(3)), "waits for the client's size");
        assert_eq!(
            layout.place(id(3), (300, 200)),
            Some(Rect::new(250, 200, 300, 200))
        );
        assert_eq!(layout.focused(), Some(id(3)));
        // Tiling it puts it next to the leaf that was focused when it opened (2).
        layout.toggle(id(3));
        assert_eq!(rect(&layout, 2), Rect::new(400, 0, 400, 300));
        assert_eq!(rect(&layout, 3), Rect::new(400, 300, 400, 300));
        layout.check_invariants();
    }

    #[test]
    fn closing_a_floating_window_removes_its_placeholder() {
        let mut layout = layout_with(3); // 1 | (2 / 3)
        layout.toggle(id(2));
        layout.focus(id(1));
        layout.focus(id(2));
        assert!(layout.close(id(2)));
        assert_eq!(
            rect(&layout, 3),
            Rect::new(400, 0, 400, 600),
            "sibling takes the parent's place"
        );
        assert!(layout.floating_rects().is_empty());
        assert_eq!(
            layout.focused(),
            Some(id(1)),
            "most recently focused overall"
        );
        layout.check_invariants();
    }

    #[test]
    fn closing_a_tiled_window_focuses_the_sibling_subtree() {
        let mut layout = layout_with(3); // 1 | (2 / 3)
        layout.focus(id(2));
        layout.focus(id(1));
        layout.close(id(1));
        assert_eq!(
            layout.focused(),
            Some(id(2)),
            "most recent in the sibling subtree (2 / 3)"
        );
        layout.close(id(2));
        layout.close(id(3));
        assert_eq!(layout.focused(), None);
        layout.check_invariants();
    }

    #[test]
    fn forced_floating_windows_are_centered_and_untouched_by_tile_all() {
        let mut layout = layout_with(2); // 1 | 2
        let dialog = NewWindow {
            forced: true,
            parent: Some(id(1)),
            ..Default::default()
        };
        layout.open(id(3), dialog);
        assert_eq!(layout.mode(id(3)), Some(Mode::Floating));
        assert_eq!(
            layout.place(id(3), (200, 100)),
            Some(Rect::new(100, 250, 200, 100)),
            "over its parent"
        );
        let fixed = NewWindow {
            forced: true,
            limits: SizeLimits {
                min: (300, 300),
                max: (300, 300),
            },
            ..Default::default()
        };
        layout.open(id(4), fixed);
        assert_eq!(
            layout.place(id(4), (300, 300)),
            Some(Rect::new(250, 150, 300, 300)),
            "on the output"
        );
        assert!(
            !layout.toggle(id(3)),
            "mod+F does nothing on a forced window"
        );
        layout.tile_all();
        assert_eq!(layout.mode(id(3)), Some(Mode::Floating));
        assert_eq!(layout.mode(id(4)), Some(Mode::Floating));
        layout.float_all();
        layout.tile_all();
        assert_eq!(
            rect(&layout, 3),
            Rect::new(100, 250, 200, 100),
            "not moved by bulk actions"
        );
        layout.check_invariants();
    }

    #[test]
    fn forced_status_is_rechecked() {
        let mut layout = layout_with(2); // 1 | 2
        // 2 becomes a dialog of 1: floats centered over 1 at its tile size.
        assert!(layout.set_forced(id(2), true, Some(id(1))));
        assert_eq!(rect(&layout, 1), AREA);
        assert_eq!(rect(&layout, 2), Rect::new(200, 0, 400, 600));
        // No longer forced: back to its own mode (tiled) and its leaf.
        assert!(layout.set_forced(id(2), false, None));
        assert_eq!(rect(&layout, 2), Rect::new(400, 0, 400, 600));
        assert!(!layout.set_forced(id(2), false, None), "nothing changed");
        layout.check_invariants();
    }

    #[test]
    fn keyboard_move_and_resize_float_tiled_windows_and_respect_limits() {
        let mut layout = layout_with(2);
        layout.set_limits(
            id(2),
            SizeLimits {
                min: (100, 100),
                max: (420, 0),
            },
        );
        assert_eq!(
            layout.move_by(id(2), STEP, 0),
            Some(Rect::new(440, 0, 400, 600))
        );
        assert_eq!(layout.mode(id(2)), Some(Mode::Floating));
        assert_eq!(
            layout.resize_by(id(2), STEP, -STEP),
            Some(Rect::new(440, 0, 420, 560)),
            "max width 420"
        );
        for _ in 0..20 {
            layout.resize_by(id(2), -STEP, -STEP);
        }
        assert_eq!(
            rect(&layout, 2),
            Rect::new(440, 0, 100, 100),
            "min size 100x100"
        );
        for _ in 0..40 {
            layout.move_by(id(2), STEP, STEP);
        }
        assert_eq!(
            rect(&layout, 2),
            Rect::new(736, 536, 100, 100),
            "64 px stay visible"
        );
        layout.check_invariants();
    }

    #[test]
    fn directional_focus_includes_floating_windows() {
        let mut layout = layout_with(2); // 1 | 2
        layout.open(id(3), NewWindow::default()); // splits 2: 2 / 3
        layout.toggle(id(3)); // floats at (400, 300, 400, 300)
        layout.set_floating_rect(id(3), Rect::new(150, 150, 200, 200));
        layout.focus(id(1));
        // From 1 (0,0,400,600): 3 is centered further right and overlaps vertically.
        assert_eq!(
            layout.neighbour(id(1), Direction::Right),
            Some(id(3)),
            "gap 0 (overlap), more recent than 2"
        );
        layout.focus(id(2));
        layout.focus(id(1));
        assert_eq!(layout.neighbour(id(1), Direction::Right), Some(id(2)));
        assert_eq!(layout.neighbour(id(3), Direction::Left), Some(id(1)));
        // From 2 (400,0,400,600): 1 touches the left edge (gap 0), 3 is 50 px away.
        assert_eq!(
            layout.neighbour(id(2), Direction::Left),
            Some(id(1)),
            "nearest by gap"
        );
    }

    #[test]
    fn swap_is_for_tiled_windows_only() {
        let mut layout = layout_with(3); // 1 | (2 / 3)
        layout.toggle(id(2));
        layout.focus(id(3)); // 3 fills the right half
        let tree = layout.tree().clone();
        assert!(layout.swap_focused(Direction::Left), "3 swaps with tiled 1");
        assert_ne!(layout.tree(), &tree);
        layout.focus(id(2));
        assert!(
            !layout.swap_focused(Direction::Right),
            "focused window floats: no swap"
        );
        layout.check_invariants();
    }

    #[test]
    fn hit_testing_prefers_the_topmost_floating_window() {
        let mut layout = layout_with(2);
        layout.toggle(id(2));
        layout.set_floating_rect(id(2), Rect::new(100, 100, 200, 200));
        assert_eq!(layout.window_at(150, 150), Some(id(2)));
        assert_eq!(layout.window_at(50, 50), Some(id(1)));
        assert_eq!(layout.window_at(-1, -1), None);
    }

    #[test]
    fn cycle_preview_does_not_change_history_until_committed() {
        let mut layout = layout_with(3); // history 1, 2, 3
        assert_eq!(layout.recent_first(), vec![id(3), id(2), id(1)]);
        layout.preview_focus(id(2));
        layout.preview_focus(id(1));
        assert_eq!(layout.recent_first(), vec![id(3), id(2), id(1)]);
        layout.commit_focus();
        assert_eq!(layout.recent_first(), vec![id(1), id(3), id(2)]);
    }

    #[test]
    fn window_larger_than_the_output_and_tiny_output() {
        let mut layout = MonitorLayout::new(Rect::new(0, 0, 3, 2));
        layout.set_default_mode(Mode::Floating);
        for i in 1..=3 {
            layout.open(id(i), NewWindow::default());
            layout.place(id(i), (500, 500));
        }
        layout.check_invariants();
        layout.tile_all();
        layout.check_invariants();
        let mut big = MonitorLayout::new(AREA);
        let fixed = NewWindow {
            forced: true,
            limits: SizeLimits {
                min: (2000, 1500),
                max: (2000, 1500),
            },
            ..Default::default()
        };
        big.open(id(1), fixed);
        assert_eq!(
            big.place(id(1), (2000, 1500)),
            Some(Rect::new(-600, -450, 2000, 1500))
        );
        big.move_by(id(1), 5000, 5000);
        big.check_invariants();
    }
}
