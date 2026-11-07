// SPDX-License-Identifier: Apache-2.0

//! sash-ui's first layer-shell panel (M17 E1), end to end: a test-only panel built
//! with sash-ui (a dev-dependency of sash-wm, owner M17) on an in-process privileged
//! connection, drawing a rectangle and text into wl_shm ABGR8888 on the headless
//! harness (output 800x600). The real spawner is covered by the E6 test (docs/plan.md).

use std::{
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

use sash_text::{ClientText, Image, Style, TextStack};
use sash_ui::{
    Canvas, Color, Damage, Edges, Keyboard, Layer, Panel, PanelSpec, Rect as UiRect, View,
};
use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    reexports::wayland_server::protocol::{wl_shm, wl_surface::WlSurface},
    wayland::shm::with_buffer_contents,
};

use super::harness::Harness;
use crate::layout::Rect;

/// Also the marker tools/check-release.sh looks for: in the test binary, in no sash-wm
/// binary.
pub(super) const NAMESPACE: &str = "sash-ui test panel (tests only)";
pub(super) const HEIGHT: i32 = 48;
pub(super) const WIDTH: i32 = 800;

/// The test panel: the bar fill, an accent square and one line of Manrope.
pub(super) struct TestPanel {
    text: Image,
    square: UiRect,
}

impl TestPanel {
    pub(super) fn new() -> Self {
        let mut stack = TextStack::new();
        let line = stack.line(
            &ClientText::new(b"Sash test panel"),
            Style {
                size: sash_text::UI_SIZE,
                weight: 400,
            },
            None,
        );
        Self {
            text: stack.draw(&line, 1.0, Color::rgb(0xFFFFFF).with_alpha(0.92).rgba()),
            square: UiRect::new(WIDTH - 40, 8, 32, 32),
        }
    }

    /// What the whole buffer should hold: a full draw on the CPU, here.
    pub(super) fn expected(&mut self) -> Vec<u8> {
        let mut pixels = vec![0; (WIDTH * HEIGHT * 4) as usize];
        let mut canvas = Canvas::new(
            &mut pixels,
            WIDTH as u32,
            HEIGHT as u32,
            UiRect::sized(WIDTH, HEIGHT),
        )
        .expect("canvas");
        self.draw(&mut canvas);
        pixels
    }
}

impl View for TestPanel {
    fn draw(&mut self, canvas: &mut Canvas) {
        canvas.fill(UiRect::sized(WIDTH, HEIGHT), Color::rgb(0x2C2C2E));
        canvas.fill(self.square, Color::rgb(0x0A84FF));
        canvas.text(&self.text, 12, 16);
    }
}

/// A full-width bar on the bottom edge reserving its height (visible size plus a
/// margin of 0: the first-party panel rule).
pub(super) fn spec() -> PanelSpec {
    PanelSpec {
        layer: Layer::Top,
        anchor: Edges::BOTTOM_BAR,
        size: (0, HEIGHT as u32),
        exclusive_zone: HEIGHT,
        margin: 0,
        namespace: NAMESPACE.into(),
        keyboard: Keyboard::None,
    }
}

pub(super) fn pump(
    panel: &mut Panel<TestPanel>,
    what: &str,
    done: impl Fn(&Panel<TestPanel>) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !done(panel) {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {:?}",
            panel.stats()
        );
        panel
            .dispatch(Some(Duration::from_millis(5)))
            .expect("panel");
    }
}

/// Dispatches for `time`, whatever arrives.
fn idle(panel: &mut Panel<TestPanel>, time: Duration) {
    let end = Instant::now() + time;
    while Instant::now() < end {
        panel
            .dispatch(Some(Duration::from_millis(5)))
            .expect("panel");
    }
}

pub(super) fn panel_surface(h: &Harness) -> WlSurface {
    h.server
        .state
        .layers
        .entries
        .iter()
        .find(|e| e.namespace == NAMESPACE)
        .expect("the test panel")
        .handle
        .wl_surface()
        .clone()
}

