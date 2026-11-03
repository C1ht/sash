// SPDX-License-Identifier: Apache-2.0

//! Drag and drop end to end (M10), and the data device's protocol errors: real clients
//! on the headless harness (output 800x600, modifier Alt). Window 1 is the left tile,
//! window 2 the right one.

use smithay::{
    backend::input::{ButtonState, KeyState},
    reexports::wayland_server::Resource as _,
};
use wayland_client::{
    Proxy,
    protocol::{
        wl_data_device_manager::DndAction, wl_data_offer::WlDataOffer, wl_data_source::WlDataSource,
    },
};

use super::{
    client::{PopupSpec, TestClient, TestWindow, data::DragEvent},
    harness::{
        Harness, Remote,
        keys::{ALT, L},
    },
};
use crate::{layout::Mode, seat::BTN_LEFT};

const TEXT: &str = "text/plain";
const ESCAPE: u32 = 1;

struct Two {
    c1: TestClient,
    w1: TestWindow,
    c2: TestClient,
    w2: TestWindow,
}

fn two_windows(socket: &std::path::Path) -> Two {
    let mut c1 = TestClient::connect(socket);
    c1.data_device();
    let w1 = c1.map_window();
    let mut c2 = TestClient::connect(socket);
    c2.data_device();
    let w2 = c2.map_window();
    c1.roundtrip();
    c2.roundtrip();
    Two { c1, w1, c2, w2 }
}

/// Presses the left button at (x, y) and returns the serial the client under it got.
fn press(server: &Remote, c: &mut TestClient, x: f64, y: f64) -> u32 {
    server.on_server(move |h| {
        h.pointer_to(x, y);
        h.button(BTN_LEFT, ButtonState::Pressed);
    });
    c.roundtrip();
    c.events
        .button_serial
        .expect("the press reached the client")
}

fn release(server: &Remote) {
    server.on_server(|h| h.button(BTN_LEFT, ButtonState::Released));
}

fn move_to(server: &Remote, x: f64, y: f64) {
    server.on_server(move |h| h.pointer_to(x, y));
}

/// Starts a copy/move drag of TEXT from window 1 (pressed at 100, 100).
fn start_drag(t: &mut Two, server: &Remote) -> WlDataSource {
    let serial = press(server, &mut t.c1, 100.0, 100.0);
    let source = t.c1.new_source(
        &[TEXT],
        b"dragged".to_vec(),
        Some(DndAction::Copy | DndAction::Move),
    );
    let origin = t.w1.surface.clone();
    t.c1.start_drag(Some(&source), &origin, None, serial);
    server.on_server(|h| assert!(h.server.state.clipboard.drag.is_some(), "drag started"));
    source
}

/// The target accepts TEXT with copy.
fn accept(c: &mut TestClient) -> WlDataOffer {
    c.roundtrip();
    let offer = c.events.data.drag_offer().expect("entered with an offer");
    offer.accept(0, Some(TEXT.into()));
    offer.set_actions(DndAction::Copy | DndAction::Move, DndAction::Copy);
    c.roundtrip();
    offer
}

#[test]
fn a_drag_between_two_windows_drops_transfers_and_finishes() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut t = two_windows(&socket);
        let source = start_drag(&mut t, &server);
        move_to(&server, 600.0, 300.0);
        let offer = accept(&mut t.c2);
        let info = &t.c2.events.data.offers[&offer.id()];
        assert_eq!(info.mimes, [TEXT]);
        assert_eq!(info.source_actions, Some(DndAction::Copy | DndAction::Move));
        assert_eq!(info.action, Some(DndAction::Copy));
        t.c1.roundtrip();
        let s = t.c1.events.data.source(&source);
        assert_eq!(s.targets.last(), Some(&Some(TEXT.to_string())));
        assert_eq!(s.action, Some(DndAction::Copy));
        // Drop: the target reads and finishes.
        release(&server);
        t.c2.roundtrip();
        assert_eq!(t.c2.events.data.count(&DragEvent::Drop), 1);
        let mut reader = t.c2.receive(&offer, TEXT);
        t.c2.roundtrip();
        t.c1.roundtrip();
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut reader, &mut out).unwrap();
        assert_eq!(out, b"dragged");
        offer.finish();
        t.c2.roundtrip();
        t.c1.roundtrip();
        let s = t.c1.events.data.source(&source);
        assert!(s.drop_performed && s.finished && !s.cancelled);
        server.on_server(|h| assert!(h.server.state.clipboard.drag.is_none()));
    });
}

