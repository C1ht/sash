// SPDX-License-Identifier: Apache-2.0

//! Property tests with real clients (M10): random sequences of window and popup
//! operations, focus changes, selections and drags keep every layout invariant, and the
//! clipboard's own state stays consistent.

use std::io::Read;

use proptest::prelude::*;
use smithay::backend::input::{ButtonState, KeyState};
use wayland_client::protocol::wl_data_device_manager::DndAction;

use super::{
    client::{PopupSpec, TestClient, TestPopup, TestWindow},
    harness::{
        Harness, Remote,
        keys::{ALT, H, J, K, L},
    },
};
use crate::{data_device::limits::MAX_OFFERS_PER_CLIENT, layout::WindowId, seat::BTN_LEFT};

const TEXT: &str = "text/plain";
const ESCAPE: u32 = 1;

#[derive(Clone, Debug)]
enum Op {
    Open,
    Close(usize),
    Focus(u32),
    Popup(usize),
    Copy(usize),
    Paste(usize),
    DragStart(usize),
    PointerTo(f64, f64),
    Release,
    Escape,
    MoveWindow(usize, f64, f64),
}

fn op() -> impl Strategy<Value = Op> {
    let i = 0..6usize;
    prop_oneof![
        3 => Just(Op::Open),
        1 => i.clone().prop_map(Op::Close),
        2 => prop_oneof![Just(H), Just(J), Just(K), Just(L)].prop_map(Op::Focus),
        1 => i.clone().prop_map(Op::Popup),
        2 => i.clone().prop_map(Op::Copy),
        2 => i.clone().prop_map(Op::Paste),
        2 => i.clone().prop_map(Op::DragStart),
        3 => (0.0..800.0f64, 0.0..600.0f64).prop_map(|(x, y)| Op::PointerTo(x, y)),
        2 => Just(Op::Release),
        1 => Just(Op::Escape),
        1 => (i, -200.0..200.0f64, -200.0..200.0f64).prop_map(|(i, x, y)| Op::MoveWindow(i, x, y)),
    ]
}

struct Client {
    c: TestClient,
    w: TestWindow,
    id: u64,
    popup: Option<TestPopup>,
}

fn check(server: &Remote) {
    server.on_server(|h| {
        let state = &h.server.state;
        state.windows.layout.check_invariants();
        let pointer = state.seat.get_pointer().unwrap();
        if state.clipboard.drag.is_some() {
            assert!(pointer.is_grabbed(), "a drag holds the pointer grab");
        }
    });
}

fn center(server: &Remote, id: u64) -> Option<(f64, f64)> {
    let (tx, rx) = std::sync::mpsc::channel();
    server.on_server(move |h| {
        let r = h.server.state.windows.layout.rect_of(WindowId(id));
        let _ = tx.send(r.map(|r| ((r.x + r.w / 2) as f64, (r.y + r.h / 2) as f64)));
    });
    rx.recv().unwrap()
}

fn run(ops: Vec<Op>) {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut clients: Vec<Client> = Vec::new();
        let mut next_id = 0;
        let mut pressed = false;
        for op in ops {
            let n = clients.len();
            match op {
                Op::Open if n < 5 => {
                    let mut c = TestClient::connect(&socket);
                    c.data_device();
                    let w = c.map_window();
                    next_id += 1;
                    clients.push(Client {
                        c,
                        w,
                        id: next_id,
                        popup: None,
                    });
                }
                Op::Close(i) if n > 0 => {
                    drop(clients.remove(i % n));
                }
                Op::Focus(key) => server.on_server(move |h| {
                    h.key(ALT, KeyState::Pressed);
                    h.key(key, KeyState::Pressed);
                    h.key(key, KeyState::Released);
                    h.key(ALT, KeyState::Released);
                }),
                Op::Popup(i) if n > 0 => {
                    let cl = &mut clients[i % n];
                    if cl.popup.is_none() {
                        cl.popup =
                            Some(cl.c.open_popup(&cl.w.xdg_surface, PopupSpec::default(), None));
                    }
                }
                Op::Copy(i) if n > 0 => {
                    clients[i % n].c.copy(&[TEXT], b"data".to_vec());
                }
                Op::Paste(i) if n > 0 => {
                    let k = i % n;
                    if let Some(offer) = clients[k].c.events.data.selection_offer() {
                        let mut reader = clients[k].c.receive(&offer, TEXT);
                        for cl in clients.iter_mut() {
                            cl.c.roundtrip();
                        }
                        let mut out = Vec::new();
                        reader.read_to_end(&mut out).unwrap();
                        assert!(out.is_empty() || out == b"data");
                    }
                }
                Op::DragStart(i) if n > 0 && !pressed => {
                    let k = i % n;
                    if let Some((x, y)) = center(&server, clients[k].id) {
                        server.on_server(move |h| {
                            h.pointer_to(x, y);
                            h.button(BTN_LEFT, ButtonState::Pressed);
                        });
                        pressed = true;
                        let cl = &mut clients[k];
                        cl.c.roundtrip();
                        if let Some(serial) = cl.c.events.button_serial {
                            let source =
                                cl.c.new_source(&[TEXT], b"data".to_vec(), Some(DndAction::Copy));
                            let origin = cl.w.surface.clone();
                            cl.c.start_drag(Some(&source), &origin, None, serial);
                        }
                    }
                }
                Op::PointerTo(x, y) => {
                    server.on_server(move |h| h.pointer_to(x, y));
                    // A target that was entered accepts.
                    for cl in clients.iter_mut() {
                        cl.c.roundtrip();
                        if let Some(offer) = cl.c.events.data.drag_offer() {
                            offer.accept(0, Some(TEXT.into()));
                            offer.set_actions(DndAction::Copy, DndAction::Copy);
                        }
                    }
                }
                Op::Release if pressed => {
                    server.on_server(|h| h.button(BTN_LEFT, ButtonState::Released));
                    pressed = false;
                }
                Op::Escape => server.on_server(|h| {
                    h.key(ESCAPE, KeyState::Pressed);
                    h.key(ESCAPE, KeyState::Released);
                }),
                Op::MoveWindow(i, dx, dy) if n > 0 && !pressed => {
                    if let Some((x, y)) = center(&server, clients[i % n].id) {
                        server.on_server(move |h| {
                            h.key(ALT, KeyState::Pressed);
                            h.pointer_to(x, y);
                            h.button(BTN_LEFT, ButtonState::Pressed);
                            h.pointer_to((x + dx).clamp(0.0, 799.0), (y + dy).clamp(0.0, 599.0));
                            h.button(BTN_LEFT, ButtonState::Released);
                            h.key(ALT, KeyState::Released);
                        });
                    }
                }
                _ => {}
            }
            for cl in clients.iter_mut() {
                let _ = cl.c.try_roundtrip();
                assert!(cl.c.conn.protocol_error().is_none(), "no protocol error");
                assert!(
                    cl.c.events.data.offers.len() <= MAX_OFFERS_PER_CLIENT,
                    "offer limit"
                );
            }
            check(&server);
        }
        if pressed {
            server.on_server(|h| h.button(BTN_LEFT, ButtonState::Released));
        }
        check(&server);
        server.on_server(|h| assert!(h.server.state.clipboard.drag.is_none(), "no drag left"));
        for cl in clients.iter_mut() {
            cl.c.join_writers();
        }
    });
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn selections_and_drags_keep_every_layout_invariant(ops in prop::collection::vec(op(), 1..40)) {
        run(ops);
    }
}