/// What sash-wm holds for the panel: the buffer's format, size and bytes, and the
/// surface's commit counter.
pub(super) fn server_buffer(h: &Harness) -> Option<(wl_shm::Format, i32, i32, Vec<u8>)> {
    let surface = panel_surface(h);
    let buffer = with_renderer_surface_state(&surface, |s| s.buffer().cloned())??;
    with_buffer_contents(&buffer, |ptr, len, data| {
        // Safety: the pool is mapped for `len` bytes while the closure runs.
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        let start = data.offset as usize;
        let rows = (0..data.height as usize).flat_map(|y| {
            let row = start + y * data.stride as usize;
            bytes[row..row + data.width as usize * 4].iter().copied()
        });
        (data.format, data.width, data.height, rows.collect())
    })
    .ok()
}

/// Runs until sash-wm holds a buffer for the panel equal to `expected`.
fn server_shows(h: &mut Harness, expected: &[u8]) {
    h.run_until("sash-wm to hold the panel's pixels", |h| {
        server_buffer(h).is_some_and(|(_, _, _, px)| px == expected)
    });
    let (format, w, hh, _) = server_buffer(h).expect("a buffer");
    assert_eq!(format, wl_shm::Format::Abgr8888);
    assert_eq!((w, hh), (WIDTH, HEIGHT));
}

#[test]
fn a_sash_ui_panel_maps_reserves_and_draws_abgr8888() {
    let mut harness = Harness::new();
    let stream = harness.server.state.privileged_connection_for_tests();
    harness.run_with_remote(move |server| {
        let mut panel = Panel::connect(stream, spec(), TestPanel::new()).expect("panel");
        pump(&mut panel, "the first commit", |p| p.stats().commits == 1);
        assert_eq!(panel.size(), Some((WIDTH as u32, HEIGHT as u32)));
        let expected = panel.view_mut().expected();
        assert_eq!(panel.last_pixels().as_deref(), Some(&expected[..]));
        assert_eq!(
            panel.stats().last_damage,
            vec![UiRect::sized(WIDTH, HEIGHT)]
        );
        server.on_server(move |h| {
            server_shows(h, &expected);
            let entry = h
                .server
                .state
                .layers
                .entries
                .iter()
                .find(|e| e.namespace == NAMESPACE);
            assert_eq!(
                entry.map(|e| e.rect),
                Some(Rect::new(0, 600 - HEIGHT, WIDTH, HEIGHT))
            );
            assert_eq!(
                h.server.state.windows.layout.area(),
                Rect::new(0, 0, WIDTH, 600 - HEIGHT)
            );
        });
    });
}

#[test]
fn an_idle_panel_costs_zero_frames() {
    let mut harness = Harness::new();
    let stream = harness.server.state.privileged_connection_for_tests();
    harness.run_with_remote(move |server| {
        let mut panel = Panel::connect(stream, spec(), TestPanel::new()).expect("panel");
        pump(&mut panel, "the first frame callback", |p| {
            p.stats().frame_callbacks == 1
        });
        let (tx, rx) = std::sync::mpsc::channel();
        let sent = tx.clone();
        server.on_server(move |h| sent.send(h.frames).expect("frames"));
        let before = rx.recv().expect("frames");
        idle(&mut panel, Duration::from_millis(500));
        server.on_server(move |h| tx.send(h.frames).expect("frames"));
        let after = rx.recv().expect("frames");
        println!(
            "idle 500 ms: compositor frames {before} -> {after}, panel commits {}, frame requests {}",
            panel.stats().commits,
            panel.stats().frame_requests
        );
        assert_eq!(after, before, "the compositor drew frames for an idle panel");
        assert_eq!(panel.stats().commits, 1);
        assert_eq!(panel.stats().frame_requests, 1);
    });
}