#[test]
fn a_drag_over_a_popup_enters_the_popup() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut t = two_windows(&socket);
        let popup =
            t.c2.open_popup(&t.w2.xdg_surface, PopupSpec::default(), None);
        t.c1.roundtrip();
        let _source = start_drag(&mut t, &server);
        // The popup is at window 2's (400, 0) + (10, 20), 50x40.
        move_to(&server, 430.0, 40.0);
        t.c2.roundtrip();
        match t.c2.events.data.drag.first() {
            Some(DragEvent::Enter { surface, x, y, .. }) => {
                assert_eq!(*surface, popup.surface.id(), "the popup, not its window");
                assert_eq!((*x, *y), (20.0, 20.0), "surface-local coordinates");
            }
            other => panic!("no enter: {other:?}"),
        }
        let offer = accept(&mut t.c2);
        release(&server);
        t.c2.roundtrip();
        assert_eq!(t.c2.events.data.count(&DragEvent::Drop), 1);
        offer.finish();
        t.c2.roundtrip();
    });
}

#[test]
fn a_drop_on_nothing_or_on_a_target_that_accepted_nothing_cancels() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        // Floating windows of 100x100: most of the output is empty.
        server.on_server(|h| {
            h.server
                .state
                .windows
                .layout
                .set_default_mode(Mode::Floating)
        });
        let mut c1 = TestClient::connect(&socket);
        c1.data_device();
        let w1 = c1.map_window();
        server.on_server(|h| {
            let r = h
                .server
                .state
                .windows
                .layout
                .rect_of(crate::layout::WindowId(1))
                .unwrap();
            assert_eq!((r.w, r.h), (100, 100));
        });
        let serial = press(&server, &mut c1, 400.0, 300.0); // the centered window
        let source = c1.new_source(&[TEXT], b"x".to_vec(), Some(DndAction::Copy));
        c1.start_drag(Some(&source), &w1.surface, None, serial);
        move_to(&server, 20.0, 20.0); // nothing there
        release(&server);
        c1.roundtrip();
        assert!(
            c1.events.data.source(&source).cancelled,
            "dropped on nothing"
        );

        // Over a window that accepts nothing: cancelled, and it sees the drag leave.
        let serial = press(&server, &mut c1, 400.0, 300.0);
        let source = c1.new_source(&[TEXT], b"x".to_vec(), Some(DndAction::Copy));
        c1.start_drag(Some(&source), &w1.surface, None, serial);
        move_to(&server, 410.0, 310.0);
        release(&server);
        c1.roundtrip();
        assert!(c1.events.data.source(&source).cancelled, "nothing accepted");
        // One leave per drag: when the pointer left window 1, and at the drop.
        assert_eq!(c1.events.data.count(&DragEvent::Leave), 2);
    });
}

/// At the output's edge (the pointer pinned there) the window under it is the target
/// as anywhere else.
#[test]
fn a_drop_at_the_output_edge_goes_to_the_window_there() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut t = two_windows(&socket);
        let source = start_drag(&mut t, &server);
        move_to(&server, 600.0, 300.0);
        server.on_server(|h| {
            for _ in 0..20 {
                h.server.state.pointer_moved_by((500.0, 0.0).into(), 0);
            }
            let at = h
                .server
                .state
                .seat
                .get_pointer()
                .unwrap()
                .current_location();
            assert_eq!(at.x, 799.0, "pinned at the right edge");
        });
        let offer = accept(&mut t.c2);
        release(&server);
        t.c2.roundtrip();
        t.c1.roundtrip();
        assert_eq!(t.c2.events.data.count(&DragEvent::Drop), 1);
        assert!(t.c1.events.data.source(&source).drop_performed);
        offer.finish();
        t.c2.roundtrip();
    });
}

