// SPDX-License-Identifier: Apache-2.0

//! Clipboard and drag and drop: wl_data_device_manager (version 3) and its sources,
//! devices and offers, implemented here (Smithay 0.7's data device has no hook for the
//! limits in limits.rs). Rules: CLAUDE.md, "Clipboard and drag and drop (M10)".
//!
//! The selection is offered only to the client with keyboard focus, and only that
//! client can read it. Payloads go from the source client to the reading client
//! through the reader's pipe; sash-wm passes the fd on and never reads, writes or waits
//! on it. Nothing here is recorded beyond the fact that a selection or drag happened.

mod drag;
pub mod limits;

use std::{
    collections::HashMap,
    os::fd::AsFd,
    sync::Mutex,
    time::{Duration, Instant},
};

use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
    backend::{ClientId, GlobalId, ObjectId},
    protocol::{
        wl_data_device::{self, WlDataDevice},
        wl_data_device_manager::{self, DndAction, WlDataDeviceManager},
        wl_data_offer::{self, WlDataOffer},
        wl_data_source::{self, WlDataSource},
        wl_surface::WlSurface,
    },
};

pub use self::drag::Drag;
use self::limits::{MAX_OFFERS_PER_CLIENT, MimeList, ReceiveRate};
use crate::{layout::WindowId, state::Sash};

/// The version advertised (CLAUDE.md: version 3).
const VERSION: u32 = 3;

/// Clipboard and drag-and-drop state of the (one) seat.
pub struct DataDevices {
    _global: GlobalId,
    devices: Vec<WlDataDevice>,
    /// The current selection's source.
    selection: Option<WlDataSource>,
    /// The client with keyboard focus (the only one offered the selection).
    focus: Option<ClientId>,
    /// Live offers per client.
    offers: HashMap<ClientId, usize>,
    pub drag: Option<Drag>,
    /// Receive requests refused by the rate limit (counted, logged at debug).
    pub receives_refused: u64,
    /// The clock for the receive rate (tests move it by hand).
    epoch: Instant,
    #[cfg(test)]
    pub fake_now: Option<Duration>,
}

impl DataDevices {
    pub fn new(display: &DisplayHandle) -> Self {
        Self {
            _global: display.create_global::<Sash, WlDataDeviceManager, ()>(VERSION, ()),
            devices: Vec::new(),
            selection: None,
            focus: None,
            offers: HashMap::new(),
            drag: None,
            receives_refused: 0,
            epoch: Instant::now(),
            #[cfg(test)]
            fake_now: None,
        }
    }

    fn now(&self) -> Duration {
        #[cfg(test)]
        if let Some(now) = self.fake_now {
            return now;
        }
        self.epoch.elapsed()
    }

    /// The data devices of `client`.
    fn devices_of(&self, client: &ClientId) -> Vec<WlDataDevice> {
        self.devices
            .iter()
            .filter(|d| d.client().is_some_and(|c| c.id() == *client))
            .cloned()
            .collect()
    }

    /// Creates an offer of `source` for `device` and announces it with its MIME types,
    /// unless the device's client already holds MAX_OFFERS_PER_CLIENT offers.
    fn new_offer(
        &mut self,
        dh: &DisplayHandle,
        device: &WlDataDevice,
        source: Option<&WlDataSource>,
        kind: OfferKind,
    ) -> Option<WlDataOffer> {
        let client = device.client()?;
        let held = self.offers.entry(client.id()).or_default();
        if *held >= MAX_OFFERS_PER_CLIENT {
            tracing::debug!(held = *held, "offer limit: no new offer for this client");
            return None;
        }
        let data = OfferData {
            source: source.cloned(),
            kind,
            client: client.id(),
            state: Mutex::new(OfferState::default()),
        };
        let offer = client
            .create_resource::<WlDataOffer, OfferData, Sash>(dh, device.version(), data)
            .ok()?;
        *held += 1;
        device.data_offer(&offer);
        if let Some(source) = source {
            for name in source_data(source).mimes.lock().unwrap().names() {
                offer.offer(name.clone());
            }
            if kind == OfferKind::Drag && offer.version() >= 3 {
                offer.source_actions(source_data(source).actions());
            }
        }
        Some(offer)
    }

    /// Tells the focused client's devices about the current selection.
    fn offer_selection(&mut self, dh: &DisplayHandle) {
        let Some(client) = self.focus.clone() else {
            return;
        };
        let source = self.selection.clone();
        for device in self.devices_of(&client) {
            self.offer_selection_to(dh, &device, source.as_ref());
        }
    }

