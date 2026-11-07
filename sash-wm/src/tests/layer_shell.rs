// SPDX-License-Identifier: Apache-2.0

//! Layer-shell end to end (M13b step 3): privileged test clients (an in-process
//! privileged connection, `Sash::privileged_connection_for_tests`) on the headless
//! harness (output 800x600, modifier Alt).

use std::{io::Write, os::unix::net::UnixStream};

use smithay::backend::input::{ButtonState, KeyState};
use wayland_client::{Connection, Proxy};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{Anchor, KeyboardInteractivity},
};

use super::{
    assert_compositor_alive,
    client::{
        PopupSpec, TestClient,
        layer::{LayerSpec, TestLayer},
    },
    harness::{
        Harness,
        keys::{ALT, F, Q},
    },
};
use crate::{
    layer_shell::{NAMESPACE, PER_CLIENT, PER_OUTPUT},
    layout::{Mode, Rect, WindowId},
    seat::BTN_LEFT,
};

const OUTPUT: Rect = Rect::new(0, 0, 800, 600);

fn privileged(h: &mut Harness) -> UnixStream {
    h.server.state.privileged_connection_for_tests()
}

fn client_on(stream: UnixStream) -> TestClient {
    TestClient::from_connection(Connection::from_socket(stream).expect("connection"))
}

fn usable(h: &Harness) -> Rect {
    h.server.state.windows.layout.area()
}

/// A layer surface centered on `layer`, 200x100, with keyboard mode `keyboard`.
fn centered(layer: Layer, keyboard: KeyboardInteractivity) -> LayerSpec {
    LayerSpec::centered(layer, (200, 100), keyboard)
}

fn click(h: &mut Harness, x: f64, y: f64) {
    h.pointer_to(x, y);
    h.button(BTN_LEFT, ButtonState::Pressed);
    h.button(BTN_LEFT, ButtonState::Released);
}

#[test]
fn only_privileged_clients_see_layer_shell_and_a_guessed_bind_fails() {
    let mut harness = Harness::new();
    let stream = privileged(&mut harness);
    let socket = harness.socket_path();
    harness.run_with_remote(move |_| {
        let p = client_on(stream);
        assert!(p.layer_shell().is_some(), "a privileged client sees it");
        let name = p
            .events
            .globals
            .iter()
            .find(|(_, i)| i == "zwlr_layer_shell_v1")
            .map(|(n, _)| *n)
            .expect("advertised");
        let mut normal = TestClient::connect(&socket);
        assert!(normal.layer_shell().is_none());
        assert!(
            normal
                .events
                .globals
                .iter()
                .all(|(_, i)| i != "zwlr_layer_shell_v1")
        );
        let qh = normal.qh();
        let registry = normal.events.registry.clone().expect("registry");
        let _: ZwlrLayerShellV1 = registry.bind(name, 1, &qh, ());
        assert!(normal.try_roundtrip().is_err(), "a guessed bind was served");
    });
    assert_compositor_alive(&mut harness);
}

/// The first-party panel rule end to end: zone = visible size plus margin, so windows
/// start right below the panel's visible edge.
#[test]
fn a_first_party_panel_reserves_its_visible_size_plus_margin() {
    let mut harness = Harness::new();
    let stream = privileged(&mut harness);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut window = TestClient::connect(&socket);
        let w = window.map_window();
        let mut p = client_on(stream);
        let panel = p.map_layer(&LayerSpec::top_panel(30, 10));
        assert_eq!(
            p.layer_info(&panel).configures.last().map(|c| (c.1, c.2)),
            Some((800, 30))
        );
        server.on_server(|h| {
            assert_eq!(usable(h), Rect::new(0, 40, 800, 560));
            let entry = &h.server.state.layers.entries[0];
            assert_eq!(
                entry.rect,
                Rect::new(0, 10, 800, 30),
                "the panel's visible rectangle"
            );
            assert_eq!(
                h.rect_of(1),
                Some(Rect::new(0, 40, 800, 560)),
                "the tile starts below it"
            );
        });
        window.roundtrip();
        // 1 px border on the panel edge.
        assert_eq!(window.config(&w).expect("configured").size, (798, 558));
    });
}

