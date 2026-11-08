// SPDX-License-Identifier: Apache-2.0

//! Server-side decorations end to end (M11b): real clients on the headless harness
//! (output 800x600, default mode tiled, modifier Alt). A floating window with a bar has
//! a 34 px bar and a 1 px border; its buttons (close, maximize) are on the left.

use smithay::{
    backend::input::ButtonState,
    input::pointer::{CursorIcon, CursorImageStatus},
};
use wayland_client::Proxy;
use wayland_protocols::xdg::{
    decoration::zv1::client::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
    shell::client::xdg_toplevel::ResizeEdge,
};

use super::{
    client::{PopupSpec, TestClient, TestWindow},
    harness::{
        Harness, Remote,
        keys::{ALT, F, LEFT},
    },
};
use crate::{
    layout::{Mode, Rect, WindowId, ZoneId, decor::Decoration},
    seat::BTN_LEFT,
};

const AREA: Rect = Rect::new(0, 0, 800, 600);
/// Where `floating` puts the window (outer rectangle).
pub(super) const FLOAT: Rect = Rect::new(100, 100, 400, 300);

/// A client that negotiates decorations (asking for `mode`) before its first commit.
pub(super) fn decorated(
    socket: &std::path::Path,
    mode: Option<DecorationMode>,
) -> (
    TestClient,
    TestWindow,
    wayland_protocols::xdg::decoration::zv1::client::zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
){
    let mut c = TestClient::connect(socket);
    let w = c.create_window_without_commit();
    let decoration = c.decorate(&w, mode);
    w.surface.commit();
    let seen = c.events.configures;
    c.dispatch_until("initial configure", |e| e.configures > seen);
    c.ack_and_commit(&w);
    (c, w, decoration)
}

/// Floats window `id` at FLOAT.
pub(super) fn floating(server: &Remote, id: u64) {
    server.on_server(move |h| {
        h.chord(&[ALT, F]);
        h.server
            .state
            .windows
            .layout
            .set_floating_rect(WindowId(id), FLOAT);
        h.server.state.arrange();
    });
}

fn click(server: &Remote, x: f64, y: f64) {
    server.on_server(move |h| {
        h.pointer_to(x, y);
        h.button(BTN_LEFT, ButtonState::Pressed);
        h.button(BTN_LEFT, ButtonState::Released);
    });
}

pub(super) fn size(c: &TestClient, w: &TestWindow) -> (i32, i32) {
    c.config(w).expect("configured").size
}

#[test]
fn a_negotiating_client_is_answered_server_side_also_when_it_asks_for_client_side() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w, decoration) = decorated(&socket, Some(DecorationMode::ClientSide));
        assert_eq!(
            c.decoration_modes(&decoration),
            [DecorationMode::ServerSide]
        );
        // Tiled, alone: no border (every edge is on the output boundary), no bar.
        assert_eq!(size(&c, &w), (798, 598));
        // Floating: bar and border, in one configure.
        let before = c.events.configures;
        server.on_server(|h| h.chord(&[ALT, F]));
        c.roundtrip();
        assert_eq!(c.events.configures, before + 1, "one configure");
        assert_eq!(size(&c, &w), (798, 564));
        server.on_server(|h| assert_eq!(h.rect_of(1), Some(AREA), "the outer rectangle"));
        // Asking again, for either mode: still server-side.
        decoration.set_mode(DecorationMode::ClientSide);
        c.roundtrip();
        decoration.unset_mode();
        c.roundtrip();
        assert!(
            c.decoration_modes(&decoration)
                .iter()
                .all(|m| *m == DecorationMode::ServerSide)
        );
        // Destroying the decoration object: the client draws its own again, so the
        // bar goes (the border stays).
        decoration.destroy();
        c.roundtrip();
        assert_eq!(size(&c, &w), (798, 598));
    });
}

