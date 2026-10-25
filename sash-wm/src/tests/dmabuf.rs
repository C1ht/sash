// SPDX-License-Identifier: Apache-2.0

//! zwp_linux_dmabuf_v1 and cursors without a GPU: the harness advertises a fake format
//! table (ARGB8888, XRGB8888, NV12; LINEAR) and accepts every buffer within the size
//! limit. Plane fds are plain files; nothing reads their content.

use std::os::fd::AsFd;

use smithay::{
    input::pointer::{CursorIcon, CursorImageStatus},
    reexports::wayland_server::Resource as _,
};
use wayland_client::{Proxy, backend::protocol::ProtocolError};
use wayland_protocols::wp::{
    cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape,
    linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::{self, Flags},
};

use super::{
    assert_compositor_alive,
    client::{DrmFourcc, PopupSpec, TestClient, buffer_file},
    harness::{FAKE_MAIN_DEVICE, Harness},
    protocol_error,
};
use crate::{dmabuf::MAX_DMABUF_SIZE, layout::Rect};

const LINEAR: u64 = 0;

/// Runs `client` (which must break a params rule) and returns the protocol error it
/// got; then checks the compositor still serves a new client.
fn params_error(client: impl FnOnce(&mut TestClient) + Send + 'static) -> ProtocolError {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    let error = harness.run_with_remote(move |_server| {
        let mut c = TestClient::connect(&socket);
        client(&mut c);
        let _ = c.try_roundtrip();
        protocol_error(&c)
    });
    assert_compositor_alive(&mut harness);
    error
}

fn assert_params_error(error: ProtocolError, code: zwp_linux_buffer_params_v1::Error) {
    assert_eq!(
        error.object_interface, "zwp_linux_buffer_params_v1",
        "{error:?}"
    );
    assert_eq!(error.code, code as u32, "{error:?}");
}

/// Adds plane `plane` at offset 0 of `file`.
fn add(
    params: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
    file: &std::fs::File,
    plane: u32,
    stride: u32,
    modifier: u64,
) {
    params.add(
        file.as_fd(),
        plane,
        0,
        stride,
        (modifier >> 32) as u32,
        modifier as u32,
    );
}

#[test]
fn the_default_feedback_names_the_main_device() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |_server| {
        let mut c = TestClient::connect(&socket);
        c.dmabuf_default_feedback();
        assert_eq!(c.events.dmabuf_main_device, Some(FAKE_MAIN_DEVICE));
    });
}

#[test]
fn valid_buffers_are_created_and_map_windows() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        let file = buffer_file(64 * 4 * 32);
        let params = c.dmabuf_params();
        add(&params, &file, 0, 64 * 4, LINEAR);
        params.create(64, 32, DrmFourcc::XRGB8888, Flags::empty());
        c.roundtrip();
        assert_eq!(c.events.dmabufs_created.len(), 1);
        assert_eq!(c.events.dmabufs_failed, 0);
        // A window whose buffers are all dmabufs maps like a shm one.
        c.dmabuf_buffers = true;
        let _w = c.map_window();
        server.on_server(|h| {
            assert_eq!(h.mapped_windows(), 1);
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 800, 600)));
        });
    });
}

#[test]
fn a_plane_set_twice_is_an_error() {
    let error = params_error(|c| {
        let file = buffer_file(4 * 16 * 16);
        let params = c.dmabuf_params();
        add(&params, &file, 0, 64, LINEAR);
        add(&params, &file, 0, 64, LINEAR);
    });
    assert_params_error(error, zwp_linux_buffer_params_v1::Error::PlaneSet);
}

#[test]
fn a_missing_plane_is_an_error() {
    // NV12 has two planes; only the first is given.
    let error = params_error(|c| {
        let file = buffer_file(16 * 16 * 2);
        let params = c.dmabuf_params();
        add(&params, &file, 0, 16, LINEAR);
        params.create(16, 16, DrmFourcc::NV12, Flags::empty());
    });
    assert_params_error(error, zwp_linux_buffer_params_v1::Error::Incomplete);
}

#[test]
fn create_before_all_planes_is_an_error() {
    // Only the second plane: plane 0 never came.
    let error = params_error(|c| {
        let file = buffer_file(16 * 16 * 2);
        let params = c.dmabuf_params();
        add(&params, &file, 1, 16, LINEAR);
        let _ = params.create_immed(16, 16, DrmFourcc::NV12, Flags::empty(), &c.qh(), ());
    });
    assert_params_error(error, zwp_linux_buffer_params_v1::Error::Incomplete);
}

