// SPDX-License-Identifier: Apache-2.0

//! xdg popups end to end: real clients on the headless harness (output 800x600,
//! default mode tiled, modifier Alt). Window ids are assigned in creation order.

use smithay::backend::input::ButtonState;
use wayland_client::Proxy;
use wayland_protocols::xdg::shell::client::{
    xdg_positioner::{Anchor, ConstraintAdjustment, Gravity},
    xdg_wm_base,
};

use super::{
    client::{PopupSpec, TestClient, TestWindow},
    harness::{
        Harness,
        keys::{ALT, CTRL, F, L, SHIFT},
    },
    protocol_error,
};
use crate::{layout::Rect, seat::BTN_LEFT};

/// Clicks at (x, y) and moves the pointer there first.
fn click(h: &mut Harness, x: f64, y: f64) {
    h.pointer_to(x, y);
    h.button(BTN_LEFT, ButtonState::Pressed);
    h.button(BTN_LEFT, ButtonState::Released);
}

/// A click on (x, y) delivered to the client; returns the serial of its press.
fn press_serial(server: &super::harness::Remote, c: &mut TestClient, x: f64, y: f64) -> u32 {
    let before = c.events.buttons_pressed.len();
    server.on_server(move |h| click(h, x, y));
    c.roundtrip();
    assert_eq!(
        c.events.buttons_pressed.len(),
        before + 1,
        "click delivered"
    );
    c.events.button_serial.expect("button serial")
}

/// The client's surface under the pointer after moving it to (x, y).
fn pointer_focus_at(
    server: &super::harness::Remote,
    c: &mut TestClient,
    x: f64,
    y: f64,
) -> Option<wayland_client::backend::ObjectId> {
    server.on_server(move |h| h.pointer_to(x, y));
    c.roundtrip();
    c.events.pointer_focus.clone()
}

fn one_window(socket: &std::path::Path) -> (TestClient, TestWindow) {
    let mut c = TestClient::connect(socket);
    let w = c.map_window();
    (c, w)
}

#[test]
fn popup_configure_carries_the_computed_position() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |_server| {
        let (mut c, w) = one_window(&socket); // fills 800x600
        let menu = c.open_popup(&w.xdg_surface, PopupSpec::default(), None);
        assert_eq!(c.popup_config(&menu), Some((10, 20, 50, 40)));
        // Near the right edge: slides back onto the output.
        let spec = PopupSpec {
            anchor_rect: (780, 10, 10, 10),
            ..Default::default()
        };
        let edge = c.open_popup(&w.xdg_surface, spec, None);
        assert_eq!(c.popup_config(&edge), Some((750, 20, 50, 40)));
        // Flip instead: it goes to the left of the anchor.
        let spec = PopupSpec {
            anchor_rect: (780, 10, 10, 10),
            anchor: Anchor::TopRight,
            gravity: Gravity::BottomRight,
            adjust: ConstraintAdjustment::FlipX,
            ..Default::default()
        };
        let flipped = c.open_popup(&w.xdg_surface, spec, None);
        assert_eq!(c.popup_config(&flipped), Some((730, 10, 50, 40)));
    });
}

#[test]
fn a_grabbed_popup_is_dismissed_by_a_click_outside_which_is_not_delivered() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c1, w1) = one_window(&socket);
        let (mut c2, _w2) = one_window(&socket); // 1 | 2
        server.on_server(|h| h.pointer_to(100.0, 100.0));
        let serial = press_serial(&server, &mut c1, 100.0, 100.0);
        let menu = c1.open_popup(&w1.xdg_surface, PopupSpec::default(), Some(serial));
        assert_eq!(
            c1.events.keyboard_focus,
            Some(menu.surface.id()),
            "menu has the keyboard"
        );

        // A click inside the menu is delivered to it.
        let presses = c1.events.buttons_pressed.len();
        server.on_server(|h| click(h, 30.0, 40.0));
        c1.roundtrip();
        assert_eq!(c1.events.buttons_pressed.len(), presses + 1);
        assert_eq!(c1.events.button_surfaces.last(), Some(&menu.surface.id()));
        assert!(!c1.popup_done(&menu));

        // A click on the other window: dismissed, not delivered, focus back to 1.
        server.on_server(|h| {
            click(h, 600.0, 300.0);
            assert_eq!(h.keyboard_focus(), Some(1), "the click did not focus 2");
        });
        c1.roundtrip();
        c2.roundtrip();
        assert!(c1.popup_done(&menu));
        assert!(c2.events.buttons_pressed.is_empty(), "click not delivered");
        assert_eq!(c1.events.keyboard_focus, Some(w1.surface.id()));
        c1.destroy_popup(menu);

        // A click on the parent itself, outside the menu, also dismisses it.
        let serial = press_serial(&server, &mut c1, 200.0, 200.0);
        let menu = c1.open_popup(&w1.xdg_surface, PopupSpec::default(), Some(serial));
        let presses = c1.events.buttons_pressed.len();
        server.on_server(|h| click(h, 300.0, 300.0));
        c1.roundtrip();
        assert!(c1.popup_done(&menu));
        assert_eq!(c1.events.buttons_pressed.len(), presses, "not delivered");
    });
}