#[test]
fn a_panel_appearing_over_a_floating_window_moves_it_down_and_gets_the_pointer() {
    let mut harness = Harness::new();
    let stream = privileged(&mut harness);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut window = TestClient::connect(&socket);
        let _w = window.map_window();
        server.on_server(|h| {
            h.chord(&[ALT, F]);
            let layout = &mut h.server.state.windows.layout;
            layout.set_floating_rect(WindowId(1), Rect::new(100, 5, 400, 300));
            h.server.state.arrange();
        });
        let mut p = client_on(stream);
        let panel = p.map_layer(&LayerSpec::top_panel(40, 0));
        server.on_server(|h| {
            assert_eq!(
                h.rect_of(1),
                Some(Rect::new(100, 40, 400, 300)),
                "moved below it"
            );
            h.pointer_to(300.0, 20.0);
        });
        p.roundtrip();
        assert_eq!(p.events.pointer_focus, Some(panel.surface.id()));
        // Gone again: the window stays where it was moved.
        panel.layer.destroy();
        panel.surface.destroy();
        p.roundtrip();
        server.on_server(|h| {
            assert_eq!(usable(h), OUTPUT);
            assert_eq!(h.rect_of(1), Some(Rect::new(100, 40, 400, 300)));
        });
    });
}

#[test]
fn keyboard_none_never_and_on_demand_on_click() {
    let mut harness = Harness::new();
    let stream = privileged(&mut harness);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut window = TestClient::connect(&socket);
        let w = window.map_window();
        let mut p = client_on(stream);
        let none = p.map_layer(&centered(Layer::Overlay, KeyboardInteractivity::None));
        server.on_server(|h| click(h, 400.0, 300.0));
        p.roundtrip();
        window.roundtrip();
        assert_eq!(
            p.events.keyboard_focus, None,
            "none never takes the keyboard"
        );
        assert_eq!(window.events.keyboard_focus, Some(w.surface.id()));
        p.change_layer(&none, |l| {
            l.set_keyboard_interactivity(KeyboardInteractivity::OnDemand)
        });
        server.on_server(|h| click(h, 400.0, 300.0));
        p.roundtrip();
        window.roundtrip();
        assert_eq!(
            p.events.keyboard_focus,
            Some(none.surface.id()),
            "on_demand on click"
        );
        assert_eq!(window.events.keyboard_focus, None);
        // A click on the window gives it back.
        server.on_server(|h| click(h, 50.0, 50.0));
        p.roundtrip();
        window.roundtrip();
        assert_eq!(p.events.keyboard_focus, None);
        assert_eq!(window.events.keyboard_focus, Some(w.surface.id()));
    });
}

/// Exclusive surfaces: the top-most layer wins, then the newest; closing one hands the
/// keyboard to the next by the same rule, and finally back to the window.
#[test]
fn exclusive_surfaces_hold_the_keyboard_top_layer_first_then_newest() {
    let mut harness = Harness::new();
    let stream = privileged(&mut harness);
    let socket = harness.socket_path();
    harness.run_with_remote(move |_| {
        let mut window = TestClient::connect(&socket);
        let w = window.map_window();
        let mut p = client_on(stream);
        let focus = |p: &mut TestClient| {
            p.roundtrip();
            p.events.keyboard_focus.clone()
        };
        let a = p.map_layer(&centered(Layer::Top, KeyboardInteractivity::Exclusive));
        assert_eq!(
            focus(&mut p),
            Some(a.surface.id()),
            "exclusive takes it when mapped"
        );
        let b = p.map_layer(&centered(Layer::Top, KeyboardInteractivity::Exclusive));
        assert_eq!(focus(&mut p), Some(b.surface.id()), "the newest in a layer");
        let c = p.map_layer(&centered(Layer::Overlay, KeyboardInteractivity::Exclusive));
        let d = p.map_layer(&centered(Layer::Top, KeyboardInteractivity::Exclusive));
        assert_eq!(
            focus(&mut p),
            Some(c.surface.id()),
            "overlay over a newer top surface"
        );
        let close = |p: &mut TestClient, l: TestLayer| {
            l.layer.destroy();
            l.surface.destroy();
            p.roundtrip();
        };
        close(&mut p, c);
        assert_eq!(focus(&mut p), Some(d.surface.id()));
        close(&mut p, d);
        assert_eq!(focus(&mut p), Some(b.surface.id()));
        close(&mut p, b);
        assert_eq!(focus(&mut p), Some(a.surface.id()));
        close(&mut p, a);
        assert_eq!(focus(&mut p), None);
        window.roundtrip();
        assert_eq!(
            window.events.keyboard_focus,
            Some(w.surface.id()),
            "back to the window"
        );
    });
}

