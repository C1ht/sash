// SPDX-License-Identifier: Apache-2.0

//! Privileged clients (M13a): clients sash-wm started itself on a socket it created.
//!
//! sash-wm makes a socket pair, adds one end as a client marked privileged and starts
//! the program with the other end as `WAYLAND_SOCKET`. The connection is the proof:
//! never a process id, a path or a name. Privileged globals are visible only to such
//! connections (`privileged_only`, used as a global's filter); a guessed bind from any
//! other client fails in the registry. A started client's children get no privilege:
//! they can only connect through the public socket, like any client. The privilege
//! ends with the connection.

use std::{
    io,
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    process::Command,
    sync::{Arc, atomic::AtomicUsize},
};

use nix::fcntl::{FcntlArg, FdFlag, fcntl};
use smithay::reexports::{
    calloop::{
        LoopHandle,
        channel::{self, Channel, Event, Sender},
    },
    wayland_server::Client,
};

use crate::{
    clients::{ClientState, Watch},
    server::Server,
    state::Sash,
};

/// A closed privileged connection: the program's file name and its pid.
type Closed = (String, u32);

/// What sash-wm knows about a privileged connection.
#[derive(Clone, Debug)]
pub struct Privileged {
    /// The program's file name, for logs (never used to decide anything).
    pub program: String,
    pub pid: u32,
    /// Tells the event loop when the connection closes.
    closed: Sender<Closed>,
}

impl Privileged {
    pub fn closed(&self) {
        let _ = self.closed.send((self.program.clone(), self.pid));
    }
}

/// Bookkeeping of privileged connections.
pub struct Trust {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the mechanism only; first-party clients come later"
        )
    )]
    closed: Sender<Closed>,
    /// Privileged connections open now.
    pub open: usize,
    /// Privileged clients cut off for not reading their events (limits.rs).
    pub cut_off: Arc<AtomicUsize>,
}

impl Trust {
    /// Listens for privileged connections closing.
    pub fn new(loop_handle: &LoopHandle<'static, Server>) -> Self {
        let (closed, events): (Sender<Closed>, Channel<Closed>) = channel::channel();
        let inserted = loop_handle.insert_source(events, |event, _, server| {
            if let Event::Msg((program, pid)) = event {
                server.state.privileged_closed(&program, pid);
            }
        });
        if let Err(err) = inserted {
            tracing::warn!(%err, "cannot watch privileged connections");
        }
        Self {
            closed,
            open: 0,
            cut_off: Arc::default(),
        }
    }
}

/// A global filter: true only for connections sash-wm created for a program it started.
pub fn privileged_only(client: &Client) -> bool {
    client
        .get_data::<ClientState>()
        .is_some_and(|state| state.privileged.is_some())
}

impl Sash {
    /// Starts `command` with a privileged connection: a socket pair whose sash-wm end
    /// is added as a privileged client and whose other end the program gets as
    /// `WAYLAND_SOCKET` (WAYLAND_DISPLAY is set to sash-wm's public socket, for its
    /// children). Returns the program's pid. On any failure nothing is added.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the mechanism only; first-party clients come later"
        )
    )]
    pub fn spawn_privileged(&mut self, mut command: Command) -> io::Result<u32> {
        let program = std::path::Path::new(command.get_program())
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (ours, theirs) = UnixStream::pair()?;
        // The child's end must survive exec; it is closed in sash-wm right after the
        // spawn, so no other child can inherit it.
        let theirs: OwnedFd = theirs.into();
        fcntl(&theirs, FcntlArg::F_SETFD(FdFlag::empty())).map_err(io::Error::from)?;
        command
            .env("WAYLAND_SOCKET", theirs.as_raw_fd().to_string())
            .env("WAYLAND_DISPLAY", &self.keybinds.display_name);
        let spawned = command.spawn();
        drop(theirs);
        let mut child = spawned?;
        let pid = child.id();
        let privileged = Privileged {
            program: program.clone(),
            pid,
            closed: self.trust.closed.clone(),
        };
        let watch = Watch::new(&ours, None, self.trust.cut_off.clone());
        let data = Arc::new(ClientState::privileged(privileged, watch));
        if let Err(err) = self.display_handle.insert_client(ours, data) {
            tracing::warn!(%err, program, pid, "privileged client not added");
            let _ = child.kill();
            let _ = child.wait();
            return Err(err);
        }
        self.trust.open += 1;
        tracing::info!(program, pid, "started a privileged client");
        record!(self, "client_spawned", "program" => program.clone(), "pid" => pid);
        // Reap it when it exits so it does not linger as a zombie.
        std::thread::spawn(move || match child.wait() {
            Ok(status) => tracing::debug!(program, pid, %status, "privileged client exited"),
            Err(err) => tracing::warn!(program, pid, %err, "privileged client lost"),
        });
        Ok(pid)
    }

    /// A privileged connection closed: its privilege is gone with it.
    fn privileged_closed(&mut self, program: &str, pid: u32) {
        self.trust.open = self.trust.open.saturating_sub(1);
        tracing::info!(program, pid, "privileged connection closed");
        record!(self, "privileged_closed", "program" => program, "pid" => pid);
    }
}