#[test]
fn bad_dimensions_are_an_error() {
    let error = params_error(|c| {
        let file = buffer_file(64);
        let params = c.dmabuf_params();
        add(&params, &file, 0, 64, LINEAR);
        params.create(0, 16, DrmFourcc::ARGB8888, Flags::empty());
    });
    assert_params_error(error, zwp_linux_buffer_params_v1::Error::InvalidDimensions);
}

#[test]
fn an_unsupported_format_is_an_error() {
    let error = params_error(|c| {
        let file = buffer_file(2 * 16 * 16);
        let params = c.dmabuf_params();
        add(&params, &file, 0, 32, LINEAR);
        params.create(16, 16, DrmFourcc::RGB565, Flags::empty());
    });
    assert_params_error(error, zwp_linux_buffer_params_v1::Error::InvalidFormat);
}

#[test]
fn an_unsupported_modifier_is_an_error() {
    let error = params_error(|c| {
        let file = buffer_file(4 * 16 * 16);
        let params = c.dmabuf_params();
        add(&params, &file, 0, 64, 0x0300_0000_0060_6010);
        params.create(16, 16, DrmFourcc::ARGB8888, Flags::empty());
    });
    assert_params_error(error, zwp_linux_buffer_params_v1::Error::InvalidFormat);
}

#[test]
fn planes_with_different_modifiers_are_an_error() {
    let error = params_error(|c| {
        let file = buffer_file(16 * 16 * 2);
        let params = c.dmabuf_params();
        add(&params, &file, 0, 16, LINEAR);
        add(&params, &file, 1, 16, 1);
    });
    assert_params_error(error, zwp_linux_buffer_params_v1::Error::InvalidFormat);
}

#[test]
fn params_used_twice_are_an_error() {
    let error = params_error(|c| {
        let file = buffer_file(4 * 16 * 16);
        let params = c.dmabuf_params();
        add(&params, &file, 0, 64, LINEAR);
        params.create(16, 16, DrmFourcc::ARGB8888, Flags::empty());
        params.create(16, 16, DrmFourcc::ARGB8888, Flags::empty());
    });
    assert_params_error(error, zwp_linux_buffer_params_v1::Error::AlreadyUsed);
}

#[test]
fn a_plane_beyond_its_file_is_an_error() {
    let error = params_error(|c| {
        let file = buffer_file(100);
        let params = c.dmabuf_params();
        add(&params, &file, 0, 64, LINEAR);
        params.create(16, 16, DrmFourcc::ARGB8888, Flags::empty());
    });
    assert_params_error(error, zwp_linux_buffer_params_v1::Error::OutOfBounds);
}

#[test]
fn a_buffer_above_the_size_limit_fails() {
    let big = MAX_DMABUF_SIZE + 1;
    // create: the failed event, the client lives on.
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |_server| {
        let mut c = TestClient::connect(&socket);
        let file = buffer_file(4 * big as u64 * 16);
        let params = c.dmabuf_params();
        add(&params, &file, 0, 4 * big as u32, LINEAR);
        params.create(big, 16, DrmFourcc::ARGB8888, Flags::empty());
        c.roundtrip();
        assert_eq!(c.events.dmabufs_failed, 1);
        assert!(c.events.dmabufs_created.is_empty());
        assert!(c.conn.protocol_error().is_none());
    });
    // create_immed: invalid_wl_buffer.
    let error = params_error(move |c| {
        let file = buffer_file(4 * big as u64 * 16);
        let params = c.dmabuf_params();
        add(&params, &file, 0, 4 * big as u32, LINEAR);
        let _ = params.create_immed(big, 16, DrmFourcc::ARGB8888, Flags::empty(), &c.qh(), ());
    });
    assert_params_error(error, zwp_linux_buffer_params_v1::Error::InvalidWlBuffer);
}

#[test]
fn destroying_params_after_create_immed_keeps_the_buffer() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        // Every window buffer is made with create_immed and its params destroyed at once.
        c.dmabuf_buffers = true;
        let _w = c.map_window();
        assert!(c.conn.protocol_error().is_none());
        server.on_server(|h| assert_eq!(h.mapped_windows(), 1));
    });
}

#[test]
fn a_client_disconnecting_with_a_dmabuf_attached() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        c.dmabuf_buffers = true;
        let _w = c.map_window();
        drop(c);
        let mut other = TestClient::connect(&socket);
        let _w2 = other.map_window();
        server.on_server(|h| assert_eq!(h.mapped_windows(), 1));
    });
}