#[test]
fn window_keybinds_do_nothing_while_an_exclusive_surface_holds_the_keyboard() {
    use crate::keybinds::{Action, TABLE};
    assert!(
        TABLE
            .iter()
            .all(|b| b.action.is_global() == (b.action == Action::Quit)),
        "quit is the only global table action"
    );
    let mut harness = Harness::new();
    let stream = privileged(&mut harness);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut window = TestClient::connect(&socket);
        let _w = window.map_window();
        let mut p = client_on(stream);
        let launcher = p.map_layer(&centered(Layer::Overlay, KeyboardInteractivity::Exclusive));
        server.on_server(|h| {
            h.chord(&[ALT, F]);
            assert_eq!(h.mode_of(1), Some(Mode::Tiled), "mod+F did nothing");
            h.chord(&[ALT, Q]);
            assert_eq!(h.mapped_windows(), 1, "mod+Q did nothing");
        });
        window.roundtrip();
        assert!(!window.events.close_requested);
        p.roundtrip();
        // Only the modifier itself is delivered, as for every keybind.
        assert!(
            !p.events.keys_pressed.contains(&F) && !p.events.keys_pressed.contains(&Q),
            "the keys are kept from clients too"
        );
        // Exclusive on the bottom layer behaves like on_demand: keybinds work.
        launcher.layer.destroy();
        launcher.surface.destroy();
        p.roundtrip();
        let _bottom = p.map_layer(&centered(Layer::Bottom, KeyboardInteractivity::Exclusive));
        server.on_server(|h| {
            h.chord(&[ALT, F]);
            assert_eq!(h.mode_of(1), Some(Mode::Floating), "mod+F works");
        });
    });
}

/// mod+Q pressed while an exclusive surface holds the keyboard, the surface closes,
/// then Q is released: nothing happens to the window and no key reaches it.
#[test]
fn an_exclusive_surface_closing_while_a_keybind_is_held() {
    let mut harness = Harness::new();
    let stream = privileged(&mut harness);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut window = TestClient::connect(&socket);
        let w = window.map_window();
        let mut p = client_on(stream);
        let launcher = p.map_layer(&centered(Layer::Overlay, KeyboardInteractivity::Exclusive));
        server.on_server(|h| {
            h.key(ALT, KeyState::Pressed);
            h.key(Q, KeyState::Pressed);
        });
        launcher.layer.destroy();
        launcher.surface.destroy();
        p.roundtrip();
        server.on_server(|h| {
            h.key(Q, KeyState::Released);
            h.key(ALT, KeyState::Released);
            assert_eq!(h.mapped_windows(), 1);
        });
        window.roundtrip();
        assert_eq!(
            window.events.keyboard_focus,
            Some(w.surface.id()),
            "focus is back"
        );
        assert!(
            !window.events.keys_pressed.contains(&Q),
            "Q never reached the window"
        );
        assert!(!window.events.close_requested);
    });
}

