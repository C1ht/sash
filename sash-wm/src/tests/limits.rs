// SPDX-License-Identifier: Apache-2.0

//! Resource limits (limits.rs): thousands of connections and binds, connections
//! beyond the cap, binds beyond the cap, and a client that never reads its events.
//! None of them may stall the compositor or leave anything behind.

use std::{
    io::{ErrorKind, Write},
    os::unix::net::UnixStream,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    protocol::{wl_compositor::WlCompositor, wl_registry::WlRegistry},
};

use super::{assert_compositor_alive, client::TestClient, harness::Harness};
use crate::limits::{BINDS, CONNECTIONS};

/// A client with only a registry: binds exactly what the test asks for.
#[derive(Default)]
struct Bare {
    compositor: Option<u32>,
}

impl Dispatch<WlRegistry, ()> for Bare {
    fn event(
        state: &mut Self,
        _: &WlRegistry,
        event: wayland_client::protocol::wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wayland_client::protocol::wl_registry::Event::Global {
            name, interface, ..
        } = event
            && interface == "wl_compositor"
        {
            state.compositor = Some(name);
        }
    }
}

delegate_noop!(Bare: ignore WlCompositor);

/// The longest a roundtrip of a well-behaved client took while `flood` ran.
fn slowest_roundtrip_during(
    socket: &std::path::Path,
    flood: impl FnOnce() + Send + 'static,
) -> Duration {
    let done = Arc::new(AtomicBool::new(false));
    let flag = done.clone();
    let flood = std::thread::spawn(move || {
        flood();
        flag.store(true, Ordering::Relaxed);
    });
    let mut client = TestClient::connect(socket);
    let mut slowest = Duration::ZERO;
    while !done.load(Ordering::Relaxed) {
        let start = Instant::now();
        client.roundtrip();
        slowest = slowest.max(start.elapsed());
    }
    flood.join().expect("flood");
    slowest
}

#[test]
fn thousands_of_connections_and_binds_leave_nothing_behind() {
    let mut harness = Harness::new();
    harness.timeout = Duration::from_secs(60);
    let socket = harness.socket_path();
    let slowest = harness.run_with_remote(move |_server| {
        let socket2 = socket.clone();
        slowest_roundtrip_during(&socket, move || {
            // Each binds every global it is shown (TestClient::connect), then goes.
            for _ in 0..2000 {
                drop(TestClient::connect(&socket2));
            }
        })
    });
    assert!(
        slowest < Duration::from_secs(1),
        "a roundtrip took {slowest:?}"
    );
    harness.run_until("every connection is gone", |h| {
        h.server.connections.open() == 0
    });
    assert_compositor_alive(&mut harness);
}

#[test]
fn connections_beyond_the_cap_are_refused_until_one_closes() {
    assert_eq!(CONNECTIONS, 256);
    let mut harness = Harness::new();
    // A lower cap than CONNECTIONS keeps the test within the process's descriptors.
    harness.server.connections.limit = 8;
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut held: Vec<TestClient> = (0..8).map(|_| TestClient::connect(&socket)).collect();
        for _ in 0..100 {
            let conn = Connection::from_socket(UnixStream::connect(&socket).expect("connect"))
                .expect("connection");
            assert!(
                conn.roundtrip().is_err(),
                "a connection beyond the cap was served"
            );
        }
        server.on_server(|h| assert_eq!(h.server.connections.open(), 8));
        // The held clients still work, and a closed one frees its slot.
        for client in &mut held {
            client.roundtrip();
        }
        held.pop();
        let mut slot_free = false;
        for _ in 0..100 {
            let conn = Connection::from_socket(UnixStream::connect(&socket).expect("connect"))
                .expect("connection");
            if conn.roundtrip().is_ok() {
                slot_free = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(slot_free, "the freed slot was not reused");
    });
}

#[test]
fn binds_beyond_the_cap_disconnect_only_that_client() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let conn = Connection::from_socket(UnixStream::connect(&socket).expect("connect"))
            .expect("connection");
        let mut queue = conn.new_event_queue::<Bare>();
        let qh = queue.handle();
        let registry = conn.display().get_registry(&qh, ());
        let mut bare = Bare::default();
        queue.roundtrip(&mut bare).expect("globals");
        let name = bare.compositor.expect("wl_compositor");
        let mut bound = Vec::new();
        for _ in 0..BINDS {
            bound.push(registry.bind::<WlCompositor, _, _>(name, 1, &qh, ()));
        }
        queue
            .roundtrip(&mut bare)
            .expect("binds up to the cap are served");
        bound.push(registry.bind::<WlCompositor, _, _>(name, 1, &qh, ()));
        assert!(
            queue.roundtrip(&mut bare).is_err(),
            "a bind beyond the cap was served"
        );
        // The error may still be unread when the closed socket stops the client.
        if let Some(error) = conn.protocol_error() {
            assert_eq!(error.code, 2, "{error:?}");
        }
        drop(queue);
        drop(conn);
        server.on_server(|h| {
            h.run_until("the client is gone", |h| h.server.connections.open() == 0);
        });
        // Another client is unaffected.
        let mut other = TestClient::connect(&socket);
        let _ = other.map_window();
    });
}

#[test]
fn a_client_that_never_reads_is_cut_off_without_stalling_the_others() {
    let mut harness = Harness::new();
    harness.timeout = Duration::from_secs(30);
    let socket = harness.socket_path();
    let slowest = harness.run_with_remote(move |_server| {
        let flood_socket = socket.clone();
        slowest_roundtrip_during(&socket, move || {
            // wl_display.sync requests, each answered with two events, never read.
            let mut stream = UnixStream::connect(&flood_socket).expect("connect");
            let mut written = 0usize;
            for id in 2u32.. {
                let mut request = Vec::with_capacity(12);
                request.extend_from_slice(&1u32.to_ne_bytes()); // wl_display
                request.extend_from_slice(&(12u32 << 16).to_ne_bytes()); // size 12, opcode 0 (sync)
                request.extend_from_slice(&id.to_ne_bytes());
                match stream.write_all(&request) {
                    Ok(()) => written += 1,
                    Err(err) => {
                        assert!(
                            matches!(
                                err.kind(),
                                ErrorKind::BrokenPipe | ErrorKind::ConnectionReset
                            ),
                            "{err}"
                        );
                        break;
                    }
                }
                assert!(written < 1_000_000, "never cut off");
            }
        })
    });
    assert!(
        slowest < Duration::from_secs(1),
        "a roundtrip took {slowest:?}"
    );
    harness.run_until("the cut is noticed", |h| {
        h.server.connections.cut_off.load(Ordering::Relaxed) == 1
    });
    harness.run_until("every connection is gone", |h| {
        h.server.connections.open() == 0
    });
    assert_compositor_alive(&mut harness);
}