    fn offer_selection_to(
        &mut self,
        dh: &DisplayHandle,
        device: &WlDataDevice,
        source: Option<&WlDataSource>,
    ) {
        match source {
            Some(source) => {
                if let Some(offer) = self.new_offer(dh, device, Some(source), OfferKind::Selection)
                {
                    device.selection(Some(&offer));
                }
            }
            None => device.selection(None),
        }
    }
}

/// What an offer is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfferKind {
    Selection,
    Drag,
}

/// User data of a wl_data_source.
#[derive(Debug, Default)]
pub struct SourceData {
    mimes: Mutex<MimeList>,
    /// Set by set_actions (drag and drop only).
    actions: Mutex<Option<DndAction>>,
    /// Used in a set_selection or start_drag.
    used: Mutex<bool>,
}

impl SourceData {
    pub fn actions(&self) -> DndAction {
        self.actions.lock().unwrap().unwrap_or(DndAction::empty())
    }

    pub fn has_mime(&self, name: &str) -> bool {
        self.mimes.lock().unwrap().contains(name)
    }

    /// Marks the source used (set_actions is refused after that). A source used again
    /// is accepted: the protocol only says used_source "may" be sent, and Firefox sets
    /// the same source as the selection again.
    fn mark_used(&self) {
        *self.used.lock().unwrap() = true;
    }
}

pub fn source_data(source: &WlDataSource) -> &SourceData {
    source
        .data::<SourceData>()
        .expect("sources are created with SourceData")
}

/// User data of a wl_data_offer.
#[derive(Debug)]
pub struct OfferData {
    /// None: a drag without a source (within one client).
    pub source: Option<WlDataSource>,
    pub kind: OfferKind,
    client: ClientId,
    pub state: Mutex<OfferState>,
}

#[derive(Debug)]
pub struct OfferState {
    receives: ReceiveRate,
    /// The MIME type the target accepted (drag and drop).
    pub accepted: Option<String>,
    /// The target's actions and preference, and the action chosen.
    pub target_actions: DndAction,
    pub preferred: DndAction,
    pub action: DndAction,
    /// Dropped on, waiting for finish; and finished.
    pub dropped: bool,
    pub finished: bool,
}

impl Default for OfferState {
    fn default() -> Self {
        Self {
            receives: ReceiveRate::default(),
            accepted: None,
            target_actions: DndAction::empty(),
            preferred: DndAction::empty(),
            action: DndAction::empty(),
            dropped: false,
            finished: false,
        }
    }
}

pub fn offer_data(offer: &WlDataOffer) -> &OfferData {
    offer
        .data::<OfferData>()
        .expect("offers are created with OfferData")
}

impl Sash {
    /// Keyboard focus moved to `client` (or none): from now on only that client is
    /// offered the selection; it gets the current one at once.
    pub fn data_focus_changed(&mut self, client: Option<ClientId>) {
        if self.clipboard.focus == client {
            return;
        }
        self.clipboard.focus = client;
        let dh = self.display_handle.clone();
        self.clipboard.offer_selection(&dh);
    }

    /// The window of a client (the first one), for the recorder.
    #[cfg_attr(not(feature = "debug-record"), expect(dead_code))]
    fn window_of_client(&self, client: &ClientId) -> Option<WindowId> {
        self.space.elements().find_map(|window| {
            let surface = window.toplevel()?.wl_surface();
            (surface.client()?.id() == *client)
                .then(|| self.windows.by_surface(surface).map(|m| m.id))
                .flatten()
        })
    }

    /// set_selection from `client` (checked for keyboard focus by the caller).
    fn set_selection(&mut self, source: Option<WlDataSource>) {
        if let Some(old) = self.clipboard.selection.take()
            && Some(&old) != source.as_ref()
        {
            old.cancelled();
        }
        #[cfg(feature = "debug-record")]
        if self.recorder.is_some() {
            let window = source
                .as_ref()
                .and_then(|s| s.client())
                .and_then(|c| self.window_of_client(&c.id()));
            if source.is_some() {
                record!(self, "selection_set", "window" => window);
            } else {
                record!(self, "selection_cleared", "reason" => "cleared");
            }
        }
        self.clipboard.selection = source;
        let dh = self.display_handle.clone();
        self.clipboard.offer_selection(&dh);
    }

