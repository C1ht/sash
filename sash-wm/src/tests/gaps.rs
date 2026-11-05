// SPDX-License-Identifier: Apache-2.0

//! Inner gaps end to end (M15b step 1): real clients on the headless harness (output
//! 800x600, modifier Alt) with the 8 px gap the compositor uses (the harness starts at
//! 0 for the tests written before gaps).

use smithay::backend::input::ButtonState;
use wayland_client::Proxy;

use super::{client::TestClient, harness::Harness};
use crate::{
    layout::{GAP, Rect, WindowId, ZoneId},
    seat::BTN_LEFT,
};

fn gapped() -> Harness {
    let mut harness = Harness::new();
    harness.server.state.windows.layout.set_gap(GAP);
    harness
}

/// Two tiles 8 px apart: each is configured to its visible rectangle minus its border
/// (1 px on the interior edge); the layout rectangles stay exact.
#[test]
fn tiles_are_configured_to_their_visible_rectangles() {
    assert_eq!(GAP, 8);
    let mut harness = gapped();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut a = TestClient::connect(&socket);
        let wa = a.map_window();
        server.pointer_over_focused();
        let mut b = TestClient::connect(&socket);
        let wb = b.map_window();
        a.roundtrip();
        // 400 - 4 (half the gap) - 1 (border) = 395.
        assert_eq!(a.config(&wa).expect("configured").size, (395, 600));
        assert_eq!(b.config(&wb).expect("configured").size, (395, 600));
        server.on_server(|h| {
            let layout = &h.server.state.windows.layout;
            assert_eq!(
                h.rect_of(1),
                Some(Rect::new(0, 0, 400, 600)),
                "layout exact"
            );
            assert_eq!(
                layout.shown_rect(WindowId(1)),
                Some(Rect::new(0, 0, 396, 600))
            );
            assert_eq!(
                layout.shown_rect(WindowId(2)),
                Some(Rect::new(404, 0, 396, 600))
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

/// Snapped windows follow the rule; maximized, plain floating and fullscreen windows do
/// not.
#[test]
fn snapped_windows_keep_gaps_floating_maximized_and_fullscreen_do_not() {
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
        // The left half 400 x 600, inset 4 on its right; minus the 1 px border there.
        assert_eq!(a.config(&w).expect("configured").size, (395, 600));
        server.on_server(|h| {
            let layout = &mut h.server.state.windows.layout;
            assert!(layout.snap(WindowId(1), ZoneId::FULL));
            h.server.state.arrange();
        });
        a.roundtrip();
        assert_eq!(
            a.config(&w).expect("configured").size,
            (800, 600),
            "maximized: whole"
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
            // Tile 1's visible right edge is at x 396; its zone is 393..396.
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
