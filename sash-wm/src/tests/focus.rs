// SPDX-License-Identifier: Apache-2.0

//! Focus follows the cursor (owner, M19a), the activated state, the pointer refresh and
//! the typing guard, end to end: real clients on the headless harness (output 800x600,
//! modifier Alt), with the setting turned on (the harness starts with it off). Without
//! gaps, window 1 is the left half and window 2 the right half.

use std::{sync::mpsc, time::Duration};

use proptest::prelude::*;
use smithay::backend::input::{ButtonState, KeyState};
use wayland_client::{Proxy, protocol::wl_data_device_manager::DndAction};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::Layer, zwlr_layer_surface_v1::KeyboardInteractivity,
};

use super::{
    client::{PopupSpec, TestClient, TestWindow, layer::LayerSpec},
    harness::{
        Harness, Remote,
        keys::{ALT, H, L, SHIFT, TAB},
    },
};
use crate::{
    decorations::{Piece, pieces},
    layout::{WindowId, decor::Decoration, gaps::Gaps},
    seat::{BTN_LEFT, BTN_RIGHT},
};

const ESCAPE: u32 = 1;
/// A plain letter (evdev KEY_A).
const KEY_A: u32 = 30;

fn following() -> Harness {
    let mut harness = Harness::new();
    harness.server.state.focus.follows_pointer = true;
    harness
}

fn focus(server: &Remote) -> Option<u64> {
    let (tx, rx) = mpsc::channel();
    server.on_server(move |h| {
        let _ = tx.send(h.server.state.windows.layout.focused().map(|w| w.0));
    });
    rx.recv().unwrap()
}

fn to(server: &Remote, x: f64, y: f64) {
    server.on_server(move |h| h.pointer_to(x, y));
}

/// Moves the pointer to (x, y) and returns the focused window.
fn focus_at(server: &Remote, x: f64, y: f64) -> Option<u64> {
    to(server, x, y);
    focus(server)
}

fn two(socket: &std::path::Path) -> (TestClient, TestWindow, TestClient, TestWindow) {
    let mut c1 = TestClient::connect(socket);
    let w1 = c1.map_window();
    let mut c2 = TestClient::connect(socket);
    let w2 = c2.map_window();
    c1.roundtrip();
    c2.roundtrip();
    (c1, w1, c2, w2)
}

/// Lets an idle callback (the pointer refresh) run and its events arrive.
fn settle(clients: &mut [&mut TestClient]) {
    for _ in 0..2 {
        for c in clients.iter_mut() {
            c.roundtrip();
        }
    }
}

/// The geometry check: a window that slides under a still pointer (a swap) never takes
/// focus, not even on the next small motion inside it; the pointer refresh gives it
/// wl_pointer at once.
#[test]
fn a_window_sliding_under_a_still_pointer_never_takes_focus() {
    let mut harness = following();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c1, w1, mut c2, w2) = two(&socket);
        assert_eq!(focus_at(&server, 100.0, 300.0), Some(1));
        // Swap: 1 goes right (focus follows it), 2 slides under the pointer.
        server.on_server(|h| h.chord(&[ALT, SHIFT, L]));
        assert_eq!(focus(&server), Some(1));
        settle(&mut [&mut c1, &mut c2]);
        assert_eq!(c2.events.pointer_focus, Some(w2.surface.id()), "refreshed");
        assert_ne!(c1.events.pointer_focus, Some(w1.surface.id()));
        // A real motion that stays inside 2: no window was entered.
        assert_eq!(focus_at(&server, 101.0, 300.0), Some(1));
        assert_eq!(focus_at(&server, 300.0, 200.0), Some(1));
        // Entering 1 (now right) and coming back into 2 does move it.
        assert_eq!(focus_at(&server, 600.0, 300.0), Some(1));
        assert_eq!(focus_at(&server, 100.0, 300.0), Some(2));
    });
}