/// Every protocol error disconnects only the offending client.
#[test]
fn protocol_errors_disconnect_only_the_offender() {
    let mut harness = Harness::new();
    let streams: Vec<UnixStream> = (0..6).map(|_| privileged(&mut harness)).collect();
    let socket = harness.socket_path();
    harness.run_with_remote(move |_| {
        let mut bystander = TestClient::connect(&socket);
        let _w = bystander.map_window();
        let mut streams = streams.into_iter();
        let mut next = || client_on(streams.next().expect("stream"));

        // Width 0 without both horizontal anchors.
        let mut c = next();
        let mut spec = LayerSpec::top_panel(30, 0);
        spec.anchor = Anchor::Top;
        c.create_layer(&spec);
        assert!(c.try_roundtrip().is_err(), "invalid size");
        assert_eq!(
            super::protocol_error(&c).code,
            1,
            "zwlr_layer_surface_v1.invalid_size"
        );

        // Anchor bits outside the enum.
        let mut c = next();
        let layer = c.create_layer(&LayerSpec::top_panel(30, 0));
        layer.layer.set_anchor(Anchor::from_bits_retain(0x10));
        assert!(c.try_roundtrip().is_err(), "invalid anchor");
        assert_eq!(
            super::protocol_error(&c).code,
            2,
            "zwlr_layer_surface_v1.invalid_anchor"
        );

        // A buffer in the first commit, before any configure was acked.
        let mut c = next();
        let qh = c.qh();
        let surface = c
            .events
            .compositor
            .as_ref()
            .expect("compositor")
            .create_surface(&qh, ());
        let shell = c.layer_shell().expect("shell");
        let layer = shell.get_layer_surface(&surface, None, Layer::Top, "early".into(), &qh, ());
        layer.set_size(100, 100);
        let buffer = c.buffer_for_tests((100, 100));
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        assert!(c.try_roundtrip().is_err(), "a buffer before the first ack");

        // A request on a destroyed layer surface (written by hand: the client library
        // refuses to send it).
        let mut c = next();
        let layer = c.map_layer(&LayerSpec::top_panel(30, 0));
        let id = layer.layer.id().protocol_id();
        layer.layer.destroy();
        c.roundtrip();
        let mut raw = std::fs::File::from(
            c.conn
                .backend()
                .poll_fd()
                .try_clone_to_owned()
                .expect("dup"),
        );
        let mut message = Vec::new();
        message.extend_from_slice(&id.to_ne_bytes());
        message.extend_from_slice(&(16u32 << 16).to_ne_bytes()); // set_size
        message.extend_from_slice(&1u32.to_ne_bytes());
        message.extend_from_slice(&1u32.to_ne_bytes());
        raw.write_all(&message).expect("write");
        assert!(
            c.try_roundtrip().is_err(),
            "a request on a destroyed object"
        );

        // A surface that already has another role (an xdg_toplevel).
        let mut c = next();
        let window = c.create_window_without_commit();
        let qh = c.qh();
        let shell = c.layer_shell().expect("shell");
        let _layer =
            shell.get_layer_surface(&window.surface, None, Layer::Top, "x".into(), &qh, ());
        assert!(c.try_roundtrip().is_err(), "a surface with a role already");
        assert_eq!(
            super::protocol_error(&c).code,
            0,
            "zwlr_layer_shell_v1.role"
        );

        // A second layer surface for the same wl_surface.
        let mut c = next();
        let layer = c.create_layer(&LayerSpec::top_panel(30, 0));
        let qh = c.qh();
        let shell = c.layer_shell().expect("shell");
        let _again =
            shell.get_layer_surface(&layer.surface, None, Layer::Top, "again".into(), &qh, ());
        assert!(c.try_roundtrip().is_err(), "a second layer surface");
        assert_eq!(
            super::protocol_error(&c).code,
            0,
            "zwlr_layer_surface_v1.invalid_surface_state"
        );

        // The bystander was never touched.
        bystander.roundtrip();
        let _again = bystander.map_window();
    });
    assert_compositor_alive(&mut harness);
}

