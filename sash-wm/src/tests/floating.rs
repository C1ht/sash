// SPDX-License-Identifier: Apache-2.0

//! Floating windows end to end: real clients on the headless harness (output 800x600,
//! default mode tiled, modifier Alt). Window ids are assigned in creation order.

use smithay::backend::input::{ButtonState, KeyState};
use wayland_client::Proxy;
use wayland_protocols::xdg::shell::client::xdg_toplevel::ResizeEdge;

use super::{
    client::{TestClient, ToplevelConfig},
    harness::{
        Harness,
        keys::{ALT, CTRL, DOWN, F, H, J, K, L, LEFT, SHIFT, T, TAB, UP},
    },
};
use crate::{
    layout::{Mode, Rect},
    seat::{BTN_LEFT, BTN_RIGHT},
};

const AREA: Rect = Rect::new(0, 0, 800, 600);

/// Three clients, 1 | (2 / 3), window 3 focused.
fn open_three(
    socket: &std::path::Path,
    server: &super::harness::Remote,
) -> [(TestClient, super::client::TestWindow); 3] {
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

/// The configure for a window whose outer rectangle is `size` (0x0: the client
/// chooses): the test clients do not negotiate decorations, so they get the 1 px border
/// only: on all four edges while floating, on the edges inside the output while tiled
/// (`harness::tile`).
fn cfg(size: (i32, i32), tiled: bool) -> Option<ToplevelConfig> {
    let size = if size == (0, 0) {
        size
    } else if tiled {
        super::harness::tile(size.0, size.1).expect("a size")
    } else {
        crate::layout::decor::Decoration::BORDER.client_size(size)
    };
    Some(ToplevelConfig {
        size,
        tiled,
        tiled_edges: [tiled; 4],
        ..Default::default()
    })
}

#[test]
fn floating_one_window_refills_the_others_and_toggles_back() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let [(mut c1, w1), (mut c2, w2), (mut c3, w3)] = open_three(&socket, &server);
        server.on_server(|h| {
            h.chord(&[ALT, F]);
            assert_eq!(h.mode_of(3), Some(Mode::Floating));
            assert_eq!(
                h.rect_of(3),
                Some(Rect::new(400, 300, 400, 300)),
                "floats where its tile was"
            );
            assert_eq!(
                h.rect_of(2),
                Some(Rect::new(400, 0, 400, 600)),
                "sibling takes the space"
            );
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 400, 600)));
        });
        c2.roundtrip();
        c3.roundtrip();
        assert_eq!(c2.config(&w2), cfg((400, 600), true));
        assert_eq!(
            c3.config(&w3),
            cfg((400, 300), false),
            "same size, no tiled states"
        );

        server.on_server(|h| {
            h.chord(&[ALT, F]);
            assert_eq!(h.mode_of(3), Some(Mode::Tiled));
            assert_eq!(h.rect_of(2), Some(Rect::new(400, 0, 400, 300)));
            assert_eq!(h.rect_of(3), Some(Rect::new(400, 300, 400, 300)));
        });
        c2.roundtrip();
        c3.roundtrip();
        c1.roundtrip();
        assert_eq!(c2.config(&w2), cfg((400, 300), true));
        assert_eq!(c3.config(&w3), cfg((400, 300), true));
        assert_eq!(c1.config(&w1), cfg((400, 600), true));
    });
}

