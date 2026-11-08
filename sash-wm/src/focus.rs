// SPDX-License-Identifier: Apache-2.0

//! Focus follows the cursor, sloppy (owner, M19a; docs/m19-investigation.md, B):
//!
//! - One real motion that enters a window other than the focused one focuses it, from
//!   a window or from a gap. "Enters" is judged at the current geometry: the window
//!   under the old position and the one under the new position differ. A window that
//!   slides under a still pointer never takes focus, and a keyboard focus move stays
//!   until the pointer enters a different window.
//! - Gaps, the wallpaper and layer surfaces (the dock) keep the focus.
//! - Pointer focus never raises a window and never ends fullscreen; clicks raise.
//! - Holds (motion never moves focus): a pointer grab (move, resize, drag and drop, a
//!   held button), a mod+click whose button is still down, a grabbed popup chain, a
//!   layer surface holding the keyboard, a mod+Tab cycle until mod is released and the
//!   first real motion after it. The lock screen is a hold once it exists.
//!
//! Also here: the xdg_toplevel activated state (the window with keyboard focus), the
//! pointer refresh after layout changes, and the interim typing guard.

use std::time::{Duration, Instant};

use smithay::{
    input::pointer::{CursorImageStatus, MotionEvent},
    reexports::wayland_server::Resource,
    utils::{Logical, Point, SERIAL_COUNTER},
};

use crate::{layout::WindowId, state::Sash, windows::SurfaceHit};

/// Interim focus-steal guard (owner, M19a; replaced by xdg-activation tokens in E5): a
/// window that maps within this long after keyboard input maps unfocused.
pub const TYPING_GUARD: Duration = Duration::from_millis(377);

pub struct Focus {
    /// The "focus follows cursor" setting (closed settings list; default on). A value
    /// in the state until the settings protocol exists. Debug builds only:
    /// SASH_DEBUG_FOCUS_FOLLOWS_POINTER=0 turns it off.
    pub follows_pointer: bool,
    /// A mod+Tab cycle ended: the first real motion after it is still held.
    cycle_hold: bool,
    /// The typing guard's window (TYPING_GUARD; the test harness sets zero).
    pub typing_guard: Duration,
    /// The last keystroke delivered to a client (press or release; modifier keys
    /// alone and compositor keybinds are not typing).
    last_key: Option<Instant>,
    /// A keybind ran and no window has mapped since (nor was a key typed since): the
    /// next window to map is exempt from the guard.
    after_keybind: bool,
    /// The surface the pointer was last given and its origin, to find out whether a
    /// layout change moved what is under a still pointer.
    target: Option<SurfaceHit>,
    /// The time of the last pointer event (a refresh reuses it).
    last_time: u32,
    /// A pointer refresh is scheduled (an idle callback).
    refresh_armed: bool,
    /// Counts changes that can move what is under the pointer: layout changes and
    /// surface commits.
    generation: u64,
    /// The window under the pointer as of the last motion or refresh, and the
    /// generation it was found at; valid only while the generation is unchanged.
    under: (u64, Option<WindowId>),
}

impl Focus {
    pub fn new() -> Self {
        Self {
            follows_pointer: follows_pointer_default(),
            cycle_hold: false,
            typing_guard: TYPING_GUARD,
            last_key: None,
            after_keybind: false,
            target: None,
            last_time: 0,
            refresh_armed: false,
            generation: 0,
            under: (u64::MAX, None),
        }
    }

    /// What is under the pointer may have changed.
    pub fn geometry_changed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// A key event was delivered to a client: typing, unless it is a modifier key
    /// (Alt and Super are delivered around every chord).
    pub fn note_delivered(&mut self, sym: smithay::input::keyboard::Keysym) {
        if !sym.is_modifier_key() {
            self.last_key = Some(Instant::now());
            self.after_keybind = false;
        }
    }

    /// A compositor keybind ran (it launches or controls something; never typing).
    pub fn note_keybind(&mut self) {
        self.after_keybind = true;
    }

    /// A window maps: whether the guard keeps it unfocused. Keystrokes delivered to
    /// clients within the guard's window count, except for the first window to map
    /// after a keybind (with nothing typed since).
    pub fn guard_on_map(&mut self) -> bool {
        if std::mem::take(&mut self.after_keybind) {
            return false;
        }
        self.last_key
            .is_some_and(|at| at.elapsed() < self.typing_guard)
    }

    /// A mod+Tab cycle was committed.
    pub fn cycle_ended(&mut self) {
        self.cycle_hold = true;
    }
}

/// On, unless SASH_DEBUG_FOCUS_FOLLOWS_POINTER=0 in a debug build (absent from release
/// builds, tools/check-release.sh).
fn follows_pointer_default() -> bool {
    #[cfg(debug_assertions)]
    if std::env::var("SASH_DEBUG_FOCUS_FOLLOWS_POINTER").as_deref() == Ok("0") {
        tracing::info!("debug override: focus follows the cursor off");
        return false;
    }
    true
}