#[test]
fn a_client_that_does_not_negotiate_keeps_its_own_decorations() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        let w = c.map_window();
        server.on_server(|h| h.chord(&[ALT, F]));
        c.roundtrip();
        assert_eq!(size(&c, &w), (798, 598), "no bar, the border only");
        assert!(c.events.decoration_modes.is_empty());
        server.on_server(|h| {
            assert_eq!(
                h.server.state.windows.layout.decoration(WindowId(1)),
                Decoration::BORDER
            );
        });
    });
}

/// One state change, made by a key on the server or a request from the client.
type Step = dyn Fn(&TestWindow, &Remote);

/// Tiled, floating, snapped, fullscreen and back: each change is one configure with
/// the new size; the decoration mode was sent once and never changes.
#[test]
fn mode_changes_send_one_configure_each() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w, decoration) = decorated(&socket, None);
        let steps: [(&str, Box<Step>, (i32, i32)); 5] = [
            (
                "float",
                Box::new(|_, s| s.on_server(|h| h.chord(&[ALT, F]))),
                (798, 564),
            ),
            (
                "snap left",
                Box::new(|_, s| s.on_server(|h| h.chord(&[ALT, LEFT]))),
                (398, 564),
            ),
            (
                "fullscreen",
                Box::new(|w, _| w.toplevel.set_fullscreen(None)),
                (800, 600),
            ),
            (
                "unfullscreen",
                Box::new(|w, _| w.toplevel.unset_fullscreen()),
                (398, 564),
            ),
            (
                "tile",
                Box::new(|_, s| s.on_server(|h| h.chord(&[ALT, F]))),
                (798, 598),
            ),
        ];
        for (what, step, expected) in steps {
            let before = c.events.configures;
            step(&w, &server);
            c.roundtrip();
            server.on_server(|_| {});
            c.roundtrip();
            assert_eq!(c.events.configures, before + 1, "{what}: one configure");
            assert_eq!(size(&c, &w), expected, "{what}");
            c.ack_and_commit(&w);
        }
        assert_eq!(
            c.decoration_modes(&decoration),
            [DecorationMode::ServerSide]
        );
    });
}

#[test]
fn pointer_events_over_decorations_never_reach_the_client() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w, _d) = decorated(&socket, None);
        floating(&server, 1);
        c.roundtrip();
        c.ack_and_commit(&w);
        server.on_server(|h| h.pointer_to(300.0, 300.0));
        c.roundtrip();
        assert_eq!(
            c.events.pointer_focus,
            Some(w.surface.id()),
            "the client area"
        );
        // The bar, a button, the border, a resize zone reaching into the client area:
        // the client gets a leave and nothing else.
        for (x, y) in [
            (300.0, 120.0),
            (115.0, 115.0),
            (100.0, 300.0),
            (104.0, 300.0),
        ] {
            let buttons = c.events.buttons_pressed.len();
            server.on_server(move |h| {
                h.pointer_to(x, y);
                h.button(BTN_LEFT, ButtonState::Pressed);
                h.pointer_to(x, y);
                h.button(BTN_LEFT, ButtonState::Released);
                // Put the window back (a bar press may have moved nothing, a zone
                // resized nothing: no motion happened).
                assert_eq!(h.rect_of(1), Some(FLOAT), "at ({x}, {y})");
            });
            c.roundtrip();
            assert_eq!(c.events.pointer_focus, None, "left at ({x}, {y})");
            assert_eq!(
                c.events.buttons_pressed.len(),
                buttons,
                "no button at ({x}, {y})"
            );
        }
        server.on_server(|h| h.pointer_to(300.0, 300.0));
        c.roundtrip();
        assert_eq!(c.events.pointer_focus, Some(w.surface.id()), "back in");
    });
}

#[test]
fn the_close_button_closes_on_release_over_it() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w, _d) = decorated(&socket, None);
        floating(&server, 1);
        c.roundtrip();
        c.ack_and_commit(&w);
        // Pressed, then released elsewhere: nothing.
        server.on_server(|h| {
            h.pointer_to(116.0, 116.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.pointer_to(300.0, 300.0);
            h.button(BTN_LEFT, ButtonState::Released);
        });
        c.roundtrip();
        assert!(!c.events.close_requested);
        // A click on it: what mod+Q does.
        click(&server, 116.0, 116.0);
        c.roundtrip();
        assert!(c.events.close_requested);
    });
}

