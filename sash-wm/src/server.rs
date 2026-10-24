// SPDX-License-Identifier: Apache-2.0

//! The data the event loop owns: the Wayland display and the compositor state.

use std::{ffi::OsString, time::Instant};

use smithay::reexports::{
    calloop::{EventLoop, Interest, Mode, PostAction, generic::Generic, ping::Ping},
    wayland_server::Display,
};

use crate::{clients, keybinds::Modifier, state::Sash};

pub struct Server {
    pub display: Display<Sash>,
    pub state: Sash,
    /// Name of the Wayland socket clients connect to, e.g. `wayland-2`.
    pub socket_name: OsString,
}

impl Server {
    pub fn new(
        event_loop: &EventLoop<'static, Server>,
        redraw: Ping,
        start_time: Instant,
        modifier: Modifier,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut display = Display::<Sash>::new()?;
        let socket_name = clients::listen(event_loop)?;
        let state = Sash::new(
            display.handle(),
            event_loop.get_signal(),
            redraw,
            start_time,
            modifier,
            socket_name.clone(),
        );

        // Wake up when clients send requests. A duplicate of the display's fd lets the
        // callback borrow the display and the state separately.
        let requests = display.backend().poll_fd().try_clone_to_owned()?;
        event_loop
            .handle()
            .insert_source(
                Generic::new(requests, Interest::READ, Mode::Level),
                |_, _, server: &mut Server| {
                    server.display.dispatch_clients(&mut server.state)?;
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|err| err.error)?;

        Ok(Self {
            display,
            state,
            socket_name,
        })
    }

    /// Sends queued events to clients. Run after every event loop iteration.
    pub fn flush(&mut self) {
        if let Err(err) = self.display.flush_clients() {
            tracing::warn!(%err, "flushing clients failed");
        }
    }
}