#[test]
fn nested_popups_are_dismissed_innermost_first() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w) = one_window(&socket);
        let serial = press_serial(&server, &mut c, 100.0, 100.0);
        let menu = c.open_popup(&w.xdg_surface, PopupSpec::default(), Some(serial));
        // Press inside the menu (10..60, 20..60) to open a submenu.
        let serial = press_serial(&server, &mut c, 30.0, 30.0);
        let sub_spec = PopupSpec {
            anchor_rect: (40, 0, 10, 10),
            anchor: Anchor::TopRight,
            ..Default::default()
        };
        let sub = c.open_popup(&menu.xdg_surface, sub_spec, Some(serial));
        // Relative to the menu: x = 50, y = 0.
        assert_eq!(c.popup_config(&sub), Some((50, 0, 50, 40)));
        assert_eq!(c.events.keyboard_focus, Some(sub.surface.id()));
        server.on_server(|h| click(h, 500.0, 500.0));
        c.roundtrip();
        assert_eq!(
            c.events.popups_done,
            vec![sub.popup.id(), menu.popup.id()],
            "innermost first"
        );
    });
}

#[test]
fn popups_follow_their_parent() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c1, w1) = one_window(&socket);
        let (mut c2, w2) = one_window(&socket); // 1 | 2: window 2 at x = 400
        let tooltip = c2.open_popup(&w2.xdg_surface, PopupSpec::default(), None);
        let reactive = c2.open_popup(
            &w2.xdg_surface,
            PopupSpec {
                anchor_rect: (380, 100, 10, 10),
                reactive: true,
                ..Default::default()
            },
            None,
        );
        // On window 2, at 400 + 380: slid back to x = 350 relative.
        assert_eq!(c2.popup_config(&reactive), Some((350, 110, 50, 40)));
        assert_eq!(
            pointer_focus_at(&server, &mut c2, 415.0, 25.0),
            Some(tooltip.surface.id())
        );

        // Layout change: window 1 closes, window 2 fills the output from x = 0.
        c1.destroy(w1);
        c2.roundtrip();
        assert_eq!(
            pointer_focus_at(&server, &mut c2, 15.0, 25.0),
            Some(tooltip.surface.id())
        );
        // The non-reactive tooltip got no new configure; the reactive one did: there is
        // room now, so it is where the positioner puts it.
        assert_eq!(c2.popup_config(&tooltip), Some((10, 20, 50, 40)));
        assert_eq!(c2.popup_config(&reactive), Some((380, 110, 50, 40)));
        c2.ack_and_commit_popup(&reactive);

        // Snap (client maximize of a floating window) and keyboard resize: popups stay
        // and follow.
        server.on_server(|h| {
            h.chord(&[ALT, F]); // floats at (0, 0, 800, 600)
        });
        c2.roundtrip();
        assert!(c2.popup_done(&tooltip), "mod+F dismisses the popups");
        c2.destroy_popup(tooltip);
        c2.destroy_popup(reactive);
        let tooltip = c2.open_popup(&w2.xdg_surface, PopupSpec::default(), None);
        server.on_server(|h| {
            for _ in 0..3 {
                h.chord(&[ALT, CTRL, SHIFT, super::harness::keys::H]); // narrower
            }
            assert_eq!(h.rect_of(2), Some(Rect::new(0, 0, 680, 600)));
        });
        c2.roundtrip();
        assert!(!c2.popup_done(&tooltip), "a keyboard resize keeps popups");
        // Move the window by a client move request (its title bar): the popup moves.
        server.on_server(|h| {
            h.pointer_to(300.0, 300.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
        });
        c2.roundtrip();
        c2.request_move(&w2);
        server.on_server(|h| {
            h.pointer_to(400.0, 350.0); // +100, +50
            h.button(BTN_LEFT, ButtonState::Released);
            assert_eq!(h.rect_of(2), Some(Rect::new(100, 50, 680, 600)));
        });
        assert_eq!(
            pointer_focus_at(&server, &mut c2, 115.0, 75.0),
            Some(tooltip.surface.id())
        );
        // Maximize (the top snap zone's state): the popup follows to (0, 0).
        w2.toplevel.set_maximized();
        c2.roundtrip();
        let w2_config = c2.config(&w2).expect("configured");
        assert!(w2_config.maximized);
        c2.ack_and_commit(&w2);
        assert_eq!(
            pointer_focus_at(&server, &mut c2, 15.0, 25.0),
            Some(tooltip.surface.id())
        );
        assert!(!c2.popup_done(&tooltip));
    });
}

