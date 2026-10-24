// SPDX-License-Identifier: Apache-2.0

//! The seat: keyboard and pointer focus, and forwarding backend input to clients.

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, Event, InputBackend, InputEvent, KeyboardKeyEvent,
        PointerAxisEvent, PointerButtonEvent,
    },
    delegate_seat,
    desktop::{Window, WindowSurfaceType},
    input::{
        SeatHandler, SeatState,
        keyboard::FilterResult,
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, SERIAL_COUNTER},
};

use crate::state::Sash;

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
    /// Gives keyboard focus to `window` (or to nothing) and raises it.
    pub fn focus_window(&mut self, window: Option<&Window>) {
        let surface = window
            .and_then(|w| w.toplevel())
            .map(|t| t.wl_surface().clone());
        if let Some(window) = window {
            self.space.raise_element(window, true);
        }
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, surface, SERIAL_COUNTER.next_serial());
        }
    }

    /// Focuses the top window in the stacking order, if any.
    pub fn focus_topmost_window(&mut self) {
        let top = self.space.elements().last().cloned();
        self.focus_window(top.as_ref());
    }

    /// The surface (of any window, including subsurfaces and popups) at a global
    /// position, with the global position of that surface's origin.
    fn surface_at(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let (window, window_pos) = self.space.element_under(location)?;
        let window_pos = window_pos.to_f64();
        let (surface, pos_in_window) =
            window.surface_under(location - window_pos, WindowSurfaceType::ALL)?;
        Some((surface, window_pos + pos_in_window.to_f64()))
    }

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

    /// Forwards one input event from the backend to the focused clients.
    pub fn handle_input<B: InputBackend>(&mut self, event: InputEvent<B>) {
        match event {
            InputEvent::Keyboard { event } => {
                let Some(keyboard) = self.seat.get_keyboard() else {
                    return;
                };
                keyboard.input::<(), _>(
                    self,
                    event.key_code(),
                    event.state(),
                    SERIAL_COUNTER.next_serial(),
                    event.time_msec(),
                    |_, _, _| FilterResult::Forward,
                );
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
