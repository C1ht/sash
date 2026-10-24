// SPDX-License-Identifier: Apache-2.0

//! End-to-end tests: a real wayland-client talks to sash-wm over a socket.

mod client;
mod harness;
mod tiling;

use std::{sync::mpsc, thread};

use smithay::utils::IsAlive;
use wayland_client::backend::protocol::ProtocolError;

use self::{client::TestClient, harness::Harness};

/// The error a client got, after its connection died on a protocol error.
fn protocol_error(client: &TestClient) -> ProtocolError {
    client
        .conn
        .protocol_error()
        .expect("the client should have received a protocol error")
}

/// A fresh client can still connect and complete a roundtrip.
fn assert_compositor_alive(harness: &mut Harness) {
    let socket = harness.socket_path();
    let client = thread::spawn(move || TestClient::connect(&socket).roundtrip());
    harness.run_client(client);
}

#[test]
fn client_connects_and_roundtrips() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    let client = thread::spawn(move || TestClient::connect(&socket).roundtrip());
    harness.run_client(client);
}

/// (a) A client maps an xdg_toplevel and gets keyboard and pointer focus.
#[test]
fn mapped_window_gets_keyboard_and_pointer_focus() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    let client = thread::spawn(move || {
        let mut client = TestClient::connect(&socket);
        let _window = client.map_window();
        client.dispatch_until("keyboard and pointer enter", |e| {
            e.keyboard_entered && e.pointer_entered
        });
    });

    let mut pointer_moved = false;
    harness.run_until("the test client to finish", |h| {
        if !pointer_moved && h.mapped_windows() == 1 {
            // The window is at the output origin; move the pointer over it.
            h.server.state.pointer_moved_to((10.0, 10.0).into(), 0);
            pointer_moved = true;
        }
        client.is_finished()
    });
    harness.run_client(client);
}

/// (b) While the output is not presenting, requests are served but no frame
/// callbacks are sent; when it presents again, exactly one frame follows.
#[test]
fn hidden_output_serves_requests_and_sends_no_frame_callbacks() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    let (to_server, from_client) = mpsc::channel();
    let (to_client, from_server) = mpsc::channel();

    let client = thread::spawn(move || {
        let mut client = TestClient::connect(&socket);
        let window = client.map_window();
        client.commit_with_frame_request(&window);
        client.dispatch_until("the first frame callback", |e| e.frames_done == 1);

        to_server.send("hide").unwrap();
        from_server.recv().unwrap();
        client.commit_with_frame_request(&window);
        client.commit_damage(&window);
        client.roundtrip(); // wl_display.sync is still answered
        assert_eq!(
            client.events.frames_done, 1,
            "frame callback sent while hidden"
        );

        to_server.send("unhide").unwrap();
        client.dispatch_until("the frame callback after unhide", |e| e.frames_done == 2);
        // Stay connected until the server has counted frames: disconnecting removes the
        // window, which is a redraw of its own.
        to_server.send("counted?").unwrap();
        from_server.recv().unwrap();
        client.roundtrip();
        assert_eq!(
            client.events.frames_done, 2,
            "extra frame callbacks after unhide"
        );
    });

    let mut frames_at_hide = None;
    let mut frames_at_unhide = None;
    let mut frames_after_unhide = None;
    harness.run_until("the test client to finish", |h| {
        match from_client.try_recv() {
            Ok("hide") => {
                h.presenting = false;
                frames_at_hide = Some(h.frames);
                to_client.send(()).unwrap();
            }
            Ok("unhide") => {
                frames_at_unhide = Some(h.frames);
                h.presenting = true;
            }
            Ok("counted?") => {
                for _ in 0..20 {
                    h.step();
                }
                frames_after_unhide = Some(h.frames - frames_at_unhide.unwrap());
                to_client.send(()).unwrap();
            }
            _ => {}
        }
        client.is_finished()
    });
    harness.run_client(client);

    assert_eq!(
        frames_at_unhide.unwrap() - frames_at_hide.unwrap(),
        0,
        "frames presented while hidden"
    );
    assert_eq!(
        frames_after_unhide,
        Some(1),
        "frames presented after unhide"
    );
}