#[cfg(test)]
impl Sash {
    /// Tests: a privileged connection inside the test process (no program started);
    /// returns the client's end. Same bookkeeping as `spawn_privileged`.
    pub fn privileged_connection_for_tests(&mut self) -> UnixStream {
        let (ours, theirs) = UnixStream::pair().expect("socket pair");
        let privileged = Privileged {
            program: "test".into(),
            pid: std::process::id(),
            closed: self.trust.closed.clone(),
        };
        let watch = Watch::new(&ours, None, self.trust.cut_off.clone());
        let data = Arc::new(ClientState::privileged(privileged, watch));
        self.display_handle
            .insert_client(ours, data)
            .expect("privileged test client");
        self.trust.open += 1;
        theirs
    }
}

/// A stand-in privileged global for the tests (ext_foreign_toplevel_list_v1, which
/// lists windows: the kind of data only first-party clients should see). It lists
/// nothing; it only proves the filter. Compiled into tests only, never into a build of
/// sash-wm; tools/check-release.sh checks its marker.
#[cfg(test)]
pub mod stand_in {
    use smithay::reexports::{
        wayland_protocols::ext::foreign_toplevel_list::v1::server::ext_foreign_toplevel_list_v1::{
            self, ExtForeignToplevelListV1,
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, backend::GlobalId,
        },
    };

    use crate::state::Sash;

    /// The marker tools/check-release.sh looks for.
    pub const MARKER: &str = "privileged stand-in global (tests only)";

    pub struct StandIn;

    pub fn advertise(display: &DisplayHandle) -> GlobalId {
        tracing::debug!("{MARKER}");
        display.create_global::<Sash, ExtForeignToplevelListV1, StandIn>(1, StandIn)
    }

    impl GlobalDispatch<ExtForeignToplevelListV1, StandIn> for Sash {
        fn bind(
            _: &mut Self,
            dh: &DisplayHandle,
            client: &Client,
            resource: New<ExtForeignToplevelListV1>,
            _: &StandIn,
            data_init: &mut DataInit<'_, Self>,
        ) {
            data_init.init(resource, ());
            crate::limits::count_bind(dh, client);
        }

        fn can_view(client: Client, _: &StandIn) -> bool {
            super::privileged_only(&client)
        }
    }

    impl Dispatch<ExtForeignToplevelListV1, ()> for Sash {
        fn request(
            _: &mut Self,
            _: &Client,
            resource: &ExtForeignToplevelListV1,
            request: ext_foreign_toplevel_list_v1::Request,
            _: &(),
            _: &DisplayHandle,
            _: &mut DataInit<'_, Self>,
        ) {
            if let ext_foreign_toplevel_list_v1::Request::Stop = request {
                resource.finished();
            }
        }
    }
}
