// SPDX-License-Identifier: Apache-2.0

//! Snapping, maximize and fullscreen end to end: real clients on the headless harness
//! (output 800x600, default mode tiled, modifier Alt).

use smithay::backend::input::{ButtonState, KeyState};

use super::{
    client::{TestClient, TestWindow},
    harness::{
        Harness,
        keys::{ALT, DOWN, F, H, LEFT, RIGHT, SHIFT, T, UP},
    },
};
use crate::{
    layout::{Rect, WindowId, ZoneId},
    seat::BTN_LEFT,
};

const AREA: Rect = Rect::new(0, 0, 800, 600);
const LEFT_HALF: Rect = Rect::new(0, 0, 400, 600);

fn zone_of(h: &Harness, id: u64) -> Option<ZoneId> {
    h.server.state.windows.layout.snapped_zone(WindowId(id))
}

/// One client with one window floating at (100, 100, 300, 200).
fn one_floating(
    socket: &std::path::Path,
    server: &super::harness::Remote,
) -> (TestClient, TestWindow) {
    let mut c = TestClient::connect(socket);
    let w = c.map_window();
    server.on_server(|h| {
        h.chord(&[ALT, F]);
        h.server
            .state
            .windows
            .layout
            .set_floating_rect(WindowId(1), Rect::new(100, 100, 300, 200));
        h.server.state.arrange();
    });
    c.roundtrip();
    (c, w)
}

#[test]
fn client_fullscreen_gets_the_output_size_and_unset_restores() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let _w1 = c1.map_window();
        let mut c2 = TestClient::connect(&socket);
        let w2 = c2.map_window(); // 1 | 2
        w2.toplevel.set_fullscreen(None);
        c2.roundtrip();
        let config = c2.config(&w2).expect("configured");
        assert_eq!((config.size, config.fullscreen), ((800, 600), true));
        server.on_server(|h| {
            assert_eq!(
                h.server.state.windows.layout.fullscreen(),
                Some(WindowId(2))
            );
            assert_eq!(
                h.rect_of(2),
                Some(Rect::new(400, 0, 400, 600)),
                "keeps its leaf and tile"
            );
            assert_eq!(
                h.rect_of(1),
                Some(LEFT_HALF),
                "the other window does not refill"
            );
        });
        w2.toplevel.unset_fullscreen();
        c2.roundtrip();
        let config = c2.config(&w2).expect("configured");
        assert_eq!(
            (config.size, config.fullscreen, config.tiled),
            ((400, 600), false, true)
        );
    });
}

#[test]
fn client_maximize_on_a_tiled_and_a_floating_window() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let w1 = c1.map_window();
        let mut c2 = TestClient::connect(&socket);
        let _w2 = c2.map_window(); // 1 | 2
        w1.toplevel.set_maximized();
        c1.roundtrip();
        let config = c1.config(&w1).expect("configured");
        assert_eq!((config.size, config.maximized), ((800, 600), true));
        server.on_server(|h| {
            assert_eq!(
                h.mode_of(1),
                Some(crate::layout::Mode::Tiled),
                "keeps its leaf"
            )
        });
        w1.toplevel.unset_maximized();
        c1.roundtrip();
        let config = c1.config(&w1).expect("configured");
        assert_eq!((config.size, config.maximized), ((400, 600), false));
    });

    // Floating: maximize snaps to the whole output; unmaximize restores the rectangle.
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w) = one_floating(&socket, &server);
        w.toplevel.set_maximized();
        c.roundtrip();
        let config = c.config(&w).expect("configured");
        assert_eq!((config.size, config.maximized), ((800, 600), true));
        server.on_server(|h| assert_eq!(zone_of(h, 1), Some(ZoneId::FULL)));
        w.toplevel.unset_maximized();
        c.roundtrip();
        assert_eq!(
            c.config(&w).map(|c| (c.size, c.maximized)),
            Some(((300, 200), false))
        );
        server.on_server(|h| assert_eq!(h.rect_of(1), Some(Rect::new(100, 100, 300, 200))));
    });
}