#[test]
fn escape_cancels_a_drag_and_is_not_delivered() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut t = two_windows(&socket);
        let source = start_drag(&mut t, &server);
        move_to(&server, 600.0, 300.0);
        let _offer = accept(&mut t.c2);
        server.on_server(|h| {
            h.key(ESCAPE, KeyState::Pressed);
            h.key(ESCAPE, KeyState::Released);
            assert!(h.server.state.clipboard.drag.is_none());
        });
        t.c1.roundtrip();
        t.c2.roundtrip();
        assert!(t.c1.events.data.source(&source).cancelled);
        assert_eq!(t.c2.events.data.count(&DragEvent::Leave), 1);
        assert!(
            !t.c1.events.keys_pressed.contains(&ESCAPE),
            "Escape is not delivered"
        );
        // The pointer is free again: a release and a new press reach window 2.
        release(&server);
        let before = t.c2.events.buttons_pressed.len();
        press(&server, &mut t.c2, 600.0, 300.0);
        assert_eq!(t.c2.events.buttons_pressed.len(), before + 1);
    });
}

#[test]
fn a_focus_keybind_cancels_a_drag() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut t = two_windows(&socket);
        let source = start_drag(&mut t, &server); // the press focused window 1
        move_to(&server, 600.0, 300.0);
        server.on_server(|h| {
            h.key(ALT, KeyState::Pressed);
            h.key(L, KeyState::Pressed);
            h.key(L, KeyState::Released);
            h.key(ALT, KeyState::Released);
            assert!(h.server.state.clipboard.drag.is_none(), "cancelled");
            let focused = h.server.state.windows.layout.focused();
            assert_eq!(focused, Some(crate::layout::WindowId(2)), "and focus moved");
        });
        t.c1.roundtrip();
        assert!(t.c1.events.data.source(&source).cancelled);
        release(&server);
    });
}

/// The source's client disconnects mid-drag: the target sees it leave and the pointer
/// is free again; the target's client disconnecting cancels the source at the drop.
#[test]
fn a_disconnect_on_either_side_ends_the_drag_cleanly() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut t = two_windows(&socket);
        let _source = start_drag(&mut t, &server);
        move_to(&server, 600.0, 300.0);
        t.c2.roundtrip();
        assert_eq!(t.c2.events.data.enters(), 1);
        let Two { c1, mut c2, w2, .. } = t;
        drop(c1);
        c2.roundtrip();
        server.on_server(|h| {
            for _ in 0..5 {
                h.step();
            }
            assert!(h.server.state.clipboard.drag.is_none());
        });
        c2.roundtrip();
        assert_eq!(c2.events.data.count(&DragEvent::Leave), 1);
        // Window 2 now fills the output; its client draws at the new size.
        c2.ack_and_commit(&w2);
        c2.roundtrip();
        release(&server);
        let before = c2.events.buttons_pressed.len();
        press(&server, &mut c2, 600.0, 300.0);
        assert_eq!(c2.events.buttons_pressed.len(), before + 1, "pointer free");
        release(&server);

        // Now c2 drags to a new client c3, which disconnects before the drop.
        let mut c3 = TestClient::connect(&socket);
        c3.data_device();
        let _w3 = c3.map_window(); // splits window 2 (under the pointer): left | right
        c2.roundtrip();
        c2.ack_and_commit(&w2);
        c2.roundtrip();
        let serial = press(&server, &mut c2, 200.0, 300.0);
        let source = c2.new_source(&[TEXT], b"x".to_vec(), Some(DndAction::Copy));
        c2.start_drag(Some(&source), &w2.surface, None, serial);
        move_to(&server, 600.0, 300.0);
        c3.roundtrip();
        assert_eq!(c3.events.data.enters(), 1);
        drop(c3);
        server.on_server(|h| {
            for _ in 0..5 {
                h.step();
            }
        });
        release(&server);
        c2.roundtrip();
        assert!(c2.events.data.source(&source).cancelled);
    });
}

/// The icon starts with its top left at the pointer; attach offsets after that move it
/// (the hotspot); it is never under the pointer for hit-testing.
#[test]
fn a_drag_icon_follows_the_pointer_with_its_hotspot_and_takes_no_focus() {
    drag_icon(false);
}

/// The same with an icon whose buffers are dmabufs.
#[test]
fn a_dmabuf_drag_icon_works_like_a_shm_one() {
    drag_icon(true);
}

