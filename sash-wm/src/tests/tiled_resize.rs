// SPDX-License-Identifier: Apache-2.0

//! Tiled resize end to end: real clients on the headless harness (output 800x600,
//! default mode tiled, modifier Alt). Window ids are assigned in creation order.

use std::path::Path;

use smithay::backend::input::{ButtonState, KeyState};

use super::{
    client::{TestClient, TestWindow, ToplevelConfig},
    harness::{
        Harness,
        keys::{ALT, CTRL, F, H, J, K, L, SHIFT},
    },
};
use crate::{
    layout::{Mode, Rect},
    seat::BTN_RIGHT,
};

/// Three clients, 1 | (2 / 3), window 3 focused.
fn open_three(socket: &Path, server: &super::harness::Remote) -> [(TestClient, TestWindow); 3] {
    let mut c1 = TestClient::connect(socket);
    let w1 = c1.map_window();
    server.pointer_over_focused();
    let mut c2 = TestClient::connect(socket);
    let w2 = c2.map_window();
    server.pointer_over_focused();
    let mut c3 = TestClient::connect(socket);
    let w3 = c3.map_window();
    c1.roundtrip();
    c2.roundtrip();
    [(c1, w1), (c2, w2), (c3, w3)]
}

fn size(client: &mut TestClient, window: &TestWindow) -> Option<(i32, i32)> {
    client.roundtrip();
    client.config(window).map(|c: ToplevelConfig| c.size)
}

/// mod+Ctrl+Shift+<key>: L wider, H narrower, J taller, K shorter.
fn resize_key(h: &mut Harness, key: u32) {
    h.chord(&[ALT, CTRL, SHIFT, key]);
}

#[test]
fn keyboard_resize_moves_the_split_on_the_right_or_bottom_edge() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let [(mut c1, w1), (mut c2, w2), (mut c3, w3)] = open_three(&socket, &server);
        server.on_server(|h| {
            // Window 3's right and bottom edges are on the output boundary.
            for key in [H, J, K, L] {
                resize_key(h, key);
            }
            assert_eq!(h.rect_of(3), Some(Rect::new(400, 300, 400, 300)));
            h.chord(&[ALT, H]); // focus 1
            resize_key(h, L);
            assert_eq!(h.mode_of(1), Some(Mode::Tiled), "stays tiled");
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 440, 600)));
            h.chord(&[ALT, L]); // focus 3 (most recent of 2 and 3)
            h.chord(&[ALT, K]); // focus 2
            resize_key(h, J);
            resize_key(h, J);
            resize_key(h, K);
        });
        assert_eq!(size(&mut c1, &w1), Some((440, 600)));
        assert_eq!(size(&mut c2, &w2), Some((360, 340)));
        assert_eq!(size(&mut c3, &w3), Some((360, 260)));
        for client in [&c1, &c2, &c3] {
            for key in [H, J, K, L] {
                assert!(
                    !client.events.keys_pressed.contains(&key),
                    "keybind key {key} was forwarded"
                );
            }
        }
    });
}

/// Sends commands to the synthetic input socket, checking each reply.
#[cfg(debug_assertions)]
fn script(socket: &Path, commands: &[&str]) {
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixStream,
    };
    let mut stream = UnixStream::connect(socket).expect("synthetic input socket");
    let mut replies = BufReader::new(stream.try_clone().unwrap());
    for command in commands {
        writeln!(stream, "{command}").unwrap();
        let mut reply = String::new();
        replies.read_line(&mut reply).unwrap();
        assert_eq!(reply, "ok\n", "reply to {command:?}");
    }
}

/// The synthetic input socket exists in debug builds only.
#[cfg(debug_assertions)]
#[test]
fn scripted_mouse_resize_moves_splits_through_the_synthetic_input() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    let input = harness.listen_synthetic();
    harness.run_with_remote(move |server| {
        let [(mut c1, w1), (mut c2, w2), (mut c3, w3)] = open_three(&socket, &server);
        // Center cell of window 2 (400, 0, 400, 300): its bottom-right corner. The right
        // edge is on the output boundary, so only the bottom split moves.
        script(
            &input,
            &[
                "key 56 press",
                "motion 600 150",
                "button right press",
                "motion 590 170",
                "motion 580 190",
                "motion 570 200",
            ],
        );
        server.on_server(|h| {
            assert!(h.pointer_grabbed());
            assert_eq!(h.rect_of(2), Some(Rect::new(400, 0, 400, 350)));
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 400, 600)));
        });
        assert_eq!(size(&mut c2, &w2), Some((400, 350)));
        assert_eq!(size(&mut c3, &w3), Some((400, 250)));
        // Left cell of window 3: its left edge is the vertical split.
        script(
            &input,
            &[
                "button right release",
                "motion 420 450",
                "button right press",
                "motion 300 450",
                "button right release",
                "key 56 release",
            ],
        );
        server.on_server(|h| {
            assert!(!h.pointer_grabbed());
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 280, 600)));
            assert_eq!(h.rect_of(3), Some(Rect::new(280, 350, 520, 250)));
        });
        assert_eq!(size(&mut c1, &w1), Some((280, 600)));
        assert_eq!(size(&mut c3, &w3), Some((520, 250)));
        assert!(
            [&c1, &c2, &c3]
                .iter()
                .all(|c| c.events.buttons_pressed.is_empty()),
            "mod+clicks reach no client"
        );
    });
}

