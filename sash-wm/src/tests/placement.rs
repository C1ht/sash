// SPDX-License-Identifier: Apache-2.0

//! Placement at the pointer and the minimum tile (owner, M19b) and dropping a dragged
//! tile (owner, M19c), end to end: real clients on the headless harness with the real
//! gaps (outer 13, inner 8). After every case the layout's invariants hold (every tile
//! at least 233 x 144 visible) and the gaps are exact in physical pixels at scales 1,
//! 1.25, 1.5, 1.75 and 2.
//!
//! Four columns on a 3200 x 1200 output, left to right windows 1, 4, 2, 3, each cell
//! 800 wide.

use smithay::backend::input::{ButtonState, KeyState};
use wayland_client::Connection;
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::Layer, zwlr_layer_surface_v1::KeyboardInteractivity,
};

use super::{
    client::{TestClient, TestWindow, layer::LayerSpec},
    harness::{
        Harness, Remote,
        keys::{ALT, CTRL, L, SHIFT},
    },
};
use crate::{
    layout::{Mode, Rect, WindowId, gaps::Gaps, proptests::check_physical},
    seat::{BTN_LEFT, BTN_RIGHT},
};

fn gapped(w: i32, h: i32) -> Harness {
    let mut harness = Harness::new();
    harness.server.state.windows.layout.set_gaps(Gaps::DEFAULT);
    harness.server.state.set_layout_area((w, h).into());
    harness
}

/// The layout's invariants and the exact physical gaps.
fn check(server: &Remote) {
    server.on_server(|h| {
        let layout = &h.server.state.windows.layout;
        layout.check_invariants();
        check_physical(layout);
    });
}

fn cell(server: &Remote, n: u64) -> Option<Rect> {
    let (tx, rx) = std::sync::mpsc::channel();
    server.on_server(move |h| {
        let layout = &h.server.state.windows.layout;
        let tiled = layout.mode(WindowId(n)) == Some(Mode::Tiled);
        let _ = tx.send(tiled.then(|| layout.rect_of(WindowId(n))).flatten());
    });
    rx.recv().unwrap()
}

fn mode(server: &Remote, n: u64) -> Option<Mode> {
    let (tx, rx) = std::sync::mpsc::channel();
    server.on_server(move |h| {
        let _ = tx.send(h.server.state.windows.layout.mode(WindowId(n)));
    });
    rx.recv().unwrap()
}

/// Opens a window with the pointer at (x, y).
fn open_at(server: &Remote, socket: &std::path::Path, x: f64, y: f64) -> (TestClient, TestWindow) {
    server.on_server(move |h| h.pointer_to(x, y));
    let mut c = TestClient::connect(socket);
    let w = c.map_window();
    check(server);
    (c, w)
}

/// Windows 1, 4, 2, 3 in four 800 px columns (each placed by a right-zone pointer).
fn four_columns(server: &Remote, socket: &std::path::Path) -> Vec<(TestClient, TestWindow)> {
    let mut c1 = TestClient::connect(socket);
    let w1 = c1.map_window(); // the pointer never moved: fills the area
    let mut out = vec![(c1, w1)];
    out.push(open_at(server, socket, 3100.0, 600.0)); // 1 | 2
    out.push(open_at(server, socket, 3100.0, 600.0)); // 1 | (2 | 3)
    out.push(open_at(server, socket, 1500.0, 600.0)); // (1 | 4) | (2 | 3)
    for (n, x) in [(1, 0), (4, 800), (2, 1600), (3, 2400)] {
        assert_eq!(
            cell(server, n),
            Some(Rect::new(x, 0, 800, 1200)),
            "column of {n}"
        );
    }
    out
}