/// Not errors, per the protocol: a zone where it does not apply, destroying a mapped
/// surface or its wl_surface, a null-buffer unmap and remap, a client gone mid-frame.
#[test]
fn what_the_protocol_allows_keeps_the_client_and_frees_the_reservation() {
    let mut harness = Harness::new();
    let streams: Vec<UnixStream> = (0..2).map(|_| privileged(&mut harness)).collect();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut window = TestClient::connect(&socket);
        let w = window.map_window();
        let mut streams = streams.into_iter();
        let mut p = client_on(streams.next().expect("stream"));
        // A positive zone on a corner reserves nothing.
        let mut corner = LayerSpec::top_panel(30, 0);
        corner.anchor = Anchor::Top | Anchor::Left;
        corner.size = (100, 30);
        corner.zone = 50;
        let _corner = p.map_layer(&corner);
        assert!(p.try_roundtrip().is_ok());
        server.on_server(|h| assert_eq!(usable(h), OUTPUT));

        // Destroying the layer_surface of a mapped panel.
        let panel = p.map_layer(&LayerSpec::top_panel(40, 0));
        server.on_server(|h| assert_eq!(usable(h).y, 40));
        panel.layer.destroy();
        p.roundtrip();
        server.on_server(|h| assert_eq!(usable(h), OUTPUT));
        panel.surface.destroy();

        // Destroying the wl_surface first.
        let panel = p.map_layer(&LayerSpec::top_panel(40, 0));
        server.on_server(|h| assert_eq!(usable(h).y, 40));
        panel.surface.destroy();
        p.roundtrip();
        server.on_server(|h| assert_eq!(usable(h), OUTPUT));
        panel.layer.destroy();
        assert!(p.try_roundtrip().is_ok());
        window.roundtrip();
        assert_eq!(window.config(&w).expect("configured").size, (798, 598));

        // Unmap with a null buffer, then map again after a new configure.
        let panel = p.map_layer(&LayerSpec::top_panel(40, 0));
        p.unmap_layer(&panel);
        server.on_server(|h| assert_eq!(usable(h), OUTPUT));
        let configures = p.layer_info(&panel).configures.len();
        panel.surface.commit();
        p.roundtrip();
        assert_eq!(p.layer_info(&panel).configures.len(), configures + 1);
        p.ack_and_commit_layer(&panel);
        server.on_server(|h| assert_eq!(usable(h).y, 40));

        // Another client gone in the middle of a frame (a frame callback pending).
        let mut q = client_on(streams.next().expect("stream"));
        let dock = q.map_layer(&LayerSpec {
            anchor: Anchor::Bottom | Anchor::Left | Anchor::Right,
            namespace: "dock".into(),
            ..LayerSpec::top_panel(50, 0)
        });
        server.on_server(|h| assert_eq!(usable(h), Rect::new(0, 40, 800, 510)));
        let qh = q.qh();
        dock.surface.frame(&qh, ());
        dock.surface.commit();
        let _ = q.conn.flush();
        drop(dock);
        drop(q);
        server.on_server(|h| {
            h.run_until("the dock is gone", |h| {
                usable(h) == Rect::new(0, 40, 800, 560)
            });
        });
        assert!(p.try_roundtrip().is_ok());
    });
}

#[test]
fn limits_refuse_with_closed_and_keep_the_client() {
    let mut harness = Harness::new();
    let streams: Vec<UnixStream> = (0..3).map(|_| privileged(&mut harness)).collect();
    harness.run_with_remote(move |server| {
        let mut streams = streams.into_iter();
        let mut a = client_on(streams.next().expect("stream"));
        let spec = centered(Layer::Top, KeyboardInteractivity::None);
        let mine: Vec<TestLayer> = (0..PER_CLIENT).map(|_| a.create_layer(&spec)).collect();
        a.roundtrip();
        assert!(mine.iter().all(|l| !a.layer_info(l).closed));
        let extra = a.create_layer(&spec);
        a.roundtrip();
        assert!(a.layer_info(&extra).closed, "beyond PER_CLIENT");
        let mut long = spec.clone();
        long.namespace = "n".repeat(NAMESPACE + 1);
        let mut b = client_on(streams.next().expect("stream"));
        let named = b.create_layer(&long);
        b.roundtrip();
        assert!(b.layer_info(&named).closed, "namespace too long");
        long.namespace = "n".repeat(NAMESPACE);
        let fits = b.create_layer(&long);
        b.roundtrip();
        assert!(!b.layer_info(&fits).closed);
        // Fill the output: 16 + 15 more + this one.
        let more: Vec<TestLayer> = (0..PER_OUTPUT - PER_CLIENT - 1)
            .map(|_| b.create_layer(&spec))
            .collect();
        b.roundtrip();
        assert!(more.iter().all(|l| !b.layer_info(l).closed));
        let mut c = client_on(streams.next().expect("stream"));
        let over = c.create_layer(&spec);
        c.roundtrip();
        assert!(c.layer_info(&over).closed, "beyond PER_OUTPUT");
        server.on_server(|h| assert_eq!(h.server.state.layers.entries.len(), PER_OUTPUT));
        assert!(
            a.try_roundtrip().is_ok() && b.try_roundtrip().is_ok() && c.try_roundtrip().is_ok()
        );
    });
}