#[test]
fn drag_configures_are_batched_per_frame() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let w1 = c1.map_window();
        let mut c2 = TestClient::connect(&socket);
        let w2 = c2.map_window(); // 1 | 2
        c1.roundtrip();
        let configures = (c1.events.configures, c2.events.configures);
        server.on_server(|h| {
            h.key(ALT, KeyState::Pressed);
            h.pointer_to(300.0, 300.0); // center cell of 1: its right edge
            h.button(BTN_RIGHT, ButtonState::Pressed);
            // Three motions before the next frame.
            for x in [310.0, 320.0, 330.0] {
                h.pointer_to(x, 300.0);
            }
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 430, 600)));
        });
        assert_eq!(size(&mut c1, &w1), Some((430, 600)));
        assert_eq!(size(&mut c2, &w2), Some((370, 600)));
        assert_eq!(
            (c1.events.configures, c2.events.configures),
            (configures.0 + 1, configures.1 + 1),
            "one configure per window for the frame"
        );
        server.on_server(|h| {
            h.button(BTN_RIGHT, ButtonState::Released);
            h.key(ALT, KeyState::Released);
        });
    });
}

#[test]
fn a_drag_on_an_output_boundary_edge_does_nothing() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let w1 = c1.map_window();
        let mut c2 = TestClient::connect(&socket);
        let _w2 = c2.map_window(); // 1 | 2
        c1.roundtrip();
        let configures = c1.events.configures;
        server.on_server(|h| {
            h.key(ALT, KeyState::Pressed);
            // Top-left cell of window 1: top and left edges are both the boundary.
            h.pointer_to(20.0, 20.0);
            h.button(BTN_RIGHT, ButtonState::Pressed);
            assert!(!h.pointer_grabbed(), "no grab starts");
            h.pointer_to(120.0, 120.0);
            h.button(BTN_RIGHT, ButtonState::Released);
            h.key(ALT, KeyState::Released);
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 400, 600)));
            assert_eq!(h.mode_of(1), Some(Mode::Tiled));
        });
        assert_eq!(size(&mut c1, &w1), Some((400, 600)));
        assert_eq!(c1.events.configures, configures, "no configure");
        assert!(c1.events.buttons_pressed.is_empty());
    });
}

#[test]
fn ratios_survive_toggling_and_output_resizes() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let [(mut c1, w1), (mut c2, w2), (_c3, _w3)] = open_three(&socket, &server);
        server.on_server(|h| {
            h.chord(&[ALT, H]); // focus 1
            for _ in 0..3 {
                resize_key(h, L); // 520 of 800: 13/20
            }
            h.chord(&[ALT, F]);
            h.chord(&[ALT, F]);
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 520, 600)));
            h.chord(&[ALT, SHIFT, F]);
            h.chord(&[ALT, SHIFT, crate::tests::harness::keys::T]);
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 520, 600)));
            h.server.state.set_layout_area((1000, 700).into());
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 650, 700)));
        });
        assert_eq!(size(&mut c1, &w1), Some((650, 700)));
        assert_eq!(size(&mut c2, &w2), Some((350, 350)));
    });
}

#[test]
fn closing_windows_during_a_resize() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let [(mut c1, w1), (mut c2, w2), (mut c3, w3)] = open_three(&socket, &server);
        // Drag the split between 2 and 3 from window 2's center cell.
        server.on_server(|h| {
            h.key(ALT, KeyState::Pressed);
            h.pointer_to(600.0, 150.0);
            h.button(BTN_RIGHT, ButtonState::Pressed);
            h.pointer_to(600.0, 200.0);
        });
        // The other side of the split closes: the handle now points past the tree.
        c3.destroy(w3);
        server.on_server(|h| {
            assert_eq!(h.rect_of(2), Some(Rect::new(400, 0, 400, 600)));
            h.pointer_to(600.0, 100.0);
            assert_eq!(
                h.rect_of(2),
                Some(Rect::new(400, 0, 400, 600)),
                "no split left"
            );
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 400, 600)));
        });
        // The dragged window itself closes.
        c2.destroy(w2);
        server.on_server(|h| {
            assert!(!h.pointer_grabbed(), "the grab ends with its window");
            h.pointer_to(500.0, 100.0);
            h.button(BTN_RIGHT, ButtonState::Released);
            h.key(ALT, KeyState::Released);
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 800, 600)));
        });
        assert_eq!(size(&mut c1, &w1), Some((800, 600)));
    });
}

#[test]
fn resizing_while_the_output_is_hidden_still_sends_configures() {
    let mut harness = Harness::new();
    harness.presenting = false;
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let w1 = c1.map_window();
        let mut c2 = TestClient::connect(&socket);
        let w2 = c2.map_window(); // 1 | 2
        server.on_server(|h| {
            h.key(ALT, KeyState::Pressed);
            h.pointer_to(300.0, 300.0); // center cell of 1: its right edge
            h.button(BTN_RIGHT, ButtonState::Pressed);
            h.pointer_to(360.0, 300.0);
        });
        // No frame is presented; the fallback timer sends the configures while the
        // button is still held.
        c1.dispatch_until("the configure from the fallback timer", |e| {
            e.toplevel_size == Some((460, 600))
        });
        c2.dispatch_until("the configure from the fallback timer", |e| {
            e.toplevel_size == Some((340, 600))
        });
        server.on_server(|h| {
            assert!(h.pointer_grabbed(), "still dragging");
            h.button(BTN_RIGHT, ButtonState::Released);
            h.key(ALT, KeyState::Released);
        });
        assert_eq!(size(&mut c1, &w1), Some((460, 600)));
        assert_eq!(size(&mut c2, &w2), Some((340, 600)));
    });
    assert_eq!(harness.frames, 0);
}
