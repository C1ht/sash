// SPDX-License-Identifier: Apache-2.0

//! One monitor's windows: the BSP tree (every window has a leaf), the floating layout,
//! each window's mode, and focus with its history.
//!
//! A window's effective mode is floating if it is forced floating (dialog, fixed
//! size) and otherwise its own mode. Mode changes only flip leaf modes in the tree,
//! so tiling again restores the tree exactly.

use std::collections::BTreeMap;

use super::{
    BspTree, Direction, Edges, FloatingLayout, InsertHint, Layout, Mode, Rect, Side, SizeLimits,
    SplitHandle, WindowId,
    geometry::pick_in_direction,
    snap::{SnapStep, ZoneId, Zones, snap_step},
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
    /// The zone the window is snapped to. Kept while tile-all has it tiled, so floating
    /// again restores the zone.
    snap: Option<Snapped>,
    /// Maximized while tiled: drawn over the whole output, keeping its leaf and tile.
    maximized_tiled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Snapped {
    zone: ZoneId,
    /// The floating rectangle before snapping.
    restore: Rect,
}

/// How a window is drawn, bottom to top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// In its tile, clipped to it.
    Tiled,
    /// A maximized tiled window over the whole output.
    Maximized,
    Floating,
    Fullscreen,
}

/// What a window's configure says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Presentation {
    /// None: the client chooses (a floating window without a rectangle yet).
    pub size: Option<(i32, i32)>,
    /// The xdg tiled states, one per edge.
    pub tiled: Edges,
    pub maximized: bool,
    pub fullscreen: bool,
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

