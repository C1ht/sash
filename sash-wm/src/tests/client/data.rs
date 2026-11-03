// SPDX-License-Identifier: Apache-2.0

//! The test client's clipboard and drag and drop: a data device, sources that serve
//! their payload from a thread (as a real client writes to the pipe), offers and the
//! events seen on all of them.

use std::{
    collections::HashMap,
    io::{PipeReader, Write},
    os::fd::AsFd,
    sync::Arc,
    thread::JoinHandle,
};

use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, WEnum,
    backend::ObjectId,
    event_created_child,
    protocol::{
        wl_data_device::{self, WlDataDevice},
        wl_data_device_manager::{DndAction, WlDataDeviceManager},
        wl_data_offer::{self, WlDataOffer},
        wl_data_source::{self, WlDataSource},
        wl_surface::WlSurface,
    },
};

use super::{Events, TestClient};

/// What an offer advertised.
#[derive(Debug)]
pub struct OfferInfo {
    pub offer: WlDataOffer,
    pub mimes: Vec<String>,
    pub source_actions: Option<DndAction>,
    pub action: Option<DndAction>,
}

/// What a source was told, and the payload it serves for every MIME type.
#[derive(Debug, Default)]
pub struct SourceInfo {
    payload: Arc<Vec<u8>>,
    pub sends: Vec<String>,
    pub targets: Vec<Option<String>>,
    pub cancelled: bool,
    pub drop_performed: bool,
    pub finished: bool,
    pub action: Option<DndAction>,
    writers: Vec<JoinHandle<()>>,
}

/// A drag event seen by the data device.
#[derive(Clone, Debug, PartialEq)]
pub enum DragEvent {
    Enter {
        surface: ObjectId,
        x: f64,
        y: f64,
        offer: Option<ObjectId>,
    },
    Motion,
    Leave,
    Drop,
}

#[derive(Debug, Default)]
pub struct DataEvents {
    pub(super) manager: Option<WlDataDeviceManager>,
    device: Option<WlDataDevice>,
    pub offers: HashMap<ObjectId, OfferInfo>,
    /// The last selection event: None before any, Some(None) for "no selection".
    pub selection: Option<Option<ObjectId>>,
    pub selection_events: u32,
    pub drag: Vec<DragEvent>,
    pub sources: HashMap<ObjectId, SourceInfo>,
}

impl DataEvents {
    pub fn source(&self, source: &WlDataSource) -> &SourceInfo {
        &self.sources[&source.id()]
    }

    /// The offer of the current selection.
    pub fn selection_offer(&self) -> Option<WlDataOffer> {
        let id = self.selection.clone().flatten()?;
        Some(self.offers[&id].offer.clone())
    }

    /// The offer of the last drag enter.
    pub fn drag_offer(&self) -> Option<WlDataOffer> {
        self.drag.iter().rev().find_map(|e| match e {
            DragEvent::Enter {
                offer: Some(id), ..
            } => Some(self.offers[id].offer.clone()),
            _ => None,
        })
    }

    pub fn count(&self, event: &DragEvent) -> usize {
        self.drag.iter().filter(|e| *e == event).count()
    }

    pub fn enters(&self) -> usize {
        self.drag
            .iter()
            .filter(|e| matches!(e, DragEvent::Enter { .. }))
            .count()
    }
}

impl TestClient {
    /// The client's data device (created on first use).
    pub fn data_device(&mut self) -> WlDataDevice {
        if let Some(device) = &self.events.data.device {
            return device.clone();
        }
        let qh = self.qh();
        let manager = self
            .events
            .data
            .manager
            .clone()
            .expect("wl_data_device_manager");
        let seat = self.events.seat.clone().expect("wl_seat");
        let device = manager.get_data_device(&seat, &qh, ());
        self.events.data.device = Some(device.clone());
        self.roundtrip();
        device
    }

    /// A source offering `mimes`, serving `payload` for each; with drag actions if set.
    pub fn new_source(
        &mut self,
        mimes: &[&str],
        payload: Vec<u8>,
        actions: Option<DndAction>,
    ) -> WlDataSource {
        let qh = self.qh();
        let manager = self.events.data.manager.clone().expect("manager");
        let source = manager.create_data_source(&qh, ());
        for mime in mimes {
            source.offer(mime.to_string());
        }
        if let Some(actions) = actions {
            source.set_actions(actions);
        }
        self.events.data.sources.insert(
            source.id(),
            SourceInfo {
                payload: Arc::new(payload),
                ..Default::default()
            },
        );
        source
    }

    /// Copies: a new source with `mimes` and `payload` becomes the selection.
    pub fn copy(&mut self, mimes: &[&str], payload: Vec<u8>) -> WlDataSource {
        let device = self.data_device();
        let source = self.new_source(mimes, payload, None);
        device.set_selection(Some(&source), 0);
        self.roundtrip();
        source
    }

