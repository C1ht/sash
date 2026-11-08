// SPDX-License-Identifier: Apache-2.0

//! The seat: keybinds, pointer focus, and forwarding backend input to clients.
//! Keyboard focus follows the layout's focus (see `windows.rs`).

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, ButtonState, Event, InputBackend, InputEvent, KeyState,
        KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
    },
    input::{
        Seat, SeatHandler, SeatState,
        keyboard::{FilterResult, Keycode, Keysym},
        pointer::{AxisFrame, ButtonEvent, CursorImageStatus, GrabStartData, MotionEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, SERIAL_COUNTER},
};

use crate::{
    keybinds::{self, Action},
    state::Sash,
    windows::DragKind,
};

/// Linux input event codes (linux/input-event-codes.h).
pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;

/// A key press sash-wm keeps: a keybind, or a VT switch for the session.
enum Intercepted {
    Action(Action),
    Vt(i32),
    /// mod+F11 with debug recording on: mark a bug and write a bundle.
    #[cfg(feature = "debug-record")]
    DebugMark,
    /// Escape during a drag and drop: cancel it.
    CancelDrag,
}

impl SeatHandler for Sash {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.protocols.seat
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.set_cursor(image);
    }

    /// Keyboard focus moved: the selection is offered to the newly focused client only.
    fn focus_changed(&mut self, _seat: &Seat<Self>, focused: Option<&WlSurface>) {
        self.data_focus_changed(crate::data_device::client_of(focused));
    }
}

// What `delegate_seat!` does, with binds counted (limits.rs).
crate::limits::counted_global!(smithay::reexports::wayland_server::protocol::wl_seat::WlSeat: smithay::wayland::seat::SeatGlobalData<Sash> => smithay::input::SeatState<Sash>);
smithay::reexports::wayland_server::delegate_dispatch!(Sash: [smithay::reexports::wayland_server::protocol::wl_seat::WlSeat: smithay::wayland::seat::SeatUserData<Sash>] => smithay::input::SeatState<Sash>);
smithay::reexports::wayland_server::delegate_dispatch!(Sash: [smithay::reexports::wayland_server::protocol::wl_pointer::WlPointer: smithay::wayland::seat::PointerUserData<Sash>] => smithay::input::SeatState<Sash>);
smithay::reexports::wayland_server::delegate_dispatch!(Sash: [smithay::reexports::wayland_server::protocol::wl_keyboard::WlKeyboard: smithay::wayland::seat::KeyboardUserData<Sash>] => smithay::input::SeatState<Sash>);
smithay::reexports::wayland_server::delegate_dispatch!(Sash: [smithay::reexports::wayland_server::protocol::wl_touch::WlTouch: smithay::wayland::seat::TouchUserData<Sash>] => smithay::input::SeatState<Sash>);