/// Window to window, gap to window; gaps and the outer gap keep the focus; pointer
/// focus never raises.
#[test]
fn window_to_window_and_gap_to_window() {
    let mut harness = following();
    harness.server.state.windows.layout.set_gaps(Gaps::DEFAULT);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (_c1, _w1, _c2, _w2) = two(&socket);
        // Visible: 1 is x 13..396, 2 is x 404..787.
        assert_eq!(focus_at(&server, 100.0, 300.0), Some(1));
        assert_eq!(focus_at(&server, 600.0, 300.0), Some(2));
        assert_eq!(
            focus_at(&server, 400.0, 300.0),
            Some(2),
            "the inner gap keeps it"
        );
        assert_eq!(focus_at(&server, 300.0, 300.0), Some(1), "gap to window");
        assert_eq!(
            focus_at(&server, 5.0, 300.0),
            Some(1),
            "the outer gap keeps it"
        );
        assert_eq!(focus_at(&server, 600.0, 5.0), Some(1));
        assert_eq!(focus_at(&server, 600.0, 100.0), Some(2));
        // A keyboard move, then out of 1 into the gap and back: entering 1 refocuses it.
        assert_eq!(focus_at(&server, 300.0, 300.0), Some(1));
        server.on_server(|h| h.chord(&[ALT, L]));
        assert_eq!(focus(&server), Some(2));
        assert_eq!(focus_at(&server, 398.0, 300.0), Some(2));
        assert_eq!(focus_at(&server, 300.0, 300.0), Some(1));
        // Floating, overlapping: focusing the lower window by pointer does not raise it.
        server.on_server(|h| {
            let layout = &mut h.server.state.windows.layout;
            layout.float_all();
            layout.set_floating_rect(WindowId(1), crate::layout::Rect::new(50, 50, 400, 300));
            layout.set_floating_rect(WindowId(2), crate::layout::Rect::new(300, 200, 400, 300));
            layout.focus(WindowId(2)); // 2 raised, on top
            h.server.state.arrange();
            let order = |h: &Harness| -> Vec<u64> {
                h.server
                    .state
                    .windows
                    .layout
                    .floating_rects()
                    .iter()
                    .map(|(w, _)| w.0)
                    .collect()
            };
            let before = order(h);
            h.pointer_to(100.0, 100.0); // only 1 here
            assert_eq!(h.server.state.windows.layout.focused(), Some(WindowId(1)));
            assert_eq!(order(h), before, "not raised");
            h.pointer_to(350.0, 250.0); // the overlap: 2 is on top there
            assert_eq!(h.server.state.windows.layout.focused(), Some(WindowId(2)));
        });
    });
}

/// A keyboard focus move stays while the pointer moves inside the window it is in.
#[test]
fn a_keyboard_focus_move_stays_until_the_pointer_enters_another_window() {
    let mut harness = following();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (_c1, _w1, _c2, _w2) = two(&socket);
        assert_eq!(focus_at(&server, 100.0, 300.0), Some(1));
        server.on_server(|h| h.chord(&[ALT, L]));
        assert_eq!(focus(&server), Some(2));
        for (x, y) in [(150.0, 300.0), (10.0, 10.0), (399.0, 599.0)] {
            assert_eq!(focus_at(&server, x, y), Some(2), "inside 1 at {x},{y}");
        }
        assert_eq!(focus_at(&server, 600.0, 300.0), Some(2));
        assert_eq!(
            focus_at(&server, 100.0, 300.0),
            Some(1),
            "re-entering refocuses"
        );
        // The setting off: click-to-focus only.
        server.on_server(|h| h.server.state.focus.follows_pointer = false);
        assert_eq!(focus_at(&server, 600.0, 300.0), Some(1));
    });
}

