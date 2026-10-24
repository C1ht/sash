// SPDX-License-Identifier: Apache-2.0

//! BSP tiling end to end: real clients on the headless harness (output 800x600).
//! Window ids are assigned in creation order, starting at 1.

use super::{
    client::TestClient,
    harness::{
        Harness,
        keys::{ALT, H, J, K, L, Q, SHIFT},
    },
};
use crate::layout::Rect;

fn assert_tiles(h: &Harness, expected: &[(u64, Rect)]) {
    for &(id, rect) in expected {
        assert_eq!(h.rect_of(id), Some(rect), "tile of window {id}");
    }
}

/// Three clients: 1 | (2 / 3). Opens them in order and checks configure sizes.
fn open_three(socket: &std::path::Path) -> [TestClient; 3] {
    let mut c1 = TestClient::connect(socket);
    let _ = c1.map_window();
    let mut c2 = TestClient::connect(socket);
    let _ = c2.map_window();
    let mut c3 = TestClient::connect(socket);
    let _ = c3.map_window();
    c1.roundtrip();
    c2.roundtrip();
    [c1, c2, c3]
}

#[test]
fn three_windows_get_tile_sizes_and_refill_when_the_middle_closes() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let _w1 = c1.map_window();
        assert_eq!(
            c1.events.toplevel_size,
            Some((800, 600)),
            "first window fills the output"
        );
        assert!(c1.events.tiled, "configure carries the tiled states");

        let mut c2 = TestClient::connect(&socket);
        let w2 = c2.map_window();
        assert_eq!(
            c2.events.toplevel_size,
            Some((400, 600)),
            "initial configure has the tile size"
        );
        c1.roundtrip();
        assert_eq!(c1.events.toplevel_size, Some((400, 600)));

        let mut c3 = TestClient::connect(&socket);
        let _w3 = c3.map_window();
        assert_eq!(c3.events.toplevel_size, Some((400, 300)));
        let c1_configures = c1.events.configures;
        c1.roundtrip();
        c2.roundtrip();
        assert_eq!(c2.events.toplevel_size, Some((400, 300)));
        assert_eq!(
            c1.events.configures, c1_configures,
            "window 1's tile did not change"
        );

        server.on_server(|h| {
            assert_tiles(
                h,
                &[
                    (1, Rect::new(0, 0, 400, 600)),
                    (2, Rect::new(400, 0, 400, 300)),
                    (3, Rect::new(400, 300, 400, 300)),
                ],
            );
            assert_eq!(h.keyboard_focus(), Some(3), "the newest window has focus");
        });

        // Close the middle window: its sibling (3) takes the parent's place.
        c2.destroy(w2);
        c3.roundtrip();
        c1.roundtrip();
        assert_eq!(c3.events.toplevel_size, Some((400, 600)));
        assert_eq!(c1.events.toplevel_size, Some((400, 600)));
        server.on_server(|h| {
            assert_tiles(
                h,
                &[
                    (1, Rect::new(0, 0, 400, 600)),
                    (3, Rect::new(400, 0, 400, 600)),
                ],
            );
            assert_eq!(h.rect_of(2), None);
        });
    });
}