    /// The source `id` went away (destroyed, or its client disconnected): a selection
    /// it provided ends; a drag it provided is cancelled.
    fn source_gone(&mut self, id: &ObjectId) {
        if self
            .clipboard
            .selection
            .as_ref()
            .is_some_and(|s| s.id() == *id)
        {
            self.clipboard.selection = None;
            record!(self, "selection_cleared", "reason" => "source gone");
            let dh = self.display_handle.clone();
            self.clipboard.offer_selection(&dh);
        }
        if self
            .clipboard
            .drag
            .as_ref()
            .is_some_and(|d| d.source.as_ref().is_some_and(|s| s.id() == *id))
        {
            self.cancel_drag("source gone");
        }
    }

    /// wl_data_offer.receive: passes `fd` to the source, or closes it.
    fn receive(&mut self, offer: &WlDataOffer, mime_type: String, fd: std::os::fd::OwnedFd) {
        let data = offer_data(offer);
        let Some(source) = data.source.as_ref().filter(|s| s.is_alive()) else {
            return; // fd dropped: closed
        };
        let allowed = match data.kind {
            // Only the focused client reads the selection, and only the current one.
            OfferKind::Selection => {
                self.clipboard.focus.as_ref() == Some(&data.client)
                    && self.clipboard.selection.as_ref() == Some(source)
            }
            // The drag's target while it runs, or after the drop until finished.
            OfferKind::Drag => {
                let state = data.state.lock().unwrap();
                (state.dropped && !state.finished)
                    || self
                        .clipboard
                        .drag
                        .as_ref()
                        .is_some_and(|d| d.offers_include(offer))
            }
        };
        if !allowed {
            tracing::debug!("receive refused: not the focused client or not current");
            return;
        }
        if !source_data(source).has_mime(&mime_type) {
            tracing::debug!("receive refused: a MIME type the source does not offer");
            return;
        }
        let now = self.clipboard.now();
        if !data.state.lock().unwrap().receives.allow(now) {
            self.clipboard.receives_refused += 1;
            tracing::debug!(
                refused = self.clipboard.receives_refused,
                "receive refused: rate limit"
            );
            return;
        }
        source.send(mime_type, fd.as_fd());
        // sash-wm's copy of the fd is closed here, when `fd` is dropped.
    }
}

impl GlobalDispatch<WlDataDeviceManager, ()> for Sash {
    fn bind(
        _state: &mut Self,
        dh: &DisplayHandle,
        client: &Client,
        resource: New<WlDataDeviceManager>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
        crate::limits::count_bind(dh, client);
    }
}

