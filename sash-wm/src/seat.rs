// SPDX-License-Identifier: Apache-2.0

//! The seat: keybinds, pointer focus, and forwarding backend input to clients.
//! Keyboard focus follows the layout's focus (see `windows.rs`).

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, ButtonState, Event, InputBackend, InputEvent, KeyState,
        KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
    },
    delegate_seat,
    input::{
        Seat, SeatHandler, SeatState,
        keyboard::{FilterResult, Keycode, Keysym},
        pointer::{AxisFrame, ButtonEvent, CursorImageStatus, GrabStartData, MotionEvent},
    },
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
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
}

delegate_seat!(Sash);

impl Sash {
    /// Moves the pointer to `location` (global logical coordinates) and updates pointer focus.
    pub fn pointer_moved_to(&mut self, location: Point<f64, Logical>, time_msec: u32) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let mut focus = self.surface_at(location);
        // While a popup chain is grabbed, other clients get no pointer events.
        if let Some(root) = self.popups.grab_root()
            && focus
                .as_ref()
                .is_some_and(|(surface, _)| !surface.id().same_client_as(&root.id()))
        {
            focus = None;
        }
        let previous = pointer.current_focus();
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
        // A cursor sash-wm draws follows the pointer.
        if self.cursor_surface().is_some() {
            self.request_redraw();
        }
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
            |state, mods, key| match key_state {
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
                    match keybinds::lookup(modifier, mods, key.raw_latin_sym_or_raw_current_sym()) {
                        Some(action) => {
                            state.keybinds.swallowed.push(code);
                            FilterResult::Intercept(Some(Intercepted::Action(action)))
                        }
                        None => {
                            // A key press can justify a popup grab (keyboard menus).
                            if let Some(focus) = &focus {
                                state.popups.note_key_press(serial, focus.clone());
                            }
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
                        None => FilterResult::Forward,
                    }
                }
            },
        );
        match action.flatten() {
            Some(Intercepted::Action(action)) => self.run_action(action),
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
                    && let Some(window) = under
                {
                    self.focus_window(window);
                }
                if let Some(focus) = pointer.current_focus() {
                    self.popups.note_button_press(serial, focus);
                }
            }
            ButtonState::Released => {
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
                self.pointer_moved_to(location, event.time_msec());
            }
            InputEvent::PointerMotion { event } => {
                // Relative motion (libinput): moves the pointer, kept on the output.
                let (Some(pointer), Some(output)) = (self.seat.get_pointer(), self.output.clone())
                else {
                    return;
                };
                let Some(area) = self.space.output_geometry(&output) else {
                    return;
                };
                let moved = pointer.current_location() + event.delta();
                let x = moved
                    .x
                    .clamp(area.loc.x as f64, (area.loc.x + area.size.w) as f64 - 1.0);
                let y = moved
                    .y
                    .clamp(area.loc.y as f64, (area.loc.y + area.size.h) as f64 - 1.0);
                self.pointer_moved_to((x, y).into(), event.time_msec());
            }
            InputEvent::PointerButton { event } => {
                self.pointer_button(event.button_code(), event.state(), event.time_msec());
            }
            InputEvent::PointerAxis { event } => {
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
