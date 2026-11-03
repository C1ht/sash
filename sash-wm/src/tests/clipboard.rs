// SPDX-License-Identifier: Apache-2.0

//! The clipboard end to end (M10): real clients on the headless harness (output 800x600,
//! modifier Alt). Payloads go from the source client to the reader through a pipe; the
//! server only passes the fd on.

use std::{
    io::{PipeReader, Read},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use smithay::backend::input::KeyState;
use wayland_client::Proxy;

use super::{
    client::TestClient,
    harness::{
        Harness,
        keys::{ALT, H, L},
    },
};
use crate::data_device::limits::{MAX_MIME_TYPES, MAX_OFFERS_PER_CLIENT};

const TEXT: &str = "text/plain;charset=utf-8";

fn read_all(mut reader: PipeReader) -> Vec<u8> {
    let mut out = Vec::new();
    reader.read_to_end(&mut out).expect("read the pipe");
    out
}

/// Pastes `mime` of `dst`'s selection, served by `src`.
fn paste(dst: &mut TestClient, src: &mut TestClient, mime: &str) -> Vec<u8> {
    let offer = dst
        .events
        .data
        .selection_offer()
        .expect("a selection offer");
    let reader = dst.receive(&offer, mime);
    dst.roundtrip();
    src.roundtrip();
    let out = read_all(reader);
    src.join_writers();
    out
}

/// Pastes a client's own selection (it is source and reader).
fn paste_own(c: &mut TestClient, mime: &str) -> Vec<u8> {
    let offer = c.events.data.selection_offer().expect("a selection offer");
    let reader = c.receive(&offer, mime);
    c.roundtrip();
    let out = read_all(reader);
    c.join_writers();
    out
}

/// Two windows, each client with its data device; the second one focused.
fn two_clients(socket: &std::path::Path) -> (TestClient, TestClient) {
    let mut c1 = TestClient::connect(socket);
    c1.data_device();
    let _w1 = c1.map_window();
    let mut c2 = TestClient::connect(socket);
    c2.data_device();
    let _w2 = c2.map_window();
    c1.roundtrip();
    c2.roundtrip();
    (c1, c2)
}

fn focus_key(h: &mut Harness, key: u32) {
    h.key(ALT, KeyState::Pressed);
    h.key(key, KeyState::Pressed);
    h.key(key, KeyState::Released);
    h.key(ALT, KeyState::Released);
}

/// Copy in one client, paste in another; the selection is offered with its MIME types
/// and follows keyboard focus in both directions; an unfocused client's offer reads
/// nothing.
#[test]
fn copy_and_paste_follow_keyboard_focus() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c1, mut c2) = two_clients(&socket);
        // c2 is focused; it copies. c1, unfocused, is offered nothing.
        c2.copy(&[TEXT, "text/plain"], b"hello".to_vec());
        c1.roundtrip();
        assert!(c1.events.data.offers.is_empty(), "no focus, no offer");
        assert_eq!(
            c1.events.data.selection,
            Some(None),
            "told 'none' while it had focus"
        );
        assert_eq!(paste_own(&mut c2, TEXT), b"hello");
        // Focus to c1 (Alt+H): it gets the selection with its types and can paste.
        server.on_server(|h| focus_key(h, H));
        c1.roundtrip();
        let offer = c1.events.data.selection_offer().expect("offered on focus");
        assert_eq!(
            c1.events.data.offers[&offer.id()].mimes,
            [TEXT, "text/plain"]
        );
        assert_eq!(paste(&mut c1, &mut c2, TEXT), b"hello");
        // Focus back to c2 (Alt+L): c2 gets it again; c1's offer reads nothing now.
        let before = c2.events.data.selection_events;
        server.on_server(|h| focus_key(h, L));
        c2.roundtrip();
        assert_eq!(c2.events.data.selection_events, before + 1, "offered again");
        let reader = c1.receive(&offer, TEXT);
        c1.roundtrip();
        c2.roundtrip();
        assert!(
            read_all(reader).is_empty(),
            "end of file for an unfocused client"
        );
        assert!(c2.events.data.sources.values().all(|s| s.sends.len() == 2));
    });
}

