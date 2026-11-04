// SPDX-License-Identifier: Apache-2.0

//! A minimal Wayland client for tests. Runs on its own thread against a `Harness`.

use std::{
    fs::OpenOptions,
    os::{fd::AsFd, unix::net::UnixStream},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

use std::collections::HashMap;

use wayland_client::{
    Connection, Dispatch, DispatchError, EventQueue, Proxy, QueueHandle, WEnum,
    backend::ObjectId,
    delegate_noop,
    protocol::{
        wl_buffer::WlBuffer,
        wl_callback::{self, WlCallback},
        wl_compositor::WlCompositor,
        wl_data_offer::WlDataOffer,
        wl_keyboard::{self, WlKeyboard},
        wl_pointer::{self, WlPointer},
        wl_registry::{self, WlRegistry},
        wl_seat::{self, WlSeat},
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
        wl_subcompositor::WlSubcompositor,
        wl_subsurface::WlSubsurface,
        wl_surface::WlSurface,
    },
};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1;
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1},
    wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1},
    zwp_linux_dmabuf_feedback_v1::{self, ZwpLinuxDmabufFeedbackV1},
    zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
    zxdg_toplevel_decoration_v1::{self, ZxdgToplevelDecorationV1},
};
use wayland_protocols::xdg::shell::client::{
    xdg_popup::{self, XdgPopup},
    xdg_positioner::{Anchor, ConstraintAdjustment, Gravity, XdgPositioner},
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::{self, XdgToplevel},
    xdg_wm_base::{self, XdgWmBase},
};

pub mod data;

pub const WINDOW_SIZE: (i32, i32) = (100, 100);

/// Globals the client bound and the events it saw.
/// The last xdg_toplevel.configure of one window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ToplevelConfig {
    pub size: (i32, i32),
    /// All four tiled states.
    pub tiled: bool,
    /// The tiled states present: left, right, top, bottom.
    pub tiled_edges: [bool; 4],
    pub resizing: bool,
    pub maximized: bool,
    pub fullscreen: bool,
}

#[derive(Default)]
pub struct Events {
    compositor: Option<WlCompositor>,
    seat: Option<WlSeat>,
    shm: Option<WlShm>,
    wm_base: Option<XdgWmBase>,
    subcompositor: Option<WlSubcompositor>,
    dmabuf: Option<ZwpLinuxDmabufV1>,
    cursor_shape: Option<WpCursorShapeManagerV1>,
    decoration_manager: Option<ZxdgDecorationManagerV1>,
    registry: Option<WlRegistry>,
    /// Every global advertised to this client: (name, interface).
    pub globals: Vec<(u32, String)>,
    /// Per xdg_toplevel_decoration: the modes its configure events gave, in order.
    pub decoration_modes: HashMap<ObjectId, Vec<zxdg_toplevel_decoration_v1::Mode>>,
    keyboard: Option<WlKeyboard>,
    pointer: Option<WlPointer>,
    pub configure_serial: Option<u32>,
    /// Number of xdg_surface.configure events received.
    pub configures: u32,
    /// Size and tiled flag of the last xdg_toplevel.configure.
    pub toplevel_size: Option<(i32, i32)>,
    pub tiled: bool,
    pub keyboard_entered: bool,
    /// Whether the client's surface has keyboard focus now (enter minus leave).
    pub has_keyboard_focus: bool,
    /// Evdev codes of wl_keyboard.key presses received.
    pub keys_pressed: Vec<u32>,
    pub pointer_entered: bool,
    /// Serial of the last wl_pointer.enter (for set_cursor and cursor shapes).
    pub pointer_enter_serial: Option<u32>,
    pub frames_done: u32,
    pub close_requested: bool,
    /// Per window: last configure (by xdg_toplevel) and its serial (by xdg_surface).
    configs: HashMap<ObjectId, ToplevelConfig>,
    pub serials: HashMap<ObjectId, u32>,
    /// Serial of the last wl_pointer.button press, and all buttons pressed.
    pub button_serial: Option<u32>,
    pub buttons_pressed: Vec<u32>,
    /// The surface each button press went to.
    pub button_surfaces: Vec<ObjectId>,
    /// The surfaces with keyboard and pointer focus now.
    pub keyboard_focus: Option<ObjectId>,
    pub pointer_focus: Option<ObjectId>,
    /// Per popup: the last xdg_popup.configure (x, y, width, height).
    popup_configs: HashMap<ObjectId, (i32, i32, i32, i32)>,
    /// Popups that got popup_done, in order.
    pub popups_done: Vec<ObjectId>,
    /// Tokens of xdg_popup.repositioned events.
    pub repositioned: Vec<u32>,
    /// Buffers from zwp_linux_buffer_params_v1.created, and failed events.
    pub dmabufs_created: Vec<WlBuffer>,
    pub dmabufs_failed: u32,
    /// main_device of the default dmabuf feedback.
    pub dmabuf_main_device: Option<u64>,
    pub dmabuf_feedback_done: bool,
    /// Clipboard and drag and drop (client/data.rs).
    pub data: data::DataEvents,
}