/// Move and resize drags, a held button, drag and drop: no focus change during them.
#[test]
fn pointer_grabs_hold_focus() {
    let mut harness = following();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c1, w1, mut c2, _w2) = two(&socket);
        assert_eq!(focus_at(&server, 300.0, 300.0), Some(1));
        // A resize drag (tiled: the split moves).
        server.on_server(|h| {
            h.key(ALT, KeyState::Pressed);
            h.button(BTN_RIGHT, ButtonState::Pressed);
            h.pointer_to(500.0, 300.0);
            h.pointer_to(700.0, 300.0);
            assert_eq!(h.server.state.windows.layout.focused(), Some(WindowId(1)));
            h.button(BTN_RIGHT, ButtonState::Released);
            h.key(ALT, KeyState::Released);
        });
        // A move drag.
        assert_eq!(focus_at(&server, 100.0, 300.0), Some(1));
        server.on_server(|h| {
            h.key(ALT, KeyState::Pressed);
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.pointer_to(500.0, 300.0);
            h.pointer_to(750.0, 500.0);
            assert_eq!(h.server.state.windows.layout.focused(), Some(WindowId(1)));
            h.button(BTN_LEFT, ButtonState::Released);
            h.key(ALT, KeyState::Released);
            // Tile it again for the next cases.
            h.server.state.windows.layout.tile_all();
            h.server.state.arrange();
        });
        c1.roundtrip();
        c2.roundtrip();
        // A held button (the implicit grab, e.g. a text selection leaving the window).
        assert_eq!(focus_at(&server, 100.0, 300.0), Some(1));
        server.on_server(|h| {
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.pointer_to(600.0, 300.0);
            assert_eq!(h.server.state.windows.layout.focused(), Some(WindowId(1)));
            h.button(BTN_LEFT, ButtonState::Released);
            // No crossing after the release: still 1.
            h.pointer_to(610.0, 300.0);
            assert_eq!(h.server.state.windows.layout.focused(), Some(WindowId(1)));
        });
        // Drag and drop from window 1.
        c1.data_device();
        c2.data_device();
        c1.roundtrip();
        c2.roundtrip();
        assert_eq!(focus_at(&server, 100.0, 100.0), Some(1));
        server.on_server(|h| h.button(BTN_LEFT, ButtonState::Pressed));
        c1.roundtrip();
        let serial = c1.events.button_serial.expect("press delivered");
        let source = c1.new_source(&["text/plain"], b"x".to_vec(), Some(DndAction::Copy));
        let origin = w1.surface.clone();
        c1.start_drag(Some(&source), &origin, None, serial);
        server.on_server(|h| {
            assert!(h.server.state.clipboard.drag.is_some(), "drag started");
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.pointer_to(600.0, 300.0);
            h.pointer_to(650.0, 350.0);
            assert_eq!(h.server.state.windows.layout.focused(), Some(WindowId(1)));
            h.key(ESCAPE, KeyState::Pressed);
            h.key(ESCAPE, KeyState::Released);
            h.button(BTN_LEFT, ButtonState::Released);
        });
    });
}

/// A grabbed popup chain holds focus; so does a layer surface holding the keyboard
/// (and the window is not activated meanwhile).
#[test]
fn a_menu_and_a_layer_surface_hold_focus() {
    let mut harness = following();
    let stream = harness.server.state.privileged_connection_for_tests();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c1, w1, mut c2, w2) = two(&socket);
        assert_eq!(focus_at(&server, 100.0, 100.0), Some(1));
        server.on_server(|h| {
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.button(BTN_LEFT, ButtonState::Released);
        });
        c1.roundtrip();
        let serial = c1.events.button_serial.expect("click delivered");
        let menu = c1.open_popup(&w1.xdg_surface, PopupSpec::default(), Some(serial));
        assert_eq!(c1.events.keyboard_focus, Some(menu.surface.id()));
        assert_eq!(
            focus_at(&server, 600.0, 300.0),
            Some(1),
            "the menu holds focus"
        );
        // A click outside ends the menu (not delivered); the pointer focus is free again.
        server.on_server(|h| {
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.button(BTN_LEFT, ButtonState::Released);
        });
        c1.roundtrip();
        assert!(c1.popup_done(&menu));
        c1.destroy_popup(menu);
        assert_eq!(focus_at(&server, 100.0, 300.0), Some(1));
        assert_eq!(focus_at(&server, 600.0, 300.0), Some(2));

        // An on_demand layer surface, clicked: it holds the keyboard.
        let mut p = TestClient::from_connection(
            wayland_client::Connection::from_socket(stream).expect("connection"),
        );
        let _panel = p.map_layer(&LayerSpec::centered(
            Layer::Top,
            (200, 100),
            KeyboardInteractivity::OnDemand,
        ));
        server.on_server(|h| {
            h.pointer_to(400.0, 300.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.button(BTN_LEFT, ButtonState::Released);
            assert!(h.server.state.layer_keyboard_target().is_some());
            assert!(h.server.state.windows.activated_for_tests().is_empty());
            h.pointer_to(100.0, 300.0); // into window 1
            assert_eq!(h.server.state.windows.layout.focused(), Some(WindowId(2)));
            assert!(h.server.state.layer_keyboard_target().is_some(), "kept");
        });
        c2.roundtrip();
        assert!(
            !c2.activated(&w2),
            "not activated while the layer has the keyboard"
        );
        // A click on a window gives it back.
        server.on_server(|h| {
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.button(BTN_LEFT, ButtonState::Released);
        });
        c1.roundtrip();
        assert_eq!(focus(&server), Some(1));
        assert!(c1.activated(&w1));
    });
}

