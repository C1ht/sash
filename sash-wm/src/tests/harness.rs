// SPDX-License-Identifier: Apache-2.0

//! A headless server for tests: the real compositor state and event loop, with a
//! stand-in for the backend whose "presenting" flag the test controls.

use std::{
    cell::Cell,
    path::PathBuf,
    rc::Rc,
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        allocator::{Format, Fourcc, Modifier as DrmModifier},
        input::{ButtonState, KeyState},
    },
    input::keyboard::Keycode,
    output::Output,
    reexports::calloop::{EventLoop, ping},
};

use crate::{
    keybinds::Modifier,
    layout::{Mode, Rect, WindowId},
    output,
    server::Server,
};

const TIMEOUT: Duration = Duration::from_secs(5);

/// The dmabuf main device the harness advertises: 226:128 (a render node's numbers).
/// The configure size of a window without a bar in a w x h tile (or a snapped or
/// maximized rectangle): the rectangle minus its 1 px border on all four edges
/// (owner, M18a; before, the edges on the output boundary had none).
pub fn tile(w: i32, h: i32) -> Option<(i32, i32)> {
    // The 1 px border on every edge (owner, M18a: screen edges too).
    Some(((w - 2).max(1), (h - 2).max(1)))
}

pub const FAKE_MAIN_DEVICE: u64 = (226 << 8) | 128;

/// The harness's dmabuf format table: no GPU behind it, every buffer "imports".
fn fake_dmabuf_formats() -> Vec<Format> {
    [Fourcc::Argb8888, Fourcc::Xrgb8888, Fourcc::Nv12]
        .into_iter()
        .map(|code| Format {
            code,
            modifier: DrmModifier::Linear,
        })
        .collect()
}

pub struct Harness {
    event_loop: EventLoop<'static, Server>,
    pub server: Server,
    output: Output,
    /// Whether the output is presenting. While false, requested frames are held back,
    /// like a hidden nested window.
    pub presenting: bool,
    /// Frames presented so far.
    pub frames: u32,
    /// How long `run_until` waits (TIMEOUT; longer for replays).
    pub timeout: Duration,
    redraw_requested: Rc<Cell<bool>>,
}

impl Harness {
    /// A harness advertising the fake dmabuf format table.
    pub fn new() -> Self {
        Self::with_dmabuf_formats(fake_dmabuf_formats())
    }

    /// A harness advertising `formats` as dmabufs (every buffer "imports"; a GPU test
    /// imports for real when it renders).
    pub fn with_dmabuf_formats(formats: Vec<Format>) -> Self {
        Self::build(formats, Modifier::Alt)
    }

    /// A harness whose keybind modifier is `modifier` (replays of DRM bundles: Super).
    #[cfg(feature = "debug-record")]
    pub fn with_modifier(modifier: Modifier) -> Self {
        Self::build(fake_dmabuf_formats(), modifier)
    }

    fn build(formats: Vec<Format>, modifier: Modifier) -> Self {
        let event_loop = EventLoop::try_new().expect("event loop");
        let (redraw, redraw_requests) = ping::make_ping().expect("ping");
        let mut server =
            Server::new(&event_loop, redraw, Instant::now(), modifier).expect("server");

        let redraw_requested = Rc::new(Cell::new(false));
        let flag = redraw_requested.clone();
        event_loop
            .handle()
            .insert_source(redraw_requests, move |_, _, _| flag.set(true))
            .expect("redraw source");
        server
            .state
            .enable_dmabuf(FAKE_MAIN_DEVICE, formats, Box::new(|_| true));
        // The tests written before gaps (M15b) expect exact tiles: the harness starts
        // with no gap; the gap tests set it (tests/gaps.rs).
        server
            .state
            .windows
            .layout
            .set_gaps(crate::layout::gaps::Gaps::NONE);
        // Likewise square corners (M15b step 2); the rounded-corner tests set a radius.
        server.state.windows.layout.set_radius(0);
        let output = output::create(&mut server.state, "headless", (800, 600).into());
        server.state.set_layout_area((800, 600).into());

        Self {
            event_loop,
            server,
            output,
            presenting: true,
            frames: 0,
            timeout: TIMEOUT,
            redraw_requested,
        }
    }

    /// Opens the synthetic input socket (debug builds) and returns its path.
    #[cfg(debug_assertions)]
    pub fn listen_synthetic(&self) -> PathBuf {
        crate::synthetic::listen(&self.event_loop, &self.server.socket_name)
            .expect("synthetic input socket")
    }

    pub fn socket_path(&self) -> PathBuf {
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR must be set");
        PathBuf::from(runtime_dir).join(&self.server.socket_name)
    }

    /// One iteration: handle client requests, present one frame if one was requested
    /// and the output is presenting, then flush events to clients.
    pub fn step(&mut self) {
        self.event_loop
            .dispatch(Some(Duration::from_millis(2)), &mut self.server)
            .expect("event loop dispatch");
        if self.presenting && self.redraw_requested.replace(false) {
            self.server.state.frame_started();
            self.server.state.flush_pending_arrange();
            // What drawing does for the bars' titles (shaping, the rate limit).
            self.server.state.draw_bar_titles();
            self.frames += 1;
            self.server.state.frame_presented(&self.output);
            // As the DRM backend does after each frame.
            self.server.state.schedule_cursor_animation();
        }
        self.server.flush();
    }