#[test]
fn the_parent_unmapping_dismisses_its_popups() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w) = one_window(&socket);
        let serial = press_serial(&server, &mut c, 100.0, 100.0);
        let menu = c.open_popup(&w.xdg_surface, PopupSpec::default(), Some(serial));
        let tooltip = c.open_popup(
            &w.xdg_surface,
            PopupSpec {
                anchor_rect: (300, 300, 1, 1),
                ..Default::default()
            },
            None,
        );
        c.unmap(&w);
        assert!(c.popup_done(&menu));
        assert!(c.popup_done(&tooltip));
        server.on_server(|h| assert!(h.server.state.popups.grab_root().is_none()));
    });
}

#[test]
fn destroying_a_popup_before_its_child_is_a_protocol_error() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |_server| {
        let (mut c, w) = one_window(&socket);
        let menu = c.open_popup(&w.xdg_surface, PopupSpec::default(), None);
        let _sub = c.open_popup(&menu.xdg_surface, PopupSpec::default(), None);
        menu.popup.destroy();
        let _ = c.try_roundtrip();
        let error = protocol_error(&c);
        assert_eq!(error.object_interface, "xdg_wm_base");
        assert_eq!(error.code, xdg_wm_base::Error::NotTheTopmostPopup as u32);
    });
}

#[test]
fn an_invalid_grab_serial_dismisses_the_popup() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w) = one_window(&socket);
        let serial = press_serial(&server, &mut c, 100.0, 100.0);
        let menu = c.create_popup(&w.xdg_surface, PopupSpec::default(), Some(serial + 1000));
        assert!(c.popup_done(&menu));
        assert_eq!(c.events.keyboard_focus, Some(w.surface.id()));
        server.on_server(|h| assert!(h.server.state.popups.grab_root().is_none()));
    });
}

#[test]
fn a_grab_on_a_dismissed_parent_is_dismissed() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w) = one_window(&socket);
        let serial = press_serial(&server, &mut c, 100.0, 100.0);
        let menu = c.open_popup(&w.xdg_surface, PopupSpec::default(), Some(serial));
        // Dismissed by a keybind; the client opens a submenu on it before it notices.
        server.on_server(|h| h.chord(&[ALT, F]));
        c.roundtrip();
        assert!(c.popup_done(&menu));
        let sub = c.create_popup(&menu.xdg_surface, PopupSpec::default(), Some(serial));
        assert!(
            c.popup_done(&sub),
            "a grab on a dismissed parent is dismissed"
        );
        assert!(c.conn.protocol_error().is_none());
    });
}

#[test]
fn a_focus_changing_keybind_dismisses_the_chain() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c1, w1) = one_window(&socket);
        let (mut c2, w2) = one_window(&socket);
        let serial = press_serial(&server, &mut c1, 100.0, 100.0);
        let menu = c1.open_popup(&w1.xdg_surface, PopupSpec::default(), Some(serial));
        let sub = c1.open_popup(&menu.xdg_surface, PopupSpec::default(), Some(serial));
        server.on_server(|h| {
            h.chord(&[ALT, L]);
            assert_eq!(h.keyboard_focus(), Some(2));
        });
        c1.roundtrip();
        c2.roundtrip();
        assert_eq!(c1.events.popups_done, vec![sub.popup.id(), menu.popup.id()]);
        assert_eq!(c2.events.keyboard_focus, Some(w2.surface.id()));
        assert!(
            !c1.events.keys_pressed.contains(&L),
            "the key went to sash-wm"
        );
    });
}

#[test]
fn a_tooltip_never_takes_focus() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w) = one_window(&socket);
        let tooltip = c.open_popup(&w.xdg_surface, PopupSpec::default(), None);
        assert_eq!(c.events.keyboard_focus, Some(w.surface.id()));
        // Clicking it delivers the click but keeps keyboard focus on the window.
        server.on_server(|h| click(h, 30.0, 40.0));
        c.roundtrip();
        assert_eq!(c.events.button_surfaces.last(), Some(&tooltip.surface.id()));
        assert_eq!(c.events.keyboard_focus, Some(w.surface.id()));
    });
}