/// A mod+Tab cycle holds focus until mod is released and the first real motion after
/// it (owner, M19a).
#[test]
fn a_mod_tab_cycle_holds_until_release_and_the_next_motion() {
    let mut harness = following();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (_c1, _w1, _c2, _w2) = two(&socket);
        assert_eq!(focus_at(&server, 600.0, 300.0), Some(2));
        server.on_server(|h| {
            h.key(ALT, KeyState::Pressed);
            h.key(TAB, KeyState::Pressed);
            h.key(TAB, KeyState::Released);
            assert_eq!(h.server.state.windows.layout.focused(), Some(WindowId(1)));
            h.pointer_to(100.0, 300.0); // into 1
            h.pointer_to(650.0, 300.0); // into 2: held
            h.pointer_to(100.0, 300.0);
            assert_eq!(h.server.state.windows.layout.focused(), Some(WindowId(1)));
            h.key(ALT, KeyState::Released);
            assert_eq!(h.server.state.windows.layout.focused(), Some(WindowId(1)));
            h.pointer_to(600.0, 300.0); // the first motion after release: held
            assert_eq!(h.server.state.windows.layout.focused(), Some(WindowId(1)));
            h.pointer_to(100.0, 300.0);
            h.pointer_to(600.0, 300.0); // free again
            assert_eq!(h.server.state.windows.layout.focused(), Some(WindowId(2)));
        });
    });
}

/// A fullscreen window keeps focus and stays fullscreen whatever the pointer does.
#[test]
fn pointer_focus_never_ends_fullscreen() {
    let mut harness = following();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c1, w1, _c2, _w2) = two(&socket);
        assert_eq!(focus_at(&server, 100.0, 300.0), Some(1));
        w1.toplevel.set_fullscreen(None);
        c1.roundtrip();
        c1.ack_and_commit(&w1);
        server.on_server(|h| {
            let layout = &h.server.state.windows.layout;
            assert_eq!(layout.fullscreen(), Some(WindowId(1)));
            for x in (0..800).step_by(37) {
                h.pointer_to(f64::from(x), 300.0);
            }
            let layout = &h.server.state.windows.layout;
            assert_eq!(layout.fullscreen(), Some(WindowId(1)));
            assert_eq!(layout.focused(), Some(WindowId(1)));
            // Directly too: the layout refuses pointer focus for another window.
            let layout = &mut h.server.state.windows.layout;
            assert!(!layout.pointer_focus(WindowId(2)));
            assert_eq!(layout.fullscreen(), Some(WindowId(1)));
        });
    });
}