#[test]
fn each_zone_of_a_column() {
    // (the pointer, the column's window, its cell after, the new window's cell)
    let cases = [
        // The top half of column 3: the new window on top.
        (
            (2000.0, 100.0),
            2,
            Rect::new(1600, 600, 800, 600),
            Rect::new(1600, 0, 800, 600),
        ),
        // The left part of column 2: the new window takes the left half.
        (
            (820.0, 600.0),
            4,
            Rect::new(1200, 0, 400, 1200),
            Rect::new(800, 0, 400, 1200),
        ),
        // The right part of column 1: the right half.
        (
            (780.0, 600.0),
            1,
            Rect::new(0, 0, 400, 1200),
            Rect::new(400, 0, 400, 1200),
        ),
        // The bottom of column 4: the new window below.
        (
            (2800.0, 1180.0),
            3,
            Rect::new(2400, 0, 800, 600),
            Rect::new(2400, 600, 800, 600),
        ),
        // The centre of column 2: the longer side (height), the new window second.
        (
            (1200.0, 600.0),
            4,
            Rect::new(800, 0, 800, 600),
            Rect::new(800, 600, 800, 600),
        ),
        // The inner gap right of column 2 (x 1596..1604; 1598 is in its cell): right.
        (
            (1598.0, 600.0),
            4,
            Rect::new(800, 0, 400, 1200),
            Rect::new(1200, 0, 400, 1200),
        ),
        // The outer gap left of column 1: left.
        (
            (5.0, 600.0),
            1,
            Rect::new(400, 0, 400, 1200),
            Rect::new(0, 0, 400, 1200),
        ),
    ];
    for (n, ((x, y), target, after, new)) in cases.into_iter().enumerate() {
        let mut harness = gapped(3200, 1200);
        let socket = harness.socket_path();
        harness.run_with_remote(move |server| {
            let _columns = four_columns(&server, &socket);
            let _new = open_at(&server, &socket, x, y);
            assert_eq!(cell(&server, 5), Some(new), "case {n}: the new window");
            assert_eq!(
                cell(&server, target),
                Some(after),
                "case {n}: window {target}"
            );
        });
    }
}

/// The pointer never moved: the focused tile, split as centre (the longer side, the new
/// window second).
#[test]
fn a_pointer_that_never_moved_splits_the_focused_tile() {
    let mut harness = gapped(3200, 1200);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut clients = Vec::new();
        for _ in 0..3 {
            let mut c = TestClient::connect(&socket);
            let w = c.map_window();
            clients.push((c, w));
            check(&server);
        }
        server.on_server(|h| assert!(!h.server.state.pointer_has_moved));
        assert_eq!(cell(&server, 1), Some(Rect::new(0, 0, 1600, 1200)));
        assert_eq!(cell(&server, 2), Some(Rect::new(1600, 0, 800, 1200)));
        assert_eq!(cell(&server, 3), Some(Rect::new(2400, 0, 800, 1200)));
    });
}

/// Over a panel (outside the usable area) or a layer surface above the windows inside
/// it: the focused tile, split as centre, not the tile beneath.
#[test]
fn over_a_panel_or_a_layer_surface_the_focused_tile_is_split() {
    let mut harness = gapped(3200, 1200);
    let stream = harness.server.state.privileged_connection_for_tests();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let _columns = four_columns(&server, &socket);
        let mut p = TestClient::from_connection(Connection::from_socket(stream).unwrap());
        let _panel = p.map_layer(&LayerSpec::top_panel(30, 0));
        let _over = p.map_layer(&LayerSpec::centered(
            Layer::Top,
            (200, 100),
            KeyboardInteractivity::None,
        ));
        p.roundtrip();
        // The panel took 30 px at the top: tiles start at y 30.
        assert_eq!(cell(&server, 2), Some(Rect::new(1600, 30, 800, 1170)));
        // Focus is on window 4 (the last to map). Over the panel: 4 is split.
        let _a = open_at(&server, &socket, 2000.0, 10.0);
        assert_eq!(cell(&server, 4), Some(Rect::new(800, 30, 800, 585)));
        assert_eq!(cell(&server, 5), Some(Rect::new(800, 615, 800, 585)));
        // Over the centred layer surface (x 1500..1700, y 565..665), above column 3
        // (window 2): the focused window, 5 now, is split, not 2.
        let _b = open_at(&server, &socket, 1650.0, 600.0);
        assert_eq!(
            cell(&server, 2),
            Some(Rect::new(1600, 30, 800, 1170)),
            "untouched"
        );
        server.on_server(|h| {
            let tree = h.server.state.windows.layout.tree();
            assert_eq!(tree.sibling_leaf(WindowId(6)), Some(WindowId(5)));
        });
    });
}