#[test]
fn mouse_drag_to_the_left_edge_snaps_and_dragging_out_restores() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w) = one_floating(&socket, &server);
        server.on_server(|h| {
            h.key(ALT, KeyState::Pressed);
            h.pointer_to(250.0, 200.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.pointer_to(2.0, 300.0);
            assert_eq!(
                h.server.state.overlay.preview(),
                Some(LEFT_HALF),
                "preview shows"
            );
            h.pointer_to(200.0, 300.0);
            assert_eq!(
                h.server.state.overlay.preview(),
                None,
                "preview hides outside the zone"
            );
            h.pointer_to(2.0, 300.0);
            h.button(BTN_LEFT, ButtonState::Released);
            assert_eq!(
                h.server.state.overlay.preview(),
                None,
                "preview hides when the drag ends"
            );
            assert_eq!(h.rect_of(1), Some(LEFT_HALF));
            assert_eq!(zone_of(h, 1), Some(ZoneId::LEFT));
        });
        c.roundtrip();
        let config = c.config(&w).expect("configured");
        assert_eq!(config.size, (400, 600));
        assert_eq!(
            config.tiled_edges,
            [true, false, true, true],
            "tiled left, top, bottom"
        );

        // Drag it out: the pre-snap size (300x200) under the pointer, at the same relative
        // position (pointer at 25% across, 50% down the snapped window).
        server.on_server(|h| {
            h.pointer_to(100.0, 300.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            assert_eq!(zone_of(h, 1), Some(ZoneId::LEFT), "no unsnap on press");
            h.pointer_to(300.0, 310.0);
            assert_eq!(zone_of(h, 1), None);
            let r = h.rect_of(1).expect("placed");
            assert_eq!((r.w, r.h), (300, 200));
            assert_eq!(
                (300 - r.x, 310 - r.y),
                (75, 100),
                "pointer stays at 25% / 50%"
            );
            h.button(BTN_LEFT, ButtonState::Released);
            h.key(ALT, KeyState::Released);
        });
        c.roundtrip();
        assert_eq!(c.config(&w).map(|c| c.size), Some((300, 200)));
    });
}

#[test]
fn drop_exactly_on_a_zone_border() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (_c, _w) = one_floating(&socket, &server);
        server.on_server(|h| {
            h.key(ALT, KeyState::Pressed);
            h.key(SHIFT, KeyState::Pressed); // no edge snapping, so the drop point decides
            // x = 8 is the first pixel outside the 8 px edge band: no snap.
            h.pointer_to(250.0, 200.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.pointer_to(8.0, 300.0);
            h.button(BTN_LEFT, ButtonState::Released);
            assert_eq!(zone_of(h, 1), None);
            // x = 7 is the last pixel inside: snaps.
            let r = h.rect_of(1).expect("placed");
            h.pointer_to((r.x + 10) as f64, (r.y + 10) as f64);
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.pointer_to(7.0, 300.0);
            h.button(BTN_LEFT, ButtonState::Released);
            assert_eq!(zone_of(h, 1), Some(ZoneId::LEFT));
            h.key(SHIFT, KeyState::Released);
            h.key(ALT, KeyState::Released);
        });
    });
}

#[test]
fn fullscreen_ends_when_another_window_is_focused() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let _w1 = c1.map_window();
        let mut c2 = TestClient::connect(&socket);
        let w2 = c2.map_window();
        w2.toplevel.set_fullscreen(None);
        c2.roundtrip();
        server.on_server(|h| {
            h.chord(&[ALT, H]); // focus the window to the left
            assert_eq!(h.keyboard_focus(), Some(1));
            assert_eq!(h.server.state.windows.layout.fullscreen(), None);
        });
        c2.roundtrip();
        assert_eq!(
            c2.config(&w2).map(|c| (c.size, c.fullscreen)),
            Some(((400, 600), false))
        );
    });
}