/// What a test popup asks for (an xdg_positioner).
#[derive(Clone, Copy, Debug)]
pub struct PopupSpec {
    pub size: (i32, i32),
    pub anchor_rect: (i32, i32, i32, i32),
    pub anchor: Anchor,
    pub gravity: Gravity,
    pub adjust: ConstraintAdjustment,
    pub offset: (i32, i32),
    pub reactive: bool,
}

impl Default for PopupSpec {
    /// A 50x40 menu below a 10x10 anchor at (10, 10), sliding to stay on the output.
    fn default() -> Self {
        Self {
            size: (50, 40),
            anchor_rect: (10, 10, 10, 10),
            anchor: Anchor::BottomLeft,
            gravity: Gravity::BottomRight,
            adjust: ConstraintAdjustment::SlideX | ConstraintAdjustment::SlideY,
            offset: (0, 0),
            reactive: false,
        }
    }
}

pub struct TestPopup {
    pub surface: WlSurface,
    pub xdg_surface: XdgSurface,
    pub popup: XdgPopup,
}

pub struct TestWindow {
    pub surface: WlSurface,
    pub xdg_surface: XdgSurface,
    pub toplevel: XdgToplevel,
    /// The size this client draws when the compositor lets it choose.
    pub preferred: (i32, i32),
    /// Fill colour of its buffers, ARGB8888 (0: transparent).
    pub color: u32,
}

pub struct TestClient {
    pub conn: Connection,
    queue: EventQueue<Events>,
    pub events: Events,
    /// Window and popup buffers are dmabufs instead of wl_shm.
    pub dmabuf_buffers: bool,
}

impl TestClient {
    pub fn connect(socket: &Path) -> Self {
        let stream = UnixStream::connect(socket).expect("connect to sash-wm");
        let conn = Connection::from_socket(stream).expect("wayland connection");
        Self::from_connection(conn)
    }

    /// A client on an existing connection (a spawned helper's WAYLAND_SOCKET).
    pub fn from_connection(conn: Connection) -> Self {
        let queue = conn.new_event_queue();
        let registry = conn.display().get_registry(&queue.handle(), ());
        let mut client = Self {
            conn,
            queue,
            events: Events::default(),
            dmabuf_buffers: false,
        };
        client.events.registry = Some(registry);
        client.roundtrip(); // globals
        client.roundtrip(); // seat capabilities
        client
    }

    pub fn qh(&self) -> QueueHandle<Events> {
        self.queue.handle()
    }

    pub fn roundtrip(&mut self) {
        self.try_roundtrip().expect("roundtrip");
    }

    pub fn try_roundtrip(&mut self) -> Result<usize, DispatchError> {
        self.queue.roundtrip(&mut self.events)
    }

    pub fn dispatch_until(&mut self, what: &str, done: impl Fn(&Events) -> bool) {
        while !done(&self.events) {
            self.queue
                .blocking_dispatch(&mut self.events)
                .unwrap_or_else(|err| panic!("waiting for {what}: {err}"));
        }
    }

