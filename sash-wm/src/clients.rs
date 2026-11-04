// SPDX-License-Identifier: Apache-2.0

//! Accepting client connections and the data kept per client.

use std::{
    ffi::OsString,
    io::{ErrorKind, Read},
    os::unix::net::UnixStream,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use smithay::{
    reexports::{
        calloop::EventLoop,
        wayland_protocols::xdg::shell::server::xdg_wm_base::XdgWmBase,
        wayland_server::backend::{ClientData, ClientId, DisconnectReason},
    },
    wayland::{compositor::CompositorClientState, socket::ListeningSocketSource},
};

use crate::{
    limits::{self, Slot},
    server::Server,
    trust::Privileged,
};

#[derive(Default)]
pub struct ClientState {
    pub compositor: CompositorClientState,
    /// The client's xdg_wm_base, for protocol errors Smithay 0.7 cannot send itself
    /// (recorded from its requests, see `shell.rs`).
    xdg_wm_base: Mutex<Option<XdgWmBase>>,
    /// Set only for a connection sash-wm created for a program it started (trust.rs).
    pub privileged: Option<Privileged>,
    /// Global binds so far (limits.rs).
    pub binds: AtomicUsize,
    /// Taken when the client disconnects.
    watch: Mutex<Option<Watch>>,
}

/// What sash-wm keeps per connection to tell why it ended, and its slot in the
/// connection count (limits.rs).
pub struct Watch {
    /// A duplicate of the connection: still open on the client's side means the
    /// server cut the client off.
    peer: UnixStream,
    _slot: Option<Slot>,
    cut_off: Arc<AtomicUsize>,
}

impl Watch {
    /// Watches `stream`, holding `slot` until the client goes.
    pub fn new(stream: &UnixStream, slot: Option<Slot>, cut_off: Arc<AtomicUsize>) -> Option<Self> {
        let peer = stream.try_clone().ok()?;
        peer.set_nonblocking(true).ok()?;
        Some(Self {
            peer,
            _slot: slot,
            cut_off,
        })
    }

    /// Whether the client still holds its end of the connection (it may also have
    /// unread requests queued).
    fn client_end_open(&self) -> bool {
        // The library is done with the connection, so reading from it takes nothing.
        match (&self.peer).read(&mut [0]) {
            Ok(0) => false,
            Ok(_) => true,
            Err(err) => err.kind() == ErrorKind::WouldBlock,
        }
    }
}

impl ClientState {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the mechanism only; first-party clients come later"
        )
    )]
    pub fn privileged(privileged: Privileged, watch: Option<Watch>) -> Self {
        Self {
            privileged: Some(privileged),
            watch: Mutex::new(watch),
            ..Self::default()
        }
    }

    fn watched(watch: Option<Watch>) -> Self {
        Self {
            watch: Mutex::new(watch),
            ..Self::default()
        }
    }

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
        let watch = self.watch.lock().ok().and_then(|mut watch| watch.take());
        if let Some(watch) = watch
            && matches!(reason, DisconnectReason::ConnectionClosed)
            && watch.client_end_open()
        {
            watch.cut_off.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                ?client,
                limit = limits::EVENT_BUFFER,
                "client cut off: its unread events filled the buffer, or it sent malformed data"
            );
        }
        if let Some(privileged) = &self.privileged {
            privileged.closed();
        }
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
            // Dropping a refused stream closes it.
            let Some(slot) = server.connections.admit() else {
                return;
            };
            let watch = Watch::new(&stream, Some(slot), server.connections.cut_off.clone());
            let data = Arc::new(ClientState::watched(watch));
            if let Err(err) = server.display.handle().insert_client(stream, data) {
                tracing::warn!(%err, "could not add client");
            }
        })
        .map_err(|err| err.error)?;
    Ok(name)
}