#[test]
fn float_all_and_tile_all() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let [(mut c1, w1), _c2, (mut c3, w3)] = open_three(&socket, &server);
        server.on_server(|h| {
            h.chord(&[ALT, SHIFT, F]);
            for (id, rect) in [
                (1, Rect::new(0, 0, 400, 600)),
                (2, Rect::new(400, 0, 400, 300)),
                (3, Rect::new(400, 300, 400, 300)),
            ] {
                assert_eq!(h.mode_of(id), Some(Mode::Floating), "window {id}");
                assert_eq!(h.rect_of(id), Some(rect), "window {id} floats at its tile");
            }
            // Move the focused window (3) and make it narrower.
            h.chord(&[ALT, CTRL, L]);
            h.chord(&[ALT, CTRL, SHIFT, H]);
            assert_eq!(h.rect_of(3), Some(Rect::new(434, 300, 366, 300)));
        });
        c1.roundtrip();
        c3.roundtrip();
        assert_eq!(c1.config(&w1), cfg((400, 600), false));
        assert_eq!(c3.config(&w3), cfg((366, 300), false));

        server.on_server(|h| {
            h.chord(&[ALT, SHIFT, T]);
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 400, 600)));
            assert_eq!(h.rect_of(2), Some(Rect::new(400, 0, 400, 300)));
            assert_eq!(
                h.rect_of(3),
                Some(Rect::new(400, 300, 400, 300)),
                "back in its leaf"
            );
        });
        c1.roundtrip();
        c3.roundtrip();
        assert_eq!(c1.config(&w1), cfg((400, 600), true));
        assert_eq!(c3.config(&w3), cfg((400, 300), true));

        // Floating it again restores where it was moved to.
        server.on_server(|h| {
            h.chord(&[ALT, F]);
            assert_eq!(h.rect_of(3), Some(Rect::new(434, 300, 366, 300)));
        });
    });
}

#[test]
fn dialog_with_a_parent_opens_centered_over_it_and_floating() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        let parent = c.map_window();
        let mut other = TestClient::connect(&socket);
        let _right = other.map_window(); // parent left half, other right half

        let mut dialog = c.create_window_without_commit();
        dialog.toplevel.set_parent(Some(&parent.toplevel));
        dialog.preferred = (200, 100);
        dialog.surface.commit();
        // Waits for the dialog's own configure (the parent's count changed with the
        // activated state, M19a).
        let id = dialog.toplevel.id();
        c.dispatch_until("the dialog's configure", |e| e.configs.contains_key(&id));
        assert_eq!(
            c.config(&dialog),
            cfg((0, 0), false),
            "floating: the client chooses its size"
        );
        c.ack_and_commit(&dialog);
        server.on_server(|h| {
            assert_eq!(h.mode_of(3), Some(Mode::Floating));
            assert_eq!(
                h.rect_of(3),
                Some(Rect::new(99, 249, 202, 102)),
                "centered over the parent"
            );
            assert_eq!(
                h.rect_of(1),
                Some(Rect::new(0, 0, 400, 600)),
                "nothing else moves"
            );
            assert_eq!(h.rect_of(2), Some(Rect::new(400, 0, 400, 600)));
            assert_eq!(h.keyboard_focus(), Some(3));
            // Tile-all never touches it.
            h.chord(&[ALT, SHIFT, T]);
            assert_eq!(h.mode_of(3), Some(Mode::Floating));
        });
    });
}

#[test]
fn fixed_size_window_floats_centered_on_the_output() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let _w1 = c1.map_window();
        let mut c2 = TestClient::connect(&socket);
        let mut fixed = c2.create_window_without_commit();
        fixed.toplevel.set_min_size(300, 200);
        fixed.toplevel.set_max_size(300, 200);
        fixed.preferred = (300, 200);
        fixed.surface.commit();
        c2.dispatch_until("configure", |e| e.configures >= 1);
        assert_eq!(c2.config(&fixed), cfg((0, 0), false));
        c2.ack_and_commit(&fixed);
        server.on_server(|h| {
            assert_eq!(h.mode_of(2), Some(Mode::Floating));
            assert_eq!(h.rect_of(2), Some(Rect::new(249, 199, 302, 202)));
            assert_eq!(
                h.rect_of(1),
                Some(AREA),
                "the tiled window keeps the output"
            );
            h.chord(&[ALT, F]);
            assert_eq!(
                h.mode_of(2),
                Some(Mode::Floating),
                "mod+F does nothing on a fixed-size window"
            );
        });
    });
}