    /// Binds global `name` as the stand-in privileged interface, whether or not it was
    /// advertised (a guess); the server's answer shows on the next roundtrip.
    pub fn bind_guessed_stand_in(&mut self, name: u32) {
        let qh = self.qh();
        let registry = self.events.registry.as_ref().expect("registry");
        let _: ExtForeignToplevelListV1 = registry.bind(name, 1, &qh, ());
    }

    /// Passes this client's own connection descriptor as the fd of a
    /// wl_shm.create_pool request (a socket is no memory to map).
    pub fn create_pool_from_own_connection(&mut self) {
        let qh = self.qh();
        let shm = self.events.shm.clone().expect("wl_shm");
        let _pool = shm.create_pool(self.conn.backend().poll_fd(), 4096, &qh, ());
        let _ = self.conn.flush();
    }

    /// Asks for `mime` of `offer` with this client's own connection descriptor as the
    /// fd: sash-wm passes it to the selection's source, which writes its payload there.
    pub fn receive_into_own_connection(&mut self, offer: &WlDataOffer, mime: &str) {
        offer.receive(mime.to_string(), self.conn.backend().poll_fd());
        let _ = self.conn.flush();
    }

    /// Negotiates xdg-decoration for a window (before or after its initial commit),
    /// asking for `mode` if given.
    pub fn decorate(
        &mut self,
        window: &TestWindow,
        mode: Option<zxdg_toplevel_decoration_v1::Mode>,
    ) -> ZxdgToplevelDecorationV1 {
        let qh = self.qh();
        let decoration = self
            .events
            .decoration_manager
            .as_ref()
            .expect("zxdg_decoration_manager_v1")
            .get_toplevel_decoration(&window.toplevel, &qh, ());
        if let Some(mode) = mode {
            decoration.set_mode(mode);
        }
        decoration
    }

    /// The modes the compositor's decoration configures gave for `decoration`.
    pub fn decoration_modes(
        &self,
        decoration: &ZxdgToplevelDecorationV1,
    ) -> Vec<zxdg_toplevel_decoration_v1::Mode> {
        self.events
            .decoration_modes
            .get(&decoration.id())
            .cloned()
            .unwrap_or_default()
    }

    /// Creates a toplevel and sends the initial commit (no buffer).
    pub fn create_window(&mut self) -> TestWindow {
        let window = self.create_window_without_commit();
        window.surface.commit();
        window
    }

    pub fn create_window_without_commit(&mut self) -> TestWindow {
        let qh = self.qh();
        let surface = self
            .events
            .compositor
            .as_ref()
            .expect("wl_compositor")
            .create_surface(&qh, ());
        let xdg_surface = self
            .events
            .wm_base
            .as_ref()
            .expect("xdg_wm_base")
            .get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg_surface.get_toplevel(&qh, ());
        toplevel.set_title("sash-wm test".into());
        TestWindow {
            surface,
            xdg_surface,
            toplevel,
            preferred: WINDOW_SIZE,
            color: 0,
        }
    }

    /// Creates a toplevel, waits for its configure, acks it and commits a buffer.
    pub fn map_window(&mut self) -> TestWindow {
        let window = self.create_window();
        let seen = self.events.configures;
        self.dispatch_until("initial configure", |e| e.configures > seen);
        self.ack_and_commit(&window);
        window
    }

    /// Acks the last configure and commits a buffer of the configured size, like a
    /// client that redraws on every configure.
    pub fn ack_and_commit(&mut self, window: &TestWindow) {
        let serial = self.events.serials[&window.xdg_surface.id()];
        window.xdg_surface.ack_configure(serial);
        self.attach_buffer(window);
        window.surface.commit();
        self.roundtrip();
    }

    /// Acks the last configure but commits a buffer of its own size, like a client
    /// that rounds to its cell grid.
    pub fn ack_and_commit_size(&mut self, window: &TestWindow, size: (i32, i32)) {
        let serial = self.events.serials[&window.xdg_surface.id()];
        window.xdg_surface.ack_configure(serial);
        let buffer = self.buffer(size, window.color);
        window.surface.attach(Some(&buffer), 0, 0);
        window.surface.damage_buffer(0, 0, size.0, size.1);
        window.surface.commit();
        self.roundtrip();
    }

