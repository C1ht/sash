// SPDX-License-Identifier: Apache-2.0

//! Drag and drop: the drag's state, its pointer grab, the drag icon and the action
//! negotiation. A drag starts only from the press that began the client's implicit
//! grab; the target is whatever surface the normal hit-testing finds (never the icon).

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
    reexports::wayland_server::{
        Client, Resource,
        backend::ClientId,
        protocol::{
            wl_data_device::{self, WlDataDevice},
            wl_data_device_manager::DndAction,
            wl_data_offer::WlDataOffer,
            wl_data_source::WlDataSource,
            wl_surface::WlSurface,
        },
    },
    utils::{IsAlive, Logical, Point, SERIAL_COUNTER, Serial},
    wayland::compositor::{self, SurfaceAttributes},
};

use super::{OfferKind, offer_data, source_data};
use crate::state::Sash;

/// The role of a drag icon surface.
const ICON_ROLE: &str = "dnd_icon";

type PointerFocus = Option<(<Sash as SeatHandler>::PointerFocus, Point<f64, Logical>)>;

/// A drag in progress.
#[derive(Debug)]
pub struct Drag {
    /// None: a drag within one client (it handles the data itself).
    pub source: Option<WlDataSource>,
    origin_client: ClientId,
    icon: Option<WlSurface>,
    /// The icon's position relative to the pointer (its hotspot, negated).
    icon_offset: Point<i32, Logical>,
    /// The surface under the pointer that got enter, and its devices' offers.
    target: Option<Target>,
}

#[derive(Debug)]
struct Target {
    surface: WlSurface,
    devices: Vec<(WlDataDevice, Option<WlDataOffer>)>,
}

impl Drag {
    pub fn offers_include(&self, offer: &WlDataOffer) -> bool {
        self.target
            .as_ref()
            .is_some_and(|t| t.devices.iter().any(|(_, o)| o.as_ref() == Some(offer)))
    }
}

/// The action for a drop: the target's preferred one if both sides allow it, else the
/// first of copy, move, ask both allow, else none.
pub fn choose_action(source: DndAction, target: DndAction, preferred: DndAction) -> DndAction {
    let both = source & target;
    if !preferred.is_empty() && both.contains(preferred) {
        return preferred;
    }
    [DndAction::Copy, DndAction::Move, DndAction::Ask]
        .into_iter()
        .find(|a| both.contains(*a))
        .unwrap_or(DndAction::empty())
}

