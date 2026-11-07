// SPDX-License-Identifier: Apache-2.0

//! A layer-shell surface drawn on the CPU into wl_shm (smithay-client-toolkit inside;
//! none of its types are part of this API).
//!
//! Drawing follows damage, and only damage:
//! - `Panel::damage` marks a region; `dispatch` draws what was marked, all of it in
//!   one frame. Nothing is drawn until a region is marked (or
//!   the compositor configures a new size): an idle panel commits nothing and asks for
//!   no frame callbacks, so it costs the compositor 0 frames.
//! - At most one frame is in flight: after a commit the panel waits for its frame
//!   callback before it draws again, and damage marked meanwhile is merged.
//! - Buffers are reused. Each remembers the damage drawn into the others since it was
//!   last drawn ("buffer age"), so a reused buffer gets the new damage plus what it
//!   missed, and the compositor is told only the new damage (`damage_buffer`), which
//!   sash-wm uploads alone.
//! - Pixels are tiny-skia's premultiplied RGBA, which is wl_shm ABGR8888: no swap.
//!   A compositor without ABGR8888 is refused (sash-wm advertises it, M17).
//!
//! The buffer scale is 1: sash-wm's output scale is 1 until fractional scale exists.

use std::{
    io,
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    delegate_dispatch2, delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::EventLoop,
        calloop_wayland_source::WaylandSource,
        client::{
            Connection, QueueHandle,
            globals::registry_queue_init,
            protocol::{wl_output, wl_shm, wl_surface},
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{self, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
    },
    shm::{
        Shm, ShmHandler,
        slot::{Buffer, SlotPool},
    },
};

use crate::{canvas::Canvas, damage::Damage, geometry::Rect};

/// Buffers a panel keeps at most. Two alternate while the compositor holds one; a
/// third covers a compositor that has not released the older one yet.
pub const MAX_BUFFERS: usize = 3;

/// What draws the panel's content.
pub trait View {
    /// Draws the whole scene; the canvas keeps only what lies inside its clip (one
    /// rectangle of the damage, already cleared to transparent).
    fn draw(&mut self, canvas: &mut Canvas);
}

/// The layer a panel is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Background,
    Bottom,
    Top,
    Overlay,
}

/// The edges a panel is anchored to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Edges {
    pub top: bool,
    pub bottom: bool,
    pub left: bool,
    pub right: bool,
}

impl Edges {
    /// The bottom edge, the full width.
    pub const BOTTOM_BAR: Edges = Edges {
        top: false,
        bottom: true,
        left: true,
        right: true,
    };
}

/// How a panel asks to be placed (wlr-layer-shell).
#[derive(Clone, Debug)]
pub struct PanelSpec {
    pub layer: Layer,
    pub anchor: Edges,
    /// Logical size; 0 on an axis anchored to both edges means "the output's".
    pub size: (u32, u32),
    /// Pixels reserved from the anchored edge (first-party panels: visible size plus
    /// margin, owner after M13b step 1).
    pub exclusive_zone: i32,
    /// The margin on every anchored edge.
    pub margin: i32,
    pub namespace: String,
}

#[derive(Debug)]
pub enum Error {
    Connect(String),
    /// A global the panel needs is missing (its interface name).
    Missing(&'static str),
    /// The compositor offers no wl_shm ABGR8888.
    NoAbgr8888,
    Io(io::Error),
    /// The compositor closed the panel's layer surface.
    Closed,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Connect(err) => write!(f, "cannot connect: {err}"),
            Error::Missing(name) => write!(f, "the compositor has no {name}"),
            Error::NoAbgr8888 => write!(f, "the compositor offers no wl_shm ABGR8888"),
            Error::Io(err) => write!(f, "{err}"),
            Error::Closed => write!(f, "the compositor closed the panel"),
        }
    }
}

impl std::error::Error for Error {}

/// Counters, for tests and measurements.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Buffers committed.
    pub commits: u64,
    /// Frame callbacks asked for, and received.
    pub frame_requests: u64,
    pub frame_callbacks: u64,
    /// Pixels drawn, over every commit (overlapping damage counted twice).
    pub pixels_drawn: u64,
    /// The damage told to the compositor with the last commit.
    pub last_damage: Vec<Rect>,
    /// Buffers created so far (a new size creates new ones).
    pub buffers_created: u64,
    /// CPU time of the last frame's drawing (clear and view, every damaged rectangle).
    pub last_draw: Duration,
}

struct Slot {
    buffer: Buffer,
    /// What the other buffers got since this one was drawn.
    missed: Damage,
}

struct Inner<V> {
    conn: Connection,
    registry: RegistryState,
    outputs: OutputState,
    shm: Shm,
    pool: SlotPool,
    layer: LayerSurface,
    qh: QueueHandle<Inner<V>>,
    view: V,
    /// The configured size; None until the first configure.
    size: Option<(u32, u32)>,
    slots: Vec<Slot>,
    /// The slot committed last.
    last: Option<usize>,
    pending: Damage,
    frame_pending: bool,
    closed: bool,
    stats: Stats,
}