impl Sash {
    /// What the pointer at `location` is over: the window there (as `window_at`) and,
    /// for wl_pointer, the surface there (as `surface_at`), or no surface outside a
    /// grabbed popup chain's client. One hit test for both.
    pub fn pointer_hit(
        &self,
        location: Point<f64, Logical>,
    ) -> (Option<WindowId>, Option<SurfaceHit>) {
        let (window, surface) = match self.layer_above(location) {
            Some((_, surface)) => (None, Some(surface)),
            None => match self.hit(location) {
                Some((id, surface)) => (Some(id), surface),
                None => (None, self.layer_below(location).map(|(_, s)| s)),
            },
        };
        (window, self.outside_grab_filtered(surface))
    }

    fn outside_grab_filtered(&self, focus: Option<SurfaceHit>) -> Option<SurfaceHit> {
        match self.popups.grab_root() {
            Some(root)
                if focus
                    .as_ref()
                    .is_some_and(|(surface, _)| !surface.id().same_client_as(&root.id())) =>
            {
                None
            }
            _ => focus,
        }
    }

    /// Remembers what wl_pointer was given (for the refresh), the window under the
    /// pointer at the current generation, and the event time.
    pub fn note_pointer_target(
        &mut self,
        window: Option<WindowId>,
        target: Option<SurfaceHit>,
        time_msec: u32,
    ) {
        self.focus.target = target;
        self.focus.under = (self.focus.generation, window);
        self.focus.last_time = time_msec;
    }

    /// The window under `from` (the pointer's position before a motion) at the
    /// current geometry: the one found at the last motion if nothing changed since,
    /// else a new hit test.
    pub fn window_before(&self, from: Point<f64, Logical>) -> Option<WindowId> {
        match self.focus.under {
            (generation, window) if generation == self.focus.generation => window,
            _ => self.window_at(from),
        }
    }

    /// Whether pointer motion may move focus now (no hold).
    fn pointer_focus_held(&self) -> bool {
        self.seat.get_pointer().is_some_and(|p| p.is_grabbed())
            || !self.windows.swallowed_buttons.is_empty()
            || self.clipboard.drag.is_some()
            || self.popups.grab_root().is_some()
            || self.layer_keyboard_target().is_some()
            || self.keybinds.cycle.is_some()
    }

    /// A real motion, after the pointer was moved. `before` is the window that was
    /// under the old position at the current geometry, `after` the one under the new
    /// position (both None over a gap, the wallpaper or a layer surface above windows).
    pub fn pointer_crossed(&mut self, before: Option<WindowId>, after: Option<WindowId>) {
        if std::mem::take(&mut self.focus.cycle_hold) {
            return;
        }
        if !self.focus.follows_pointer || self.pointer_focus_held() {
            return;
        }
        let Some(after) = after else {
            return;
        };
        if Some(after) == before || Some(after) == self.windows.layout.focused() {
            return;
        }
        if !self.windows.by_id(after).is_some_and(|m| m.mapped) {
            return;
        }
        record_context!(self, "pointer");
        if self.windows.layout.pointer_focus(after) {
            // No geometry changes (no raise, fullscreen untouched): the keyboard and
            // the activated state follow, and the bar tones, outlines and dim redraw.
            self.sync_keyboard_focus();
            self.request_redraw();
            #[cfg(feature = "debug-record")]
            if self.recorder.is_some() {
                let now = crate::record::layout_json(self);
                if let Some(recorder) = self.recorder.as_mut() {
                    recorder.layout_after_arrange(now);
                }
            }
        }
    }

    /// The window that has keyboard focus (itself or one of its popups): the layout's
    /// focused window when no layer surface holds the keyboard.
    pub fn activated_window(&self) -> Option<WindowId> {
        if self.layer_keyboard_target().is_some() {
            return None;
        }
        self.windows
            .layout
            .focused()
            .filter(|id| self.windows.by_id(*id).is_some_and(|m| m.mapped))
    }

    /// Asks for a pointer refresh after the current dispatch: the layout changed, so
    /// what is under a still pointer may have changed.
    pub fn schedule_pointer_refresh(&mut self) {
        self.focus.geometry_changed();
        if self.focus.refresh_armed || !self.pointer_has_moved {
            return;
        }
        self.focus.refresh_armed = true;
        self.loop_handle
            .insert_idle(|server| server.state.refresh_pointer());
    }

    /// Gives wl_pointer to the surface under the still pointer if that changed (leave
    /// and enter, or a motion when the surface moved under it), and updates the hover
    /// and the cursor. Never a real motion: keyboard focus and the moved flag stay.
    /// Skipped while a grab owns the pointer.
    pub fn refresh_pointer(&mut self) {
        self.focus.refresh_armed = false;
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        if pointer.is_grabbed() {
            return;
        }
        let location = pointer.current_location();
        let (window, target) = self.pointer_hit(location);
        self.focus.under = (self.focus.generation, window);
        if target == self.focus.target {
            return;
        }
        let previous = pointer.current_focus();
        let motion = MotionEvent {
            location,
            serial: SERIAL_COUNTER.next_serial(),
            time: self.focus.last_time,
        };
        self.focus.target = target.clone();
        pointer.motion(self, target, &motion);
        pointer.frame(self);
        if pointer.current_focus() != previous {
            self.pointer_focus_changed();
        }
        self.decoration_hover(location);
        record!(self, "pointer_refresh", "window" => window);
        if matches!(self.cursor.image(), CursorImageStatus::Surface(_)) {
            self.request_redraw();
        }
    }
}