/// The minimum tile: a split that does not fit tries the other axis; when that does not
/// fit either, the new window floats.
#[test]
fn the_minimum_tile_tries_the_other_axis_then_floats() {
    let mut harness = gapped(3200, 1200);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let _columns = four_columns(&server, &socket);
        // Column 2 split to the left: 5 is 400 wide at the left.
        let _a = open_at(&server, &socket, 820.0, 600.0);
        assert_eq!(cell(&server, 5), Some(Rect::new(800, 0, 400, 1200)));
        // Its left zone again: 200 px halves show 192 (under 233), so the other axis,
        // the pointer's lower half: 6 below 5.
        let _b = open_at(&server, &socket, 810.0, 900.0);
        assert_eq!(cell(&server, 5), Some(Rect::new(800, 0, 400, 600)));
        assert_eq!(cell(&server, 6), Some(Rect::new(800, 600, 400, 600)));
    });
    // A small output: no axis fits, the new window floats.
    let mut harness = gapped(800, 400);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let _w1 = c1.map_window();
        let _a = open_at(&server, &socket, 790.0, 200.0); // 1 | 2
        let _b = open_at(&server, &socket, 790.0, 200.0); // 2 is 400 x 400: 3 below
        assert_eq!(cell(&server, 2), Some(Rect::new(400, 0, 400, 200)));
        assert_eq!(cell(&server, 3), Some(Rect::new(400, 200, 400, 200)));
        // 3's left zone: halves 200 x 200 show 192 wide; stacked, 100 high shows 83.
        let _c = open_at(&server, &socket, 410.0, 300.0);
        assert_eq!(mode(&server, 4), Some(Mode::Floating), "neither axis fits");
        assert_eq!(
            cell(&server, 3),
            Some(Rect::new(400, 200, 400, 200)),
            "untouched"
        );
    });
}

/// Dividers stop at the minimum tile (233 visible), or at a client's own minimum plus
/// its border where that is larger; by keyboard and by mouse.
#[test]
fn dividers_stop_at_the_minimum_and_at_a_client_minimum() {
    let mut harness = gapped(800, 600);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let _w1 = c1.map_window();
        let (mut c2, w2) = open_at(&server, &socket, 790.0, 300.0); // 1 | 2
        server.on_server(|h| {
            // Focus 1; mod+Ctrl+Shift+L widens it in 34 px steps.
            h.server.state.focus_window(WindowId(1));
            for _ in 0..10 {
                h.chord(&[ALT, CTRL, SHIFT, L]);
            }
        });
        check(&server);
        // 2's cell is 250: 250 - 4 - 13 = 233 visible.
        assert_eq!(cell(&server, 2), Some(Rect::new(550, 0, 250, 600)));
        // Back to the middle, then client 2 asks for at least 300 x 0.
        server.on_server(|h| {
            for _ in 0..5 {
                h.chord(&[ALT, CTRL, SHIFT, super::harness::keys::H]);
            }
        });
        w2.toplevel.set_min_size(300, 0);
        c2.ack_and_commit(&w2);
        c2.roundtrip();
        // A mouse resize of 1 from its right edge, far to the right.
        server.on_server(|h| {
            let r = h.server.state.windows.layout.rect_of(WindowId(1)).unwrap();
            h.pointer_to(f64::from(r.right() - 20), 300.0);
            h.key(ALT, KeyState::Pressed);
            h.button(BTN_RIGHT, ButtonState::Pressed);
            h.pointer_to(780.0, 300.0);
            h.button(BTN_RIGHT, ButtonState::Released);
            h.key(ALT, KeyState::Released);
        });
        check(&server);
        // 300 + 2 (the border) visible: a 319 cell.
        assert_eq!(cell(&server, 2), Some(Rect::new(481, 0, 319, 600)));
    });
}