#[test]
fn click_focuses_raises_and_is_delivered() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let _w1 = c1.map_window();
        let mut c2 = TestClient::connect(&socket);
        let _w2 = c2.map_window();
        server.on_server(|h| {
            assert_eq!(h.keyboard_focus(), Some(2));
            h.pointer_to(100.0, 100.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.button(BTN_LEFT, ButtonState::Released);
            assert_eq!(h.keyboard_focus(), Some(1), "click focuses");
        });
        c1.roundtrip();
        assert_eq!(
            c1.events.buttons_pressed,
            vec![BTN_LEFT],
            "the focusing click is delivered"
        );

        // Two overlapping floating windows; 2 is on top. Clicking the visible part of 1
        // raises it, so the overlap now belongs to 1.
        server.on_server(|h| {
            h.chord(&[ALT, SHIFT, F]);
            let layout = &mut h.server.state.windows.layout;
            layout.set_floating_rect(crate::layout::WindowId(1), Rect::new(0, 0, 300, 300));
            layout.set_floating_rect(crate::layout::WindowId(2), Rect::new(200, 200, 300, 300));
            layout.focus(crate::layout::WindowId(2));
            h.server.state.arrange();
            assert_eq!(
                h.server.state.window_at((250.0, 250.0).into()).map(|w| w.0),
                Some(2)
            );
            h.pointer_to(50.0, 50.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.button(BTN_LEFT, ButtonState::Released);
            assert_eq!(h.keyboard_focus(), Some(1));
            assert_eq!(
                h.server.state.window_at((250.0, 250.0).into()).map(|w| w.0),
                Some(1),
                "raised"
            );
        });
    });
}

#[test]
fn client_move_request_floats_and_moves_a_tiled_window() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        let w = c.map_window();
        server.on_server(|h| {
            h.pointer_to(100.0, 100.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
        });
        c.roundtrip();
        c.request_move(&w); // e.g. a drag on its title bar
        server.on_server(|h| {
            assert!(h.pointer_grabbed());
            h.pointer_to(150.0, 130.0);
            assert_eq!(h.mode_of(1), Some(Mode::Floating));
            assert_eq!(
                h.rect_of(1),
                Some(Rect::new(50, 30, 800, 600)),
                "followed the pointer"
            );
            // Shift at release: it stays floating (M19c; without it, it is tiled again).
            h.key(SHIFT, KeyState::Pressed);
            h.button(BTN_LEFT, ButtonState::Released);
            h.key(SHIFT, KeyState::Released);
            assert!(!h.pointer_grabbed(), "release ends the move");
            h.pointer_to(400.0, 400.0);
            assert_eq!(h.rect_of(1), Some(Rect::new(50, 30, 800, 600)));
        });
        c.roundtrip();
        assert_eq!(c.config(&w), cfg((800, 600), false));
    });
}

#[test]
fn client_resize_request_resizes_a_floating_window() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        let w = c.map_window();
        server.on_server(|h| {
            h.chord(&[ALT, F]); // floats at (0, 0, 800, 600)
            // In the client's area, clear of the decorations' resize zones.
            h.pointer_to(770.0, 570.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
        });
        c.roundtrip();
        c.request_resize(&w, ResizeEdge::BottomRight);
        server.on_server(|h| {
            h.pointer_to(670.0, 520.0);
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 700, 550)));
        });
        c.roundtrip();
        assert_eq!(
            c.config(&w),
            Some(ToplevelConfig {
                size: (698, 548),
                resizing: true,
                ..Default::default()
            })
        );
        server.on_server(|h| h.button(BTN_LEFT, ButtonState::Released));
        c.roundtrip();
        assert_eq!(
            c.config(&w),
            cfg((700, 550), false),
            "resizing state cleared"
        );

        // A resize request on a tiled window is ignored, also with a split next to
        // the edge.
        let mut c2 = TestClient::connect(&socket);
        let _w2 = c2.map_window();
        server.on_server(|h| {
            h.chord(&[ALT, H]); // focus 1
            h.chord(&[ALT, F]); // 1 | 2
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 400, 600)));
            // In the client's area (the tile's resize zones would start a resize).
            h.pointer_to(370.0, 570.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
        });
        c.roundtrip();
        c.request_resize(&w, ResizeEdge::BottomRight);
        server.on_server(|h| {
            assert!(!h.server.state.dragged_window().is_some());
            h.pointer_to(300.0, 300.0);
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 400, 600)));
            h.button(BTN_LEFT, ButtonState::Released);
        });
    });
}