impl Sash {
    /// wl_data_device.start_drag. Started only when `serial` is the press that began the
    /// current implicit grab, that press went to this client, and no sash-wm move or
    /// popup grab runs; otherwise ignored (as the protocol allows).
    pub(super) fn start_drag(
        &mut self,
        client: &Client,
        device: &WlDataDevice,
        source: Option<WlDataSource>,
        origin: WlSurface,
        icon: Option<WlSurface>,
        serial: Serial,
    ) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let pressed_here = pointer.has_grab(serial)
            && pointer.grab_start_data().is_some_and(|start| {
                start
                    .focus
                    .as_ref()
                    .and_then(|(s, _)| s.client())
                    .is_some_and(|c| c.id() == client.id())
            });
        let own_grab = self.windows.drag.is_some() || self.popups.grab_root().is_some();
        if !pressed_here
            || own_grab
            || self.clipboard.drag.is_some()
            || origin.client().map(|c| c.id()) != Some(client.id())
        {
            tracing::debug!("start_drag refused: no matching implicit grab");
            return;
        }
        if let Some(icon) = &icon
            && compositor::give_role(icon, ICON_ROLE).is_err()
            && compositor::get_role(icon) != Some(ICON_ROLE)
        {
            device.post_error(wl_data_device::Error::Role, "the icon has another role");
            return;
        }
        if let Some(source) = &source {
            source_data(source).mark_used();
        }
        let Some(start) = pointer.grab_start_data() else {
            return;
        };
        #[cfg(feature = "debug-record")]
        {
            let window = self.window_of_client(&client.id());
            record!(self, "dnd_start", "window" => window);
        }
        self.clipboard.drag = Some(Drag {
            source,
            origin_client: client.id(),
            icon,
            icon_offset: Point::default(),
            target: None,
        });
        pointer.set_grab(self, DndGrab { start }, serial, Focus::Clear);
        let location = pointer.current_location();
        self.dnd_motion(location, 0);
        self.request_redraw();
    }

    /// The drag icon to draw and the global position of its origin.
    pub fn drag_icon(&self) -> Option<(WlSurface, Point<i32, Logical>)> {
        let drag = self.clipboard.drag.as_ref()?;
        let icon = drag.icon.as_ref().filter(|i| i.alive())?;
        let pointer = self.seat.get_pointer()?.current_location().to_i32_round();
        Some((icon.clone(), pointer + drag.icon_offset))
    }

    /// A commit on `surface`: if it is the drag icon, its offset moves by the buffer
    /// delta (attach x/y, or wl_surface.offset). Returns whether it was the icon.
    pub fn drag_icon_committed(&mut self, surface: &WlSurface) -> bool {
        let Some(drag) = self.clipboard.drag.as_mut() else {
            return false;
        };
        if drag.icon.as_ref() != Some(surface) {
            return false;
        }
        let delta = compositor::with_states(surface, |states| {
            states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .buffer_delta
                .take()
        });
        if let Some(delta) = delta {
            drag.icon_offset += delta;
        }
        self.request_redraw();
        true
    }

    /// The pointer moved to `location` during a drag: enter, leave and motion for the
    /// surface under it.
    pub fn dnd_motion(&mut self, location: Point<f64, Logical>, time: u32) {
        if self.clipboard.drag.is_none() {
            return;
        }
        let hit = self.drop_target_at(location);
        let same = matches!(
            (&self.clipboard.drag.as_ref().and_then(|d| d.target.as_ref()), &hit),
            (Some(t), Some((s, _))) if t.surface == *s && s.alive()
        );
        if !same {
            self.dnd_leave();
            if let Some((surface, origin)) = hit {
                self.dnd_enter(surface, location - origin);
            }
        } else if let Some((_, origin)) = hit
            && let Some(target) = self.clipboard.drag.as_ref().and_then(|d| d.target.as_ref())
        {
            let at = location - origin;
            for (device, _) in &target.devices {
                device.motion(time, at.x, at.y);
            }
        }
        // The icon follows the pointer (and the input-to-frame latency is logged).
        if self
            .clipboard
            .drag
            .as_ref()
            .is_some_and(|d| d.icon.is_some())
        {
            self.windows.note_input(std::time::Instant::now());
            self.request_redraw();
        }
    }

    fn dnd_enter(&mut self, surface: WlSurface, at: Point<f64, Logical>) {
        let Some(client) = surface.client() else {
            return;
        };
        let Some(drag) = self.clipboard.drag.as_ref() else {
            return;
        };
        // Without a source, only the origin client sees the drag.
        if drag.source.is_none() && client.id() != drag.origin_client {
            return;
        }
        let source = drag.source.clone();
        let dh = self.display_handle.clone();
        let serial = SERIAL_COUNTER.next_serial();
        let mut devices = Vec::new();
        for device in self.clipboard.devices_of(&client.id()) {
            let offer = match &source {
                Some(source) => {
                    let offer =
                        self.clipboard
                            .new_offer(&dh, &device, Some(source), OfferKind::Drag);
                    if offer.is_none() {
                        continue; // offer limit: this device sees no drag
                    }
                    offer
                }
                None => None,
            };
            device.enter(serial.into(), &surface, at.x, at.y, offer.as_ref());
            devices.push((device, offer));
        }
        if let Some(drag) = self.clipboard.drag.as_mut() {
            drag.target = Some(Target { surface, devices });
        }
    }

    fn dnd_leave(&mut self) {
        let Some(target) = self.clipboard.drag.as_mut().and_then(|d| d.target.take()) else {
            return;
        };
        for (device, _) in &target.devices {
            if device.is_alive() {
                device.leave();
            }
        }
    }

    /// wl_data_offer.accept on a drag offer.
    pub(super) fn drag_accept(&mut self, offer: &WlDataOffer, mime_type: Option<String>) {
        let data = offer_data(offer);
        let accepted = mime_type.filter(|m| {
            data.source
                .as_ref()
                .is_none_or(|s| source_data(s).has_mime(m))
        });
        data.state.lock().unwrap().accepted = accepted.clone();
        if let Some(source) = data.source.as_ref().filter(|s| s.is_alive()) {
            source.target(accepted);
        }
    }

    /// wl_data_offer.set_actions: the action is chosen again.
    pub(super) fn drag_target_actions(
        &mut self,
        offer: &WlDataOffer,
        actions: DndAction,
        preferred: DndAction,
    ) {
        let data = offer_data(offer);
        let source_actions = data
            .source
            .as_ref()
            .map_or(DndAction::all(), |s| source_data(s).actions());
        let chosen = choose_action(source_actions, actions, preferred);
        let mut s = data.state.lock().unwrap();
        s.target_actions = actions;
        s.preferred = preferred;
        if s.action != chosen {
            s.action = chosen;
            drop(s);
            if offer.version() >= 3 {
                offer.action(chosen);
            }
            if let Some(source) = data
                .source
                .as_ref()
                .filter(|s| s.is_alive() && s.version() >= 3)
            {
                source.action(chosen);
            }
        }
    }

    /// All buttons released: a drop if the target accepted a type and an action (a
    /// drag without a source: if the target is the origin client), else a cancel.
    fn dnd_drop(&mut self) {
        let Some(mut drag) = self.clipboard.drag.take() else {
            return;
        };
        let target = drag.target.take();
        let drop_on = target.as_ref().and_then(|t| {
            t.devices.iter().find(|(device, offer)| {
                device.is_alive()
                    && match offer {
                        Some(offer) => {
                            let s = offer_data(offer).state.lock().unwrap();
                            s.accepted.is_some() && (offer.version() < 3 || !s.action.is_empty())
                        }
                        None => drag.source.is_none(),
                    }
            })
        });
        match drop_on {
            Some((device, offer)) => {
                #[cfg(feature = "debug-record")]
                {
                    let action = offer
                        .as_ref()
                        .map(|o| offer_data(o).state.lock().unwrap().action)
                        .unwrap_or(DndAction::empty());
                    let window = device.client().and_then(|c| self.window_of_client(&c.id()));
                    record!(self, "dnd_drop", "window" => window, "action" => format!("{action:?}"));
                }
                if let Some(offer) = offer {
                    offer_data(offer).state.lock().unwrap().dropped = true;
                }
                device.drop();
                if let Some(source) = drag
                    .source
                    .as_ref()
                    .filter(|s| s.is_alive() && s.version() >= 3)
                {
                    source.dnd_drop_performed();
                }
                // The other devices of the target (if any) see the drag leave.
                if let Some(target) = &target {
                    for (other, _) in &target.devices {
                        if other != device && other.is_alive() {
                            other.leave();
                        }
                    }
                }
            }
            None => {
                record!(self, "dnd_cancel", "reason" => "dropped on nothing that accepted");
                if let Some(target) = &target {
                    for (device, _) in &target.devices {
                        if device.is_alive() {
                            device.leave();
                        }
                    }
                }
                if let Some(source) = drag.source.as_ref().filter(|s| s.is_alive()) {
                    source.cancelled();
                }
            }
        }
        self.request_redraw();
    }

    /// Ends a drag without a drop (Escape, a focus keybind, its source going away): the
    /// target sees it leave, the source is cancelled, the pointer grab ends.
    #[cfg_attr(not(feature = "debug-record"), expect(unused_variables))]
    pub fn cancel_drag(&mut self, reason: &'static str) {
        if self.clipboard.drag.is_none() {
            return;
        }
        record!(self, "dnd_cancel", "reason" => reason);
        self.dnd_leave();
        if let Some(source) = self
            .clipboard
            .drag
            .take()
            .and_then(|d| d.source)
            .filter(|s| s.is_alive())
        {
            source.cancelled();
        }
        if let Some(pointer) = self.seat.get_pointer() {
            pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), 0);
        }
        self.request_redraw();
    }

    /// A drag offer went away (destroyed, or its client disconnected).
    pub(super) fn drag_offer_gone(&mut self, offer: &WlDataOffer) {
        if let Some(target) = self.clipboard.drag.as_mut().and_then(|d| d.target.as_mut()) {
            for (_, o) in target.devices.iter_mut() {
                if o.as_ref() == Some(offer) {
                    *o = None;
                }
            }
        }
    }
}