/// The usable area shrinks: the newest tiles float until the rest fit; with room again
/// they return, newest-floated first, unless touched while floating.
#[test]
fn a_shrinking_area_floats_the_newest_tiles_and_they_return() {
    let mut harness = gapped(3200, 1200);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut columns = four_columns(&server, &socket);
        let before = |server: &Remote| {
            let (tx, rx) = std::sync::mpsc::channel();
            server.on_server(move |h| {
                let _ = tx.send(h.server.state.windows.layout.tree().clone());
            });
            rx.recv().unwrap()
        };
        let tree = before(&server);
        // 900 wide: four 225 px columns show 208 at most; 4, then 3 float.
        server.on_server(|h| h.server.state.set_layout_area((900, 1200).into()));
        check(&server);
        assert_eq!(mode(&server, 4), Some(Mode::Floating));
        assert_eq!(mode(&server, 3), Some(Mode::Floating));
        assert_eq!(cell(&server, 1), Some(Rect::new(0, 0, 450, 1200)));
        assert_eq!(cell(&server, 2), Some(Rect::new(450, 0, 450, 1200)));
        for (c, _) in &mut columns {
            c.roundtrip();
        }
        // Room again: 3 (floated last) returns first, then 4: the same tree.
        server.on_server(|h| h.server.state.set_layout_area((3200, 1200).into()));
        check(&server);
        assert_eq!(before(&server), tree, "the tree as it was");
        // Again, and this time 3 is clicked while floating: it stays floating, 4
        // returns.
        server.on_server(|h| h.server.state.set_layout_area((900, 1200).into()));
        check(&server);
        server.on_server(|h| {
            let r = h.server.state.windows.layout.rect_of(WindowId(3)).unwrap();
            h.server.state.windows.layout.focus(WindowId(3)); // on top
            h.server.state.arrange();
            h.pointer_to(f64::from(r.x + r.w / 2), f64::from(r.y + r.h / 2));
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.button(BTN_LEFT, ButtonState::Released);
        });
        server.on_server(|h| h.server.state.set_layout_area((3200, 1200).into()));
        check(&server);
        assert_eq!(mode(&server, 3), Some(Mode::Floating), "touched: stays");
        assert_eq!(mode(&server, 4), Some(Mode::Tiled), "untouched: back");
        // Never closed.
        server.on_server(|h| assert_eq!(h.mapped_windows(), 4));
    });
}

// Dropping a dragged tile (owner, M19c; docs/plan.md, "M19c rules").

/// The drag preview, as drawn (the visible rectangle).
fn preview(server: &Remote) -> Option<Rect> {
    let (tx, rx) = std::sync::mpsc::channel();
    server.on_server(move |h| {
        let _ = tx.send(h.server.state.overlay.preview());
    });
    rx.recv().unwrap()
}

/// What a tile in `cell` shows (the one gap function).
fn visible(server: &Remote, cell: Rect) -> Rect {
    let (tx, rx) = std::sync::mpsc::channel();
    server.on_server(move |h| {
        let _ = tx.send(h.server.state.windows.layout.inset(cell));
    });
    rx.recv().unwrap()
}

/// mod+left press at (x, y): starts dragging the window there.
fn grab(server: &Remote, x: f64, y: f64) {
    server.on_server(move |h| {
        h.pointer_to(x, y);
        h.key(ALT, KeyState::Pressed);
        h.button(BTN_LEFT, ButtonState::Pressed);
    });
}

fn release(server: &Remote) {
    server.on_server(|h| {
        h.button(BTN_LEFT, ButtonState::Released);
        h.key(ALT, KeyState::Released);
    });
    check(server);
}