    pub fn clear_selection(&mut self) {
        let device = self.data_device();
        device.set_selection(None, 0);
        self.roundtrip();
    }

    /// Asks for `mime` of `offer`; the payload arrives on the returned pipe once the
    /// source client dispatches its send event.
    pub fn receive(&mut self, offer: &WlDataOffer, mime: &str) -> PipeReader {
        let (reader, writer) = std::io::pipe().expect("pipe");
        offer.receive(mime.to_string(), writer.as_fd());
        drop(writer);
        self.conn.flush().expect("flush");
        reader
    }

    /// Waits for the threads writing this client's payloads.
    pub fn join_writers(&mut self) {
        for info in self.events.data.sources.values_mut() {
            for writer in info.writers.drain(..) {
                let _ = writer.join();
            }
        }
    }

    pub fn start_drag(
        &mut self,
        source: Option<&WlDataSource>,
        origin: &WlSurface,
        icon: Option<&WlSurface>,
        serial: u32,
    ) {
        let device = self.data_device();
        device.start_drag(source, origin, icon, serial);
        self.roundtrip();
    }

    /// A drag icon surface of `size` (attached at 0, 0: before the drag its position is
    /// the pointer's).
    pub fn icon_surface(&mut self, size: (i32, i32)) -> WlSurface {
        let qh = self.qh();
        let surface = self
            .events
            .compositor
            .as_ref()
            .expect("wl_compositor")
            .create_surface(&qh, ());
        self.attach_icon(&surface, size, (0, 0));
        surface
    }

    /// Attaches a new icon buffer at (dx, dy) relative to the current one and commits:
    /// the icon moves by that much relative to the pointer.
    pub fn attach_icon(&mut self, surface: &WlSurface, size: (i32, i32), at: (i32, i32)) {
        let buffer = self.buffer(size, 0xff00_ff00);
        surface.attach(Some(&buffer), at.0, at.1);
        surface.damage_buffer(0, 0, size.0, size.1);
        surface.commit();
    }
}

impl Dispatch<WlDataDeviceManager, ()> for Events {
    fn event(
        _: &mut Self,
        _: &WlDataDeviceManager,
        _: <WlDataDeviceManager as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlDataDevice, ()> for Events {
    fn event(
        events: &mut Self,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let data = &mut events.data;
        match event {
            wl_data_device::Event::DataOffer { id } => {
                data.offers.insert(
                    id.id(),
                    OfferInfo {
                        offer: id,
                        mimes: Vec::new(),
                        source_actions: None,
                        action: None,
                    },
                );
            }
            wl_data_device::Event::Selection { id } => {
                data.selection = Some(id.map(|o| o.id()));
                data.selection_events += 1;
            }
            wl_data_device::Event::Enter {
                surface, x, y, id, ..
            } => data.drag.push(DragEvent::Enter {
                surface: surface.id(),
                x,
                y,
                offer: id.map(|o| o.id()),
            }),
            wl_data_device::Event::Motion { .. } => data.drag.push(DragEvent::Motion),
            wl_data_device::Event::Leave => data.drag.push(DragEvent::Leave),
            wl_data_device::Event::Drop => data.drag.push(DragEvent::Drop),
            _ => {}
        }
    }

    event_created_child!(Events, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ()),
    ]);
}

impl Dispatch<WlDataOffer, ()> for Events {
    fn event(
        events: &mut Self,
        offer: &WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(info) = events.data.offers.get_mut(&offer.id()) else {
            return;
        };
        match event {
            wl_data_offer::Event::Offer { mime_type } => info.mimes.push(mime_type),
            wl_data_offer::Event::SourceActions {
                source_actions: WEnum::Value(a),
            } => info.source_actions = Some(a),
            wl_data_offer::Event::Action {
                dnd_action: WEnum::Value(a),
            } => info.action = Some(a),
            _ => {}
        }
    }
}

impl Dispatch<WlDataSource, ()> for Events {
    fn event(
        events: &mut Self,
        source: &WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(info) = events.data.sources.get_mut(&source.id()) else {
            return;
        };
        match event {
            wl_data_source::Event::Send { mime_type, fd } => {
                info.sends.push(mime_type);
                // Like a real client: write the payload, then close the pipe.
                let payload = info.payload.clone();
                info.writers.push(std::thread::spawn(move || {
                    let mut file = std::fs::File::from(fd);
                    let _ = file.write_all(&payload);
                }));
            }
            wl_data_source::Event::Target { mime_type } => info.targets.push(mime_type),
            wl_data_source::Event::Cancelled => info.cancelled = true,
            wl_data_source::Event::DndDropPerformed => info.drop_performed = true,
            wl_data_source::Event::DndFinished => info.finished = true,
            wl_data_source::Event::Action {
                dnd_action: WEnum::Value(a),
            } => info.action = Some(a),
            _ => {}
        }
    }
}