/// A mouse resize of a tiled window: the split on each moving axis and the length of its
/// first part when the drag started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TiledResize {
    horizontal: Option<(SplitHandle, i32)>,
    vertical: Option<(SplitHandle, i32)>,
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
    zones: Zones,
    /// At most one fullscreen window per monitor.
    fullscreen: Option<WindowId>,
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
            zones: Zones::default(),
            fullscreen: None,
        }
    }

    pub fn area(&self) -> Rect {
        self.area
    }

    /// Output resize: tiles are recomputed, floating windows clamped back.
    pub fn set_area(&mut self, area: Rect) {
        self.area = area;
        self.tree.set_area(area);
        self.floating.set_area(area);
        // Snapped windows follow their zone.
        let snapped: Vec<(WindowId, ZoneId)> = self
            .windows
            .iter()
            .filter(|(_, s)| s.mode() == Mode::Floating)
            .filter_map(|(w, s)| s.snap.map(|snap| (*w, snap.zone)))
            .collect();
        for (window, zone) in snapped {
            if let Some(rect) = self.zone_rect(zone) {
                self.floating.set_rect(window, rect);
            }
        }
    }

    pub fn zones(&self) -> &Zones {
        &self.zones
    }

    /// A zone's rectangle on this monitor.
    pub fn zone_rect(&self, zone: ZoneId) -> Option<Rect> {
        self.zones.get(zone).map(|z| z.rect(self.area))
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
        let Some(state) = self.windows.get_mut(&window) else {
            return;
        };
        state.limits = limits;
        // Fixed-size windows never snap or maximize.
        if limits.is_fixed() {
            state.maximized_tiled = false;
            if state.snap.is_some() {
                self.unsnap_restore(window);
            }
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
            snap: None,
            maximized_tiled: false,
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
        if state.mode() == Mode::Floating
            && !self.floating.contains(window)
            && let Some(zone) = state.snap.and_then(|s| self.zone_rect(s.zone))
        {
            self.floating.insert_at(window, zone);
        } else if state.mode() == Mode::Floating && !self.floating.contains(window) {
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
        if self.fullscreen == Some(window) {
            self.fullscreen = None;
        }
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
    /// Focusing a window other than the fullscreen one ends fullscreen.
    pub fn commit_focus(&mut self) {
        if self.fullscreen.is_some() && self.fullscreen != self.focused {
            self.fullscreen = None;
        }
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

    /// The topmost window at a point. A fixed-size fullscreen window's backdrop
    /// covers the rest of the output: points there hit nothing.
    pub fn window_at(&self, x: i32, y: i32) -> Option<WindowId> {
        if self.fullscreen_backdrop().is_some_and(|b| b.contains(x, y)) {
            let f = self.fullscreen?;
            return self.shown_rect(f).filter(|r| r.contains(x, y)).map(|_| f);
        }
        self.stacking()
            .into_iter()
            .rev()
            .find(|(_, r, _)| r.contains(x, y))
            .map(|(w, _, _)| w)
    }

    /// Where the window is drawn: fullscreen and maximized windows cover the output
    /// (a fixed-size fullscreen window is centered at its size), others are at their
    /// tile or floating rectangle.
    pub fn shown_rect(&self, window: WindowId) -> Option<Rect> {
        let state = self.windows.get(&window)?;
        if self.fullscreen == Some(window) {
            let limits = state.limits;
            return Some(if limits.is_fixed() {
                let (w, h) = limits.min;
                Rect::new(
                    self.area.x + (self.area.w - w).div_euclid(2),
                    self.area.y + (self.area.h - h).div_euclid(2),
                    w,
                    h,
                )
            } else {
                self.area
            });
        }
        if state.maximized_tiled && state.mode() == Mode::Tiled {
            return Some(self.area);
        }
        self.rect_of(window)
    }

    /// Behind a fixed-size fullscreen window, an opaque backdrop covers the output.
    pub fn fullscreen_backdrop(&self) -> Option<Rect> {
        let f = self.fullscreen?;
        self.windows.get(&f)?.limits.is_fixed().then_some(self.area)
    }

    /// Every placed window in drawing order, bottom first: tiled, maximized tiled
    /// (focused last), floating by stacking order, then the fullscreen window.
    pub fn stacking(&self) -> Vec<(WindowId, Rect, Layer)> {
        let fullscreen = self.fullscreen;
        let maximized = |w: &WindowId| self.windows.get(w).is_some_and(|s| s.maximized_tiled);
        let mut out: Vec<(WindowId, Rect, Layer)> = self
            .tree
            .rects()
            .into_iter()
            .filter(|(w, _)| Some(*w) != fullscreen && !maximized(w))
            .map(|(w, r)| (w, r, Layer::Tiled))
            .collect();
        let mut maxed: Vec<(WindowId, Rect)> = self
            .tree
            .rects()
            .into_iter()
            .filter(|(w, _)| Some(*w) != fullscreen && maximized(w))
            .collect();
        maxed.sort_by_key(|(w, _)| Some(*w) == self.focused);
        out.extend(
            maxed
                .into_iter()
                .map(|(w, _)| (w, self.area, Layer::Maximized)),
        );
        out.extend(
            self.floating
                .rects()
                .into_iter()
                .filter(|(w, _)| Some(*w) != fullscreen)
                .map(|(w, r)| (w, r, Layer::Floating)),
        );
        if let Some(f) = fullscreen
            && let Some(rect) = self.shown_rect(f)
        {
            out.push((f, rect, Layer::Fullscreen));
        }
        out
    }

    pub fn fullscreen(&self) -> Option<WindowId> {
        self.fullscreen
    }

    /// The zone a floating window is snapped to.
    pub fn snapped_zone(&self, window: WindowId) -> Option<ZoneId> {
        let state = self.windows.get(&window)?;
        (state.mode() == Mode::Floating).then_some(state.snap?.zone)
    }

    /// Maximized: tiled and maximized, or snapped to a maximized zone.
    pub fn is_maximized(&self, window: WindowId) -> bool {
        let Some(state) = self.windows.get(&window) else {
            return false;
        };
        (state.maximized_tiled && state.mode() == Mode::Tiled)
            || self
                .snapped_zone(window)
                .and_then(|z| self.zones.get(z))
                .is_some_and(|z| z.maximized)
    }

    /// What the window's configure should say.
    pub fn presentation(&self, window: WindowId) -> Option<Presentation> {
        let state = self.windows.get(&window)?;
        let size = |r: Rect| Some((r.w.max(1), r.h.max(1)));
        let plain = Presentation {
            size: None,
            tiled: Edges::default(),
            maximized: false,
            fullscreen: false,
        };
        if self.fullscreen == Some(window) {
            return Some(Presentation {
                size: size(self.area),
                fullscreen: true,
                ..plain
            });
        }
        if self.is_maximized(window) {
            return Some(Presentation {
                size: size(self.area),
                maximized: true,
                ..plain
            });
        }
        let rect = self.rect_of(window);
        Some(match state.mode() {
            Mode::Tiled => Presentation {
                size: rect.and_then(size),
                tiled: Edges::ALL,
                ..plain
            },
            Mode::Floating => Presentation {
                size: rect.and_then(size),
                tiled: self
                    .snapped_zone(window)
                    .and_then(|z| self.zones.get(z))
                    .map(|z| z.output_edges())
                    .unwrap_or_default(),
                ..plain
            },
        })
    }

    /// Snaps a window to a zone. A tiled window floats first (its tile becomes the
    /// pre-snap rectangle). Fixed-size windows never snap. Returns false if nothing
    /// happened.
    pub fn snap(&mut self, window: WindowId, zone: ZoneId) -> bool {
        let Some(rect) = self.zone_rect(zone) else {
            return false;
        };
        let Some(state) = self.windows.get(&window) else {
            return false;
        };
        if state.limits.is_fixed() {
            return false;
        }
        if state.mode() == Mode::Tiled {
            self.float_in_place(window);
        }
        let Some(current) = self.floating.rect_of(window) else {
            return false;
        };
        let state = self.windows.get_mut(&window).expect("checked above");
        let restore = state.snap.map_or(current, |s| s.restore);
        state.snap = Some(Snapped { zone, restore });
        self.floating.set_rect(window, rect);
        true
    }

    /// Unsnaps a window back to its pre-snap rectangle. Returns false if it was not
    /// snapped.
    pub fn unsnap_restore(&mut self, window: WindowId) -> bool {
        if self.mode(window) != Some(Mode::Floating) {
            return false;
        }
        let Some(snap) = self.windows.get_mut(&window).and_then(|s| s.snap.take()) else {
            return false;
        };
        self.set_floating_rect(window, snap.restore);
        true
    }

    /// A drag pulls a snapped window out of its zone: it takes its pre-snap size, and
    /// the point where it was grabbed stays at the same relative position in the
    /// window, now under the pointer. Returns the new rectangle, or None if the window
    /// was not snapped.
    pub fn unsnap_for_drag(
        &mut self,
        window: WindowId,
        grabbed: (i32, i32),
        pointer: (i32, i32),
    ) -> Option<Rect> {
        let snap = self
            .snapped_zone(window)
            .and(self.windows.get(&window)?.snap)?;
        let current = self.floating.rect_of(window)?;
        let (w, h) = (snap.restore.w, snap.restore.h);
        let fraction = |pos: i32, start: i32, len: i32| (pos - start) as f64 / len.max(1) as f64;
        let x = pointer.0 - (fraction(grabbed.0, current.x, current.w) * w as f64).round() as i32;
        let y = pointer.1 - (fraction(grabbed.1, current.y, current.h) * h as f64).round() as i32;
        self.set_floating_rect(window, Rect::new(x, y, w, h))
    }

    /// mod+arrow, by the state table (snap::snap_step). Maximize on a tiled window
    /// keeps its leaf; other zones float a tiled window first.
    pub fn snap_key(&mut self, window: WindowId, key: Direction) -> bool {
        let Some(state) = self.windows.get(&window) else {
            return false;
        };
        if state.limits.is_fixed() {
            return false;
        }
        let tiled = state.mode() == Mode::Tiled;
        let current = if tiled {
            state
                .maximized_tiled
                .then(|| self.zones_maximized())
                .flatten()
        } else {
            state.snap.map(|s| s.zone)
        };
        match snap_step(current, key) {
            SnapStep::To(zone) => {
                let maximized = self.zones.get(zone).is_some_and(|z| z.maximized);
                if tiled && maximized {
                    self.windows
                        .get_mut(&window)
                        .expect("checked")
                        .maximized_tiled = true;
                    true
                } else {
                    self.snap(window, zone)
                }
            }
            SnapStep::Restore if tiled => {
                let state = self.windows.get_mut(&window).expect("checked");
                std::mem::replace(&mut state.maximized_tiled, false)
            }
            SnapStep::Restore => self.unsnap_restore(window),
            SnapStep::Nothing => false,
        }
    }

    /// The id of the maximized (whole-output) zone.
    fn zones_maximized(&self) -> Option<ZoneId> {
        [ZoneId::FULL]
            .into_iter()
            .find(|z| self.zones.get(*z).is_some_and(|z| z.maximized))
    }

    /// Client set_maximized / unset_maximized. A tiled window keeps its leaf; a
    /// floating one snaps to the maximized zone. Fixed-size windows never maximize.
    pub fn set_maximized(&mut self, window: WindowId, maximized: bool) -> bool {
        let Some(state) = self.windows.get(&window) else {
            return false;
        };
        let tiled = state.mode() == Mode::Tiled;
        if maximized {
            if state.limits.is_fixed() {
                return false;
            }
            if tiled {
                self.windows
                    .get_mut(&window)
                    .expect("checked")
                    .maximized_tiled = true;
                return true;
            }
            return self.zones_maximized().is_some_and(|z| self.snap(window, z));
        }
        if tiled {
            let state = self.windows.get_mut(&window).expect("checked");
            return std::mem::replace(&mut state.maximized_tiled, false);
        }
        if self.is_maximized(window) {
            return self.unsnap_restore(window);
        }
        false
    }

    /// Client set_fullscreen / unset_fullscreen. One fullscreen window per monitor: a
    /// new one replaces the old, which keeps its previous state. A window that goes
    /// fullscreen gets focus.
    pub fn set_fullscreen(&mut self, window: WindowId, fullscreen: bool) -> bool {
        if !self.contains(window) {
            return false;
        }
        if fullscreen {
            self.fullscreen = Some(window);
            self.focus(window);
            true
        } else if self.fullscreen == Some(window) {
            self.fullscreen = None;
            true
        } else {
            false
        }
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
                // A remembered zone wins over the last floating rectangle.
                let zone_rect = state.snap.and_then(|s| self.zone_rect(s.zone));
                let rect = zone_rect
                    .or(state.last_floating)
                    .or_else(|| self.tree.rect_of(window));
                self.windows
                    .get_mut(&window)
                    .expect("checked")
                    .maximized_tiled = false;
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
        if let Some(state) = self.windows.get_mut(&window) {
            state.maximized_tiled = false;
            state.snap = None;
        }
        self.set_own_mode(window, Mode::Floating);
        self.floating.insert_at(window, rect);
        true
    }

    /// mod+Shift+F: every tiled window floats at its current tile, or in its zone if
    /// it was snapped before tile-all.
    pub fn float_all(&mut self) {
        let tiles = self.tree.rects();
        for (window, tile) in tiles {
            let state = self
                .windows
                .get_mut(&window)
                .expect("every leaf is a window");
            state.maximized_tiled = false;
            let zone = state.snap.map(|s| s.zone);
            let rect = zone.and_then(|z| self.zone_rect(z)).unwrap_or(tile);
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
                // Forced windows are centered: a zone remembered from before tile-all
                // no longer applies.
                if let Some(state) = self.windows.get_mut(&window) {
                    state.snap = None;
                }
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
    /// is clamped to the window's limits, the position by the visibility rule. A
    /// snapped window stops being snapped.
    pub fn set_floating_rect(&mut self, window: WindowId, rect: Rect) -> Option<Rect> {
        let state = self.windows.get_mut(&window)?;
        if state.mode() == Mode::Floating {
            state.snap = None;
        }
        let limits = state.limits;
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

    /// Keyboard resize by (dw, dh). A floating window keeps its top-left corner; a
    /// tiled window moves the split on its right (dw) and bottom (dh) edges, and nothing
    /// happens on an edge that lies on the output boundary.
    pub fn resize_by(&mut self, window: WindowId, dw: i32, dh: i32) -> Option<Rect> {
        if self.mode(window) == Some(Mode::Tiled) {
            if dw != 0 {
                self.tree.resize_edge(window, Side::Right, dw);
            }
            if dh != 0 {
                self.tree.resize_edge(window, Side::Bottom, dh);
            }
            return self.rect_of(window);
        }
        let r = self.floating.rect_of(window)?;
        self.set_floating_rect(window, Rect::new(r.x, r.y, r.w + dw, r.h + dh))
    }

    /// Starts a mouse resize of a tiled window from `edges` (from the 3x3 grid): the
    /// splits on those edges and their lengths now. Edges on the output boundary are left
    /// out; None if nothing can move.
    pub fn begin_tiled_resize(&self, window: WindowId, edges: Edges) -> Option<TiledResize> {
        let horizontal = if edges.left {
            self.tree.edge_split(window, Side::Left)
        } else if edges.right {
            self.tree.edge_split(window, Side::Right)
        } else {
            None
        };
        let vertical = if edges.top {
            self.tree.edge_split(window, Side::Top)
        } else if edges.bottom {
            self.tree.edge_split(window, Side::Bottom)
        } else {
            None
        };
        (horizontal.is_some() || vertical.is_some()).then_some(TiledResize {
            horizontal,
            vertical,
        })
    }

    /// Applies a mouse resize of a tiled window: each split moves by the pointer's
    /// offset since the press. Returns whether any ratio changed.
    pub fn drag_tiled_resize(&mut self, resize: &TiledResize, dx: i32, dy: i32) -> bool {
        let mut changed = false;
        if let Some((handle, start)) = &resize.horizontal {
            changed |= self.tree.set_split_first(handle, start + dx);
        }
        if let Some((handle, start)) = &resize.vertical {
            changed |= self.tree.set_split_first(handle, start + dy);
        }
        changed
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
        for (window, state) in &self.windows {
            if let (Mode::Floating, Some(snap), Some(rect)) =
                (state.mode(), state.snap, self.floating.rect_of(*window))
            {
                assert_eq!(
                    Some(rect),
                    self.zone_rect(snap.zone),
                    "{window:?} is not in its zone"
                );
            }
            if state.snap.is_some() || state.maximized_tiled {
                assert!(
                    !state.limits.is_fixed(),
                    "fixed-size {window:?} is snapped or maximized"
                );
            }
        }
        if let Some(f) = self.fullscreen {
            assert!(self.contains(f), "fullscreen window is unknown");
        }
        let drawn: Vec<WindowId> = self.stacking().iter().map(|(w, _, _)| *w).collect();
        let mut unique = drawn.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), drawn.len(), "a window is drawn twice");
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

    /// 1 | (2 / 3) with the vertical split at 520 (13/20) and 2's bottom at 340 (17/30).
    fn resized_layout() -> MonitorLayout {
        let mut layout = layout_with(3);
        assert_eq!(
            layout.resize_by(id(1), 3 * STEP, 0),
            Some(Rect::new(0, 0, 520, 600))
        );
        assert_eq!(
            layout.resize_by(id(2), 0, STEP),
            Some(Rect::new(520, 0, 280, 340))
        );
        assert_eq!(
            layout.mode(id(1)),
            Some(Mode::Tiled),
            "tiled resize does not float"
        );
        layout
    }

    #[test]
    fn toggling_keeps_tiled_resize_ratios() {
        let mut layout = resized_layout();
        let tree = layout.tree().clone();
        let tiles = layout.tiled_rects();
        assert!(layout.toggle(id(2)));
        assert!(layout.toggle(id(2)));
        assert_eq!(layout.tree(), &tree, "identical tree, ratios included");
        layout.float_all();
        layout.resize_by(id(1), -STEP, STEP);
        layout.tile_all();
        assert_eq!(layout.tree(), &tree, "identical tree, ratios included");
        assert_eq!(layout.tiled_rects(), tiles, "identical rectangles");
        layout.check_invariants();
    }

    #[test]
    fn output_resize_keeps_tiled_resize_ratios() {
        let mut layout = resized_layout();
        layout.set_area(Rect::new(0, 0, 1000, 1001));
        // 1000 * 13/20 = 650; 1001 * 17/30 = 567.2, floored.
        assert_eq!(rect(&layout, 1), Rect::new(0, 0, 650, 1001));
        assert_eq!(rect(&layout, 2), Rect::new(650, 0, 350, 567));
        assert_eq!(rect(&layout, 3), Rect::new(650, 567, 350, 434));
        // The same while everything floats, applied at tile-all.
        let mut floated = resized_layout();
        floated.float_all();
        floated.set_area(Rect::new(0, 0, 1000, 1001));
        floated.tile_all();
        assert_eq!(floated.tree(), layout.tree());
        layout.check_invariants();
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
    fn keyboard_move_floats_tiled_windows_and_resize_respects_limits() {
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

    fn snapped(layout: &MonitorLayout, n: u64) -> Option<ZoneId> {
        layout.snapped_zone(id(n))
    }

    #[test]
    fn snapping_remembers_and_restores_the_pre_snap_rectangle() {
        let mut layout = layout_with(2); // 1 | 2
        layout.toggle(id(2));
        layout.set_floating_rect(id(2), Rect::new(100, 120, 300, 200));
        assert!(layout.snap(id(2), ZoneId::LEFT));
        assert_eq!(rect(&layout, 2), Rect::new(0, 0, 400, 600));
        // Moving between zones keeps the original pre-snap rectangle.
        assert!(layout.snap(id(2), ZoneId::TOP_RIGHT));
        assert_eq!(rect(&layout, 2), Rect::new(400, 0, 400, 300));
        assert!(layout.unsnap_restore(id(2)));
        assert_eq!(rect(&layout, 2), Rect::new(100, 120, 300, 200));
        assert_eq!(snapped(&layout, 2), None);
        assert!(!layout.unsnap_restore(id(2)), "not snapped any more");
        layout.check_invariants();
    }

    #[test]
    fn snapping_a_tiled_window_floats_it_with_its_tile_as_pre_snap_rectangle() {
        let mut layout = layout_with(2);
        assert!(layout.snap(id(1), ZoneId::RIGHT));
        assert_eq!(layout.mode(id(1)), Some(Mode::Floating));
        assert_eq!(rect(&layout, 2), AREA, "the other window refills");
        layout.unsnap_restore(id(1));
        assert_eq!(
            rect(&layout, 1),
            Rect::new(0, 0, 400, 600),
            "restored to where its tile was"
        );
        layout.check_invariants();
    }

    #[test]
    fn drag_out_restores_the_size_under_the_pointer() {
        let mut layout = layout_with(1);
        layout.toggle(id(1));
        layout.set_floating_rect(id(1), Rect::new(200, 200, 200, 100));
        layout.snap(id(1), ZoneId::LEFT); // (0, 0, 400, 600)
        // Pointer at 25% across and 10% down the snapped window.
        let r = layout
            .unsnap_for_drag(id(1), (100, 60), (100, 60))
            .expect("was snapped");
        assert_eq!(r, Rect::new(50, 50, 200, 100), "pointer stays at 25% / 10%");
        assert_eq!(snapped(&layout, 1), None);
        assert_eq!(layout.unsnap_for_drag(id(1), (100, 60), (100, 60)), None);
        // Grabbed at 25% / 10%, pointer already moved to (300, 300) on the first motion:
        // the grab point keeps its relative position under the pointer.
        layout.snap(id(1), ZoneId::LEFT);
        let r = layout
            .unsnap_for_drag(id(1), (100, 60), (300, 300))
            .expect("was snapped");
        assert_eq!(r, Rect::new(250, 290, 200, 100));
        layout.check_invariants();
    }

    #[test]
    fn snap_keys_follow_the_state_table() {
        let mut layout = layout_with(2); // 1 | 2, focus 2
        // Tiled: Up maximizes and keeps the leaf; Down restores the tile.
        assert!(layout.snap_key(id(2), Direction::Up));
        assert_eq!(layout.mode(id(2)), Some(Mode::Tiled));
        assert!(layout.is_maximized(id(2)));
        assert_eq!(layout.shown_rect(id(2)), Some(AREA));
        assert_eq!(
            rect(&layout, 1),
            Rect::new(0, 0, 400, 600),
            "the tile stays reserved"
        );
        assert!(layout.snap_key(id(2), Direction::Down));
        assert!(!layout.is_maximized(id(2)));
        assert_eq!(layout.shown_rect(id(2)), Some(Rect::new(400, 0, 400, 600)));
        // Tiled + Left: floats and snaps to the left half.
        assert!(layout.snap_key(id(2), Direction::Left));
        assert_eq!(snapped(&layout, 2), Some(ZoneId::LEFT));
        assert!(layout.snap_key(id(2), Direction::Left));
        assert_eq!(snapped(&layout, 2), Some(ZoneId::BOTTOM_LEFT));
        assert!(layout.snap_key(id(2), Direction::Up));
        assert_eq!(snapped(&layout, 2), Some(ZoneId::TOP_LEFT));
        assert!(layout.snap_key(id(2), Direction::Up));
        assert_eq!(snapped(&layout, 2), Some(ZoneId::FULL));
        assert!(layout.is_maximized(id(2)));
        assert!(layout.snap_key(id(2), Direction::Down));
        assert_eq!(snapped(&layout, 2), None);
        assert_eq!(
            rect(&layout, 2),
            Rect::new(400, 0, 400, 600),
            "pre-snap = its old tile"
        );
        assert!(
            !layout.snap_key(id(2), Direction::Down),
            "unsnapped: Down does nothing"
        );
        // Maximized tiled + Right: floats and snaps to the right half.
        layout.toggle(id(2));
        layout.snap_key(id(2), Direction::Up);
        assert!(layout.snap_key(id(2), Direction::Right));
        assert_eq!(snapped(&layout, 2), Some(ZoneId::RIGHT));
        assert!(!layout.is_maximized(id(2)));
        layout.check_invariants();
    }

    #[test]
    fn tile_all_then_float_again_restores_the_zone_for_the_current_output() {
        let mut layout = layout_with(2);
        layout.snap(id(1), ZoneId::TOP_RIGHT);
        layout.tile_all();
        assert_eq!(layout.mode(id(1)), Some(Mode::Tiled));
        layout.set_area(Rect::new(0, 0, 1000, 801));
        layout.float_all();
        assert_eq!(snapped(&layout, 1), Some(ZoneId::TOP_RIGHT));
        assert_eq!(
            rect(&layout, 1),
            Rect::new(500, 0, 500, 400),
            "recomputed for 1000x801"
        );
        // The same with mod+F.
        layout.tile_all();
        layout.toggle(id(1));
        assert_eq!(rect(&layout, 1), Rect::new(500, 0, 500, 400));
        layout.check_invariants();
    }

    #[test]
    fn output_resize_recomputes_snapped_windows() {
        let mut layout = layout_with(1);
        layout.snap(id(1), ZoneId::BOTTOM_RIGHT);
        layout.set_area(Rect::new(0, 0, 1001, 701));
        assert_eq!(rect(&layout, 1), Rect::new(500, 350, 501, 351));
        layout.check_invariants();
    }

    #[test]
    fn manual_moves_and_resizes_unsnap_and_keep_the_rectangle() {
        let mut layout = layout_with(1);
        layout.snap(id(1), ZoneId::LEFT);
        layout.move_by(id(1), STEP, 0);
        assert_eq!(snapped(&layout, 1), None);
        assert_eq!(rect(&layout, 1), Rect::new(40, 0, 400, 600));
        layout.snap(id(1), ZoneId::RIGHT);
        layout.resize_by(id(1), -STEP, 0);
        assert_eq!(rect(&layout, 1), Rect::new(400, 0, 360, 600));
        layout.check_invariants();
    }

    #[test]
    fn fixed_size_windows_never_snap_or_maximize() {
        let mut layout = MonitorLayout::new(AREA);
        let fixed = NewWindow {
            forced: true,
            limits: SizeLimits {
                min: (300, 200),
                max: (300, 200),
            },
            ..Default::default()
        };
        layout.open(id(1), fixed);
        layout.place(id(1), (300, 200));
        assert!(!layout.snap(id(1), ZoneId::LEFT));
        assert!(!layout.snap_key(id(1), Direction::Up));
        assert!(!layout.set_maximized(id(1), true));
        assert_eq!(rect(&layout, 1), Rect::new(250, 200, 300, 200));
        layout.check_invariants();
    }

    #[test]
    fn client_maximize_on_floating_and_tiled_windows() {
        let mut layout = layout_with(2);
        assert!(layout.set_maximized(id(1), true));
        assert_eq!(
            layout.mode(id(1)),
            Some(Mode::Tiled),
            "tiled windows keep their leaf"
        );
        assert_eq!(
            layout.presentation(id(1)).map(|p| (p.size, p.maximized)),
            Some((Some((800, 600)), true))
        );
        assert!(layout.set_maximized(id(1), false));
        assert_eq!(
            layout.presentation(id(1)).map(|p| (p.size, p.maximized)),
            Some((Some((400, 600)), false))
        );
        layout.toggle(id(2));
        layout.set_floating_rect(id(2), Rect::new(10, 20, 300, 200));
        assert!(layout.set_maximized(id(2), true));
        assert_eq!(snapped(&layout, 2), Some(ZoneId::FULL));
        assert!(layout.set_maximized(id(2), false));
        assert_eq!(rect(&layout, 2), Rect::new(10, 20, 300, 200));
        layout.check_invariants();
    }

    #[test]
    fn one_fullscreen_window_ends_on_focus_and_close() {
        let mut layout = layout_with(3); // 1 | (2 / 3)
        assert!(layout.set_fullscreen(id(2), true));
        assert_eq!(layout.fullscreen(), Some(id(2)));
        assert_eq!(
            layout.stacking().last().map(|(w, r, l)| (*w, *r, *l)),
            Some((id(2), AREA, Layer::Fullscreen))
        );
        assert_eq!(
            rect(&layout, 2),
            Rect::new(400, 0, 400, 300),
            "keeps its leaf and tile"
        );
        assert_eq!(layout.window_at(10, 10), Some(id(2)), "above everything");
        // A new fullscreen window replaces it.
        layout.set_fullscreen(id(3), true);
        assert_eq!(layout.fullscreen(), Some(id(3)));
        // Previewing another window (mod+Tab) does not end it; committing does.
        layout.preview_focus(id(1));
        assert_eq!(layout.fullscreen(), Some(id(3)));
        layout.commit_focus();
        assert_eq!(layout.fullscreen(), None);
        // Closing the fullscreen window restores the layout normally.
        layout.set_fullscreen(id(1), true);
        layout.close(id(1));
        assert_eq!(layout.fullscreen(), None);
        assert_eq!(rect(&layout, 2), Rect::new(0, 0, 800, 300));
        layout.check_invariants();
    }

    #[test]
    fn fixed_size_fullscreen_is_centered_over_a_backdrop() {
        let mut layout = layout_with(1);
        let fixed = NewWindow {
            forced: true,
            limits: SizeLimits {
                min: (300, 200),
                max: (300, 200),
            },
            ..Default::default()
        };
        layout.open(id(2), fixed);
        layout.place(id(2), (300, 200));
        assert!(layout.set_fullscreen(id(2), true));
        assert_eq!(
            layout.shown_rect(id(2)),
            Some(Rect::new(250, 200, 300, 200))
        );
        assert_eq!(layout.fullscreen_backdrop(), Some(AREA));
        assert_eq!(layout.window_at(300, 300), Some(id(2)));
        assert_eq!(layout.window_at(10, 10), None, "the backdrop takes clicks");
        layout.check_invariants();
    }

    #[test]
    fn snapping_on_a_tiny_output_keeps_the_rules() {
        let mut layout = MonitorLayout::new(Rect::new(0, 0, 3, 2));
        for i in 1..=3 {
            layout.open(id(i), NewWindow::default());
        }
        for (i, zone) in [
            (1, ZoneId::LEFT),
            (2, ZoneId::BOTTOM_RIGHT),
            (3, ZoneId::FULL),
        ] {
            assert!(layout.snap(id(i), zone));
            layout.check_invariants();
        }
        layout.set_area(Rect::new(0, 0, 1, 1));
        layout.check_invariants();
    }

    /// Timing of a zone lookup (every pointer motion of a drag) and of snapping and
    /// restoring a window. Run in release: `cargo test --release -- --ignored bench_`.
    #[test]
    #[ignore = "benchmark"]
    fn bench_zone_lookup_and_snap() {
        use std::{hint::black_box, time::Instant};
        let layout = layout_with(3);
        let area = layout.area();
        let n = 1_000_000;
        let start = Instant::now();
        let mut hits = 0;
        for i in 0..n {
            let (x, y) = ((i * 7919) % area.w, (i * 104729) % area.h);
            hits += black_box(layout.zones().at(area, x, y)).is_some() as u32;
        }
        let lookup = start.elapsed();
        let mut layout = layout_with(3);
        let rounds = 100_000;
        let start = Instant::now();
        for i in 0..rounds {
            let zone = [ZoneId::LEFT, ZoneId::TOP_RIGHT, ZoneId::FULL][i % 3];
            black_box(layout.snap(id(3), zone));
            black_box(layout.unsnap_restore(id(3)));
        }
        let snap = start.elapsed();
        println!(
            "zone lookup: {:.1} ns/op ({hits} hits in {n}); snap + restore: {:.1} ns/op",
            lookup.as_nanos() as f64 / n as f64,
            snap.as_nanos() as f64 / rounds as f64
        );
    }

    /// Timing of a tiled resize step during a drag (ratio update) and of recomputing
    /// every tiled rectangle afterwards. Run in release:
    /// `cargo test --release -- --ignored bench_`.
    #[test]
    #[ignore = "benchmark"]
    fn bench_tiled_resize() {
        use std::{hint::black_box, time::Instant};
        for n in [3, 16] {
            let mut layout = layout_with(n);
            layout.set_area(Rect::new(0, 0, 2560, 1440));
            let resize = layout
                .begin_tiled_resize(id(n - 1), Edges::BOTTOM_RIGHT)
                .expect("window n-1 has splits");
            let rounds = 1_000_000;
            let start = Instant::now();
            for i in 0..rounds {
                let d = i % 200 - 100;
                black_box(layout.drag_tiled_resize(&resize, d, -d));
            }
            let update = start.elapsed();
            let start = Instant::now();
            for _ in 0..rounds {
                black_box(layout.tiled_rects());
            }
            let rects = start.elapsed();
            println!(
                "{n} windows: ratio update {:.1} ns/op; recompute all rects {:.1} ns/op",
                update.as_nanos() as f64 / rounds as f64,
                rects.as_nanos() as f64 / rounds as f64
            );
        }
    }
}