fn pointer_to(server: &Remote, x: f64, y: f64) {
    server.on_server(move |h| h.pointer_to(x, y));
}

fn outline(server: &Remote) -> Vec<crate::layout::OutlineNode> {
    let (tx, rx) = std::sync::mpsc::channel();
    server.on_server(move |h| {
        let _ = tx.send(h.server.state.windows.layout.tree().outline());
    });
    rx.recv().unwrap()
}

/// A dragged tile is tiled again where it is released, by the M19b zones; the preview
/// shows exactly the rectangle it gets.
#[test]
fn a_dragged_tile_drops_into_the_zone_under_the_pointer() {
    let mut harness = gapped(3200, 1200);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let _columns = four_columns(&server, &socket);
        // Window 1 (column 1) floats on the press; 4 takes its space.
        grab(&server, 400.0, 600.0);
        assert_eq!(mode(&server, 1), Some(Mode::Floating));
        assert_eq!(cell(&server, 4), Some(Rect::new(0, 0, 1600, 1200)));
        check(&server);
        // The top half of column 3 (window 2).
        pointer_to(&server, 2000.0, 100.0);
        let top = Rect::new(1600, 0, 800, 600);
        assert_eq!(preview(&server), Some(visible(&server, top)));
        release(&server);
        assert_eq!(cell(&server, 1), Some(top));
        assert_eq!(cell(&server, 2), Some(Rect::new(1600, 600, 800, 600)));
        assert_eq!(cell(&server, 4), Some(Rect::new(0, 0, 1600, 1200)));
        assert_eq!(preview(&server), None, "gone after the release");
        // Window 3 into the left part of 4 (in its visible rectangle, past the outer
        // gap): the left half.
        grab(&server, 2800.0, 600.0);
        pointer_to(&server, 20.0, 600.0);
        let left = Rect::new(0, 0, 800, 1200);
        assert_eq!(preview(&server), Some(visible(&server, left)));
        release(&server);
        assert_eq!(cell(&server, 3), Some(left));
        assert_eq!(cell(&server, 4), Some(Rect::new(800, 0, 800, 1200)));
    });
}

/// Released inside its own visible rectangle (from the press), a tile returns to its
/// leaf: the tree is exactly as before the drag.
#[test]
fn a_release_inside_its_own_rectangle_restores_its_leaf() {
    let mut harness = gapped(3200, 1200);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let _columns = four_columns(&server, &socket);
        let before = outline(&server);
        grab(&server, 2000.0, 600.0); // window 2
        assert_eq!(cell(&server, 3), Some(Rect::new(1600, 0, 1600, 1200)));
        // Inside 2's old rectangle, and in the bottom zone of 3's (now wider) cell: by
        // the zones 2 would go below 3; its own rectangle wins.
        pointer_to(&server, 2350.0, 1100.0);
        let own = Rect::new(1600, 0, 800, 1200);
        assert_eq!(preview(&server), Some(visible(&server, own)));
        release(&server);
        assert_eq!(outline(&server), before);
        for (n, x) in [(1, 0), (4, 800), (2, 1600), (3, 2400)] {
            assert_eq!(cell(&server, n), Some(Rect::new(x, 0, 800, 1200)));
        }
    });
}