/// A set_selection from a client without keyboard focus is ignored.
#[test]
fn an_unfocused_client_cannot_set_the_selection() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |_server| {
        let (mut c1, mut c2) = two_clients(&socket);
        c2.copy(&[TEXT], b"from the focused one".to_vec());
        c1.copy(&[TEXT], b"from the unfocused one".to_vec());
        c2.roundtrip();
        assert_eq!(paste_own(&mut c2, TEXT), b"from the focused one");
    });
}

/// A new copy replaces the selection (the old source is cancelled); clearing it tells
/// the focused client there is none.
#[test]
fn the_selection_is_replaced_and_cleared() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |_server| {
        let (_c1, mut c2) = two_clients(&socket);
        let first = c2.copy(&[TEXT], b"first".to_vec());
        let _second = c2.copy(&[TEXT], b"second".to_vec());
        assert!(
            c2.events.data.source(&first).cancelled,
            "the old source is cancelled"
        );
        assert_eq!(paste_own(&mut c2, TEXT), b"second");
        c2.clear_selection();
        assert_eq!(c2.events.data.selection, Some(None), "no selection");
    });
}

/// 10 MB go through the pipe while the event loop keeps running: its iterations are
/// timed during the transfer.
#[test]
fn a_10_mb_paste_does_not_block_the_event_loop() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    let steps = Arc::new(Mutex::new(Vec::<Duration>::new()));
    let timed = steps.clone();
    harness.run_with_remote(move |server| {
        let (mut c1, mut c2) = two_clients(&socket);
        let payload: Vec<u8> = (0..10 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
        c2.copy(&["application/octet-stream"], payload.clone());
        server.on_server(|h| focus_key(h, H));
        c1.roundtrip();
        let offer = c1.events.data.selection_offer().expect("offered");
        let reader = c1.receive(&offer, "application/octet-stream");
        c1.roundtrip();
        c2.roundtrip(); // the send event: c2's writer thread starts
        let done = Arc::new(AtomicBool::new(false));
        let finished = done.clone();
        let started = Instant::now();
        let read = std::thread::spawn(move || {
            let out = read_all(reader);
            finished.store(true, Ordering::SeqCst);
            out
        });
        while !done.load(Ordering::SeqCst) {
            let timed = timed.clone();
            server.on_server(move |h| {
                let t = Instant::now();
                h.step();
                timed.lock().unwrap().push(t.elapsed());
            });
        }
        let out = read.join().unwrap();
        let took = started.elapsed();
        c2.join_writers();
        assert_eq!(out.len(), payload.len());
        assert!(out == payload, "the payload arrives intact");
        println!("10 MB transfer: {took:?}");
    });
    let mut steps = steps.lock().unwrap().clone();
    steps.sort();
    let max = *steps.last().unwrap();
    let median = steps[steps.len() / 2];
    println!(
        "event loop iterations during the transfer: {} (median {median:?}, max {max:?})",
        steps.len()
    );
    // An iteration waits at most 2 ms for events; nothing waits on the payload.
    assert!(
        max < Duration::from_millis(50),
        "the loop was blocked: {max:?}"
    );
}

/// The source disconnects in the middle of a transfer: the reader gets what was
/// written and then end of file, the selection ends, the server goes on.
#[test]
fn a_source_disconnecting_mid_transfer_ends_the_selection() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c1, mut c2) = two_clients(&socket);
        c1.copy(&[TEXT], b"never sent".to_vec()); // c1 has no focus: ignored
        // c2 copies a source whose payload is written by the test, half, by hand.
        server.on_server(|h| focus_key(h, H)); // focus c1
        c1.roundtrip();
        c1.copy(&[TEXT], vec![b'x'; 4 * 1024 * 1024]);
        server.on_server(|h| focus_key(h, L)); // focus c2, which reads
        c2.roundtrip();
        let offer = c2.events.data.selection_offer().expect("offered");
        let reader = c2.receive(&offer, TEXT);
        c2.roundtrip();
        // c1 disconnects before dispatching the send event: its pipe end goes away.
        drop(c1);
        let out = read_all(reader);
        assert!(out.len() < 4 * 1024 * 1024, "the transfer ended early");
        c2.roundtrip();
        assert_eq!(c2.events.data.selection, Some(None), "the selection ended");
        // The server still serves a new client.
        let mut c3 = TestClient::connect(&socket);
        let _w3 = c3.map_window();
    });
}

