// SPDX-License-Identifier: Apache-2.0

//! Gaps end to end (owner, M18a; M15b before it): real clients on the headless harness
//! (output 800x600, modifier Alt) with the gaps the compositor uses, outer 13 and inner
//! 8 (the harness starts at none for the tests written before gaps). Every edge has
//! its 1 px border, so a client gets its visible rectangle minus 2 on each axis.

use smithay::backend::input::ButtonState;
use wayland_client::Proxy;

use super::{client::TestClient, harness::Harness};
use crate::{
    layout::{
        Rect, WindowId, ZoneId,
        gaps::{Gaps, INNER_GAP, OUTER_GAP},
    },
    seat::BTN_LEFT,
};

fn gapped() -> Harness {
    let mut harness = Harness::new();
    harness.server.state.windows.layout.set_gaps(Gaps::DEFAULT);
    harness
}

/// Two tiles 8 px apart and 13 px from the output's edges: each is configured to its
/// visible rectangle minus its border; the layout rectangles stay exact.
#[test]
fn tiles_are_configured_to_their_visible_rectangles() {
    assert_eq!((OUTER_GAP, INNER_GAP), (13, 8));
    let mut harness = gapped();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut a = TestClient::connect(&socket);
        let wa = a.map_window();
        server.pointer_over_focused();
        let mut b = TestClient::connect(&socket);
        let wb = b.map_window();
        a.roundtrip();
        // 400 - 13 - 4 = 383 wide, 600 - 26 = 574 high, minus the border (2 each way).
        assert_eq!(a.config(&wa).expect("configured").size, (381, 572));
        assert_eq!(b.config(&wb).expect("configured").size, (381, 572));
        server.on_server(|h| {
            let layout = &h.server.state.windows.layout;
            assert_eq!(
                h.rect_of(1),
                Some(Rect::new(0, 0, 400, 600)),
                "layout exact"
            );
            assert_eq!(
                layout.shown_rect(WindowId(1)),
                Some(Rect::new(13, 13, 383, 574))
            );
            assert_eq!(
                layout.shown_rect(WindowId(2)),
                Some(Rect::new(404, 13, 383, 574))
            );
        });
    });
}

/// The gap belongs to nothing: the pointer there is over no surface, a click there
/// focuses nothing and reaches no client. Rule A still uses the layout rectangles.
#[test]
fn the_gap_belongs_to_nothing_and_rule_a_is_unchanged() {
    let mut harness = gapped();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut a = TestClient::connect(&socket);
        let _wa = a.map_window();
        server.pointer_over_focused();
        let mut b = TestClient::connect(&socket);
        let wb = b.map_window();
        server.on_server(|h| {
            h.pointer_to(398.0, 300.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.button(BTN_LEFT, ButtonState::Released);
            assert_eq!(
                h.keyboard_focus(),
                Some(2),
                "a click in the gap focuses nothing"
            );
        });
        a.roundtrip();
        b.roundtrip();
        assert_eq!(a.events.pointer_focus, None, "no pointer focus in the gap");
        assert_eq!(b.events.pointer_focus, None);
        assert!(a.events.buttons_pressed.is_empty() && b.events.buttons_pressed.is_empty());
        assert_eq!(b.events.keyboard_focus, Some(wb.surface.id()));
        // A new window with the pointer in the gap at x 398 splits tile 1 (its layout
        // rectangle is 0..400), as without gaps.
        let mut c = TestClient::connect(&socket);
        let _wc = c.map_window();
        server.on_server(|h| {
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 400, 300)));
            assert_eq!(h.rect_of(3), Some(Rect::new(0, 300, 400, 300)));
        });
    });
}