#[test]
fn the_maximize_button_toggles_the_top_zone_and_restores() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w, _d) = decorated(&socket, None);
        floating(&server, 1);
        c.roundtrip();
        c.ack_and_commit(&w);
        // Maximize: the second button, at (135, 101) + 34x34 from the outer corner.
        click(&server, 150.0, 116.0);
        c.roundtrip();
        let config = c.config(&w).expect("configured");
        // The whole output: the bar, no border (all edges on the output boundary).
        assert_eq!((config.size, config.maximized), ((798, 564), true));
        server.on_server(|h| {
            assert_eq!(h.rect_of(1), Some(AREA));
            assert_eq!(
                h.server.state.windows.layout.snapped_zone(WindowId(1)),
                Some(ZoneId::FULL)
            );
        });
        // Again (the button is now at (35, 1)): restored.
        click(&server, 50.0, 16.0);
        c.roundtrip();
        assert!(!c.config(&w).expect("configured").maximized);
        server.on_server(|h| assert_eq!(h.rect_of(1), Some(FLOAT)));
        // From a snapped half: maximized, then restored to the pre-snap rectangle.
        server.on_server(|h| {
            h.chord(&[ALT, LEFT]);
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 400, 600)));
        });
        click(&server, 50.0, 16.0);
        server.on_server(|h| assert_eq!(h.rect_of(1), Some(AREA)));
        click(&server, 50.0, 16.0);
        server.on_server(|h| assert_eq!(h.rect_of(1), Some(FLOAT), "the pre-snap rectangle"));
    });
}

/// The slot reserved for minimize (E5, owner M18b): no button, no hover square and no
/// action of its own; it is bar: a click focuses and raises, a drag moves the window.
#[test]
fn the_reserved_slot_is_bar() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w, _d) = decorated(&socket, None);
        floating(&server, 1);
        c.roundtrip();
        c.ack_and_commit(&w);
        // The third slot: x 169..203 (bar from 101, 34 px slots), below the top zone.
        let (x, y) = (186.0, 120.0);
        server.on_server(move |h| {
            let at = (x, y).into();
            assert_eq!(
                h.server.state.decoration_at(at),
                Some((WindowId(1), crate::layout::decor::Part::Bar))
            );
            h.pointer_to(x, y);
            let look = h.server.state.decoration_look(WindowId(1), true);
            assert_eq!(look.hover, None, "no hover square");
        });
        click(&server, x, y);
        c.roundtrip();
        assert!(!c.events.close_requested, "not close");
        server.on_server(move |h| {
            assert_eq!(h.rect_of(1), Some(FLOAT), "not maximize, not moved");
            assert!(!h.server.state.windows.layout.is_maximized(WindowId(1)));
            assert_eq!(h.server.state.windows.layout.focused(), Some(WindowId(1)));
            // A drag from it moves the window.
            h.pointer_to(x, y);
            h.button(BTN_LEFT, ButtonState::Pressed);
            assert!(h.pointer_grabbed(), "a move grab");
            h.pointer_to(x + 30.0, y + 20.0);
            h.button(BTN_LEFT, ButtonState::Released);
            assert_eq!(h.rect_of(1), Some(Rect::new(130, 120, 400, 300)));
        });
        c.roundtrip();
        assert_eq!(c.events.buttons_pressed, Vec::<u32>::new());
    });
}