fn drag_icon(dmabuf: bool) {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut t = two_windows(&socket);
        t.c1.dmabuf_buffers = dmabuf;
        let serial = press(&server, &mut t.c1, 100.0, 100.0);
        let source =
            t.c1.new_source(&[TEXT], b"x".to_vec(), Some(DndAction::Copy));
        let icon = t.c1.icon_surface((16, 16));
        let origin = t.w1.surface.clone();
        t.c1.start_drag(Some(&source), &origin, Some(&icon), serial);
        move_to(&server, 600.0, 300.0);
        server.on_server(|h| {
            let (_, at) = h.server.state.drag_icon().expect("an icon");
            assert_eq!((at.x, at.y), (600, 300), "its top left at the pointer");
        });
        t.c1.attach_icon(&icon, (16, 16), (-4, -6));
        t.c1.roundtrip();
        let icon_id = icon.id();
        server.on_server(move |h| {
            let (surface, at) = h.server.state.drag_icon().expect("an icon");
            assert_eq!(surface.id().protocol_id(), icon_id.protocol_id());
            assert_eq!((at.x, at.y), (596, 294), "the hotspot at (4, 6)");
            let hit = h.server.state.surface_at((598.0, 296.0).into());
            assert!(hit.is_some_and(|(s, _)| s.id().protocol_id() != icon_id.protocol_id()));
        });
        // A later attach offset moves it further.
        t.c1.attach_icon(&icon, (16, 16), (2, 3));
        t.c1.roundtrip();
        server.on_server(|h| {
            let (_, at) = h.server.state.drag_icon().unwrap();
            assert_eq!((at.x, at.y), (598, 297));
        });
        t.c2.roundtrip();
        assert_eq!(
            t.c2.events.data.enters(),
            1,
            "window 2 is the target, not the icon"
        );
        assert!(t.c1.conn.protocol_error().is_none());
        release(&server);
    });
}

/// A start_drag without the serial of the press that began the implicit grab does
/// nothing; neither does one during a mod+drag (no press reached the client).
#[test]
fn a_drag_needs_the_serial_of_the_press() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut t = two_windows(&socket);
        let serial = press(&server, &mut t.c1, 100.0, 100.0);
        let source = t.c1.new_source(&[TEXT], b"x".to_vec(), None);
        let origin = t.w1.surface.clone();
        t.c1.start_drag(Some(&source), &origin, None, serial + 1000);
        server.on_server(|h| assert!(h.server.state.clipboard.drag.is_none(), "wrong serial"));
        release(&server);
        // A release ended the implicit grab: the old serial is no good either.
        let source = t.c1.new_source(&[TEXT], b"x".to_vec(), None);
        t.c1.start_drag(Some(&source), &origin, None, serial);
        server.on_server(|h| assert!(h.server.state.clipboard.drag.is_none(), "grab over"));
        // During a mod+drag move the client got no press; its last serial is refused
        // and the move goes on.
        server.on_server(|h| {
            h.key(ALT, KeyState::Pressed);
            h.button(BTN_LEFT, ButtonState::Pressed);
            assert!(h.server.state.windows.drag.is_some());
        });
        let source = t.c1.new_source(&[TEXT], b"x".to_vec(), None);
        t.c1.start_drag(Some(&source), &origin, None, serial);
        server.on_server(|h| {
            assert!(
                h.server.state.clipboard.drag.is_none(),
                "no drag during a move"
            );
            h.pointer_to(150.0, 150.0);
            assert!(h.server.state.windows.drag.is_some(), "the move goes on");
            h.button(BTN_LEFT, ButtonState::Released);
            h.key(ALT, KeyState::Released);
        });
        t.c2.roundtrip();
        assert_eq!(t.c2.events.data.enters(), 0);
    });
}