#[test]
fn the_top_edge_never_goes_above_the_output() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        let w = c.map_window();
        server.on_server(|h| {
            h.chord(&[ALT, F]); // floats at (0, 0, 800, 600)
            // Shift held through the drags: no edge snapping, so the numbers are exact.
            h.key(ALT, KeyState::Pressed);
            h.key(SHIFT, KeyState::Pressed);
            // Shrink to 400x300 from the bottom-right corner, then move it to y = 100.
            h.pointer_to(400.0, 300.0);
            h.button(BTN_RIGHT, ButtonState::Pressed);
            h.pointer_to(0.0, 0.0);
            h.button(BTN_RIGHT, ButtonState::Released);
            h.pointer_to(100.0, 100.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.pointer_to(150.0, 200.0);
            h.button(BTN_LEFT, ButtonState::Released);
            assert_eq!(h.rect_of(1), Some(Rect::new(50, 100, 400, 300)));
            // Mouse move upwards past the top: the top edge stops at 0; the window
            // follows the pointer back down from there.
            h.pointer_to(200.0, 200.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.pointer_to(220.0, 20.0);
            assert_eq!(h.rect_of(1), Some(Rect::new(70, 0, 400, 300)));
            h.pointer_to(220.0, 0.0);
            assert_eq!(h.rect_of(1), Some(Rect::new(70, 0, 400, 300)));
            h.pointer_to(220.0, 130.0);
            assert_eq!(h.rect_of(1), Some(Rect::new(70, 30, 400, 300)));
            h.button(BTN_LEFT, ButtonState::Released);
            // Resize by the top edge (top-middle cell) past the top: the top edge stops
            // at 0, the bottom edge stays.
            h.pointer_to(270.0, 40.0);
            h.button(BTN_RIGHT, ButtonState::Pressed);
            h.pointer_to(270.0, 0.0);
            assert_eq!(h.rect_of(1), Some(Rect::new(70, 0, 400, 330)));
            h.button(BTN_RIGHT, ButtonState::Released);
            h.key(SHIFT, KeyState::Released);
            h.key(ALT, KeyState::Released);
            // Keyboard move up (mod+Ctrl+K, 34 px): stays at 0.
            h.chord(&[ALT, CTRL, J]);
            assert_eq!(h.rect_of(1), Some(Rect::new(70, 34, 400, 330)));
            h.chord(&[ALT, CTRL, K]);
            h.chord(&[ALT, CTRL, K]);
            assert_eq!(h.rect_of(1), Some(Rect::new(70, 0, 400, 330)));
            // Restore from a snap by dragging out, grabbed near the bottom: the grab
            // point keeps its horizontal position, the top clamps at 0, and the window
            // then follows the pointer.
            h.chord(&[ALT, LEFT]);
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 400, 600)));
            h.key(ALT, KeyState::Pressed);
            h.key(SHIFT, KeyState::Pressed);
            h.pointer_to(100.0, 540.0); // 25% across, 90% down
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.pointer_to(300.0, 100.0);
            assert_eq!(h.rect_of(1), Some(Rect::new(200, 0, 400, 330)));
            h.pointer_to(310.0, 140.0);
            assert_eq!(h.rect_of(1), Some(Rect::new(210, 40, 400, 330)));
            h.button(BTN_LEFT, ButtonState::Released);
            h.key(SHIFT, KeyState::Released);
            h.key(ALT, KeyState::Released);
            // Restore from maximize: back at its rectangle, on the output.
            h.chord(&[ALT, UP]);
            assert_eq!(h.rect_of(1), Some(AREA));
            h.chord(&[ALT, DOWN]);
            assert_eq!(h.rect_of(1), Some(Rect::new(210, 40, 400, 330)));
            // An output resize moves it up just enough for 55 px, never above the top.
            h.server.state.set_layout_area((800, 90).into());
            assert_eq!(h.rect_of(1), Some(Rect::new(210, 35, 400, 330)));
            h.server.state.set_layout_area((800, 20).into());
            assert_eq!(h.rect_of(1), Some(Rect::new(210, 0, 400, 330)));
            h.server.state.set_layout_area((800, 600).into());
            h.pointer_to(300.0, 100.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
        });
        // A client move request (a title-bar drag) upwards past the top.
        c.roundtrip();
        c.request_move(&w);
        server.on_server(|h| {
            h.key(SHIFT, KeyState::Pressed);
            h.pointer_to(300.0, 10.0);
            assert_eq!(h.rect_of(1).map(|r| r.y), Some(0));
            h.button(BTN_LEFT, ButtonState::Released);
            h.key(SHIFT, KeyState::Released);
        });
    });
}

