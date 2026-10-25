// SPDX-License-Identifier: Apache-2.0

//! Accepting client connections and the data kept per client.

use std::{
    ffi::OsString,
    sync::{Arc, Mutex},
};

use smithay::{
    reexports::{
        calloop::EventLoop,
        wayland_protocols::xdg::shell::server::xdg_wm_base::XdgWmBase,
        wayland_server::backend::{ClientData, ClientId, DisconnectReason},
    },
    wayland::{compositor::CompositorClientState, socket::ListeningSocketSource},
};

use crate::server::Server;

#[derive(Default)]
pub struct ClientState {
    pub compositor: CompositorClientState,
    /// The client's xdg_wm_base, for protocol errors Smithay 0.7 cannot send itself
    /// (recorded from its requests, see `shell.rs`).
    xdg_wm_base: Mutex<Option<XdgWmBase>>,
}

impl ClientState {
    pub fn xdg_wm_base(&self) -> Option<XdgWmBase> {
        self.xdg_wm_base.lock().ok()?.clone()
    }

    pub fn note_xdg_wm_base(&self, wm_base: &XdgWmBase) {
        if let Ok(mut slot) = self.xdg_wm_base.lock()
            && slot.is_none()
        {
            *slot = Some(wm_base.clone());
        }
    }
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