#[test]
fn a_buffer_destroyed_before_commit() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        c.dmabuf_buffers = true;
        let w = c.map_window();
        let file = buffer_file(4 * 800 * 600);
        let params = c.dmabuf_params();
        add(&params, &file, 0, 4 * 800, LINEAR);
        let buffer =
            params.create_immed(800, 600, DrmFourcc::ARGB8888, Flags::empty(), &c.qh(), ());
        w.surface.attach(Some(&buffer), 0, 0);
        buffer.destroy();
        w.surface.commit();
        let _ = c.try_roundtrip();
        assert!(
            c.conn.protocol_error().is_none(),
            "{:?}",
            c.conn.protocol_error()
        );
        server.on_server(|h| assert_eq!(h.mapped_windows(), 1));
    });
    assert_compositor_alive(&mut harness);
}

#[test]
fn dmabuf_windows_resize_with_other_buffer_sizes() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c1 = TestClient::connect(&socket);
        c1.dmabuf_buffers = true;
        let w1 = c1.map_window();
        let mut c2 = TestClient::connect(&socket);
        let _w2 = c2.map_window(); // 1 | 2: window 1 is told 400x600
        c1.roundtrip();
        // A smaller buffer, then a larger one than configured: the tile stays.
        c1.ack_and_commit_size(&w1, (300, 500));
        c1.commit_size(&w1, (500, 700));
        server.on_server(|h| {
            assert_eq!(h.rect_of(1), Some(Rect::new(0, 0, 400, 600)));
            assert_eq!(h.rect_of(2), Some(Rect::new(400, 0, 400, 600)));
        });
    });
}

#[test]
fn a_dmabuf_popup() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        c.dmabuf_buffers = true;
        let w = c.map_window();
        let menu = c.open_popup(&w.xdg_surface, PopupSpec::default(), None);
        assert_eq!(c.popup_config(&menu), Some((10, 20, 50, 40)));
        server.on_server(|h| h.pointer_to(30.0, 40.0));
        c.roundtrip();
        assert_eq!(c.events.pointer_focus, Some(menu.surface.id()));
    });
}

#[test]
fn cursor_shapes_and_surfaces() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        let _w = c.map_window();
        // Without pointer focus a shape is ignored.
        c.set_cursor_shape(1, Shape::Text);
        server.on_server(|h| {
            assert_eq!(
                h.server.state.cursor.image(),
                CursorImageStatus::Named(CursorIcon::Default)
            );
            h.pointer_to(100.0, 100.0);
        });
        c.roundtrip();
        let serial = c.events.pointer_enter_serial.expect("entered");
        c.set_cursor_shape(serial, Shape::Text);
        server.on_server(|h| {
            assert_eq!(
                h.server.state.cursor.image(),
                CursorImageStatus::Named(CursorIcon::Text)
            );
        });
        // A cursor surface with a hotspot: drawn at the pointer minus the hotspot.
        let cursor = c.set_cursor_surface((24, 24), (4, 6));
        let cursor_id = cursor.id().protocol_id();
        server.on_server(move |h| {
            let (surface, origin) = h.server.state.cursor_surface().expect("a cursor surface");
            assert_eq!(surface.id().protocol_id(), cursor_id);
            assert_eq!((origin.x, origin.y), (96, 94));
        });
    });
}

#[test]
fn a_cursor_shape_from_another_client_is_ignored() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let (mut c1, mut c2) = (TestClient::connect(&socket), TestClient::connect(&socket));
        let _w1 = c1.map_window();
        let _w2 = c2.map_window(); // 1 | 2
        server.on_server(|h| h.pointer_to(100.0, 100.0)); // over window 1
        c1.roundtrip();
        c2.roundtrip();
        // Client 2 has no pointer focus; even a plausible serial does not count.
        let serial = c1.events.pointer_enter_serial.expect("entered");
        c2.set_cursor_shape(serial, Shape::Wait);
        server.on_server(|h| {
            assert_eq!(
                h.server.state.cursor.image(),
                CursorImageStatus::Named(CursorIcon::Default)
            );
        });
        // Client 1's shape lasts until the pointer moves onto another surface.
        c1.set_cursor_shape(serial, Shape::Text);
        server.on_server(|h| {
            assert_eq!(
                h.server.state.cursor.image(),
                CursorImageStatus::Named(CursorIcon::Text)
            );
            h.pointer_to(600.0, 100.0); // over window 2
            assert_eq!(
                h.server.state.cursor.image(),
                CursorImageStatus::Named(CursorIcon::Default)
            );
        });
    });
}