/// The focused window closes while the pointer rests on another: the close fallback
/// decides; the pointer takes over only when it enters a window.
#[test]
fn closing_the_focused_window_uses_the_fallback_until_the_pointer_enters() {
    let mut harness = following();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let _w1 = c1.map_window();
        let mut c2 = TestClient::connect(&socket);
        let _w2 = c2.map_window();
        let mut c3 = TestClient::connect(&socket);
        let w3 = c3.map_window(); // 1 | (2 / 3)
        assert_eq!(focus_at(&server, 100.0, 300.0), Some(1));
        server.on_server(|h| h.chord(&[ALT, L]));
        assert_eq!(focus(&server), Some(3), "the most recent neighbour");
        c3.destroy(w3);
        c3.roundtrip();
        server.on_server(|h| {
            let layout = &h.server.state.windows.layout;
            assert!(!layout.contains(WindowId(3)));
            assert_eq!(layout.focused(), Some(WindowId(2)), "the sibling subtree");
        });
        assert_eq!(
            focus_at(&server, 120.0, 310.0),
            Some(2),
            "inside 1: no entry"
        );
        assert_eq!(focus_at(&server, 600.0, 300.0), Some(2));
        assert_eq!(focus_at(&server, 100.0, 300.0), Some(1));
    });
}

/// The dim follows focus: the unfocused window has it, the focused one does not.
#[test]
fn the_dim_follows_pointer_focus() {
    let mut harness = following();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (_c1, _w1, _c2, _w2) = two(&socket);
        for (x, focused, dimmed) in [(100.0, 1, 2), (600.0, 2, 1), (100.0, 1, 2)] {
            server.on_server(move |h| {
                h.pointer_to(x, 300.0);
                let state = &h.server.state;
                let theme = state.theme();
                let dim_of = |id: u64| {
                    let id = WindowId(id);
                    let layout = &state.windows.layout;
                    let outer = layout.shown_rect(id).unwrap();
                    // As a window with decorations draws it (the stand-ins negotiate none).
                    pieces(
                        theme,
                        outer,
                        Decoration::BORDER,
                        state.decoration_look(id, true),
                    )
                    .iter()
                    .any(|p| matches!(p, Piece::ShapeFill(_, c) if *c == theme.dim))
                };
                assert!(!dim_of(focused), "{focused} focused: no dim");
                assert!(dim_of(dimmed), "{dimmed} unfocused: dimmed");
            });
        }
    });
}

/// The interim typing guard: a window mapping within 377 ms of keyboard input maps
/// unfocused and not activated; later ones take focus.
#[test]
fn the_typing_guard_maps_unfocused_right_after_keyboard_input() {
    let mut harness = following();
    harness.server.state.focus.typing_guard = crate::focus::TYPING_GUARD;
    assert_eq!(crate::focus::TYPING_GUARD, Duration::from_millis(377));
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let w1 = c1.map_window();
        server.on_server(|h| {
            h.key(KEY_A, KeyState::Pressed);
            h.key(KEY_A, KeyState::Released);
        });
        let mut c2 = TestClient::connect(&socket);
        let w2 = c2.map_window();
        c1.roundtrip();
        c2.roundtrip();
        assert_eq!(
            focus(&server),
            Some(1),
            "typing: the new window maps unfocused"
        );
        assert!(c1.activated(&w1) && !c2.activated(&w2));
        server.on_server(|h| assert_eq!(h.keyboard_focus(), Some(1)));
        std::thread::sleep(Duration::from_millis(400));
        let mut c3 = TestClient::connect(&socket);
        let w3 = c3.map_window();
        c1.roundtrip();
        assert_eq!(focus(&server), Some(3), "no input for 400 ms: focused");
        assert!(c3.activated(&w3) && !c1.activated(&w1));
    });
}

/// Types one letter into the focused client (a keystroke delivered: typing).
fn type_a(h: &mut Harness) {
    h.key(KEY_A, KeyState::Pressed);
    h.key(KEY_A, KeyState::Released);
}

fn guarded() -> Harness {
    let mut harness = following();
    harness.server.state.focus.typing_guard = crate::focus::TYPING_GUARD;
    harness
}