impl Sash {
    /// Moves the pointer to `location` (global logical coordinates) and updates pointer focus.
    pub fn pointer_moved_to(&mut self, location: Point<f64, Logical>, time_msec: u32) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        // While a popup chain is grabbed, other clients get no pointer events.
        let (after, focus) = self.pointer_hit(location);
        let previous = pointer.current_focus();
        let from = pointer.current_location();
        let moved = from != location;
        // Focus follows the cursor: the window under the old position, at the current
        // geometry (a window that slid under a still pointer is "before" too). The
        // startup position is no position (as for rule A): the first motion enters.
        let before = (moved && self.pointer_has_moved)
            .then(|| self.window_before(from))
            .flatten();
        self.pointer_has_moved |= moved;
        self.note_pointer_target(after, focus.clone(), time_msec);
        let motion = MotionEvent {
            location,
            serial: SERIAL_COUNTER.next_serial(),
            time: time_msec,
        };
        pointer.motion(self, focus, &motion);
        pointer.frame(self);
        if pointer.current_focus() != previous {
            self.pointer_focus_changed();
        }
        // After the focus change, which resets the cursor to the arrow.
        if !pointer.is_grabbed() {
            self.decoration_hover(location);
        }
        if moved {
            self.pointer_crossed(before, after);
        }
        #[cfg(feature = "debug-record")]
        if let Some(recorder) = &self.recorder {
            crate::record::Counters::bump(&recorder.counters.position_updates);
        }
        record!(
            self,
            "pointer_position",
            "x" => location.x,
            "y" => location.y,
            "window" => self.window_at(location),
            "grabbed" => pointer.is_grabbed(),
        );
        // A cursor sash-wm draws (a client's cursor surface; on DRM also the arrow)
        // follows the pointer: one frame, merged until the next frame starts.
        let drawn = match self.cursor.image() {
            CursorImageStatus::Surface(_) => true,
            CursorImageStatus::Named(_) => self.draws_named_cursor,
            CursorImageStatus::Hidden => false,
        };
        if moved && drawn && self.cursor_frames.moved() {
            #[cfg(feature = "debug-record")]
            if let Some(recorder) = &self.recorder {
                crate::record::Counters::bump(&recorder.counters.pointer_frame_requests);
            }
            self.request_redraw();
        }
    }

    /// Relative motion (libinput): moves the pointer by `delta`, kept on the output. At
    /// an edge the clamped position stays where it was, which is no motion.
    pub fn pointer_moved_by(&mut self, delta: Point<f64, Logical>, time_msec: u32) {
        let (Some(pointer), Some(output)) = (self.seat.get_pointer(), self.output.clone()) else {
            return;
        };
        let Some(area) = self.space.output_geometry(&output) else {
            return;
        };
        let moved = pointer.current_location() + delta;
        let x = moved
            .x
            .clamp(area.loc.x as f64, (area.loc.x + area.size.w) as f64 - 1.0);
        let y = moved
            .y
            .clamp(area.loc.y as f64, (area.loc.y + area.size.h) as f64 - 1.0);
        self.pointer_moved_to((x, y).into(), time_msec);
    }

    /// Handles a key press or release: runs a keybind, or forwards it to the focused
    /// client. A key whose press ran a keybind has its release swallowed too.
    pub fn key(&mut self, code: Keycode, key_state: KeyState, time_msec: u32) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let modifier = self.keybinds.modifier;
        let serial = SERIAL_COUNTER.next_serial();
        let focus = keyboard.current_focus();
        let action = keyboard.input(
            self,
            code,
            key_state,
            serial,
            time_msec,
            |state, mods, key| {
                record!(
                    state,
                    "key",
                    "pressed" => key_state == KeyState::Pressed,
                    "key" => crate::record::key_label(
                        key.modified_sym(),
                        mods,
                        state.recorder.as_ref().is_some_and(|r| r.keys_full()),
                    ),
                    "mods" => crate::record::mods_json(mods),
                );
                match key_state {
                    KeyState::Pressed => {
                        // Ctrl+Alt+F1..F12 (the keymap's VT switch keys) go to the session
                        // first, so the keyboard can never be trapped (DRM backend only).
                        let sym = key.modified_sym().raw();
                        let first = Keysym::XF86_Switch_VT_1.raw();
                        if state.vt_switch.is_some() && (first..first + 12).contains(&sym) {
                            state.keybinds.swallowed.push(code);
                            return FilterResult::Intercept(Some(Intercepted::Vt(
                                (sym - first + 1) as i32,
                            )));
                        }
                        // Escape cancels a drag and drop (never delivered).
                        if state.clipboard.drag.is_some() && key.modified_sym() == Keysym::Escape {
                            state.keybinds.swallowed.push(code);
                            return FilterResult::Intercept(Some(Intercepted::CancelDrag));
                        }
                        // mod+F11 marks a bug for the debug recorder (never a client's key).
                        #[cfg(feature = "debug-record")]
                        if state.recorder.is_some()
                            && modifier.held(mods)
                            && key.modified_sym() == Keysym::F11
                        {
                            state.keybinds.swallowed.push(code);
                            return FilterResult::Intercept(Some(Intercepted::DebugMark));
                        }
                        match keybinds::lookup(
                            modifier,
                            mods,
                            key.raw_latin_sym_or_raw_current_sym(),
                        ) {
                            // While an exclusive layer surface holds the keyboard,
                            // window keybinds do nothing (the key is still kept).
                            Some(action)
                                if !action.is_global() && state.exclusive_layer().is_some() =>
                            {
                                state.keybinds.swallowed.push(code);
                                tracing::debug!(?action, "keybind blocked: exclusive layer surface");
                                record!(state, "keybind_blocked", "action" => format!("{action:?}"));
                                FilterResult::Intercept(None)
                            }
                            Some(action) => {
                                state.keybinds.swallowed.push(code);
                                FilterResult::Intercept(Some(Intercepted::Action(action)))
                            }
                            None => {
                                // A key press can justify a popup grab (keyboard menus).
                                if let Some(focus) = &focus {
                                    state.popups.note_key_press(serial, focus.clone());
                                }
                                state.focus.note_delivered(key.modified_sym());
                                FilterResult::Forward
                            }
                        }
                    }
                    KeyState::Released => {
                        let swallowed = &mut state.keybinds.swallowed;
                        match swallowed.iter().position(|c| *c == code) {
                            Some(index) => {
                                swallowed.swap_remove(index);
                                FilterResult::Intercept(None)
                            }
                            None => {
                                state.focus.note_delivered(key.modified_sym());
                                FilterResult::Forward
                            }
                        }
                    }
                }
            },
        );
        match action.flatten() {
            Some(Intercepted::Action(action)) => {
                record!(self, "keybind", "action" => format!("{action:?}"));
                // A keybind that changes focus cancels a drag and drop first.
                if action.changes_focus() {
                    self.cancel_drag("focus keybind");
                }
                self.run_action(action)
            }
            Some(Intercepted::CancelDrag) => self.cancel_drag("Escape"),
            #[cfg(feature = "debug-record")]
            Some(Intercepted::DebugMark) => crate::record::mark(self),
            Some(Intercepted::Vt(vt)) => {
                if let Some(switch) = self.vt_switch.as_mut() {
                    tracing::info!(vt, "switching VT");
                    switch(vt);
                }
            }
            None => {}
        }
        // Releasing the modifier ends a mod+Tab cycle.
        if self.keybinds.cycle.is_some() && !modifier.held(&keyboard.modifier_state()) {
            self.commit_cycle();
        }
    }

    /// Whether Shift is held (disables edge snapping during a drag).
    pub fn shift_held(&self) -> bool {
        self.seat
            .get_keyboard()
            .is_some_and(|k| k.modifier_state().shift)
    }

    /// Handles a pointer button. A plain press on a window focuses and raises it and is
    /// delivered. mod+left-drag moves and mod+right-drag resizes; mod+clicks are never
    /// delivered, nor are their releases. While a popup chain is grabbed, a press
    /// outside it dismisses it and is not delivered; a mod+click dismisses it too,
    /// except a resize of its own window.
    pub fn pointer_button(&mut self, button: u32, state: ButtonState, time_msec: u32) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let serial = SERIAL_COUNTER.next_serial();
        let location = pointer.current_location();
        record!(
            self,
            "pointer_button",
            "button" => button,
            "pressed" => state == ButtonState::Pressed,
            "x" => location.x,
            "y" => location.y,
        );
        match state {
            ButtonState::Pressed if !pointer.is_grabbed() => {
                let mods = self
                    .seat
                    .get_keyboard()
                    .map(|k| k.modifier_state())
                    .unwrap_or_default();
                let modifier = self.keybinds.modifier;
                let under = self.window_at(location);
                let grab_window = self
                    .popups
                    .grab_root()
                    .and_then(|root| self.windows.by_surface(&root).map(|m| m.id));
                if modifier.held(&mods) {
                    // Every mod+click is kept from clients, bound or not.
                    self.windows.swallowed_buttons.push(button);
                    // Window mouse bindings do nothing under an exclusive layer surface.
                    if self.exclusive_layer().is_some() {
                        return;
                    }
                    let action = keybinds::lookup_button(modifier, &mods, button);
                    // A resize keeps the window's popups (they follow it); any other
                    // mod+click dismisses them, and a grabbed chain elsewhere.
                    let resize = action == Some(Action::ResizeDrag);
                    if !(resize && grab_window.is_some() && grab_window == under) {
                        self.dismiss_grab();
                    }
                    if !resize && let Some(window) = under {
                        self.dismiss_popups(window);
                    }
                    let kind = match action {
                        Some(Action::MoveDrag) => Some(DragKind::Move),
                        Some(Action::ResizeDrag) => under
                            .and_then(|w| self.resize_edges_at(w, location))
                            .map(DragKind::Resize),
                        _ => None,
                    };
                    if let (Some(window), Some(kind)) = (under, kind) {
                        let start = GrabStartData {
                            focus: None,
                            button,
                            location,
                        };
                        self.start_pointer_drag(window, kind, start, serial);
                    }
                    return;
                }
                // A press on decorations is the compositor's (never delivered).
                if grab_window.is_none()
                    && let Some((window, part)) = self.decoration_at(location)
                {
                    self.windows.swallowed_buttons.push(button);
                    self.decoration_press(window, part, button, location, serial);
                    return;
                }
                if grab_window.is_some() {
                    let in_chain = self
                        .surface_at(location)
                        .is_some_and(|(surface, _)| self.popups.in_grab_chain(&surface));
                    if !in_chain {
                        // Outside the grabbed chain: dismiss it; the click is not
                        // delivered, nor its release.
                        self.dismiss_grab();
                        self.windows.swallowed_buttons.push(button);
                        return;
                    }
                } else if keybinds::lookup_button(modifier, &mods, button)
                    == Some(Action::ClickFocus)
                {
                    if let Some(window) = under {
                        record_context!(self, "click");
                        self.focus_window(window);
                    } else if let Some(layer) = self.layer_at(location) {
                        self.layer_clicked(layer);
                    }
                }
                if let Some(focus) = pointer.current_focus() {
                    self.popups.note_button_press(serial, focus);
                }
            }
            ButtonState::Released => {
                self.decoration_release(button, location);
                let swallowed = &mut self.windows.swallowed_buttons;
                if let Some(index) = swallowed.iter().position(|b| *b == button) {
                    swallowed.swap_remove(index);
                    // A drag grab still needs the release to end.
                    if !pointer.is_grabbed() {
                        return;
                    }
                }
            }
            ButtonState::Pressed => {}
        }
        let event = ButtonEvent {
            button,
            state,
            serial,
            time: time_msec,
        };
        pointer.button(self, &event);
        pointer.frame(self);
    }

    /// Counts a pointer event from the backend (debug recording).
    fn count_pointer_in(&self) {
        #[cfg(feature = "debug-record")]
        if let Some(recorder) = &self.recorder {
            crate::record::Counters::bump(&recorder.counters.pointer_in);
        }
    }

    /// Forwards one input event from the backend to the focused clients.
    pub fn handle_input<B: InputBackend>(&mut self, event: InputEvent<B>) {
        match event {
            InputEvent::Keyboard { event } => {
                self.key(event.key_code(), event.state(), event.time_msec());
            }
            InputEvent::PointerMotionAbsolute { event } => {
                let Some(output) = self.output.clone() else {
                    return;
                };
                let Some(area) = self.space.output_geometry(&output) else {
                    return;
                };
                let location = event.position_transformed(area.size) + area.loc.to_f64();
                self.count_pointer_in();
                record!(self, "pointer_in", "type" => "absolute", "x" => event.x(), "y" => event.y());
                self.pointer_moved_to(location, event.time_msec());
            }
            InputEvent::PointerMotion { event } => {
                self.count_pointer_in();
                record!(
                    self,
                    "pointer_in",
                    "type" => "relative",
                    "dx" => event.delta_x(),
                    "dy" => event.delta_y(),
                    "dx_unaccel" => event.delta_x_unaccel(),
                    "dy_unaccel" => event.delta_y_unaccel(),
                );
                self.pointer_moved_by(event.delta(), event.time_msec());
            }
            InputEvent::PointerButton { event } => {
                self.count_pointer_in();
                record!(
                    self,
                    "pointer_in",
                    "type" => "button",
                    "button" => event.button_code(),
                    "pressed" => event.state() == ButtonState::Pressed,
                );
                self.pointer_button(event.button_code(), event.state(), event.time_msec());
            }
            InputEvent::PointerAxis { event } => {
                self.count_pointer_in();
                record!(
                    self,
                    "pointer_in",
                    "type" => "axis",
                    "vertical" => event.amount(Axis::Vertical),
                    "horizontal" => event.amount(Axis::Horizontal),
                );
                let Some(pointer) = self.seat.get_pointer() else {
                    return;
                };
                let mut frame = AxisFrame::new(event.time_msec()).source(event.source());
                for axis in [Axis::Horizontal, Axis::Vertical] {
                    if let Some(amount) = event.amount(axis) {
                        frame = frame.value(axis, amount);
                    }
                    if let Some(steps) = event.amount_v120(axis) {
                        frame = frame.v120(axis, steps as i32);
                    }
                }
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            _ => {}
        }
    }
}