    /// Commits a buffer of its own size without acking anything.
    pub fn commit_size(&mut self, window: &TestWindow, size: (i32, i32)) {
        let buffer = self.buffer(size, window.color);
        window.surface.attach(Some(&buffer), 0, 0);
        window.surface.damage_buffer(0, 0, size.0, size.1);
        window.surface.commit();
        self.roundtrip();
    }

    /// The last configure this window got.
    pub fn config(&self, window: &TestWindow) -> Option<ToplevelConfig> {
        self.events.configs.get(&window.toplevel.id()).copied()
    }

    /// Size of the next buffer: the configured size, or the client's own choice if the
    /// compositor left it to the client (0x0).
    fn buffer_size(&self, window: &TestWindow) -> (i32, i32) {
        match self.config(window).map(|c| c.size) {
            Some((w, h)) if w > 0 && h > 0 => (w, h),
            _ => window.preferred,
        }
    }

    pub fn attach_buffer(&mut self, window: &TestWindow) {
        let size = self.buffer_size(window);
        let buffer = self.buffer(size, window.color);
        window.surface.attach(Some(&buffer), 0, 0);
        window.surface.damage_buffer(0, 0, size.0, size.1);
    }

    /// Destroys the window (what a client does on close).
    pub fn destroy(&mut self, window: TestWindow) {
        window.toplevel.destroy();
        window.xdg_surface.destroy();
        window.surface.destroy();
        self.roundtrip();
    }

    /// Asks the compositor to move the window, with the serial of the last button press.
    pub fn request_move(&mut self, window: &TestWindow) {
        let serial = self.events.button_serial.expect("a button press");
        window
            .toplevel
            ._move(self.events.seat.as_ref().expect("wl_seat"), serial);
        self.roundtrip();
    }

    /// Asks the compositor to resize the window from the given edge.
    pub fn request_resize(&mut self, window: &TestWindow, edge: xdg_toplevel::ResizeEdge) {
        let serial = self.events.button_serial.expect("a button press");
        window
            .toplevel
            .resize(self.events.seat.as_ref().expect("wl_seat"), serial, edge);
        self.roundtrip();
    }

    /// Unmaps the window by committing a null buffer; it stays alive.
    pub fn unmap(&mut self, window: &TestWindow) {
        window.surface.attach(None, 0, 0);
        window.surface.commit();
        self.roundtrip();
    }

    /// Requests a frame callback, damages the surface and commits.
    pub fn commit_with_frame_request(&mut self, window: &TestWindow) {
        window.surface.frame(&self.qh(), ());
        self.commit_damage(window);
    }

    pub fn commit_damage(&mut self, window: &TestWindow) {
        window
            .surface
            .damage_buffer(0, 0, WINDOW_SIZE.0, WINDOW_SIZE.1);
        window.surface.commit();
        self.conn.flush().expect("flush");
    }

    fn positioner(&self, spec: PopupSpec) -> XdgPositioner {
        let wm_base = self.events.wm_base.as_ref().expect("xdg_wm_base");
        let positioner = wm_base.create_positioner(&self.qh(), ());
        positioner.set_size(spec.size.0, spec.size.1);
        let (x, y, w, h) = spec.anchor_rect;
        positioner.set_anchor_rect(x, y, w, h);
        positioner.set_anchor(spec.anchor);
        positioner.set_gravity(spec.gravity);
        positioner.set_constraint_adjustment(spec.adjust);
        positioner.set_offset(spec.offset.0, spec.offset.1);
        if spec.reactive {
            positioner.set_reactive();
        }
        positioner
    }

    /// Creates a popup on `parent` (a toplevel's or popup's xdg_surface), grabs with
    /// `grab` as the serial if given, and sends the initial commit.
    pub fn create_popup(
        &mut self,
        parent: &XdgSurface,
        spec: PopupSpec,
        grab: Option<u32>,
    ) -> TestPopup {
        let qh = self.qh();
        let positioner = self.positioner(spec);
        let surface = self
            .events
            .compositor
            .as_ref()
            .expect("wl_compositor")
            .create_surface(&qh, ());
        let wm_base = self.events.wm_base.as_ref().expect("xdg_wm_base");
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
        let popup = xdg_surface.get_popup(Some(parent), &positioner, &qh, ());
        positioner.destroy();
        if let Some(serial) = grab {
            popup.grab(self.events.seat.as_ref().expect("wl_seat"), serial);
        }
        surface.commit();
        self.roundtrip();
        TestPopup {
            surface,
            xdg_surface,
            popup,
        }
    }