/// Snapped and maximized windows follow the rule; plain floating and fullscreen windows
/// do not.
#[test]
fn snapped_and_maximized_windows_keep_gaps_floating_and_fullscreen_do_not() {
    let mut harness = gapped();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut a = TestClient::connect(&socket);
        let w = a.map_window();
        server.on_server(|h| {
            let layout = &mut h.server.state.windows.layout;
            assert!(layout.snap(WindowId(1), ZoneId::LEFT));
            h.server.state.arrange();
        });
        a.roundtrip();
        // The left half 400 x 600: 13 on the outer edges, 4 on the right; minus the
        // border.
        assert_eq!(a.config(&w).expect("configured").size, (381, 572));
        server.on_server(|h| {
            let layout = &mut h.server.state.windows.layout;
            assert!(layout.snap(WindowId(1), ZoneId::FULL));
            h.server.state.arrange();
        });
        a.roundtrip();
        assert_eq!(
            a.config(&w).expect("configured").size,
            (772, 572),
            "maximized: the usable area minus 13 on every side"
        );
        server.on_server(|h| {
            let layout = &mut h.server.state.windows.layout;
            layout.unsnap_restore(WindowId(1));
            layout.set_floating_rect(WindowId(1), Rect::new(100, 100, 300, 200));
            h.server.state.arrange();
            assert_eq!(
                h.server.state.windows.layout.shown_rect(WindowId(1)),
                Some(Rect::new(100, 100, 300, 200)),
                "plain floating: untouched"
            );
        });
        w.toplevel.set_fullscreen(None);
        a.roundtrip();
        assert_eq!(
            a.config(&w).expect("configured").size,
            (800, 600),
            "fullscreen: whole"
        );
    });
}

/// A tile's resize zone lies inside its visible rectangle: a press just inside the
/// visible right edge (not in the gap) moves the split.
#[test]
fn resize_zones_sit_inside_the_visible_rectangle() {
    let mut harness = gapped();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut a = TestClient::connect(&socket);
        let _wa = a.map_window();
        server.pointer_over_focused();
        let mut b = TestClient::connect(&socket);
        let _wb = b.map_window();
        server.on_server(|h| {
            // Tile 1's visible right edge is at x 396; its zone is 390..396.
            h.pointer_to(394.0, 300.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.pointer_to(444.0, 300.0);
            h.button(BTN_LEFT, ButtonState::Released);
            assert_eq!(
                h.rect_of(1),
                Some(Rect::new(0, 0, 450, 600)),
                "the split moved by 50"
            );
        });
    });
}

/// With the gaps and radius 13 (owner, M18a): the outermost pixel of a tile's rounded
/// corner falls through to what is below (here the background in the gap), and the
/// corner's resize zone still works inside the shape.
#[test]
fn a_cut_off_corner_falls_through_and_the_corner_zone_works() {
    let mut harness = gapped();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut clients = Vec::new();
        for n in 0..3 {
            if n > 0 {
                server.pointer_over_focused();
            }
            clients.push(super::decorations::decorated(&socket, None));
        }
        server.on_server(|h| {
            h.server
                .state
                .windows
                .layout
                .set_radius(crate::layout::shape::RADIUS);
            let state = &h.server.state;
            let layout = &state.windows.layout;
            // 1 | 2 / 3; tile 2 (top right) shows 404..787 x 13..296.
            assert_eq!(
                layout.shown_rect(WindowId(2)),
                Some(Rect::new(404, 13, 383, 283))
            );
            let (x, y) = (404, 295);
            assert_eq!(
                layout.window_at(x, y),
                None,
                "the cut-off corner falls through"
            );
            assert_eq!(
                state.decoration_at((x as f64 + 0.5, y as f64 + 0.5).into()),
                None
            );
            // Four pixels in on each axis is inside the 13 px arc: the corner's zone.
            let (x, y) = (408, 291);
            assert_eq!(layout.window_at(x, y), Some(WindowId(2)));
            assert_eq!(
                state.decoration_at((x as f64 + 0.5, y as f64 + 0.5).into()),
                Some((
                    WindowId(2),
                    crate::layout::decor::Part::Resize(crate::layout::Edges {
                        left: true,
                        top: false,
                        right: false,
                        bottom: true,
                    })
                ))
            );
        });
        drop(clients);
    });
}