#[test]
fn mod_drag_moves_and_resizes_without_reaching_clients() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let _w1 = c1.map_window();
        let mut c2 = TestClient::connect(&socket);
        let _w2 = c2.map_window(); // 1 | 2
        server.on_server(|h| {
            h.key(ALT, KeyState::Pressed);
            // Left-drag on tiled window 1: it floats on the press and follows the pointer.
            h.pointer_to(100.0, 100.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            assert_eq!(h.mode_of(1), Some(Mode::Floating), "floats on press");
            assert_eq!(h.rect_of(2), Some(AREA));
            h.pointer_to(130.0, 140.0);
            // Shift at release: it stays floating (M19c).
            h.key(SHIFT, KeyState::Pressed);
            h.button(BTN_LEFT, ButtonState::Released);
            h.key(SHIFT, KeyState::Released);
            assert_eq!(h.rect_of(1), Some(Rect::new(30, 40, 400, 600)));
            // Right-drag in the window's center cell resizes from the bottom-right corner.
            h.pointer_to(230.0, 340.0);
            h.button(BTN_RIGHT, ButtonState::Pressed);
            h.pointer_to(270.0, 320.0);
            h.button(BTN_RIGHT, ButtonState::Released);
            assert_eq!(h.rect_of(1), Some(Rect::new(30, 40, 440, 580)));
            // Right-drag in the top-left cell moves the top-left corner. Shift held:
            // no edge snapping (x = 10 would otherwise snap to the output edge).
            h.key(SHIFT, KeyState::Pressed);
            h.pointer_to(40.0, 50.0);
            h.button(BTN_RIGHT, ButtonState::Pressed);
            h.pointer_to(20.0, 60.0);
            h.button(BTN_RIGHT, ButtonState::Released);
            h.key(SHIFT, KeyState::Released);
            assert_eq!(h.rect_of(1), Some(Rect::new(10, 50, 460, 570)));
            // mod+right-drag on a tiled window with no split on its edges does nothing.
            h.pointer_to(700.0, 300.0);
            h.button(BTN_RIGHT, ButtonState::Pressed);
            h.pointer_to(600.0, 200.0);
            h.button(BTN_RIGHT, ButtonState::Released);
            assert_eq!(h.mode_of(2), Some(Mode::Tiled));
            h.key(ALT, KeyState::Released);
        });
        c1.roundtrip();
        c2.roundtrip();
        assert!(
            c1.events.buttons_pressed.is_empty(),
            "mod+clicks are not delivered"
        );
        assert!(
            c2.events.buttons_pressed.is_empty(),
            "mod+clicks are not delivered"
        );
    });
}