/// The reader disconnects before reading: the source's write fails, nothing hangs,
/// the selection stays.
#[test]
fn a_destination_disconnecting_mid_transfer_leaves_the_selection() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c1, mut c2) = two_clients(&socket);
        c2.copy(&[TEXT], vec![b'y'; 2 * 1024 * 1024]);
        server.on_server(|h| focus_key(h, H));
        c1.roundtrip();
        let offer = c1.events.data.selection_offer().expect("offered");
        let reader = c1.receive(&offer, TEXT);
        c1.roundtrip();
        drop(reader);
        drop(c1);
        c2.roundtrip(); // the send event: the writer fails on the closed pipe
        c2.join_writers();
        server.on_server(|h| {
            for _ in 0..5 {
                h.step();
            }
        });
        assert_eq!(
            paste_own(&mut c2, TEXT).len(),
            2 * 1024 * 1024,
            "still there"
        );
    });
}

/// Owner's limit: 40 pastes of one selection over 10 seconds all succeed; 100 in one
/// second get end of file beyond the 16th (sash-wm's clock is moved by hand).
#[test]
fn receives_are_rate_limited_per_offer() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (_c1, mut c2) = two_clients(&socket);
        c2.copy(&[TEXT], b"paste me".to_vec());
        for i in 0..40u64 {
            server.on_server(move |h| {
                h.server.state.clipboard.fake_now = Some(Duration::from_millis(i * 250));
            });
            assert_eq!(paste_own(&mut c2, TEXT), b"paste me", "paste {i}");
        }
        let mut got = Vec::new();
        for i in 0..100u64 {
            server.on_server(move |h| {
                h.server.state.clipboard.fake_now =
                    Some(Duration::from_secs(100) + Duration::from_millis(i * 10));
            });
            got.push(!paste_own(&mut c2, TEXT).is_empty());
        }
        assert!(got[..16].iter().all(|g| *g), "the first 16 in a second");
        assert!(got[16..].iter().all(|g| !*g), "end of file beyond the 16th");
        server.on_server(|h| assert_eq!(h.server.state.clipboard.receives_refused, 84));
    });
}

/// A source's MIME list is capped; a client holding MAX_OFFERS_PER_CLIENT offers gets
/// no new ones until it destroys some.
#[test]
fn mime_lists_and_offers_are_limited() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c1, mut c2) = two_clients(&socket);
        let many: Vec<String> = (0..40).map(|i| format!("type/{i}")).collect();
        let names: Vec<&str> = many.iter().map(String::as_str).collect();
        c2.copy(&names, b"x".to_vec());
        let offer = c2.events.data.selection_offer().unwrap();
        assert_eq!(
            c2.events.data.offers[&offer.id()].mimes.len(),
            MAX_MIME_TYPES
        );
        // Every focus change to c1 offers it the selection; it never destroys offers.
        for _ in 0..MAX_OFFERS_PER_CLIENT + 4 {
            server.on_server(|h| focus_key(h, H));
            server.on_server(|h| focus_key(h, L));
        }
        c1.roundtrip();
        assert_eq!(c1.events.data.offers.len(), MAX_OFFERS_PER_CLIENT);
        // Destroying offers makes room again.
        for info in c1.events.data.offers.values() {
            info.offer.destroy();
        }
        c1.roundtrip();
        server.on_server(|h| focus_key(h, H));
        c1.roundtrip();
        assert_eq!(c1.events.data.offers.len(), MAX_OFFERS_PER_CLIENT + 1);
    });
}