#[test]
fn a_bar_drag_moves_and_snaps_like_mod_drag() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w, _d) = decorated(&socket, None);
        floating(&server, 1);
        c.roundtrip();
        c.ack_and_commit(&w);
        server.on_server(|h| {
            h.pointer_to(300.0, 120.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            assert!(h.pointer_grabbed(), "a move grab");
            h.pointer_to(350.0, 170.0);
            assert_eq!(h.rect_of(1), Some(Rect::new(150, 150, 400, 300)));
            // To the left edge: the left-half zone, previewed, then snapped.
            h.pointer_to(2.0, 300.0);
            assert!(h.server.state.overlay.preview().is_some());
            h.button(BTN_LEFT, ButtonState::Released);
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 400, 600)));
            assert_eq!(
                h.server.state.windows.layout.snapped_zone(WindowId(1)),
                Some(ZoneId::LEFT)
            );
        });
        c.roundtrip();
        assert_eq!(c.events.buttons_pressed, Vec::<u32>::new());
    });
}

/// Every zone of a floating window: the cursor shape on hover, then a drag by (20, 20)
/// moves exactly that zone's edges; min and max sizes hold.
#[test]
fn resize_from_each_zone_with_min_and_max() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        let w = c.create_window_without_commit();
        let _d = c.decorate(&w, None);
        w.toplevel.set_min_size(100, 100);
        w.toplevel.set_max_size(500, 0);
        w.surface.commit();
        c.dispatch_until("initial configure", |e| e.configures >= 1);
        c.ack_and_commit(&w);
        floating(&server, 1);
        c.roundtrip();
        let (l, t, r, b) = (FLOAT.x, FLOAT.y, FLOAT.right() - 1, FLOAT.bottom() - 1);
        let cases = [
            ((l, 300), CursorIcon::WResize, Rect::new(120, 100, 380, 300)),
            ((r, 300), CursorIcon::EResize, Rect::new(100, 100, 420, 300)),
            ((300, t), CursorIcon::NResize, Rect::new(100, 120, 400, 280)),
            ((300, b), CursorIcon::SResize, Rect::new(100, 100, 400, 320)),
            ((l, t), CursorIcon::NwResize, Rect::new(120, 120, 380, 280)),
            ((r, t), CursorIcon::NeResize, Rect::new(100, 120, 420, 280)),
            ((l, b), CursorIcon::SwResize, Rect::new(120, 100, 380, 320)),
            ((r, b), CursorIcon::SeResize, Rect::new(100, 100, 420, 320)),
        ];
        for ((x, y), icon, expected) in cases {
            server.on_server(move |h| {
                let (x, y) = (x as f64, y as f64);
                h.pointer_to(x, y);
                assert_eq!(
                    h.server.state.cursor.image(),
                    CursorImageStatus::Named(icon),
                    "cursor at ({x}, {y})"
                );
                h.button(BTN_LEFT, ButtonState::Pressed);
                h.pointer_to(x + 20.0, y + 20.0);
                h.button(BTN_LEFT, ButtonState::Released);
                assert_eq!(h.rect_of(1), Some(expected), "{icon:?}");
                let layout = &mut h.server.state.windows.layout;
                layout.set_floating_rect(WindowId(1), FLOAT);
                h.server.state.arrange();
            });
        }
        // Max width 500 (outer 502) and min height 100 (outer 136 with the bar).
        server.on_server(move |h| {
            h.pointer_to(r as f64, b as f64);
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.pointer_to(r as f64 + 250.0, b as f64 - 280.0);
            h.button(BTN_LEFT, ButtonState::Released);
            assert_eq!(h.rect_of(1), Some(Rect::new(100, 100, 502, 136)));
        });
        // Leaving the zone for the client area: the client's cursor rules again
        // (the arrow until it sets one).
        server.on_server(|h| {
            h.pointer_to(300.0, 300.0);
            assert_eq!(
                h.server.state.cursor.image(),
                CursorImageStatus::Named(CursorIcon::Default)
            );
        });
    });
}