/// Each protocol error is the one the protocol defines; the server survives and a new
/// client can still connect.
#[test]
fn data_device_protocol_errors() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let expect_error = |c: &mut TestClient, interface: &str, code: u32| {
            let _ = c.try_roundtrip();
            let error = c.conn.protocol_error().expect("a protocol error");
            assert_eq!(
                (error.object_interface.as_str(), error.code),
                (interface, code),
                "{error:?}"
            );
        };
        // set_actions twice: invalid_source (1).
        let mut c = TestClient::connect(&socket);
        let s = c.new_source(&[TEXT], vec![], Some(DndAction::Copy));
        s.set_actions(DndAction::Move);
        expect_error(&mut c, "wl_data_source", 1);
        // An unknown action bit: invalid_action_mask (0).
        let mut c = TestClient::connect(&socket);
        let s = c.new_source(&[TEXT], vec![], None);
        s.set_actions(DndAction::from_bits_retain(0x80));
        expect_error(&mut c, "wl_data_source", 0);
        // A drag source used for the selection: invalid_source (1) on the source.
        let mut c = TestClient::connect(&socket);
        let _w = c.map_window();
        let device = c.data_device();
        let s = c.new_source(&[TEXT], vec![], Some(DndAction::Copy));
        device.set_selection(Some(&s), 0);
        expect_error(&mut c, "wl_data_source", 1);
        // A source used twice is accepted (the protocol says used_source "may" be sent;
        // Firefox sets the same source again), but set_actions after use is an error.
        let mut c = TestClient::connect(&socket);
        let _w = c.map_window();
        let device = c.data_device();
        let s = c.new_source(&[TEXT], vec![], None);
        device.set_selection(Some(&s), 0);
        device.set_selection(Some(&s), 0);
        c.roundtrip();
        assert!(c.conn.protocol_error().is_none(), "reuse is no error");
        s.set_actions(DndAction::Copy);
        expect_error(&mut c, "wl_data_source", 1);
        // finish on a selection offer: invalid_finish (0); set_actions: invalid_offer (3).
        for (finish, code) in [(true, 0), (false, 3)] {
            let mut c = TestClient::connect(&socket);
            let _w = c.map_window();
            c.copy(&[TEXT], b"x".to_vec());
            let offer = c
                .events
                .data
                .selection_offer()
                .expect("own selection offered");
            if finish {
                offer.finish();
            } else {
                offer.set_actions(DndAction::Copy, DndAction::Copy);
            }
            expect_error(&mut c, "wl_data_offer", code);
        }
        // A drag icon with another role (a window's surface): role (0) on the device.
        let mut c = TestClient::connect(&socket);
        let w = c.map_window();
        server.pointer_over_focused();
        let serial = press(&server, &mut c, 400.0, 300.0);
        let s = c.new_source(&[TEXT], vec![], Some(DndAction::Copy));
        let device = c.data_device();
        device.start_drag(Some(&s), &w.surface, Some(&w.surface), serial);
        expect_error(&mut c, "wl_data_device", 0);
        release(&server);
        // On a drag offer: a preferred action of two bits: invalid_action (2); finish
        // before a drop: invalid_finish (0).
        for code in [2, 0] {
            let mut t = two_windows(&socket);
            let _source = start_drag(&mut t, &server);
            move_to(&server, 600.0, 300.0);
            t.c2.roundtrip();
            let offer = t.c2.events.data.drag_offer().unwrap();
            if code == 2 {
                offer.set_actions(DndAction::Copy, DndAction::Copy | DndAction::Move);
            } else {
                offer.accept(0, Some(TEXT.into()));
                offer.finish();
            }
            expect_error(&mut t.c2, "wl_data_offer", code);
            release(&server);
        }
        // The server survives: a new client maps a window.
        let mut c = TestClient::connect(&socket);
        let _w = c.map_window();
        server.on_server(|h| assert!(h.mapped_windows() >= 1));
    });
}

/// Measurement (M10): during a drag with an icon, the time from a pointer motion to
/// the frame that shows the icon at the new position, as sash-wm sees it (headless: no
/// GPU, so this is sash-wm's own share of pointer-to-frame latency).
#[test]
fn pointer_to_frame_latency_during_a_drag() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut t = two_windows(&socket);
        let serial = press(&server, &mut t.c1, 100.0, 100.0);
        let source =
            t.c1.new_source(&[TEXT], b"x".to_vec(), Some(DndAction::Copy));
        let icon = t.c1.icon_surface((32, 32));
        let origin = t.w1.surface.clone();
        t.c1.start_drag(Some(&source), &origin, Some(&icon), serial);
        server.on_server(|h| {
            let mut samples = Vec::new();
            for i in 0..500 {
                let x = 100.0 + (i % 600) as f64;
                let start = std::time::Instant::now();
                let frames = h.frames;
                h.pointer_to(x, 300.0);
                while h.frames == frames {
                    h.step();
                }
                samples.push(start.elapsed());
            }
            samples.sort();
            let p = |q: f64| samples[((samples.len() - 1) as f64 * q) as usize];
            println!(
                "pointer to frame during a drag (headless, {} motions): p50 {:?} p99 {:?} max {:?}",
                samples.len(),
                p(0.5),
                p(0.99),
                p(1.0)
            );
            assert!(p(0.5) < std::time::Duration::from_millis(5));
        });
        release(&server);
    });
}
