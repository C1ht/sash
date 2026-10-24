// SPDX-License-Identifier: Apache-2.0

//! The seat: keybinds, pointer focus, and forwarding backend input to clients.
//! Keyboard focus follows the layout's focus (see `tiling.rs`).

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, Event, InputBackend, InputEvent, KeyState, KeyboardKeyEvent,
        PointerAxisEvent, PointerButtonEvent,
    },
    delegate_seat,
    input::{
        SeatHandler, SeatState,
        keyboard::{FilterResult, Keycode},
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, SERIAL_COUNTER},
};

use crate::{keybinds, state::Sash};

impl SeatHandler for Sash {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.protocols.seat
    }
}

delegate_seat!(Sash);

impl Sash {
    /// Moves the pointer to `location` (global logical coordinates) and updates pointer focus.
    pub fn pointer_moved_to(&mut self, location: Point<f64, Logical>, time_msec: u32) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let focus = self.surface_at(location);
        let motion = MotionEvent {
            location,
            serial: SERIAL_COUNTER.next_serial(),
            time: time_msec,
        };
        pointer.motion(self, focus, &motion);
        pointer.frame(self);
    }

    /// Handles a key press or release: runs a keybind, or forwards it to the focused
    /// client. A key whose press ran a keybind has its release swallowed too.
    pub fn key(&mut self, code: Keycode, key_state: KeyState, time_msec: u32) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let modifier = self.keybinds.modifier;
        let action = keyboard.input(
            self,
            code,
            key_state,
            SERIAL_COUNTER.next_serial(),
            time_msec,
            |state, mods, key| match key_state {
                KeyState::Pressed => {
                    match keybinds::lookup(modifier, mods, key.raw_latin_sym_or_raw_current_sym()) {
                        Some(action) => {
                            state.keybinds.swallowed.push(code);
                            FilterResult::Intercept(Some(action))
                        }
                        None => FilterResult::Forward,
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
        if let Some(action) = action.flatten() {
            self.run_action(action);
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
                self.pointer_moved_to(location, event.time_msec());
            }
            InputEvent::PointerButton { event } => {
                let Some(pointer) = self.seat.get_pointer() else {
                    return;
                };
                let button = ButtonEvent {
                    button: event.button_code(),
                    state: event.state(),
                    serial: SERIAL_COUNTER.next_serial(),
                    time: event.time_msec(),
                };
                // Buttons are only delivered; click-to-focus comes with M6.
                pointer.button(self, &button);
                pointer.frame(self);
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
