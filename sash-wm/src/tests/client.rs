// SPDX-License-Identifier: Apache-2.0

//! A minimal Wayland client for tests. Runs on its own thread against a `Harness`.

use std::{
    fs::OpenOptions,
    os::{fd::AsFd, unix::net::UnixStream},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

use wayland_client::{
    Connection, Dispatch, DispatchError, EventQueue, QueueHandle, WEnum, delegate_noop,
    protocol::{
        wl_buffer::WlBuffer,
        wl_callback::{self, WlCallback},
        wl_compositor::WlCompositor,
        wl_keyboard::{self, WlKeyboard},
        wl_pointer::{self, WlPointer},
        wl_registry::{self, WlRegistry},
        wl_seat::{self, WlSeat},
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    },
};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::{self, XdgToplevel},
    xdg_wm_base::{self, XdgWmBase},
};

pub const WINDOW_SIZE: (i32, i32) = (100, 100);

/// Globals the client bound and the events it saw.
#[derive(Default)]
pub struct Events {
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    wm_base: Option<XdgWmBase>,
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
    pub frames_done: u32,
    pub close_requested: bool,
}

pub struct TestWindow {
    pub surface: WlSurface,
    pub xdg_surface: XdgSurface,
    pub toplevel: XdgToplevel,
}

pub struct TestClient {
    pub conn: Connection,
    queue: EventQueue<Events>,
    pub events: Events,
}

impl TestClient {
    pub fn connect(socket: &Path) -> Self {
        let stream = UnixStream::connect(socket).expect("connect to sash-wm");
        let conn = Connection::from_socket(stream).expect("wayland connection");
        let queue = conn.new_event_queue();
        conn.display().get_registry(&queue.handle(), ());
        let mut client = Self {
            conn,
            queue,
            events: Events::default(),
        };
        client.roundtrip(); // globals
        client.roundtrip(); // seat capabilities
        client
    }

    fn qh(&self) -> QueueHandle<Events> {
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
        let serial = self.events.configure_serial.expect("configure serial");
        window.xdg_surface.ack_configure(serial);
        self.attach_buffer(window);
        window.surface.commit();
        self.roundtrip();
    }

    /// Size of the next buffer: the configured size, or a default if the compositor
    /// left it to the client.
    fn buffer_size(&self) -> (i32, i32) {
        match self.events.toplevel_size {
            Some((w, h)) if w > 0 && h > 0 => (w, h),
            _ => WINDOW_SIZE,
        }
    }

    pub fn attach_buffer(&mut self, window: &TestWindow) {
        let size = self.buffer_size();
        let buffer = self.buffer(size);
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

    /// An ARGB8888 shm buffer, backed by an unlinked file in XDG_RUNTIME_DIR.
    fn buffer(&mut self, (width, height): (i32, i32)) -> WlBuffer {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let runtime_dir =
            PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"));
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
            .expect("shm file");
        std::fs::remove_file(&path).expect("unlink shm file");
        let stride = width * 4;
        let size = stride * height;
        file.set_len(size as u64).expect("size shm file");

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

impl Dispatch<WlRegistry, ()> for Events {
    fn event(
        events: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        match interface.as_str() {
            "wl_compositor" => {
                events.compositor = Some(registry.bind(name, version.min(4), qh, ()))
            }
            "wl_shm" => events.shm = Some(registry.bind(name, 1, qh, ())),
            "xdg_wm_base" => events.wm_base = Some(registry.bind(name, version.min(3), qh, ())),
            "wl_seat" => {
                registry.bind::<WlSeat, _, _>(name, version.min(5), qh, ());
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
            wl_keyboard::Event::Enter { .. } => {
                events.keyboard_entered = true;
                events.has_keyboard_focus = true;
            }
            wl_keyboard::Event::Leave { .. } => events.has_keyboard_focus = false,
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
        if let wl_pointer::Event::Enter { .. } = event {
            events.pointer_entered = true;
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
        _: &XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            events.configure_serial = Some(serial);
            events.configures += 1;
        }
    }
}

impl Dispatch<XdgToplevel, ()> for Events {
    fn event(
        events: &mut Self,
        _: &XdgToplevel,
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

delegate_noop!(Events: WlCompositor);
delegate_noop!(Events: WlShmPool);
delegate_noop!(Events: ignore WlShm);
delegate_noop!(Events: ignore WlBuffer);
delegate_noop!(Events: ignore WlSurface);
