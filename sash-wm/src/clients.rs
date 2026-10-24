// SPDX-License-Identifier: Apache-2.0

//! Accepting client connections and the data kept per client.

use std::{ffi::OsString, sync::Arc};

use smithay::{
    reexports::{
        calloop::EventLoop,
        wayland_server::backend::{ClientData, ClientId, DisconnectReason},
    },
    wayland::{compositor::CompositorClientState, socket::ListeningSocketSource},
};

use crate::server::Server;

#[derive(Default)]
pub struct ClientState {
    pub compositor: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, client: ClientId) {
        tracing::debug!(?client, "client connected");
    }

    fn disconnected(&self, client: ClientId, reason: DisconnectReason) {
        tracing::debug!(?client, ?reason, "client disconnected");
    }
}

/// Opens the next free `wayland-N` socket and accepts clients on it. Returns the socket name.
pub fn listen(
    event_loop: &EventLoop<'static, Server>,
) -> Result<OsString, Box<dyn std::error::Error>> {
    let socket = ListeningSocketSource::new_auto()?;
    let name = socket.socket_name().to_os_string();
    event_loop
        .handle()
        .insert_source(socket, |stream, _, server| {
            let data = Arc::new(ClientState::default());
            if let Err(err) = server.display.handle().insert_client(stream, data) {
                tracing::warn!(%err, "could not add client");
            }
        })
        .map_err(|err| err.error)?;
    Ok(name)
}