/// (c) A client disconnects while a frame callback is pending: no panic, and the
/// window is gone.
#[test]
fn disconnect_with_pending_frame_leaves_no_window() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    let (to_server, from_client) = mpsc::channel();
    let (to_client, from_server) = mpsc::channel();

    let client = thread::spawn(move || {
        let mut client = TestClient::connect(&socket);
        let window = client.map_window();
        to_server.send(()).unwrap();
        from_server.recv().unwrap();
        client.commit_with_frame_request(&window);
        client.roundtrip();
        // Dropping the client closes the connection with the frame still pending.
    });

    let mut window = None;
    harness.run_until("the test client to finish", |h| {
        if from_client.try_recv().is_ok() {
            h.presenting = false; // keep the frame pending
            window = h.server.state.space.elements().next().cloned();
            to_client.send(()).unwrap();
        }
        client.is_finished()
    });
    harness.run_client(client);
    harness.run_until("the window to be removed", |h| {
        h.mapped_windows() == 0 && h.unmapped_windows() == 0
    });

    let window = window.expect("the window was mapped");
    assert!(!window.alive(), "window still alive after disconnect");
    harness.presenting = true;
    harness.server.state.request_redraw();
    for _ in 0..10 {
        harness.step();
    }
    assert_compositor_alive(&mut harness);
}

/// (d) A buffer before the first ack_configure is a protocol error; the compositor keeps running.
#[test]
fn buffer_before_ack_configure_is_a_protocol_error() {
    let mut harness = Harness::new();

    // Buffer after the initial configure was sent but not acked.
    let socket = harness.socket_path();
    let client = thread::spawn(move || {
        let mut client = TestClient::connect(&socket);
        let window = client.create_window();
        client.dispatch_until("initial configure", |e| e.configure_serial.is_some());
        client.attach_buffer(&window);
        window.surface.commit();
        assert!(client.try_roundtrip().is_err());
        protocol_error(&client)
    });
    let error = harness.run_client(client);
    assert_eq!(
        (error.object_interface.as_str(), error.code),
        ("xdg_surface", 1)
    );

    // Buffer on the very first commit.
    let socket = harness.socket_path();
    let client = thread::spawn(move || {
        let mut client = TestClient::connect(&socket);
        let window = client.create_window_without_commit();
        client.attach_buffer(&window);
        window.surface.commit();
        assert!(client.try_roundtrip().is_err());
        protocol_error(&client)
    });
    let error = harness.run_client(client);
    assert_eq!(
        (error.object_interface.as_str(), error.code),
        ("xdg_surface", 1)
    );

    harness.run_until("windows to be removed", |h| {
        h.mapped_windows() + h.unmapped_windows() == 0
    });
    assert_compositor_alive(&mut harness);
}

/// (d) Invalid min/max sizes are a protocol error; the compositor keeps running.
#[test]
fn invalid_size_is_a_protocol_error() {
    let mut harness = Harness::new();
    let cases: [((i32, i32), (i32, i32)); 2] = [((-1, 10), (0, 0)), ((200, 200), (100, 100))];
    for (min, max) in cases {
        let socket = harness.socket_path();
        let client = thread::spawn(move || {
            let mut client = TestClient::connect(&socket);
            let window = client.map_window();
            window.toplevel.set_min_size(min.0, min.1);
            window.toplevel.set_max_size(max.0, max.1);
            window.surface.commit();
            assert!(
                client.try_roundtrip().is_err(),
                "min {min:?} max {max:?} accepted"
            );
            protocol_error(&client)
        });
        let error = harness.run_client(client);
        // xdg_toplevel.error.invalid_size
        assert_eq!(
            (error.object_interface.as_str(), error.code),
            ("xdg_toplevel", 2)
        );
    }
    harness.run_until("windows to be removed", |h| {
        h.mapped_windows() + h.unmapped_windows() == 0
    });
    assert_compositor_alive(&mut harness);
}

/// The compositor asks a window to close; the client destroys it and it is removed.
#[test]
fn close_request_removes_window_when_client_destroys_it() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    let client = thread::spawn(move || {
        let mut client = TestClient::connect(&socket);
        let window = client.map_window();
        client.dispatch_until("xdg_toplevel.close", |e| e.close_requested);
        window.toplevel.destroy();
        window.xdg_surface.destroy();
        window.surface.destroy();
        client.roundtrip();
    });

    let mut close_sent = false;
    harness.run_until("the test client to finish", |h| {
        if !close_sent && let Some(window) = h.server.state.space.elements().next().cloned() {
            h.server.state.request_close(&window);
            close_sent = true;
        }
        client.is_finished()
    });
    harness.run_client(client);
    harness.run_until("the window to be removed", |h| {
        h.mapped_windows() == 0 && h.unmapped_windows() == 0
    });
}