    /// Creates a popup, waits for its configure, acks it and commits a buffer of the
    /// configured size.
    pub fn open_popup(
        &mut self,
        parent: &XdgSurface,
        spec: PopupSpec,
        grab: Option<u32>,
    ) -> TestPopup {
        let popup = self.create_popup(parent, spec, grab);
        let id = popup.xdg_surface.id();
        self.dispatch_until("popup configure", |e| e.serials.contains_key(&id));
        self.ack_and_commit_popup(&popup);
        popup
    }

    pub fn ack_and_commit_popup(&mut self, popup: &TestPopup) {
        let serial = self.events.serials[&popup.xdg_surface.id()];
        popup.xdg_surface.ack_configure(serial);
        let (_, _, w, h) = self.popup_config(popup).expect("popup configured");
        let buffer = self.buffer((w, h), 0xff20_8020);
        popup.surface.attach(Some(&buffer), 0, 0);
        popup.surface.damage_buffer(0, 0, w, h);
        popup.surface.commit();
        self.roundtrip();
    }

    /// The last xdg_popup.configure: position relative to the parent, and size.
    pub fn popup_config(&self, popup: &TestPopup) -> Option<(i32, i32, i32, i32)> {
        self.events.popup_configs.get(&popup.popup.id()).copied()
    }

    pub fn popup_done(&self, popup: &TestPopup) -> bool {
        self.events.popups_done.contains(&popup.popup.id())
    }

