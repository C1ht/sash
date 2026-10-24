// SPDX-License-Identifier: Apache-2.0

use std::{ffi::OsString, sync::Arc, time::Instant};

use smithay::{
    reexports::{
        calloop::{EventLoop, Interest, LoopSignal, Mode, PostAction, generic::Generic},
        wayland_server::{
            Display, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
        },
    },
    wayland::socket::ListeningSocketSource,
};

pub struct Sash {
    pub start_time: Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,
}

impl Sash {
    pub fn new(
        event_loop: &EventLoop<'static, Sash>,
        display: Display<Sash>,
        start_time: Instant,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let display_handle = display.handle();
        let loop_handle = event_loop.handle();

        let socket = ListeningSocketSource::new_auto()?;
        let socket_name = socket.socket_name().to_os_string();
        loop_handle
            .insert_source(socket, |stream, _, state| {
                if let Err(err) = state
                    .display_handle
                    .insert_client(stream, Arc::new(ClientState))
                {
                    tracing::warn!(%err, "failed to add client");
                }
            })
            .map_err(|err| err.error)?;

        loop_handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    // Safety: the display is never dropped while the event loop runs.
                    unsafe {
                        display.get_mut().dispatch_clients(state)?;
                    }
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|err| err.error)?;

        Ok(Self {
            start_time,
            socket_name,
            display_handle,
            loop_signal: event_loop.get_signal(),
        })
    }
}

struct ClientState;

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

#[cfg(test)]
mod tests {
    use std::{
        os::unix::net::UnixStream,
        path::PathBuf,
        time::{Duration, Instant},
    };

    use smithay::reexports::{calloop::EventLoop, wayland_server::Display};

    use super::Sash;

    #[test]
    fn client_connects_and_roundtrips() {
        let mut event_loop: EventLoop<Sash> = EventLoop::try_new().unwrap();
        let display: Display<Sash> = Display::new().unwrap();
        let mut state = Sash::new(&event_loop, display, Instant::now()).unwrap();

        let runtime_dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap());
        let socket_path = runtime_dir.join(&state.socket_name);

        let client = std::thread::spawn(move || {
            let stream = UnixStream::connect(socket_path).unwrap();
            let connection = wayland_client::Connection::from_socket(stream).unwrap();
            connection.roundtrip().unwrap();
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        while !client.is_finished() {
            assert!(Instant::now() < deadline, "client roundtrip timed out");
            event_loop
                .dispatch(Some(Duration::from_millis(10)), &mut state)
                .unwrap();
            state.display_handle.flush_clients().unwrap();
        }
        client.join().unwrap();
    }
}