#[test]
fn a_popup_outside_the_parents_tile_receives_pointer_input() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c1, w1) = one_window(&socket);
        let (_c2, _w2) = one_window(&socket); // 1 | 2: tile of 1 ends at x = 400
        let spec = PopupSpec {
            anchor_rect: (380, 10, 10, 10),
            adjust: ConstraintAdjustment::empty(),
            ..Default::default()
        };
        let menu = c1.open_popup(&w1.xdg_surface, spec, None);
        assert_eq!(c1.popup_config(&menu), Some((380, 20, 50, 40)));
        // (420, 30) is over window 2's tile, but the popup is on top.
        assert_eq!(
            pointer_focus_at(&server, &mut c1, 420.0, 30.0),
            Some(menu.surface.id())
        );
        server.on_server(|h| click(h, 425.0, 35.0));
        c1.roundtrip();
        assert_eq!(c1.events.button_surfaces.last(), Some(&menu.surface.id()));
    });
}

#[test]
fn reposition_sends_the_new_position() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |_server| {
        let (mut c, w) = one_window(&socket);
        let menu = c.open_popup(&w.xdg_surface, PopupSpec::default(), None);
        let spec = PopupSpec {
            anchor_rect: (200, 200, 20, 20),
            size: (80, 30),
            ..Default::default()
        };
        c.reposition(&menu, spec, 7);
        assert_eq!(c.events.repositioned, vec![7]);
        assert_eq!(c.popup_config(&menu), Some((200, 220, 80, 30)));
        c.ack_and_commit_popup(&menu);
    });
}

#[test]
fn a_popup_on_an_unmapped_parent_is_dismissed() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |_server| {
        let mut c = TestClient::connect(&socket);
        let w = c.create_window(); // initial commit, no buffer
        let menu = c.create_popup(&w.xdg_surface, PopupSpec::default(), None);
        assert!(c.popup_done(&menu));
        assert!(c.popup_config(&menu).is_none(), "never configured");
    });
}

#[test]
fn a_client_disconnecting_with_an_open_chain() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w) = one_window(&socket);
        let serial = press_serial(&server, &mut c, 100.0, 100.0);
        let menu = c.open_popup(&w.xdg_surface, PopupSpec::default(), Some(serial));
        let _sub = c.open_popup(&menu.xdg_surface, PopupSpec::default(), Some(serial));
        drop(c);
        let (mut other, w2) = one_window(&socket);
        server.on_server(|h| {
            assert!(
                h.server.state.popups.grab_root().is_none(),
                "the grab is gone"
            );
            assert_eq!(h.mapped_windows(), 1);
            assert!(
                h.server
                    .state
                    .protocols
                    .xdg_shell
                    .popup_surfaces()
                    .is_empty()
            );
        });
        other.roundtrip();
        assert_eq!(other.events.keyboard_focus, Some(w2.surface.id()));
    });
}

#[test]
fn popups_while_the_output_is_hidden() {
    let mut harness = Harness::new();
    harness.presenting = false;
    let socket = harness.socket_path();
    harness.run_with_remote(move |_server| {
        let (mut c, w) = one_window(&socket);
        let menu = c.open_popup(&w.xdg_surface, PopupSpec::default(), None);
        assert_eq!(c.popup_config(&menu), Some((10, 20, 50, 40)));
        c.popup_frame_request(&menu);
        assert_eq!(c.events.frames_done, 0, "no frame callbacks while hidden");
    });
    assert_eq!(harness.frames, 0);
}

#[test]
fn an_output_resize_reconstrains_reactive_popups_and_dismisses_the_others() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w) = one_window(&socket);
        let spec = PopupSpec {
            anchor_rect: (700, 500, 10, 10),
            ..Default::default()
        };
        let menu = c.open_popup(&w.xdg_surface, spec, None);
        let reactive = c.open_popup(
            &w.xdg_surface,
            PopupSpec {
                reactive: true,
                ..spec
            },
            None,
        );
        assert_eq!(c.popup_config(&reactive), Some((700, 510, 50, 40)));
        // The same size again (winit repeats resizes): nothing happens.
        server.on_server(|h| h.server.state.set_layout_area((800, 600).into()));
        c.roundtrip();
        assert!(!c.popup_done(&menu));
        server.on_server(|h| h.server.state.set_layout_area((600, 400).into()));
        c.roundtrip();
        // Slid onto the smaller output (the window is now 600x400 too).
        assert_eq!(c.popup_config(&reactive), Some((550, 360, 50, 40)));
        assert!(!c.popup_done(&reactive));
        assert!(c.popup_done(&menu), "non-reactive: dismissed");
    });
}

