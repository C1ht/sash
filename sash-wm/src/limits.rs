// SPDX-License-Identifier: Apache-2.0

//! Per-client resource limits (owner, after M13a). Each is a constant and each is
//! logged when a client hits it:
//!
//! - open connections on the public socket: a connection beyond `CONNECTIONS` is closed
//!   at once;
//! - global binds per connection: the bind beyond `BINDS` disconnects the client with
//!   wl_display's no_memory error;
//! - events not yet read by a client: wayland-backend buffers at most `EVENT_BUFFER`
//!   bytes per client beyond what the socket holds, then cuts the client off. The value
//!   is the library's own: changing it needs a wayland-server feature that pulls in
//!   wayland-sys (a new crate). The library gives no reason for the cut, so
//!   `clients.rs` keeps a duplicate of every connection and tells a cut from a close by
//!   whether the client's end is still open.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use smithay::reexports::wayland_server::{Client, DisplayHandle, backend::protocol::ProtocolError};

use crate::clients::ClientState;

/// Open connections accepted on the public socket at once.
pub const CONNECTIONS: usize = 256;
/// Global binds one connection may make over its lifetime.
pub const BINDS: usize = 1024;
/// Bytes of unread events kept per client, beyond the socket's own buffer
/// (wayland-backend 0.3's fixed default, stated here for the logs and docs).
pub const EVENT_BUFFER: usize = 4096;

/// Open connections on the public socket.
#[derive(Clone)]
pub struct Connections {
    open: Arc<AtomicUsize>,
    /// The cap: `CONNECTIONS`, lower in some tests.
    pub limit: usize,
    /// Connections refused since the last one was accepted.
    refused: usize,
    /// Clients cut off for not reading their events (or for malformed data).
    pub cut_off: Arc<AtomicUsize>,
}

impl Default for Connections {
    fn default() -> Self {
        Self {
            open: Arc::default(),
            limit: CONNECTIONS,
            refused: 0,
            cut_off: Arc::default(),
        }
    }
}

impl Connections {
    pub fn open(&self) -> usize {
        self.open.load(Ordering::Relaxed)
    }

    /// A new connection: a slot that frees itself when the client goes, or `None` when
    /// the cap is reached.
    pub fn admit(&mut self) -> Option<Slot> {
        if self.open() >= self.limit {
            self.refused += 1;
            if self.refused == 1 {
                tracing::warn!(
                    limit = self.limit,
                    "connection refused: too many open connections"
                );
            } else {
                tracing::debug!(refused = self.refused, "connection refused");
            }
            return None;
        }
        if self.refused > 0 {
            tracing::info!(
                refused = self.refused,
                "accepting connections again after refusing some"
            );
            self.refused = 0;
        }
        self.open.fetch_add(1, Ordering::Relaxed);
        Some(Slot(self.open.clone()))
    }
}

/// One counted connection; dropping it frees the slot.
pub struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Counts a bind and disconnects the client on the one beyond `BINDS`.
pub fn count_bind(dh: &DisplayHandle, client: &Client) {
    let Some(state) = client.get_data::<ClientState>() else {
        return;
    };
    let binds = state.binds.fetch_add(1, Ordering::Relaxed) + 1;
    if binds == BINDS + 1 {
        tracing::warn!(
            client = ?client.id(),
            limit = BINDS,
            "client disconnected: too many global binds"
        );
        client.kill(
            dh,
            ProtocolError {
                code: 2, // wl_display.error.no_memory
                object_id: 1,
                object_interface: "wl_display".into(),
                message: format!("more than {BINDS} global binds"),
            },
        );
    }
}

/// `GlobalDispatch` for a global whose state lives in Smithay: forwards everything, and
/// counts the bind (what `delegate_global_dispatch!` does, plus `count_bind`).
macro_rules! counted_global {
    ($interface:ty : $data:ty => $state:ty) => {
        impl smithay::reexports::wayland_server::GlobalDispatch<$interface, $data>
            for crate::state::Sash
        {
            fn bind(
                state: &mut Self,
                dh: &smithay::reexports::wayland_server::DisplayHandle,
                client: &smithay::reexports::wayland_server::Client,
                resource: smithay::reexports::wayland_server::New<$interface>,
                data: &$data,
                init: &mut smithay::reexports::wayland_server::DataInit<'_, Self>,
            ) {
                <$state as smithay::reexports::wayland_server::GlobalDispatch<
                    $interface,
                    $data,
                    Self,
                >>::bind(state, dh, client, resource, data, init);
                crate::limits::count_bind(dh, client);
            }

            fn can_view(client: smithay::reexports::wayland_server::Client, data: &$data) -> bool {
                <$state as smithay::reexports::wayland_server::GlobalDispatch<
                    $interface,
                    $data,
                    Self,
                >>::can_view(client, data)
            }
        }
    };
}

pub(crate) use counted_global;