#[test]
fn tile_all_then_float_all_restores_snapped_zones() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let _w1 = c1.map_window();
        let mut c2 = TestClient::connect(&socket);
        let _w2 = c2.map_window(); // 1 | 2, focus 2
        server.on_server(|h| {
            h.chord(&[ALT, LEFT]); // 2 floats and snaps to the left half
            assert_eq!(zone_of(h, 2), Some(ZoneId::LEFT));
            h.chord(&[ALT, UP]); // top-left quarter
            assert_eq!(zone_of(h, 2), Some(ZoneId::TOP_LEFT));
            h.chord(&[ALT, SHIFT, T]);
            assert_eq!(
                h.rect_of(2),
                Some(Rect::new(400, 0, 400, 600)),
                "back in its leaf"
            );
            h.chord(&[ALT, SHIFT, F]);
            assert_eq!(zone_of(h, 2), Some(ZoneId::TOP_LEFT));
            assert_eq!(h.rect_of(2), Some(Rect::new(0, 0, 400, 300)));
            // Down twice: the left half, then the pre-snap rectangle (its old tile).
            h.chord(&[ALT, DOWN]);
            h.chord(&[ALT, DOWN]);
            assert_eq!(zone_of(h, 2), None);
            assert_eq!(h.rect_of(2), Some(Rect::new(400, 0, 400, 600)));
        });
    });
}

#[test]
fn snap_keybind_keys_never_reach_clients() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        let _w = c.map_window();
        server.on_server(|h| {
            for key in [LEFT, UP, DOWN, DOWN, RIGHT, UP, DOWN] {
                h.chord(&[ALT, key]);
            }
        });
        c.roundtrip();
        for key in [LEFT, RIGHT, UP, DOWN] {
            assert!(
                !c.events.keys_pressed.contains(&key),
                "keybind key {key} was forwarded"
            );
        }
    });
}

#[test]
fn window_destroyed_while_snapped_or_fullscreen() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let _w1 = c1.map_window();
        let mut c2 = TestClient::connect(&socket);
        let w2 = c2.map_window();
        server.on_server(|h| h.chord(&[ALT, LEFT]));
        c2.destroy(w2);
        server.on_server(|h| {
            assert_eq!(h.rect_of(1), Some(AREA), "the other window refills");
            h.server.state.windows.layout.check_invariants();
        });
        let mut c3 = TestClient::connect(&socket);
        let w3 = c3.map_window();
        w3.toplevel.set_fullscreen(None);
        c3.roundtrip();
        c3.destroy(w3);
        server.on_server(|h| {
            assert_eq!(h.server.state.windows.layout.fullscreen(), None);
            assert_eq!(h.rect_of(1), Some(AREA));
            h.server.state.windows.layout.check_invariants();
        });
    });
}

#[test]
fn output_resize_while_fullscreen() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        let w = c.map_window();
        w.toplevel.set_fullscreen(None);
        c.roundtrip();
        server.on_server(|h| h.server.state.set_layout_area((1000, 700).into()));
        c.roundtrip();
        assert_eq!(
            c.config(&w).map(|c| (c.size, c.fullscreen)),
            Some(((1000, 700), true))
        );
    });
}

#[test]
fn fixed_size_window_requesting_fullscreen_is_centered_over_a_backdrop() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        let mut fixed = c.create_window_without_commit();
        fixed.toplevel.set_min_size(300, 200);
        fixed.toplevel.set_max_size(300, 200);
        fixed.preferred = (300, 200);
        fixed.surface.commit();
        c.dispatch_until("configure", |e| e.configures >= 1);
        c.ack_and_commit(&fixed);
        fixed.toplevel.set_fullscreen(None);
        c.roundtrip();
        assert_eq!(
            c.config(&fixed).map(|c| c.fullscreen),
            Some(true),
            "granted"
        );
        server.on_server(|h| {
            let layout = &h.server.state.windows.layout;
            assert_eq!(
                layout.shown_rect(WindowId(1)),
                Some(Rect::new(250, 200, 300, 200))
            );
            assert_eq!(layout.fullscreen_backdrop(), Some(AREA));
            // Snap keys and maximize do nothing on a fixed-size window.
            h.chord(&[ALT, LEFT]);
            assert_eq!(zone_of(h, 1), None);
        });
    });
}

#[test]
fn snapping_on_a_tiny_output() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let _w1 = c1.map_window();
        let mut c2 = TestClient::connect(&socket);
        let _w2 = c2.map_window();
        server.on_server(|h| {
            h.server.state.set_layout_area((3, 2).into());
            for key in [LEFT, LEFT, UP, UP, DOWN, DOWN] {
                h.chord(&[ALT, key]);
                h.server.state.windows.layout.check_invariants();
            }
        });
        c1.roundtrip();
        c2.roundtrip();
    });
}