/// A tile's zones are on its inner edges only and move the split; an edge on the
/// output boundary is plain border.
#[test]
fn tiled_zones_move_the_split() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        let _w1 = c1.map_window();
        let mut c2 = TestClient::connect(&socket);
        let _w2 = c2.map_window(); // 1 | 2
        server.on_server(|h| {
            assert_eq!(h.mode_of(1), Some(Mode::Tiled));
            h.pointer_to(397.0, 300.0);
            assert_eq!(
                h.server.state.cursor.image(),
                CursorImageStatus::Named(CursorIcon::EResize)
            );
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.pointer_to(447.0, 300.0);
            h.button(BTN_LEFT, ButtonState::Released);
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 450, 600)));
            assert_eq!(h.rect_of(2), Some(Rect::new(450, 0, 350, 600)));
            // Window 2's left zone moves the same split back.
            h.pointer_to(452.0, 300.0);
            assert_eq!(
                h.server.state.cursor.image(),
                CursorImageStatus::Named(CursorIcon::WResize)
            );
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.pointer_to(402.0, 300.0);
            h.button(BTN_LEFT, ButtonState::Released);
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 400, 600)));
            // The output boundary: the border (owner, M18a: every edge has one) but no
            // zone (zones unchanged); the pixel inside the border is the client's.
            h.pointer_to(0.0, 300.0);
            assert_eq!(
                h.server.state.cursor.image(),
                CursorImageStatus::Named(CursorIcon::Default)
            );
            h.pointer_to(1.0, 300.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            // The client's implicit grab, not a move or resize.
            assert_eq!(h.server.state.dragged_window(), None);
            h.pointer_to(50.0, 300.0);
            h.button(BTN_LEFT, ButtonState::Released);
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 400, 600)));
            assert_eq!(h.mode_of(1), Some(Mode::Tiled));
        });
        c1.roundtrip();
        assert_eq!(
            c1.events.buttons_pressed.len(),
            1,
            "only the press inside the output-edge border reached the client"
        );
    });
}

#[test]
fn a_fullscreen_window_has_no_decorations() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w, _d) = decorated(&socket, None);
        w.toplevel.set_fullscreen(None);
        c.roundtrip();
        assert_eq!(size(&c, &w), (800, 600));
        c.ack_and_commit(&w);
        server.on_server(|h| {
            assert_eq!(
                h.server.state.windows.layout.decoration(WindowId(1)),
                Decoration::NONE
            );
            h.pointer_to(0.0, 0.0);
        });
        c.roundtrip();
        assert_eq!(
            c.events.pointer_focus,
            Some(w.surface.id()),
            "the very corner"
        );
    });
}

/// A popup drawn over its window's bar gets the pointer there (popups are above the
/// decorations); a client resize request still works on a decorated window.
#[test]
fn popups_over_the_bar_and_client_requests() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w, _d) = decorated(&socket, None);
        floating(&server, 1);
        c.roundtrip();
        c.ack_and_commit(&w);
        // The client area starts at (101, 135); a 50x40 popup 30 px above it reaches
        // over the bar and the close button.
        let spec = PopupSpec {
            anchor_rect: (10, -40, 10, 10),
            ..PopupSpec::default()
        };
        let popup = c.open_popup(&w.xdg_surface, spec, None);
        assert_eq!(c.popup_config(&popup), Some((10, -30, 50, 40)));
        server.on_server(|h| h.pointer_to(120.0, 110.0));
        c.roundtrip();
        assert_eq!(c.events.pointer_focus, Some(popup.surface.id()));
        // Client move request from a press in the client area: it moves.
        server.on_server(|h| {
            h.pointer_to(300.0, 300.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
        });
        c.roundtrip();
        c.request_resize(&w, ResizeEdge::Right);
        server.on_server(|h| {
            h.pointer_to(330.0, 300.0);
            assert_eq!(h.rect_of(1), Some(Rect::new(100, 100, 430, 300)));
            h.button(BTN_LEFT, ButtonState::Released);
        });
    });
}