/// The pointer grab of a drag: clients see no pointer events; motion feeds the drag;
/// releasing every button drops.
pub struct DndGrab {
    start: GrabStartData<Sash>,
}

impl PointerGrab<Sash> for DndGrab {
    fn motion(
        &mut self,
        data: &mut Sash,
        handle: &mut PointerInnerHandle<'_, Sash>,
        _focus: PointerFocus,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        data.dnd_motion(event.location, event.time);
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
        if event.state == ButtonState::Released && handle.current_pressed().is_empty() {
            data.dnd_drop();
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(
        &mut self,
        _data: &mut Sash,
        _handle: &mut PointerInnerHandle<'_, Sash>,
        _details: AxisFrame,
    ) {
    }

    fn frame(&mut self, data: &mut Sash, handle: &mut PointerInnerHandle<'_, Sash>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        _: &mut Sash,
        _: &mut PointerInnerHandle<'_, Sash>,
        _: &GestureSwipeBeginEvent,
    ) {
    }

    fn gesture_swipe_update(
        &mut self,
        _: &mut Sash,
        _: &mut PointerInnerHandle<'_, Sash>,
        _: &GestureSwipeUpdateEvent,
    ) {
    }

    fn gesture_swipe_end(
        &mut self,
        _: &mut Sash,
        _: &mut PointerInnerHandle<'_, Sash>,
        _: &GestureSwipeEndEvent,
    ) {
    }

    fn gesture_pinch_begin(
        &mut self,
        _: &mut Sash,
        _: &mut PointerInnerHandle<'_, Sash>,
        _: &GesturePinchBeginEvent,
    ) {
    }

    fn gesture_pinch_update(
        &mut self,
        _: &mut Sash,
        _: &mut PointerInnerHandle<'_, Sash>,
        _: &GesturePinchUpdateEvent,
    ) {
    }

    fn gesture_pinch_end(
        &mut self,
        _: &mut Sash,
        _: &mut PointerInnerHandle<'_, Sash>,
        _: &GesturePinchEndEvent,
    ) {
    }

    fn gesture_hold_begin(
        &mut self,
        _: &mut Sash,
        _: &mut PointerInnerHandle<'_, Sash>,
        _: &GestureHoldBeginEvent,
    ) {
    }

    fn gesture_hold_end(
        &mut self,
        _: &mut Sash,
        _: &mut PointerInnerHandle<'_, Sash>,
        _: &GestureHoldEndEvent,
    ) {
    }

    fn start_data(&self) -> &GrabStartData<Sash> {
        &self.start
    }

    /// The grab ended some other way (replaced, or the pointer went away): the drag is
    /// cancelled if it still runs.
    fn unset(&mut self, data: &mut Sash) {
        if data.clipboard.drag.is_some() {
            data.dnd_leave();
            if let Some(source) = data
                .clipboard
                .drag
                .take()
                .and_then(|d| d.source)
                .filter(|s| s.is_alive())
            {
                source.cancelled();
            }
            data.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_action_is_the_preferred_one_both_allow_else_copy_move_ask() {
        let all = DndAction::Copy | DndAction::Move | DndAction::Ask;
        assert_eq!(choose_action(all, all, DndAction::Move), DndAction::Move);
        assert_eq!(choose_action(all, all, DndAction::empty()), DndAction::Copy);
        assert_eq!(
            choose_action(DndAction::Copy, DndAction::Move, DndAction::Move),
            DndAction::empty(),
            "nothing in common"
        );
        assert_eq!(
            choose_action(DndAction::Copy | DndAction::Ask, all, DndAction::Move),
            DndAction::Copy,
            "the preference is not allowed by the source"
        );
        assert_eq!(
            choose_action(DndAction::Ask, DndAction::Ask, DndAction::Ask),
            DndAction::Ask
        );
    }
}