#[test]
fn keybinds_move_focus_and_swap_without_reaching_clients() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let [mut c1, mut c2, mut c3] = open_three(&socket);

        // Focus is on 3 (bottom right). Alt+H: left to 1.
        server.on_server(|h| {
            h.chord(&[ALT, H]);
            assert_eq!(h.keyboard_focus(), Some(1));
        });
        c1.roundtrip();
        c3.roundtrip();
        assert!(c1.events.has_keyboard_focus);
        assert!(!c3.events.has_keyboard_focus);

        // Alt+L from 1: 2 and 3 both touch 1's right edge; 3 was focused more recently.
        // Alt+K: up to 2. Alt+J: down to 3. Alt+L from 3: nothing to the right, no wrap.
        server.on_server(|h| {
            h.chord(&[ALT, L]);
            assert_eq!(h.keyboard_focus(), Some(3));
            h.chord(&[ALT, K]);
            assert_eq!(h.keyboard_focus(), Some(2));
            h.chord(&[ALT, J]);
            assert_eq!(h.keyboard_focus(), Some(3));
            h.chord(&[ALT, L]);
            assert_eq!(h.keyboard_focus(), Some(3), "no wrap-around");
        });

        // Alt+Shift+H on 3 swaps it with 1: positions and sizes change, structure stays,
        // and focus follows window 3.
        server.on_server(|h| {
            h.chord(&[ALT, SHIFT, H]);
            assert_tiles(
                h,
                &[
                    (3, Rect::new(0, 0, 400, 600)),
                    (2, Rect::new(400, 0, 400, 300)),
                    (1, Rect::new(400, 300, 400, 300)),
                ],
            );
            assert_eq!(h.keyboard_focus(), Some(3));
        });
        c1.roundtrip();
        c3.roundtrip();
        assert_eq!(c3.events.toplevel_size, Some((400, 600)));
        assert_eq!(c1.events.toplevel_size, Some((400, 300)));

        // Keybind keys never reach clients (Alt and Shift alone are forwarded).
        c2.roundtrip();
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

#[test]
fn close_keybind_asks_the_focused_window_to_close() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let _w1 = c1.map_window();
        let mut c2 = TestClient::connect(&socket);
        let w2 = c2.map_window();

        server.on_server(|h| h.chord(&[ALT, Q]));
        c2.dispatch_until("xdg_toplevel.close", |e| e.close_requested);
        c1.roundtrip();
        assert!(
            !c1.events.close_requested,
            "only the focused window is asked"
        );
        c2.destroy(w2);
        c1.roundtrip();
        assert_eq!(c1.events.toplevel_size, Some((800, 600)));
        server.on_server(|h| assert_eq!(h.keyboard_focus(), Some(1)));
    });
}

#[test]
fn layout_updates_while_the_output_is_hidden() {
    let mut harness = Harness::new();
    harness.presenting = false;
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let [mut c1, _c2, mut c3] = open_three(&socket);
        assert_eq!(c3.events.toplevel_size, Some((400, 300)));
        server.on_server(|h| h.chord(&[ALT, SHIFT, H]));
        c1.roundtrip();
        c3.roundtrip();
        assert_eq!(c3.events.toplevel_size, Some((400, 600)));
        assert_eq!(c1.events.toplevel_size, Some((400, 300)));
    });
    assert_eq!(harness.frames, 0, "no frames while hidden");
}

#[test]
fn closing_a_window_with_an_unacked_configure() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let w1 = c1.map_window();
        let mut c2 = TestClient::connect(&socket);
        let _w2 = c2.map_window();
        // Window 1 gets a configure for its new 400x600 tile and never acks it.
        c1.roundtrip();
        assert_eq!(c1.events.toplevel_size, Some((400, 600)));
        c1.destroy(w1);
        c2.roundtrip();
        assert_eq!(c2.events.toplevel_size, Some((800, 600)));
        server.on_server(|h| {
            assert_tiles(h, &[(2, Rect::new(0, 0, 800, 600))]);
            assert_eq!(h.mapped_windows() + h.unmapped_windows(), 1);
        });
    });
}

#[test]
fn unmapped_window_leaves_the_layout_and_reenters_on_remap() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let _w1 = c1.map_window();
        let mut c2 = TestClient::connect(&socket);
        let w2 = c2.map_window();

        c2.unmap(&w2);
        c1.roundtrip();
        assert_eq!(
            c1.events.toplevel_size,
            Some((800, 600)),
            "unmap gives the space back"
        );
        server.on_server(|h| assert_eq!(h.rect_of(2), None));

        // Mapping again starts over: initial commit, configure, buffer.
        let seen = c2.events.configures;
        w2.surface.commit();
        c2.dispatch_until("a new initial configure", |e| e.configures > seen);
        assert_eq!(c2.events.toplevel_size, Some((400, 600)));
        c2.ack_and_commit(&w2);
        server.on_server(|h| {
            assert_tiles(
                h,
                &[
                    (1, Rect::new(0, 0, 400, 600)),
                    (2, Rect::new(400, 0, 400, 600)),
                ],
            );
            assert_eq!(h.keyboard_focus(), Some(2));
        });
    });
}