#[test]
fn floating_keybind_keys_never_reach_clients() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let [(mut c1, _), (mut c2, _), (mut c3, _)] = open_three(&socket, &server);
        server.on_server(|h| {
            h.chord(&[ALT, F]);
            h.chord(&[ALT, CTRL, L]);
            h.chord(&[ALT, CTRL, SHIFT, L]);
            h.chord(&[ALT, SHIFT, F]);
            h.chord(&[ALT, SHIFT, T]);
            h.chord(&[ALT, TAB]);
            h.chord(&[ALT, SHIFT, TAB]);
        });
        for client in [&mut c1, &mut c2, &mut c3] {
            client.roundtrip();
            for key in [F, L, T, TAB] {
                assert!(
                    !client.events.keys_pressed.contains(&key),
                    "keybind key {key} was forwarded"
                );
            }
        }
    });
}

#[test]
fn mod_tab_cycles_while_held_and_commits_on_release() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let _clients = open_three(&socket, &server); // focus history 1, 2, 3
        server.on_server(|h| {
            h.key(ALT, KeyState::Pressed);
            h.chord(&[TAB]);
            assert_eq!(h.keyboard_focus(), Some(2));
            h.chord(&[TAB]);
            assert_eq!(h.keyboard_focus(), Some(1));
            h.chord(&[SHIFT, TAB]);
            assert_eq!(h.keyboard_focus(), Some(2), "Shift+Tab steps back");
            h.chord(&[TAB]);
            h.key(ALT, KeyState::Released);
            assert_eq!(h.keyboard_focus(), Some(1));
            // Committed: 1 is now the most recent, so one more Alt+Tab goes to 3.
            h.chord(&[ALT, TAB]);
            assert_eq!(h.keyboard_focus(), Some(3));
        });
    });
}

#[test]
fn window_destroyed_during_a_drag_ends_the_drag() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        let w = c.map_window();
        server.on_server(|h| {
            h.key(ALT, KeyState::Pressed);
            h.pointer_to(100.0, 100.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            assert!(h.pointer_grabbed());
        });
        c.destroy(w);
        server.on_server(|h| {
            assert!(!h.pointer_grabbed(), "the grab ended with the window");
            assert_eq!(h.server.state.dragged_window(), None);
            h.pointer_to(200.0, 200.0);
            h.button(BTN_LEFT, ButtonState::Released);
            h.key(ALT, KeyState::Released);
            assert_eq!(h.mapped_windows() + h.unmapped_windows(), 0);
        });
    });
}

#[test]
fn moving_keeps_the_size_the_client_settled_on() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        let w = c.map_window();
        server.on_server(|h| {
            h.key(ALT, KeyState::Pressed);
            h.pointer_to(400.0, 300.0);
            h.button(BTN_LEFT, ButtonState::Pressed); // floats at (0, 0, 800, 600)
        });
        c.roundtrip();
        assert_eq!(c.config(&w), cfg((800, 600), false));
        // Like foot rounding to its cell grid once it is no longer tiled.
        c.ack_and_commit_size(&w, (790, 590));
        // The outer rectangle: the client's size plus its border.
        server.on_server(|h| assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 792, 592))));
        let configures = c.events.configures;
        server.on_server(|h| {
            h.pointer_to(420.0, 310.0);
            h.pointer_to(440.0, 400.0);
        });
        c.roundtrip();
        server.on_server(|h| {
            assert_eq!(h.rect_of(1), Some(Rect::new(40, 100, 792, 592)));
            // Shift at release: it stays floating (M19c).
            h.key(SHIFT, KeyState::Pressed);
            h.button(BTN_LEFT, ButtonState::Released);
            h.key(SHIFT, KeyState::Released);
            h.key(ALT, KeyState::Released);
        });
        c.roundtrip();
        assert_eq!(c.config(&w), cfg((792, 592), false));
        assert!(
            c.events.configures <= configures + 1,
            "no configure back to the size at the press"
        );
    });
}