    /// Steps until `done` returns true; panics after a timeout.
    pub fn run_until(&mut self, what: &str, mut done: impl FnMut(&mut Self) -> bool) {
        let deadline = Instant::now() + self.timeout;
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.step();
        }
    }

    /// Steps until the client thread ends, then re-raises its panic if it failed.
    pub fn run_client<T>(&mut self, client: JoinHandle<T>) -> T {
        self.run_until("the test client to finish", |_| client.is_finished());
        client
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    }

    pub fn mapped_windows(&self) -> usize {
        self.server.state.windows.all().filter(|m| m.mapped).count()
    }

    /// Toplevels the compositor knows that have not mapped (yet).
    pub fn unmapped_windows(&self) -> usize {
        self.server
            .state
            .windows
            .all()
            .filter(|m| !m.mapped)
            .count()
    }

    /// The tile of the n-th toplevel created in this harness (ids start at 1).
    pub fn rect_of(&self, id: u64) -> Option<Rect> {
        self.server.state.windows.layout.rect_of(WindowId(id))
    }

    /// Id of the window with keyboard focus.
    pub fn keyboard_focus(&self) -> Option<u64> {
        let focus = self.server.state.seat.get_keyboard()?.current_focus()?;
        self.server.state.windows.by_surface(&focus).map(|m| m.id.0)
    }

    /// Presses or releases one key (evdev code), e.g. to hold a modifier.
    pub fn key(&mut self, key: u32, state: KeyState) {
        self.server.state.key(Keycode::new(key + 8), state, 0);
        self.server.flush();
    }

    pub fn pointer_to(&mut self, x: f64, y: f64) {
        self.server.state.pointer_moved_to((x, y).into(), 0);
        self.server.flush();
    }

    pub fn button(&mut self, button: u32, state: ButtonState) {
        self.server.state.pointer_button(button, state, 0);
        self.server.flush();
    }

    pub fn mode_of(&self, id: u64) -> Option<Mode> {
        self.server.state.windows.layout.mode(WindowId(id))
    }

    pub fn pointer_grabbed(&self) -> bool {
        self.server
            .state
            .seat
            .get_pointer()
            .is_some_and(|p| p.is_grabbed())
    }

    /// Presses the keys in order, then releases them in reverse (evdev codes).
    pub fn chord(&mut self, keys: &[u32]) {
        let mut time = 0;
        for &key in keys {
            time += 1;
            self.server
                .state
                .key(Keycode::new(key + 8), KeyState::Pressed, time);
        }
        for &key in keys.iter().rev() {
            time += 1;
            self.server
                .state
                .key(Keycode::new(key + 8), KeyState::Released, time);
        }
        self.server.flush();
    }
}

/// A step the client thread asks the server thread to run, e.g. a check of the layout
/// or a key chord.
type ServerStep = Box<dyn FnOnce(&mut Harness) + Send>;

/// The client thread's handle to the server thread.
pub struct Remote {
    steps: mpsc::Sender<ServerStep>,
    done: mpsc::Receiver<()>,
}

impl Remote {
    /// Runs `step` on the server thread and waits until it has run.
    pub fn on_server(&self, step: impl FnOnce(&mut Harness) + Send + 'static) {
        self.steps.send(Box::new(step)).expect("server thread gone");
        self.done.recv().expect("server step failed");
    }

    /// Moves the pointer over the focused window, so that (rule A) the next tiled
    /// window splits it: the newest window, as when opening windows one by one.
    pub fn pointer_over_focused(&self) {
        self.on_server(|h| {
            let focused = h.server.state.windows.layout.focused();
            if let Some(r) = focused.and_then(|id| h.server.state.windows.layout.rect_of(id)) {
                h.pointer_to((r.x + r.w / 2) as f64, (r.y + r.h / 2) as f64);
            }
        });
    }
}

impl Harness {
    /// Runs `client` on its own thread while serving it; steps it sends with
    /// `Remote::on_server` run here, between event loop iterations.
    pub fn run_with_remote<T: Send + 'static>(
        &mut self,
        client: impl FnOnce(Remote) -> T + Send + 'static,
    ) -> T {
        let (steps_tx, steps_rx) = mpsc::channel::<ServerStep>();
        let (done_tx, done_rx) = mpsc::channel();
        let remote = Remote {
            steps: steps_tx,
            done: done_rx,
        };
        let handle = thread::spawn(move || client(remote));
        self.run_until("the test client to finish", |h| {
            while let Ok(step) = steps_rx.try_recv() {
                step(h);
                h.server.flush();
                let _ = done_tx.send(());
            }
            handle.is_finished()
        });
        handle
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    }
}

/// Evdev key codes used by the keybind tests.
pub mod keys {
    pub const ALT: u32 = 56;
    pub const SHIFT: u32 = 42;
    pub const H: u32 = 35;
    pub const J: u32 = 36;
    pub const K: u32 = 37;
    pub const L: u32 = 38;
    pub const Q: u32 = 16;
    pub const F: u32 = 33;
    pub const T: u32 = 20;
    pub const TAB: u32 = 15;
    pub const ENTER: u32 = 28;
    pub const CTRL: u32 = 29;
    pub const UP: u32 = 103;
    pub const LEFT: u32 = 105;
    pub const RIGHT: u32 = 106;
    pub const DOWN: u32 = 108;
    pub const Y: u32 = 21;
    pub const U: u32 = 22;
    pub const B: u32 = 48;
    pub const N: u32 = 49;
}