#[test]
fn frame_callbacks_only_while_presenting_and_dmabuf_works() {
    let mut harness = Harness::new();
    let stream = privileged(&mut harness);
    harness.run_with_remote(move |server| {
        let mut p = client_on(stream);
        p.dmabuf_buffers = true;
        let panel = p.map_layer(&LayerSpec::top_panel(30, 0));
        server.on_server(|h| {
            assert_eq!(usable(h).y, 30, "mapped with a dmabuf");
            h.presenting = false;
        });
        let qh = p.qh();
        let done = p.events.frames_done;
        panel.surface.frame(&qh, ());
        panel.surface.commit();
        p.roundtrip();
        server.on_server(|h| {
            for _ in 0..20 {
                h.step();
            }
        });
        p.roundtrip();
        assert_eq!(p.events.frames_done, done, "no frame callback while hidden");
        server.on_server(|h| {
            h.presenting = true;
            h.server.state.request_redraw();
        });
        p.dispatch_until("the frame callback", |e| e.frames_done > done);
    });
}

#[test]
fn popups_input_regions_and_output_assignment() {
    let mut harness = Harness::new();
    let stream = privileged(&mut harness);
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut window = TestClient::connect(&socket);
        let w = window.map_window();
        let mut p = client_on(stream);
        // On sash-wm's output, named explicitly.
        let output = p.bind_output();
        let spec = LayerSpec::top_panel(30, 0);
        let panel = p.create_layer_on(&spec, Some(&output));
        let id = panel.layer.id();
        p.dispatch_until("configure", |e| {
            !e.layers.surfaces[&id].configures.is_empty()
        });
        p.ack_and_commit_layer(&panel);
        server.on_server(|h| assert_eq!(usable(h).y, 30));
        // A popup below the panel, over the window: drawn and hit above it.
        let popup = p.layer_popup(&panel, PopupSpec::default());
        assert_eq!(p.popup_config(&popup), Some((10, 20, 50, 40)));
        server.on_server(|h| h.pointer_to(30.0, 50.0));
        p.roundtrip();
        assert_eq!(p.events.pointer_focus, Some(popup.surface.id()));
        // An overlay with an empty input region lets the pointer through.
        let qh = p.qh();
        let glass = p.create_layer(&centered(Layer::Overlay, KeyboardInteractivity::None));
        let region = p
            .events
            .compositor
            .as_ref()
            .expect("compositor")
            .create_region(&qh, ());
        glass.surface.set_input_region(Some(&region));
        glass.surface.commit();
        let id = glass.layer.id();
        p.dispatch_until("configure", |e| {
            !e.layers.surfaces[&id].configures.is_empty()
        });
        p.ack_and_commit_layer(&glass);
        server.on_server(|h| h.pointer_to(400.0, 300.0));
        window.roundtrip();
        assert_eq!(
            window.events.pointer_focus,
            Some(w.surface.id()),
            "through the overlay"
        );
    });
}

/// The recorder logs layer events with namespaces and sizes; never contents.
#[cfg(feature = "debug-record")]
#[test]
fn the_recorder_logs_layer_events_with_namespaces() {
    use std::time::Instant;

    use crate::record::{self, Config, Recorder};

    let dir = record::tests::scratch_dir("layer-events");
    let mut harness = Harness::new();
    harness.server.state.recorder = Some(
        Recorder::new(
            Config {
                dir: dir.clone(),
                session: false,
                keys_full: false,
                screenshots_full: false,
            },
            Instant::now(),
        )
        .unwrap(),
    );
    let stream = privileged(&mut harness);
    harness.run_with_remote(move |server| {
        let mut p = client_on(stream);
        let mut spec = LayerSpec::top_panel(30, 0);
        spec.namespace = "ns-marker-5e1".into();
        let _panel = p.map_layer(&spec);
        server.on_server(|h| {
            h.chord(&[super::harness::keys::ALT, 87]); // mod+F11
            record::flush_bundle(&mut h.server.state);
        });
    });
    let mut text = String::new();
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e != "png") {
                text += &std::fs::read_to_string(&path).unwrap_or_default();
            }
        }
    }
    for event in ["layer_new", "layer_configure", "layer_map"] {
        assert!(text.contains(event), "{event} missing");
    }
    assert!(text.contains("ns-marker-5e1"), "the namespace is recorded");
    assert!(text.contains("\"zone\""), "the zone is recorded");
}