/// A focus chord is not typing and exempts nothing (owner, after M19a): right after
/// typing and a focus chord, a background window maps unfocused; so does one mapping
/// 1 s after a focus chord while the user keeps typing.
#[test]
fn a_focus_chord_exempts_no_window_from_the_guard() {
    let mut harness = guarded();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let _w1 = c1.map_window();
        server.on_server(|h| {
            type_a(h);
            h.chord(&[ALT, L]);
        });
        let mut c2 = TestClient::connect(&socket);
        let w2 = c2.map_window();
        c2.roundtrip();
        assert_eq!(focus(&server), Some(1), "no exemption from a focus chord");
        assert!(!c2.activated(&w2));
        server.on_server(|h| h.chord(&[ALT, H]));
        for _ in 0..10 {
            std::thread::sleep(Duration::from_millis(100));
            server.on_server(type_a);
        }
        let mut c3 = TestClient::connect(&socket);
        let w3 = c3.map_window();
        c3.roundtrip();
        assert_eq!(
            focus(&server),
            Some(1),
            "1 s later, still typing: unfocused"
        );
        assert!(!c3.activated(&w3));
    });
}

/// A spawning chord (mod+Return, debug builds only: release builds have no spawning
/// keybind yet) right after typing: the spawned client takes focus; a second window
/// right after it is not exempt.
#[cfg(debug_assertions)]
#[test]
fn a_spawning_chord_exempts_only_the_first_window_to_map() {
    use super::harness::keys::ENTER;
    let mut harness = guarded();
    // A long guard, so the timing of the run cannot decide the second window.
    harness.server.state.focus.typing_guard = Duration::from_secs(60);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let _w1 = c1.map_window();
        server.on_server(|h| {
            type_a(h);
            h.chord(&[ALT, ENTER]);
            assert_eq!(h.server.state.terminals_spawned.borrow().len(), 1);
        });
        let mut c2 = TestClient::connect(&socket);
        let w2 = c2.map_window();
        c2.roundtrip();
        assert_eq!(focus(&server), Some(2), "the spawned client takes focus");
        assert!(c2.activated(&w2));
        let mut c3 = TestClient::connect(&socket);
        let w3 = c3.map_window();
        c3.roundtrip();
        assert_eq!(focus(&server), Some(2), "the second window is not exempt");
        assert!(!c3.activated(&w3));
    });
}

/// The exemption expires 13 s after the spawn (fake time): a window mapping later is
/// guarded; one mapping before is not.
#[cfg(debug_assertions)]
#[test]
fn the_spawn_exemption_expires_after_13_s() {
    use super::harness::keys::ENTER;
    let mut harness = guarded();
    harness.server.state.focus.typing_guard = Duration::from_secs(60);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let _w1 = c1.map_window();
        server.on_server(|h| {
            type_a(h);
            h.chord(&[ALT, ENTER]);
            h.server.state.focus.advance(Duration::from_secs(14));
        });
        let mut c2 = TestClient::connect(&socket);
        let w2 = c2.map_window();
        c2.roundtrip();
        assert_eq!(focus(&server), Some(1), "after 13 s: guarded");
        assert!(!c2.activated(&w2));
        server.on_server(|h| {
            type_a(h);
            h.chord(&[ALT, ENTER]);
            h.server.state.focus.advance(Duration::from_secs(12));
        });
        let mut c3 = TestClient::connect(&socket);
        let w3 = c3.map_window();
        c3.roundtrip();
        assert_eq!(focus(&server), Some(3), "within 13 s: exempt");
        assert!(c3.activated(&w3));
    });
}