#[derive(Clone, Debug)]
enum Op {
    Open {
        dmabuf: bool,
    },
    Close(usize),
    Popup {
        window: usize,
        dmabuf: bool,
        x: i32,
        y: i32,
    },
    ClosePopup(usize),
    /// Alt+F on the focused window.
    ToggleFloating,
    /// Alt+H or Alt+L.
    Focus(bool),
    /// Alt+Ctrl+Shift+L: wider (moves a split, or resizes a floating window).
    Wider,
    OutputSize(i32, i32),
}

fn op() -> impl proptest::strategy::Strategy<Value = Op> {
    use proptest::prelude::*;
    prop_oneof![
        3 => any::<bool>().prop_map(|dmabuf| Op::Open { dmabuf }),
        1 => (0..6usize).prop_map(Op::Close),
        2 => (0..6usize, any::<bool>(), -50..850i32, -50..650i32)
            .prop_map(|(window, dmabuf, x, y)| Op::Popup { window, dmabuf, x, y }),
        1 => (0..6usize).prop_map(Op::ClosePopup),
        1 => Just(Op::ToggleFloating),
        1 => any::<bool>().prop_map(Op::Focus),
        1 => Just(Op::Wider),
        1 => (200..1200i32, 200..900i32).prop_map(|(w, h)| Op::OutputSize(w, h)),
    ]
}

/// Window and popup operations with dmabuf and shm buffers mixed; after every step all
/// layout invariants hold (exact tiled coverage included) and every popup lies on its
/// parent's client.
fn run_ops(ops: Vec<Op>) {
    use super::harness::keys::{ALT, CTRL, F, H, L, SHIFT};
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        let mut windows = Vec::new();
        let mut popups = Vec::new();
        for op in ops {
            match op {
                Op::Open { dmabuf } => {
                    c.dmabuf_buffers = dmabuf;
                    windows.push(c.map_window());
                }
                Op::Close(i) if !windows.is_empty() => {
                    let window = windows.remove(i % windows.len());
                    // Its popups go first (the protocol's order).
                    let (mine, others): (Vec<_>, Vec<_>) = std::mem::take(&mut popups)
                        .into_iter()
                        .partition(|(parent, _): &(wayland_client::backend::ObjectId, _)| {
                            *parent == window.surface.id()
                        });
                    popups = others;
                    for (_, popup) in mine {
                        c.destroy_popup(popup);
                    }
                    c.destroy(window);
                }
                Op::Popup {
                    window,
                    dmabuf,
                    x,
                    y,
                } if !windows.is_empty() => {
                    let parent = &windows[window % windows.len()];
                    c.dmabuf_buffers = dmabuf;
                    let spec = PopupSpec {
                        anchor_rect: (x, y, 10, 10),
                        ..Default::default()
                    };
                    let popup = c.open_popup(&parent.xdg_surface, spec, None);
                    popups.push((parent.surface.id(), popup));
                }
                Op::ClosePopup(i) if !popups.is_empty() => {
                    let (_, popup) = popups.remove(i % popups.len());
                    c.destroy_popup(popup);
                }
                Op::ToggleFloating => server.on_server(|h| h.chord(&[ALT, F])),
                Op::Focus(right) => {
                    server.on_server(move |h| h.chord(&[ALT, if right { L } else { H }]))
                }
                Op::Wider => server.on_server(|h| h.chord(&[ALT, CTRL, SHIFT, L])),
                Op::OutputSize(w, h) => {
                    server.on_server(move |hs| hs.server.state.set_layout_area((w, h).into()))
                }
                _ => {}
            }
            c.roundtrip();
            // Keybinds may have dismissed popups; forget the dismissed ones.
            let done: Vec<_> = popups
                .iter()
                .filter(|(_, p)| c.popup_done(p))
                .map(|(_, p)| p.popup.id())
                .collect();
            let (gone, live): (Vec<_>, Vec<_>) = std::mem::take(&mut popups)
                .into_iter()
                .partition(|(_, p)| done.contains(&p.popup.id()));
            popups = live;
            for (_, popup) in gone {
                c.destroy_popup(popup);
            }
            assert!(
                c.conn.protocol_error().is_none(),
                "{:?}",
                c.conn.protocol_error()
            );
            let open = windows.len();
            server.on_server(move |h| {
                h.server.state.windows.layout.check_invariants();
                assert_eq!(h.mapped_windows(), open);
            });
        }
    });
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(32))]

    #[test]
    fn dmabuf_and_shm_windows_with_popups_keep_the_invariants(
        ops in proptest::collection::vec(op(), 1..16)
    ) {
        run_ops(ops);
    }
}