    /// Adds a desynchronized subsurface of `size` at `position` on `parent`, with a
    /// buffer (and so the default, infinite input region), and commits the parent.
    pub fn add_subsurface(
        &mut self,
        parent: &WlSurface,
        position: (i32, i32),
        size: (i32, i32),
    ) -> (WlSurface, WlSubsurface) {
        let qh = self.qh();
        let surface = self
            .events
            .compositor
            .as_ref()
            .expect("wl_compositor")
            .create_surface(&qh, ());
        let subsurface = self
            .events
            .subcompositor
            .as_ref()
            .expect("wl_subcompositor")
            .get_subsurface(&surface, parent, &qh, ());
        subsurface.set_position(position.0, position.1);
        subsurface.set_desync();
        let buffer = self.buffer(size, 0xff80_2020);
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, size.0, size.1);
        surface.commit();
        parent.commit();
        self.roundtrip();
        (surface, subsurface)
    }

    /// Sets a named cursor shape with `serial` (normally the pointer's enter serial).
    pub fn set_cursor_shape(&mut self, serial: u32, shape: Shape) {
        let manager = self
            .events
            .cursor_shape
            .as_ref()
            .expect("cursor shape manager");
        let pointer = self.events.pointer.as_ref().expect("wl_pointer");
        let device = manager.get_pointer(pointer, &self.qh(), ());
        device.set_shape(serial, shape);
        device.destroy();
        self.roundtrip();
    }

    /// Sets a cursor surface of `size` with `hotspot`, using the pointer's enter serial.
    pub fn set_cursor_surface(&mut self, size: (i32, i32), hotspot: (i32, i32)) -> WlSurface {
        let qh = self.qh();
        let surface = self
            .events
            .compositor
            .as_ref()
            .expect("wl_compositor")
            .create_surface(&qh, ());
        let buffer = self.buffer(size, 0xffff_ffff);
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, size.0, size.1);
        surface.commit();
        let serial = self.events.pointer_enter_serial.expect("pointer entered");
        let pointer = self.events.pointer.as_ref().expect("wl_pointer");
        pointer.set_cursor(serial, Some(&surface), hotspot.0, hotspot.1);
        self.roundtrip();
        surface
    }

    /// A wl_buffer for a real one-plane dmabuf (create_immed; GPU tests, debug builds).
    #[cfg(debug_assertions)]
    pub fn dmabuf_buffer(&mut self, plane: &DmabufPlane) -> WlBuffer {
        let params = self.dmabuf_params();
        params.add(
            plane.fd.as_fd(),
            0,
            plane.offset,
            plane.stride,
            (plane.modifier >> 32) as u32,
            plane.modifier as u32,
        );
        let buffer = params.create_immed(
            plane.size.0,
            plane.size.1,
            plane.fourcc,
            zwp_linux_buffer_params_v1::Flags::empty(),
            &self.qh(),
            (),
        );
        params.destroy();
        self.roundtrip();
        buffer
    }

    /// New zwp_linux_buffer_params_v1.
    pub fn dmabuf_params(&self) -> ZwpLinuxBufferParamsV1 {
        let dmabuf = self.events.dmabuf.as_ref().expect("zwp_linux_dmabuf_v1");
        dmabuf.create_params(&self.qh(), ())
    }

    /// Requests the default feedback and waits for its done event.
    pub fn dmabuf_default_feedback(&mut self) {
        let dmabuf = self.events.dmabuf.as_ref().expect("zwp_linux_dmabuf_v1");
        let _feedback = dmabuf.get_default_feedback(&self.qh(), ());
        self.dispatch_until("dmabuf feedback", |e| e.dmabuf_feedback_done);
    }

    /// Requests a frame callback on the popup and commits.
    pub fn popup_frame_request(&mut self, popup: &TestPopup) {
        popup.surface.frame(&self.qh(), ());
        popup.surface.commit();
        self.roundtrip();
    }

    pub fn reposition(&mut self, popup: &TestPopup, spec: PopupSpec, token: u32) {
        let positioner = self.positioner(spec);
        popup.popup.reposition(&positioner, token);
        positioner.destroy();
        self.roundtrip();
    }

    /// Destroys a popup's objects (xdg_popup first, as the protocol asks).
    pub fn destroy_popup(&mut self, popup: TestPopup) {
        popup.popup.destroy();
        popup.xdg_surface.destroy();
        popup.surface.destroy();
        let _ = self.try_roundtrip();
    }

    /// An ARGB8888 shm buffer, backed by an unlinked file in XDG_RUNTIME_DIR.
    /// A buffer for a window or popup: wl_shm, or a (fake) LINEAR ARGB8888 dmabuf when
    /// `dmabuf_buffers` is set.
    fn buffer(&mut self, (width, height): (i32, i32), color: u32) -> WlBuffer {
        if self.dmabuf_buffers {
            let file = buffer_file((width * 4 * height) as u64);
            let params = self.dmabuf_params();
            params.add(file.as_fd(), 0, 0, (width * 4) as u32, 0, 0);
            let buffer = params.create_immed(
                width,
                height,
                DrmFourcc::ARGB8888,
                zwp_linux_buffer_params_v1::Flags::empty(),
                &self.qh(),
                (),
            );
            params.destroy();
            return buffer;
        }
        let stride = width * 4;
        let size = stride * height;
        let file = buffer_file(size as u64);
        if color != 0 {
            use std::io::Write;
            let row: Vec<u8> = color.to_le_bytes().repeat(width as usize);
            let mut writer = std::io::BufWriter::new(&file);
            for _ in 0..height {
                writer.write_all(&row).expect("fill shm file");
            }
        }

        let qh = self.qh();
        let pool =
            self.events
                .shm
                .as_ref()
                .expect("wl_shm")
                .create_pool(file.as_fd(), size, &qh, ());
        let buffer =
            pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, &qh, ());
        pool.destroy();
        buffer
    }
}