/// The activated state: set on the window that gains focus (map, pointer, keyboard,
/// click), cleared on the one that loses it; never two at once.
#[test]
fn the_activated_state_is_sent_and_cleared() {
    let mut harness = following();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let w1 = c1.map_window();
        c1.roundtrip();
        assert!(c1.activated(&w1), "focused at its map");
        let mut c2 = TestClient::connect(&socket);
        let w2 = c2.map_window();
        c1.roundtrip();
        c2.roundtrip();
        assert!(!c1.activated(&w1) && c2.activated(&w2), "the new window");
        let step = |server: &Remote,
                    c1: &mut TestClient,
                    c2: &mut TestClient,
                    f: fn(&mut Harness)| {
            server.on_server(f);
            c1.roundtrip();
            c2.roundtrip();
            server.on_server(|h| assert!(h.server.state.windows.activated_for_tests().len() <= 1));
        };
        step(&server, &mut c1, &mut c2, |h| h.pointer_to(100.0, 300.0));
        assert!(c1.activated(&w1) && !c2.activated(&w2), "pointer");
        step(&server, &mut c1, &mut c2, |h| h.chord(&[ALT, L]));
        assert!(!c1.activated(&w1) && c2.activated(&w2), "keyboard");
        step(&server, &mut c1, &mut c2, |h| h.chord(&[ALT, H]));
        assert!(c1.activated(&w1) && !c2.activated(&w2));
        server.on_server(|h| h.server.state.focus.follows_pointer = false);
        step(&server, &mut c1, &mut c2, |h| {
            h.pointer_to(600.0, 300.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.button(BTN_LEFT, ButtonState::Released);
        });
        assert!(!c1.activated(&w1) && c2.activated(&w2), "click");
        // Each client saw its own state toggle (repeats are configures for other
        // reasons, such as a resize, that keep the state).
        let seen = |c: &TestClient, w: &TestWindow| {
            let mut v = c.events.activated[&w.toplevel.id()].clone();
            v.dedup();
            v
        };
        assert_eq!(
            seen(&c1, &w1),
            vec![false, true, false, true, false, true, false]
        );
        assert_eq!(seen(&c2, &w2), vec![false, true, false, true, false, true]);
    });
}

/// The old defect (A4 in docs/m19-investigation.md): after a swap under a still pointer,
/// a click without motion goes to the surface now under the pointer. Independent of the
/// setting, so it runs with click-to-focus.
#[test]
fn a_click_without_motion_after_a_swap_reaches_the_new_surface() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c1, _w1, mut c2, w2) = two(&socket);
        server.on_server(|h| {
            h.pointer_to(100.0, 300.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.button(BTN_LEFT, ButtonState::Released);
        });
        c1.roundtrip();
        let presses_1 = c1.events.buttons_pressed.len();
        server.on_server(|h| h.chord(&[ALT, SHIFT, L])); // 1 right, 2 under the pointer
        settle(&mut [&mut c1, &mut c2]);
        server.on_server(|h| {
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.button(BTN_LEFT, ButtonState::Released);
        });
        c1.roundtrip();
        c2.roundtrip();
        assert_eq!(c1.events.buttons_pressed.len(), presses_1, "nothing for 1");
        assert_eq!(c2.events.button_surfaces.last(), Some(&w2.surface.id()));
        assert_eq!(focus(&server), Some(2));
    });
}

#[derive(Clone, Debug)]
enum Op {
    Open,
    Close(usize),
    PointerTo(f64, f64),
    Keyboard(u32),
    Swap(u32),
    Click,
    Float,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        2 => Just(Op::Open),
        1 => (0..6usize).prop_map(Op::Close),
        5 => (0.0..800.0f64, 0.0..600.0f64).prop_map(|(x, y)| Op::PointerTo(x, y)),
        1 => prop_oneof![Just(H), Just(L)].prop_map(Op::Keyboard),
        1 => prop_oneof![Just(H), Just(L)].prop_map(Op::Swap),
        1 => Just(Op::Click),
        1 => Just(Op::Float),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    /// Random motion and layout changes: focus is never on a closed window, at most one
    /// window is activated, and it is the one with keyboard focus.
    #[test]
    fn focus_and_activation_stay_consistent(ops in prop::collection::vec(op(), 1..30)) {
        let mut harness = following();
        let socket = harness.socket_path();
        harness.run_with_remote(move |server| {
            let mut clients: Vec<(TestClient, TestWindow)> = Vec::new();
            for op in ops {
                match op {
                    Op::Open if clients.len() < 6 => {
                        let mut c = TestClient::connect(&socket);
                        let w = c.map_window();
                        clients.push((c, w));
                    }
                    Op::Open => {}
                    Op::Close(i) if !clients.is_empty() => {
                        let (mut c, w) = clients.remove(i % clients.len());
                        c.destroy(w);
                        c.roundtrip();
                    }
                    Op::Close(_) => {}
                    Op::PointerTo(x, y) => to(&server, x, y),
                    Op::Keyboard(key) => server.on_server(move |h| h.chord(&[ALT, key])),
                    Op::Swap(key) => server.on_server(move |h| h.chord(&[ALT, SHIFT, key])),
                    Op::Click => server.on_server(|h| {
                        h.button(BTN_LEFT, ButtonState::Pressed);
                        h.button(BTN_LEFT, ButtonState::Released);
                    }),
                    Op::Float => server.on_server(|h| h.chord(&[ALT, super::harness::keys::F])),
                }
                for (c, _) in clients.iter_mut() {
                    c.roundtrip();
                }
                server.on_server(|h| {
                    let state = &h.server.state;
                    let layout = &state.windows.layout;
                    if let Some(f) = layout.focused() {
                        assert!(layout.contains(f), "focus on a closed window");
                        assert!(state.windows.by_id(f).is_some());
                    }
                    let active = state.windows.activated_for_tests();
                    assert!(active.len() <= 1, "two windows activated: {active:?}");
                    let keyboard = h.keyboard_focus().map(WindowId);
                    if let Some(a) = active.first() {
                        assert_eq!(Some(*a), keyboard, "activated is the keyboard focus");
                    }
                    layout.check_invariants();
                });
                let on = clients.iter().filter(|(c, w)| c.activated(w)).count();
                assert!(on <= 1, "two clients think they are activated");
            }
        });
    }
}