#[test]
fn a_fixed_size_parent_at_the_output_edge() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        let mut w = c.create_window_without_commit();
        w.toplevel.set_min_size(200, 100);
        w.toplevel.set_max_size(200, 100);
        w.preferred = (200, 100);
        w.surface.commit();
        c.dispatch_until("configure", |e| e.configures >= 1);
        c.ack_and_commit(&w);
        server.on_server(|h| {
            // Forced floating, centered: (300, 250); move it against the right edge.
            for _ in 0..20 {
                h.chord(&[ALT, CTRL, L]);
            }
            let r = h.rect_of(1).expect("rect");
            assert_eq!(r.right(), 800 + 200 - 64, "64 px stay visible");
        });
        // A submenu to the right of the window flips to its left side.
        let spec = PopupSpec {
            anchor_rect: (0, 10, 200, 10),
            anchor: Anchor::TopRight,
            gravity: Gravity::BottomRight,
            adjust: ConstraintAdjustment::FlipX | ConstraintAdjustment::SlideY,
            ..Default::default()
        };
        let menu = c.open_popup(&w.xdg_surface, spec, None);
        assert_eq!(c.popup_config(&menu), Some((-50, 10, 50, 40)));
    });
}

#[test]
fn a_fullscreen_windows_popups_are_above_it() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c1, w1) = one_window(&socket);
        let (_c2, _w2) = one_window(&socket); // 1 | 2
        w1.toplevel.set_fullscreen(None);
        c1.roundtrip();
        assert!(c1.config(&w1).expect("configured").fullscreen);
        c1.ack_and_commit(&w1);
        let spec = PopupSpec {
            anchor_rect: (600, 300, 10, 10),
            ..Default::default()
        };
        let menu = c1.open_popup(&w1.xdg_surface, spec, None);
        assert_eq!(
            pointer_focus_at(&server, &mut c1, 620.0, 330.0),
            Some(menu.surface.id())
        );
    });
}

#[test]
fn mod_drag_dismisses_popups_except_a_resize() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w) = one_window(&socket);
        server.on_server(|h| h.chord(&[ALT, F])); // floating at (0, 0, 800, 600)
        let serial = press_serial(&server, &mut c, 100.0, 100.0);
        let menu = c.open_popup(&w.xdg_surface, PopupSpec::default(), Some(serial));
        // mod+right-drag (center cell: bottom-right corner) keeps the chain open.
        server.on_server(|h| {
            h.key(ALT, smithay::backend::input::KeyState::Pressed);
            h.pointer_to(400.0, 300.0);
            h.button(crate::seat::BTN_RIGHT, ButtonState::Pressed);
            h.pointer_to(350.0, 280.0);
            h.button(crate::seat::BTN_RIGHT, ButtonState::Released);
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 750, 580)));
        });
        c.roundtrip();
        assert!(!c.popup_done(&menu), "a resize keeps the popups");
        assert_eq!(c.events.keyboard_focus, Some(menu.surface.id()));
        // mod+left-drag dismisses them.
        server.on_server(|h| {
            h.pointer_to(400.0, 300.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.pointer_to(420.0, 300.0);
            h.button(BTN_LEFT, ButtonState::Released);
            h.key(ALT, smithay::backend::input::KeyState::Released);
        });
        c.roundtrip();
        assert!(c.popup_done(&menu), "a move dismisses the popups");
        assert_eq!(c.events.keyboard_focus, Some(w.surface.id()));
    });
}

/// Subsurfaces (investigation for M7): pointer input reaches a subsurface with an
/// input region, also on a popup.
#[test]
fn subsurfaces_receive_pointer_input() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c, w) = one_window(&socket); // tiled, fills 800x600
        let (sub, _sub_role) = c.add_subsurface(&w.surface, (100, 100), (50, 50));
        assert_eq!(
            pointer_focus_at(&server, &mut c, 120.0, 120.0),
            Some(sub.id())
        );
        assert_eq!(
            pointer_focus_at(&server, &mut c, 300.0, 300.0),
            Some(w.surface.id())
        );
        server.on_server(|h| click(h, 125.0, 125.0));
        c.roundtrip();
        assert_eq!(c.events.button_surfaces.last(), Some(&sub.id()));
        // A subsurface of a popup.
        let menu = c.open_popup(&w.xdg_surface, PopupSpec::default(), None); // (10, 20)
        let (menu_sub, _menu_sub_role) = c.add_subsurface(&menu.surface, (5, 5), (10, 10));
        assert_eq!(
            pointer_focus_at(&server, &mut c, 17.0, 27.0),
            Some(menu_sub.id())
        );
    });
}