impl Dispatch<WlDataDeviceManager, ()> for Sash {
    fn request(
        state: &mut Self,
        client: &Client,
        _manager: &WlDataDeviceManager,
        request: wl_data_device_manager::Request,
        _data: &(),
        dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_data_device_manager::Request::CreateDataSource { id } => {
                data_init.init(id, SourceData::default());
            }
            wl_data_device_manager::Request::GetDataDevice { id, .. } => {
                let device = data_init.init(id, ());
                state.clipboard.devices.push(device.clone());
                // A focused client asking for its device gets the selection at once.
                if state.clipboard.focus.as_ref() == Some(&client.id()) {
                    let source = state.clipboard.selection.clone();
                    state
                        .clipboard
                        .offer_selection_to(dh, &device, source.as_ref());
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlDataSource, SourceData> for Sash {
    fn request(
        _state: &mut Self,
        _client: &Client,
        source: &WlDataSource,
        request: wl_data_source::Request,
        data: &SourceData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_data_source::Request::Offer { mime_type } => {
                if let Err(refusal) = data.mimes.lock().unwrap().add(mime_type) {
                    tracing::debug!(?refusal, "MIME type not added: limit");
                }
            }
            wl_data_source::Request::SetActions { dnd_actions } => {
                let WEnum::Value(actions) = dnd_actions else {
                    source.post_error(
                        wl_data_source::Error::InvalidActionMask,
                        "unknown bits in the action mask",
                    );
                    return;
                };
                if *data.used.lock().unwrap() {
                    source.post_error(
                        wl_data_source::Error::InvalidSource,
                        "set_actions after the source was used",
                    );
                    return;
                }
                let mut set = data.actions.lock().unwrap();
                if set.is_some() {
                    source.post_error(
                        wl_data_source::Error::InvalidSource,
                        "set_actions may be made once only",
                    );
                    return;
                }
                *set = Some(actions);
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _client: ClientId, source: &WlDataSource, _: &SourceData) {
        state.source_gone(&source.id());
    }
}

impl Dispatch<WlDataDevice, ()> for Sash {
    fn request(
        state: &mut Self,
        client: &Client,
        device: &WlDataDevice,
        request: wl_data_device::Request,
        _data: &(),
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_data_device::Request::SetSelection { source, .. } => {
                if state.clipboard.focus.as_ref() != Some(&client.id()) {
                    tracing::debug!("set_selection from a client without keyboard focus ignored");
                    return;
                }
                if let Some(source) = &source {
                    let data = source_data(source);
                    if data.actions.lock().unwrap().is_some() {
                        source.post_error(
                            wl_data_source::Error::InvalidSource,
                            "a drag-and-drop source used for the selection",
                        );
                        return;
                    }
                    data.mark_used();
                }
                state.set_selection(source);
            }
            wl_data_device::Request::StartDrag {
                source,
                origin,
                icon,
                serial,
            } => {
                state.start_drag(client, device, source, origin, icon, serial.into());
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _client: ClientId, device: &WlDataDevice, _: &()) {
        state.clipboard.devices.retain(|d| d != device);
    }
}

impl Dispatch<WlDataOffer, OfferData> for Sash {
    fn request(
        state: &mut Self,
        _client: &Client,
        offer: &WlDataOffer,
        request: wl_data_offer::Request,
        data: &OfferData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if data.state.lock().unwrap().finished
            && !matches!(request, wl_data_offer::Request::Destroy)
        {
            offer.post_error(
                wl_data_offer::Error::InvalidOffer,
                "only destroy is allowed after finish",
            );
            return;
        }
        match request {
            wl_data_offer::Request::Accept { mime_type, .. } => {
                if data.kind == OfferKind::Drag {
                    state.drag_accept(offer, mime_type);
                }
            }
            wl_data_offer::Request::Receive { mime_type, fd } => {
                state.receive(offer, mime_type, fd);
            }
            wl_data_offer::Request::Finish => {
                if data.kind != OfferKind::Drag {
                    offer.post_error(
                        wl_data_offer::Error::InvalidFinish,
                        "finish on a selection offer",
                    );
                    return;
                }
                let mut s = data.state.lock().unwrap();
                if !s.dropped || s.accepted.is_none() || s.action.is_empty() {
                    offer.post_error(
                        wl_data_offer::Error::InvalidFinish,
                        "finish before a drop, without an accepted type or an action",
                    );
                    return;
                }
                s.finished = true;
                drop(s);
                if let Some(source) = data.source.as_ref().filter(|s| s.is_alive())
                    && source.version() >= 3
                {
                    source.dnd_finished();
                }
            }
            wl_data_offer::Request::SetActions {
                dnd_actions,
                preferred_action,
            } => {
                if data.kind != OfferKind::Drag {
                    offer.post_error(
                        wl_data_offer::Error::InvalidOffer,
                        "set_actions on a selection offer",
                    );
                    return;
                }
                let (WEnum::Value(actions), WEnum::Value(preferred)) =
                    (dnd_actions, preferred_action)
                else {
                    offer.post_error(
                        wl_data_offer::Error::InvalidActionMask,
                        "unknown bits in the action mask",
                    );
                    return;
                };
                let source_actions = data
                    .source
                    .as_ref()
                    .map_or(DndAction::all(), |s| source_data(s).actions());
                if preferred.bits().count_ones() > 1
                    || (!preferred.is_empty() && !source_actions.contains(preferred))
                {
                    offer.post_error(
                        wl_data_offer::Error::InvalidAction,
                        "the preferred action is not one action the source offers",
                    );
                    return;
                }
                state.drag_target_actions(offer, actions, preferred);
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, client: ClientId, offer: &WlDataOffer, data: &OfferData) {
        if let Some(held) = state.clipboard.offers.get_mut(&client) {
            *held = held.saturating_sub(1);
            if *held == 0 {
                state.clipboard.offers.remove(&client);
            }
        }
        // A dropped offer destroyed without finish: the drag did not complete.
        let s = data.state.lock().unwrap();
        if data.kind == OfferKind::Drag
            && s.dropped
            && !s.finished
            && let Some(source) = data.source.as_ref().filter(|s| s.is_alive())
        {
            source.cancelled();
        }
        drop(s);
        state.drag_offer_gone(offer);
    }
}

/// Keyboard focus: the client of the focused surface.
pub fn client_of(surface: Option<&WlSurface>) -> Option<ClientId> {
    surface.and_then(|s| s.client()).map(|c| c.id())
}