#[test]
fn a_change_draws_and_uploads_only_its_damage() {
    let mut harness = Harness::new();
    let stream = harness.server.state.privileged_connection_for_tests();
    harness.run_with_remote(move |server| {
        let mut panel = Panel::connect(stream, spec(), TestPanel::new()).expect("panel");
        pump(&mut panel, "the first frame callback", |p| {
            p.stats().frame_callbacks == 1
        });
        let mut moves = Vec::new();
        for step in 1..=4 {
            let (tx, rx) = std::sync::mpsc::channel();
            server.on_server(move |h| {
                let surface = panel_surface(h);
                let commit = with_renderer_surface_state(&surface, |s| s.current_commit());
                tx.send(commit).expect("commit");
            });
            let commit = rx.recv().expect("commit");
            let old = panel.view().square;
            let new = UiRect::new(old.x - 100, old.y, old.w, old.h);
            panel.view_mut().square = new;
            let drawn_before = panel.stats().pixels_drawn;
            panel.damage(old);
            panel.damage(new);
            pump(&mut panel, "the commit", |p| {
                p.stats().commits == step + 1 && p.stats().frame_callbacks == step + 1
            });
            let mut damage = Damage::default();
            damage.add(old);
            damage.add(new);
            assert_eq!(panel.stats().last_damage, damage.rects());
            moves.push(damage.clone());
            let drawn = panel.stats().pixels_drawn - drawn_before;
            // A buffer drawn two frames ago also gets what the other one got since.
            let expected_drawn = match step {
                // The second buffer is new: drawn in full once.
                1 => (WIDTH * HEIGHT) as u64,
                _ => {
                    let mut region = moves[moves.len() - 2].clone();
                    region.add_all(&damage);
                    region.area()
                }
            };
            println!(
                "move {step}: damage {:?}, {drawn} pixels drawn of {}",
                damage.rects(),
                WIDTH * HEIGHT
            );
            assert_eq!(drawn, expected_drawn, "move {step}");
            let expected = panel.view_mut().expected();
            server.on_server(move |h| {
                server_shows(h, &expected);
                let surface = panel_surface(h);
                let seen = with_renderer_surface_state(&surface, |s| {
                    s.damage_since(commit)
                        .iter()
                        .map(|r| UiRect::new(r.loc.x, r.loc.y, r.size.w, r.size.h))
                        .collect::<Vec<_>>()
                })
                .expect("surface state");
                assert_eq!(seen, damage.rects(), "the damage sash-wm got");
            });
        }
        assert_eq!(panel.stats().buffers_created, 2, "two buffers alternate");
    });
}

#[test]
fn damage_during_a_frame_waits_and_merges() {
    let mut harness = Harness::new();
    // The output does not present: the first frame callback is held back.
    harness.presenting = false;
    let stream = harness.server.state.privileged_connection_for_tests();
    harness.run_with_remote(move |server| {
        let mut panel = Panel::connect(stream, spec(), TestPanel::new()).expect("panel");
        pump(&mut panel, "the first commit", |p| p.stats().commits == 1);
        for i in 0..10 {
            panel.damage(UiRect::new(i * 20, 0, 10, 10));
            idle(&mut panel, Duration::from_millis(20));
        }
        assert_eq!(
            panel.stats().commits,
            1,
            "drawn while a frame was in flight"
        );
        server.on_server(|h| h.presenting = true);
        pump(&mut panel, "the merged commit", |p| p.stats().commits == 2);
        idle(&mut panel, Duration::from_millis(200));
        assert_eq!(panel.stats().commits, 2, "one commit for all ten");
        // The ninth rectangle collapses the first nine to their bounds (sash-ui keeps
        // at most 8); the tenth is added beside them.
        assert_eq!(
            panel.stats().last_damage,
            vec![UiRect::new(0, 0, 170, 10), UiRect::new(180, 0, 10, 10)]
        );
        let expected = panel.view_mut().expected();
        server.on_server(move |h| server_shows(h, &expected));
    });
}

#[test]
fn a_normal_client_cannot_open_a_panel() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |_| {
        let stream = UnixStream::connect(socket).expect("public socket");
        match Panel::connect(stream, spec(), TestPanel::new()) {
            Err(sash_ui::panel::Error::Missing(name)) => assert_eq!(name, "zwlr_layer_shell_v1"),
            Err(err) => panic!("unexpected error: {err}"),
            Ok(_) => panic!("a normal client opened a panel"),
        }
    });
}
