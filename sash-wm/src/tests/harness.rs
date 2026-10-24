// SPDX-License-Identifier: Apache-2.0

//! A headless server for tests: the real compositor state and event loop, with a
//! stand-in for the backend whose "presenting" flag the test controls.

use std::{
    cell::Cell,
    path::PathBuf,
    rc::Rc,
    thread::JoinHandle,
    time::{Duration, Instant},
};

use smithay::{
    output::Output,
    reexports::calloop::{EventLoop, ping},
};

use crate::{output, server::Server};

const TIMEOUT: Duration = Duration::from_secs(5);

pub struct Harness {
    event_loop: EventLoop<'static, Server>,
    pub server: Server,
    output: Output,
    /// Whether the output is presenting. While false, requested frames are held back,
    /// like a hidden nested window.
    pub presenting: bool,
    /// Frames presented so far.
    pub frames: u32,
    redraw_requested: Rc<Cell<bool>>,
}

impl Harness {
    pub fn new() -> Self {
        let event_loop = EventLoop::try_new().expect("event loop");
        let (redraw, redraw_requests) = ping::make_ping().expect("ping");
        let mut server = Server::new(&event_loop, redraw, Instant::now()).expect("server");

        let redraw_requested = Rc::new(Cell::new(false));
        let flag = redraw_requested.clone();
        event_loop
            .handle()
            .insert_source(redraw_requests, move |_, _, _| flag.set(true))
            .expect("redraw source");
        let output = output::create(&mut server.state, "headless", (800, 600).into());

        Self {
            event_loop,
            server,
            output,
            presenting: true,
            frames: 0,
            redraw_requested,
        }
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
            self.frames += 1;
            self.server.state.frame_presented(&self.output);
        }
        self.server.flush();
    }

    /// Steps until `done` returns true; panics after a timeout.
    pub fn run_until(&mut self, what: &str, mut done: impl FnMut(&mut Self) -> bool) {
        let deadline = Instant::now() + TIMEOUT;
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
        self.server.state.space.elements().count()
    }

    pub fn unmapped_windows(&self) -> usize {
        self.server.state.unmapped.len()
    }
}