/// p50 / p95 / p99 in µs.
fn percentiles(mut v: Vec<u64>) -> String {
    v.sort_unstable();
    let p = |q: f64| v[((v.len() as f64 - 1.0) * q) as usize] as f64 / 1000.0;
    format!(
        "{:.2} / {:.2} / {:.2} µs (n {})",
        p(0.5),
        p(0.95),
        p(0.99),
        v.len()
    )
}

/// The cost of a motion event and of a focus change (owner, M19a: under 10 µs p99),
/// 10 tiled and 10 floating windows at 5120x1440 with the real gaps and radius. Run in
/// release: `cargo test --release -p sash-wm -- --ignored bench_pointer_focus --nocapture`.
#[test]
#[ignore = "benchmark"]
fn bench_pointer_focus() {
    use std::time::Instant;
    for floating in [false, true] {
        let mut harness = following();
        harness.server.state.windows.layout.set_gaps(Gaps::DEFAULT);
        harness
            .server
            .state
            .windows
            .layout
            .set_radius(crate::layout::shape::RADIUS);
        harness.server.state.set_layout_area((5120, 1440).into());
        let socket = harness.socket_path();
        harness.run_with_remote(move |server| {
            let mut clients = Vec::new();
            for _ in 0..10 {
                let mut c = TestClient::connect(&socket);
                let w = c.map_window();
                clients.push((c, w));
                server.pointer_over_focused();
            }
            server.on_server(move |h| {
                if floating {
                    h.server.state.windows.layout.float_all();
                    h.server.state.arrange();
                }
                let (mut inside, mut crossing, mut changes) = (Vec::new(), Vec::new(), Vec::new());
                let n = 200_000u32;
                for i in 0..n {
                    // A sweep over the whole output, crossing every window and gap.
                    let x = f64::from((i * 7) % 5120) + 0.25;
                    let y = f64::from((i * 3) % 1440) + 0.5;
                    let before = h.server.state.windows.layout.focused();
                    let t = Instant::now();
                    h.server.state.pointer_moved_to((x, y).into(), i);
                    let ns = t.elapsed().as_nanos() as u64;
                    if h.server.state.windows.layout.focused() != before {
                        changes.push(ns);
                        crossing.push(ns);
                    } else {
                        inside.push(ns);
                        crossing.push(ns);
                    }
                    if i % 256 == 0 {
                        h.server.flush();
                    }
                }
                let scene = if floating { "10 floating" } else { "10 tiled" };
                println!("{scene}: every motion {}", percentiles(crossing));
                println!(
                    "{scene}: motion without a focus change {}",
                    percentiles(inside)
                );
                println!(
                    "{scene}: motion with a focus change {}",
                    percentiles(changes)
                );
            });
            drop(clients);
        });
    }
}
