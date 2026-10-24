// SPDX-License-Identifier: Apache-2.0

//! The pointer grab for interactive moves and resizes, started by mod+drag or by a
//! client's xdg_toplevel.move / .resize request. While it runs, clients get no pointer
//! events; releasing the button that started it ends it.

use smithay::{
    backend::input::ButtonState,
    input::{
        SeatHandler,
        pointer::{
            AxisFrame, ButtonEvent, Focus, GestureHoldBeginEvent, GestureHoldEndEvent,
            GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
            GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData,
            MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
        },
    },
    utils::{Logical, Point, Serial},
};

use crate::{
    layout::{Rect, WindowId},
    state::Sash,
    windows::{DragKind, Pointer},
};

type PointerFocus = Option<(<Sash as SeatHandler>::PointerFocus, Point<f64, Logical>)>;

pub struct WindowGrab {
    start: GrabStartData<Sash>,
    window: WindowId,
    kind: DragKind,
    /// The window's rectangle when the drag started.
    from: Rect,
}

impl PointerGrab<Sash> for WindowGrab {
    fn motion(
        &mut self,
        data: &mut Sash,
        handle: &mut PointerInnerHandle<'_, Sash>,
        _focus: PointerFocus,
        event: &MotionEvent,
    ) {
        // Clients see the pointer leave for the duration of the drag.
        handle.motion(data, None, event);
        let pointer = Pointer {
            pressed_at: self.start.location,
            now: event.location,
        };
        if !data.drag_to(self.window, self.kind, self.from, pointer) {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn relative_motion(
        &mut self,
        data: &mut Sash,
        handle: &mut PointerInnerHandle<'_, Sash>,
        _focus: PointerFocus,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, None, event);
    }

    fn button(
        &mut self,
        data: &mut Sash,
        handle: &mut PointerInnerHandle<'_, Sash>,
        event: &ButtonEvent,
    ) {
        if event.state == ButtonState::Released && event.button == self.start.button {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(
        &mut self,
        data: &mut Sash,
        handle: &mut PointerInnerHandle<'_, Sash>,
        details: AxisFrame,
    ) {
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut Sash, handle: &mut PointerInnerHandle<'_, Sash>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut Sash,
        handle: &mut PointerInnerHandle<'_, Sash>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut Sash,
        handle: &mut PointerInnerHandle<'_, Sash>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut Sash,
        handle: &mut PointerInnerHandle<'_, Sash>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut Sash,
        handle: &mut PointerInnerHandle<'_, Sash>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut Sash,
        handle: &mut PointerInnerHandle<'_, Sash>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut Sash,
        handle: &mut PointerInnerHandle<'_, Sash>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut Sash,
        handle: &mut PointerInnerHandle<'_, Sash>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut Sash,
        handle: &mut PointerInnerHandle<'_, Sash>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &GrabStartData<Sash> {
        &self.start
    }

    fn unset(&mut self, data: &mut Sash) {
        data.windows.dragging = None;
        data.end_drag(self.window);
    }
}

impl Sash {
    /// Starts a pointer drag of `window`. Does nothing if the drag does not apply (a
    /// resize of a tiled window).
    pub fn start_pointer_drag(
        &mut self,
        window: WindowId,
        kind: DragKind,
        start: GrabStartData<Sash>,
        serial: Serial,
    ) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let Some(from) = self.begin_drag(window, kind) else {
            return;
        };
        self.windows.dragging = Some(window);
        let grab = WindowGrab {
            start,
            window,
            kind,
            from,
        };
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    /// The window being dragged, if any.
    pub fn dragged_window(&self) -> Option<WindowId> {
        self.windows.dragging
    }
}