#[test]
fn drag_while_the_output_is_hidden_still_updates_the_layout() {
    let mut harness = Harness::new();
    harness.presenting = false;
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        let w = c.map_window();
        server.on_server(|h| {
            h.chord(&[ALT, F]);
            h.key(ALT, KeyState::Pressed);
            h.pointer_to(700.0, 500.0); // bottom-right cell
            h.button(BTN_RIGHT, ButtonState::Pressed);
            h.pointer_to(600.0, 450.0);
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 700, 550)));
            h.button(BTN_RIGHT, ButtonState::Released);
            h.key(ALT, KeyState::Released);
        });
        c.roundtrip();
        assert_eq!(
            c.config(&w),
            cfg((700, 550), false),
            "configures are sent while hidden"
        );
    });
    assert_eq!(harness.frames, 0);
}

#[test]
fn window_larger_than_the_output_and_tiny_output() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        let mut big = c.create_window_without_commit();
        big.toplevel.set_min_size(2000, 1500);
        big.toplevel.set_max_size(2000, 1500);
        big.preferred = (2000, 1500);
        big.surface.commit();
        c.dispatch_until("configure", |e| e.configures >= 1);
        c.ack_and_commit(&big);
        server.on_server(|h| {
            assert_eq!(h.rect_of(1), Some(Rect::new(-601, 0, 2002, 1502)));
            // Moving it far away keeps 55 px on the output.
            h.chord(&[ALT, CTRL, L]);
            for _ in 0..50 {
                h.chord(&[ALT, CTRL, L]);
            }
            assert_eq!(h.rect_of(1).map(|r| r.x), Some(745));
            // Shrinking the output to 3x2: still kept on it.
            h.server.state.set_layout_area((3, 2).into());
            let r = h.rect_of(1).expect("placed");
            assert!(
                r.x <= 0 && r.right() >= 3 && r.y <= 0 && r.bottom() >= 2,
                "{r:?}"
            );
        });
    });
}

/// Edge frames: a drag with the pointer pinned at a screen edge (relative motion
/// clamped to the output) requests no frames; a stationary pointer none either; real
/// motion at most one per frame.
#[test]
fn a_pointer_pinned_at_an_edge_during_a_drag_requests_no_frames() {
    let mut harness = Harness::new();
    harness.server.state.draws_named_cursor = true; // the arrow is drawn, as on DRM
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let _w1 = c1.map_window();
        // (push direction, a point inside the output)
        let edges = [
            ((-50.0, 0.0), (400.0, 300.0)), // left
            ((50.0, 0.0), (400.0, 300.0)),  // right
            ((0.0, -50.0), (400.0, 300.0)), // top
            ((0.0, 50.0), (400.0, 300.0)),  // bottom
        ];
        for (push, start) in edges {
            server.on_server(move |h| {
                h.key(ALT, KeyState::Pressed);
                h.pointer_to(start.0, start.1);
                h.button(BTN_LEFT, ButtonState::Pressed);
                // Into the edge, then let the frame for that motion happen.
                for _ in 0..40 {
                    h.server.state.pointer_moved_by(push.into(), 0);
                }
                for _ in 0..5 {
                    h.step();
                }
                let rect = h.rect_of(1);
                let frames = h.frames;
                // Pinned: 1000 more pushes into the edge and many steps.
                for _ in 0..1000 {
                    h.server.state.pointer_moved_by(push.into(), 0);
                }
                for _ in 0..20 {
                    h.step();
                }
                assert_eq!(h.frames, frames, "pinned at the edge {push:?}: no frames");
                assert_eq!(h.rect_of(1), rect, "nothing moved");
                // Stationary: no motion, no frame.
                for _ in 0..20 {
                    h.step();
                }
                assert_eq!(h.frames, frames, "stationary: no frames");
                // Real motion: 50 motions before the next frame are one frame.
                for i in 1..=50 {
                    let back = (-push.0.signum() * i as f64, -push.1.signum() * i as f64);
                    h.server.state.pointer_moved_by(back.into(), 0);
                }
                h.step();
                assert_eq!(h.frames, frames + 1, "real motion: one frame");
                h.button(BTN_LEFT, ButtonState::Released);
                h.key(ALT, KeyState::Released);
                for _ in 0..5 {
                    h.step();
                }
            });
        }
    });
}