/// A layer surface and its connection, driven by `dispatch`.
pub struct Panel<V: View + 'static> {
    event_loop: EventLoop<'static, Inner<V>>,
    inner: Inner<V>,
}

impl<V: View + 'static> Panel<V> {
    /// Connects through the environment (WAYLAND_SOCKET, the privileged socket
    /// sash-wm hands to programs it starts, else WAYLAND_DISPLAY).
    pub fn connect_env(spec: PanelSpec, view: V) -> Result<Self, Error> {
        let conn = Connection::connect_to_env().map_err(|err| Error::Connect(err.to_string()))?;
        Self::with_connection(conn, spec, view)
    }

    /// Connects over an already open socket.
    pub fn connect(stream: UnixStream, spec: PanelSpec, view: V) -> Result<Self, Error> {
        let conn =
            Connection::from_socket(stream).map_err(|err| Error::Connect(err.to_string()))?;
        Self::with_connection(conn, spec, view)
    }

    fn with_connection(conn: Connection, spec: PanelSpec, view: V) -> Result<Self, Error> {
        let (globals, mut queue) = registry_queue_init::<Inner<V>>(&conn)
            .map_err(|err| Error::Connect(err.to_string()))?;
        let qh = queue.handle();
        let compositor =
            CompositorState::bind(&globals, &qh).map_err(|_| Error::Missing("wl_compositor"))?;
        let layer_shell =
            LayerShell::bind(&globals, &qh).map_err(|_| Error::Missing("zwlr_layer_shell_v1"))?;
        let shm = Shm::bind(&globals, &qh).map_err(|_| Error::Missing("wl_shm"))?;
        let pool = SlotPool::new(4096, &shm).map_err(|err| Error::Io(io::Error::other(err)))?;

        let surface = compositor.create_surface(&qh);
        let layer = layer_shell.create_layer_surface(
            &qh,
            surface,
            match spec.layer {
                Layer::Background => wlr_layer::Layer::Background,
                Layer::Bottom => wlr_layer::Layer::Bottom,
                Layer::Top => wlr_layer::Layer::Top,
                Layer::Overlay => wlr_layer::Layer::Overlay,
            },
            Some(spec.namespace.clone()),
            None,
        );
        let mut anchor = wlr_layer::Anchor::empty();
        for (on, edge) in [
            (spec.anchor.top, wlr_layer::Anchor::TOP),
            (spec.anchor.bottom, wlr_layer::Anchor::BOTTOM),
            (spec.anchor.left, wlr_layer::Anchor::LEFT),
            (spec.anchor.right, wlr_layer::Anchor::RIGHT),
        ] {
            if on {
                anchor |= edge;
            }
        }
        layer.set_anchor(anchor);
        layer.set_size(spec.size.0, spec.size.1);
        layer.set_exclusive_zone(spec.exclusive_zone);
        let m = |on: bool| if on { spec.margin } else { 0 };
        layer.set_margin(
            m(spec.anchor.top),
            m(spec.anchor.right),
            m(spec.anchor.bottom),
            m(spec.anchor.left),
        );
        layer.set_keyboard_interactivity(wlr_layer::KeyboardInteractivity::None);
        // The first commit, without a buffer, asks for the first configure.
        layer.commit();

        let mut inner = Inner {
            conn: conn.clone(),
            registry: RegistryState::new(&globals),
            outputs: OutputState::new(&globals, &qh),
            shm,
            pool,
            layer,
            qh: qh.clone(),
            view,
            size: None,
            slots: Vec::new(),
            last: None,
            pending: Damage::default(),
            frame_pending: false,
            closed: false,
            stats: Stats::default(),
        };
        // The shm formats arrive with the first roundtrip.
        queue
            .roundtrip(&mut inner)
            .map_err(|err| Error::Connect(err.to_string()))?;
        if !inner.shm.formats().contains(&wl_shm::Format::Abgr8888) {
            return Err(Error::NoAbgr8888);
        }

        let event_loop = EventLoop::try_new().map_err(|err| Error::Io(io::Error::other(err)))?;
        WaylandSource::new(conn, queue)
            .insert(event_loop.handle())
            .map_err(|err| Error::Io(io::Error::other(err.error)))?;
        Ok(Self { event_loop, inner })
    }

    /// Handles events for up to `timeout` (None: until one arrives) and draws if a
    /// frame is due.
    pub fn dispatch(&mut self, timeout: Option<Duration>) -> Result<(), Error> {
        self.event_loop
            .dispatch(timeout, &mut self.inner)
            .map_err(|err| Error::Io(io::Error::other(err)))?;
        if self.inner.closed {
            return Err(Error::Closed);
        }
        self.inner.draw_if_due();
        Ok(())
    }

    /// Marks `rect` (buffer pixels) to be drawn again. Nothing is drawn here: the
    /// next `dispatch` draws everything marked since, in one frame.
    pub fn damage(&mut self, rect: Rect) {
        self.inner.pending.add(rect);
    }

    /// Marks the whole panel.
    pub fn damage_all(&mut self) {
        if let Some((w, h)) = self.inner.size {
            self.damage(Rect::sized(w as i32, h as i32));
        }
    }

    pub fn view(&self) -> &V {
        &self.inner.view
    }

    pub fn view_mut(&mut self) -> &mut V {
        &mut self.inner.view
    }

    /// The configured size, once configured.
    pub fn size(&self) -> Option<(u32, u32)> {
        self.inner.size
    }

    pub fn stats(&self) -> &Stats {
        &self.inner.stats
    }

    /// The pixels of the last committed buffer (premultiplied RGBA), for tests.
    pub fn last_pixels(&mut self) -> Option<Vec<u8>> {
        let inner = &mut self.inner;
        let slot = inner.slots.get(inner.last?)?;
        let len = (slot.buffer.height() * slot.buffer.stride()) as usize;
        let slot = slot.buffer.slot();
        Some(inner.pool.raw_data_mut(&slot)[..len].to_vec())
    }
}