/// An unlinked file of `size` bytes in XDG_RUNTIME_DIR, for shm pools and fake dmabuf
/// planes (headless tests never read their content).
pub fn buffer_file(size: u64) -> std::fs::File {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let runtime_dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"));
    let path = runtime_dir.join(format!(
        "sash-wm-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .expect("buffer file");
    std::fs::remove_file(&path).expect("unlink buffer file");
    file.set_len(size).expect("size buffer file");
    file
}

/// A one-plane dmabuf made on the server side (GPU tests), handed to a client.
#[cfg(debug_assertions)]
pub struct DmabufPlane {
    pub fd: std::os::fd::OwnedFd,
    pub offset: u32,
    pub stride: u32,
    pub modifier: u64,
    pub fourcc: u32,
    pub size: (i32, i32),
}

/// DRM fourcc codes used by the tests.
pub struct DrmFourcc;

impl DrmFourcc {
    pub const ARGB8888: u32 = u32::from_le_bytes(*b"AR24");
    pub const XRGB8888: u32 = u32::from_le_bytes(*b"XR24");
    pub const NV12: u32 = u32::from_le_bytes(*b"NV12");
    /// Not in the fake format table.
    pub const RGB565: u32 = u32::from_le_bytes(*b"RG16");
}

impl Dispatch<WlRegistry, ()> for Events {
    fn event(
        events: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let (name, interface, version) = match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => (name, interface, version),
            wl_registry::Event::GlobalRemove { name } => {
                events.globals.retain(|(n, _)| *n != name);
                return;
            }
            _ => return,
        };
        events.globals.push((name, interface.clone()));
        match interface.as_str() {
            "wl_compositor" => {
                events.compositor = Some(registry.bind(name, version.min(4), qh, ()))
            }
            "wl_shm" => events.shm = Some(registry.bind(name, 1, qh, ())),
            "wl_subcompositor" => events.subcompositor = Some(registry.bind(name, 1, qh, ())),
            "wp_cursor_shape_manager_v1" => {
                events.cursor_shape = Some(registry.bind(name, 1, qh, ()))
            }
            "zwp_linux_dmabuf_v1" => {
                events.dmabuf = Some(registry.bind(name, version.min(5), qh, ()))
            }
            "xdg_wm_base" => events.wm_base = Some(registry.bind(name, version.min(3), qh, ())),
            "zxdg_decoration_manager_v1" => {
                events.decoration_manager = Some(registry.bind(name, 1, qh, ()))
            }
            "wl_seat" => {
                events.seat = Some(registry.bind::<WlSeat, _, _>(name, version.min(5), qh, ()));
            }
            "wl_data_device_manager" => {
                events.data.manager = Some(registry.bind(name, version.min(3), qh, ()))
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for Events {
    fn event(
        events: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        {
            if capabilities.contains(wl_seat::Capability::Keyboard) && events.keyboard.is_none() {
                events.keyboard = Some(seat.get_keyboard(qh, ()));
            }
            if capabilities.contains(wl_seat::Capability::Pointer) && events.pointer.is_none() {
                events.pointer = Some(seat.get_pointer(qh, ()));
            }
        }
    }
}

impl Dispatch<WlKeyboard, ()> for Events {
    fn event(
        events: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Enter { surface, .. } => {
                events.keyboard_entered = true;
                events.has_keyboard_focus = true;
                events.keyboard_focus = Some(surface.id());
            }
            wl_keyboard::Event::Leave { .. } => {
                events.has_keyboard_focus = false;
                events.keyboard_focus = None;
            }
            wl_keyboard::Event::Key {
                key,
                state: WEnum::Value(wl_keyboard::KeyState::Pressed),
                ..
            } => events.keys_pressed.push(key),
            _ => {}
        }
    }
}

impl Dispatch<WlPointer, ()> for Events {
    fn event(
        events: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                surface, serial, ..
            } => {
                events.pointer_enter_serial = Some(serial);
                events.pointer_entered = true;
                events.pointer_focus = Some(surface.id());
            }
            wl_pointer::Event::Leave { .. } => events.pointer_focus = None,
            wl_pointer::Event::Button {
                serial,
                button,
                state: WEnum::Value(wl_pointer::ButtonState::Pressed),
                ..
            } => {
                events.button_serial = Some(serial);
                events.buttons_pressed.push(button);
                if let Some(focus) = events.pointer_focus.clone() {
                    events.button_surfaces.push(focus);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<XdgWmBase, ()> for Events {
    fn event(
        _: &mut Self,
        wm_base: &XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, ()> for Events {
    fn event(
        events: &mut Self,
        xdg_surface: &XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            events.configure_serial = Some(serial);
            events.serials.insert(xdg_surface.id(), serial);
            events.configures += 1;
        }
    }
}

impl Dispatch<XdgToplevel, ()> for Events {
    fn event(
        events: &mut Self,
        toplevel: &XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                events.toplevel_size = Some((width, height));
                // States are a u32 array; 5..=8 are tiled_left/right/top/bottom.
                let states: Vec<u32> = states
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|bytes| u32::from_ne_bytes(*bytes))
                    .collect();
                events.tiled = (5..=8).all(|tiled| states.contains(&tiled));
                // 3 is resizing.
                let config = ToplevelConfig {
                    size: (width, height),
                    tiled: events.tiled,
                    tiled_edges: [5, 6, 7, 8].map(|s| states.contains(&s)),
                    resizing: states.contains(&3),
                    maximized: states.contains(&1),
                    fullscreen: states.contains(&2),
                };
                events.configs.insert(toplevel.id(), config);
            }
            xdg_toplevel::Event::Close => events.close_requested = true,
            _ => {}
        }
    }
}

impl Dispatch<WlCallback, ()> for Events {
    fn event(
        events: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            events.frames_done += 1;
        }
    }
}

impl Dispatch<XdgPopup, ()> for Events {
    fn event(
        events: &mut Self,
        popup: &XdgPopup,
        event: xdg_popup::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_popup::Event::Configure {
                x,
                y,
                width,
                height,
            } => {
                events
                    .popup_configs
                    .insert(popup.id(), (x, y, width, height));
            }
            xdg_popup::Event::PopupDone => events.popups_done.push(popup.id()),
            xdg_popup::Event::Repositioned { token } => events.repositioned.push(token),
            _ => {}
        }
    }
}

impl Dispatch<ZwpLinuxBufferParamsV1, ()> for Events {
    fn event(
        events: &mut Self,
        _: &ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_linux_buffer_params_v1::Event::Created { buffer } => {
                events.dmabufs_created.push(buffer)
            }
            zwp_linux_buffer_params_v1::Event::Failed => events.dmabufs_failed += 1,
            _ => {}
        }
    }

    wayland_client::event_created_child!(Events, ZwpLinuxBufferParamsV1, [
        zwp_linux_buffer_params_v1::EVT_CREATED_OPCODE => (WlBuffer, ()),
    ]);
}

impl Dispatch<ZwpLinuxDmabufFeedbackV1, ()> for Events {
    fn event(
        events: &mut Self,
        _: &ZwpLinuxDmabufFeedbackV1,
        event: zwp_linux_dmabuf_feedback_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_linux_dmabuf_feedback_v1::Event::MainDevice { device } => {
                let bytes: [u8; 8] = device.as_slice().try_into().expect("dev_t");
                events.dmabuf_main_device = Some(u64::from_ne_bytes(bytes));
            }
            zwp_linux_dmabuf_feedback_v1::Event::Done => events.dmabuf_feedback_done = true,
            _ => {}
        }
    }
}

delegate_noop!(Events: ignore ZwpLinuxDmabufV1);
delegate_noop!(Events: WpCursorShapeManagerV1);
delegate_noop!(Events: ZxdgDecorationManagerV1);
delegate_noop!(Events: ignore ExtForeignToplevelListV1);

impl Dispatch<ZxdgToplevelDecorationV1, ()> for Events {
    fn event(
        events: &mut Self,
        decoration: &ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_toplevel_decoration_v1::Event::Configure {
            mode: WEnum::Value(mode),
        } = event
        {
            events
                .decoration_modes
                .entry(decoration.id())
                .or_default()
                .push(mode);
        }
    }
}
delegate_noop!(Events: WpCursorShapeDeviceV1);
delegate_noop!(Events: XdgPositioner);
delegate_noop!(Events: WlSubcompositor);
delegate_noop!(Events: WlSubsurface);
delegate_noop!(Events: WlCompositor);
delegate_noop!(Events: WlShmPool);
delegate_noop!(Events: ignore WlShm);
delegate_noop!(Events: ignore WlBuffer);
delegate_noop!(Events: ignore WlSurface);