/// The snap bands are measured from the true output edge, whatever borders windows have
/// there (owner, after M11b): during a bar drag the pointer at 0..7 px from the left or
/// top edge previews that zone, at 8 px it does not; corners within 55 px of two edges.
/// Then a window snapped to the left half has no border on its output edges, and a
/// press at x = 0 goes to its client.
#[test]
fn snap_bands_trigger_at_the_true_output_edge() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w, _d) = decorated(&socket, None);
        floating(&server, 1);
        c.roundtrip();
        c.ack_and_commit(&w);
        server.on_server(|h| {
            h.pointer_to(300.0, 120.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            let preview = |h: &mut Harness, x: f64, y: f64| {
                h.pointer_to(x, y);
                h.server.state.overlay.preview()
            };
            let left = Some(Rect::new(0, 0, 400, 600));
            assert_eq!(preview(h, 0.0, 300.0), left, "x = 0");
            assert_eq!(preview(h, 7.0, 300.0), left, "x = 7");
            assert_eq!(preview(h, 8.0, 300.0), None, "x = 8");
            assert_eq!(preview(h, 400.0, 0.0), Some(AREA), "y = 0: maximize");
            assert_eq!(preview(h, 400.0, 7.0), Some(AREA), "y = 7");
            assert_eq!(preview(h, 400.0, 8.0), None, "y = 8");
            let top_left = Some(Rect::new(0, 0, 400, 300));
            assert_eq!(preview(h, 0.0, 0.0), top_left, "the corner pixel");
            assert_eq!(preview(h, 54.0, 54.0), top_left, "54 px from both edges");
            assert_eq!(preview(h, 55.0, 300.0), None, "55 px: no corner, no edge");
            assert_eq!(preview(h, 799.0, 300.0), Some(Rect::new(400, 0, 400, 600)));
            // Release at the left edge: snapped to the left half.
            preview(h, 0.0, 300.0);
            h.button(BTN_LEFT, ButtonState::Released);
            assert_eq!(h.rect_of(1), left);
            let layout = &h.server.state.windows.layout;
            let decoration = layout.decoration(WindowId(1));
            assert_eq!(
                decoration.border,
                crate::layout::Edges::ALL,
                "{decoration:?}: a border on every edge (owner, M18a)"
            );
        });
        c.roundtrip();
        c.ack_and_commit(&w);
        // The client area starts inside the border on the output edge (owner, M18a).
        assert_eq!(size(&c, &w), (398, 564));
        server.on_server(|h| h.pointer_to(1.0, 300.0));
        c.roundtrip();
        assert_eq!(
            c.events.pointer_focus,
            Some(w.surface.id()),
            "x = 1 (inside the border) is the client's"
        );
    });
}

/// Rounded corners (M15b step 2): a pointer in the cut-off part of a corner falls
/// through to the window below; the corner's resize zone still works inside the shape.
#[test]
fn cut_off_corners_fall_through_and_corner_zones_work_inside_the_shape() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut below = TestClient::connect(&socket);
        let _below = below.map_window();
        let (mut c, _w, _d) = decorated(&socket, None);
        floating(&server, 2);
        c.roundtrip();
        server.on_server(|h| {
            h.server
                .state
                .windows
                .layout
                .set_radius(crate::layout::shape::RADIUS);
            let state = &h.server.state;
            let layout = &state.windows.layout;
            let (r, b) = (FLOAT.right() - 1, FLOAT.bottom() - 1);
            // The outermost pixel of the bottom-right corner is outside the shape.
            assert_eq!(layout.window_at(r, b), Some(WindowId(1)), "falls through");
            assert_eq!(
                state.decoration_at((r as f64 + 0.5, b as f64 + 0.5).into()),
                None
            );
            // Five pixels in on each axis is inside the arc: the corner's zone.
            let (x, y) = (r - 4, b - 4);
            assert_eq!(layout.window_at(x, y), Some(WindowId(2)));
            assert_eq!(
                state.decoration_at((x as f64 + 0.5, y as f64 + 0.5).into()),
                Some((
                    WindowId(2),
                    crate::layout::decor::Part::Resize(crate::layout::Edges {
                        left: false,
                        top: false,
                        right: true,
                        bottom: true,
                    })
                ))
            );
            // Without rounding the outermost pixel is the window's again.
            h.server.state.windows.layout.set_radius(0);
            assert_eq!(
                h.server.state.windows.layout.window_at(r, b),
                Some(WindowId(2))
            );
        });
    });
}