impl<V: View + 'static> Inner<V> {
    fn draw_if_due(&mut self) {
        if self.frame_pending || self.pending.is_empty() || self.closed {
            return;
        }
        let Some((width, height)) = self.size else {
            return;
        };
        let full = Rect::sized(width as i32, height as i32);
        self.pending.clip(full);
        if self.pending.is_empty() {
            return;
        }
        let Some(index) = self.free_slot(width, height) else {
            // Every buffer is still held: the next frame callback or dispatch retries.
            return;
        };
        let damage = std::mem::take(&mut self.pending);
        let mut region = self.slots[index].missed.clone();
        region.add_all(&damage);
        self.slots[index].missed.clear();
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if i != index {
                slot.missed.add_all(&damage);
            }
        }

        let slot = &self.slots[index];
        let Some(pixels) = slot.buffer.canvas(&mut self.pool) else {
            return;
        };
        let started = Instant::now();
        for rect in region.rects() {
            if let Some(mut canvas) = Canvas::new(pixels, width, height, *rect) {
                canvas.clear();
                self.view.draw(&mut canvas);
            }
        }
        self.stats.last_draw = started.elapsed();
        self.stats.pixels_drawn += region.area();

        let surface = self.layer.wl_surface();
        for rect in damage.rects() {
            surface.damage_buffer(rect.x, rect.y, rect.w, rect.h);
        }
        if slot.buffer.attach_to(surface).is_err() {
            return;
        }
        surface.frame(&self.qh, FrameCallbackData(surface.clone()));
        self.layer.commit();
        self.frame_pending = true;
        self.stats.commits += 1;
        self.stats.frame_requests += 1;
        self.stats.last_damage = damage.rects().to_vec();
        self.last = Some(index);
        // Sent now, not when the loop next polls.
        let _ = self.conn.flush();
    }

    /// A buffer the compositor does not hold, at this size: a free one, else a new
    /// one while fewer than MAX_BUFFERS exist.
    fn free_slot(&mut self, width: u32, height: u32) -> Option<usize> {
        if let Some(i) = self
            .slots
            .iter()
            .position(|s| s.buffer.canvas(&mut self.pool).is_some())
        {
            return Some(i);
        }
        if self.slots.len() >= MAX_BUFFERS {
            return None;
        }
        let (buffer, _) = self
            .pool
            .create_buffer(
                width as i32,
                height as i32,
                width as i32 * 4,
                wl_shm::Format::Abgr8888,
            )
            .ok()?;
        self.stats.buffers_created += 1;
        self.slots.push(Slot {
            buffer,
            missed: Damage::full(Rect::sized(width as i32, height as i32)),
        });
        Some(self.slots.len() - 1)
    }
}

impl<V: View + 'static> CompositorHandler for Inner<V> {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        self.frame_pending = false;
        self.stats.frame_callbacks += 1;
        self.draw_if_due();
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl<V: View + 'static> OutputHandler for Inner<V> {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl<V: View + 'static> LayerShellHandler for Inner<V> {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.closed = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let size = (configure.new_size.0.max(1), configure.new_size.1.max(1));
        if self.size != Some(size) {
            // A new size: new buffers, everything drawn.
            self.size = Some(size);
            self.slots.clear();
            self.last = None;
            self.pending = Damage::full(Rect::sized(size.0 as i32, size.1 as i32));
        }
        self.draw_if_due();
    }
}

impl<V: View + 'static> ShmHandler for Inner<V> {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl<V: View + 'static> ProvidesRegistryState for Inner<V> {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState];
}

delegate_registry!(@<V: View + 'static> Inner<V>);
delegate_dispatch2!(@<V: View + 'static> Inner<V>);