/// A snap zone wins; Shift at release floats; mod+F during the drag switches between
/// tiling and floating at release; the floating default mode keeps the old drag.
#[test]
fn snap_zones_shift_and_mod_f_float_instead() {
    // The left screen edge: the left-half zone, floating and snapped.
    let mut harness = gapped(3200, 1200);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let _columns = four_columns(&server, &socket);
        grab(&server, 2000.0, 600.0);
        pointer_to(&server, 2.0, 600.0);
        assert!(preview(&server).is_some(), "the zone's preview");
        release(&server);
        assert_eq!(mode(&server, 2), Some(Mode::Floating));
        server.on_server(|h| {
            assert!(
                h.server
                    .state
                    .windows
                    .layout
                    .snapped_zone(WindowId(2))
                    .is_some()
            );
        });
    });
    // Shift: no preview while held, floating where released.
    let mut harness = gapped(3200, 1200);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let _columns = four_columns(&server, &socket);
        grab(&server, 400.0, 600.0);
        pointer_to(&server, 2000.0, 100.0);
        assert!(preview(&server).is_some());
        server.on_server(|h| h.key(SHIFT, KeyState::Pressed));
        pointer_to(&server, 2001.0, 100.0);
        assert_eq!(preview(&server), None, "Shift held: it will float");
        server.on_server(|h| {
            h.button(BTN_LEFT, ButtonState::Released);
            h.key(SHIFT, KeyState::Released);
            h.key(ALT, KeyState::Released);
        });
        check(&server);
        assert_eq!(mode(&server, 1), Some(Mode::Floating));
        assert_eq!(
            cell(&server, 2),
            Some(Rect::new(1600, 0, 800, 1200)),
            "untouched"
        );
    });
    // mod+F: float at release, again: tile at release. Never tiled mid-drag.
    let mut harness = gapped(3200, 1200);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let _columns = four_columns(&server, &socket);
        let f = super::harness::keys::F;
        grab(&server, 400.0, 600.0);
        pointer_to(&server, 2000.0, 100.0);
        server.on_server(move |h| {
            h.key(f, KeyState::Pressed);
            h.key(f, KeyState::Released);
        });
        assert_eq!(preview(&server), None, "float at release");
        assert_eq!(mode(&server, 1), Some(Mode::Floating), "not tiled mid-drag");
        release(&server);
        assert_eq!(mode(&server, 1), Some(Mode::Floating));
        grab(&server, 2800.0, 600.0); // window 3, a tile
        pointer_to(&server, 2000.0, 1100.0);
        server.on_server(move |h| {
            for _ in 0..2 {
                h.key(f, KeyState::Pressed);
                h.key(f, KeyState::Released);
            }
        });
        assert!(preview(&server).is_some(), "tile at release again");
        assert_eq!(mode(&server, 3), Some(Mode::Floating), "still dragged");
        release(&server);
        assert_eq!(mode(&server, 3), Some(Mode::Tiled));
        assert_eq!(cell(&server, 3), Some(Rect::new(1600, 600, 1600, 600)));
    });
    // The floating default mode: a dragged tile stays floating, no preview.
    let mut harness = gapped(3200, 1200);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let _columns = four_columns(&server, &socket);
        server.on_server(|h| {
            h.server
                .state
                .windows
                .layout
                .set_default_mode(Mode::Floating)
        });
        grab(&server, 400.0, 600.0);
        pointer_to(&server, 2000.0, 100.0);
        assert_eq!(preview(&server), None);
        release(&server);
        assert_eq!(mode(&server, 1), Some(Mode::Floating));
    });
}

/// No room at the pointer (here the window's own minimum, raised after it mapped):
/// no preview, and it stays floating where it was dropped; nothing else moves.
#[test]
fn a_tile_that_does_not_fit_stays_floating_where_dropped() {
    let mut harness = gapped(800, 600);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let w1 = c1.map_window();
        let _two = open_at(&server, &socket, 790.0, 300.0); // 1 | 2
        w1.toplevel.set_min_size(500, 400);
        c1.ack_and_commit(&w1);
        c1.roundtrip();
        assert_eq!(mode(&server, 1), Some(Mode::Tiled), "stays tiled, clipped");
        grab(&server, 200.0, 300.0);
        pointer_to(&server, 600.0, 100.0);
        assert_eq!(preview(&server), None, "neither axis holds 502 x 402");
        release(&server);
        assert_eq!(mode(&server, 1), Some(Mode::Floating));
        assert_eq!(cell(&server, 2), Some(Rect::new(0, 0, 800, 600)));
    });
}
