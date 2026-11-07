// SPDX-License-Identifier: Apache-2.0

//! sash-ui widgets (M17 E2) end to end: a test-only panel showing a sash-ui `Ui` (a row
//! of buttons) on an in-process privileged connection, driven by sash-wm's own pointer
//! and keyboard input on the headless harness (output 800x600, panel 800x48 at the
//! bottom, keyboard on demand).

use std::{
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};

use sash_ui::{
    Canvas, Edges, IconRaster, IconSource, Keyboard, Layer, Node, NodeId, Panel, PanelSpec,
    Rect as UiRect, Size, Surface, Theme, Ui, UiEvent,
    tree::{Align, Insets},
};
use smithay::backend::input::{ButtonState, KeyState};

use super::{
    harness::{
        Harness,
        keys::{ENTER, RIGHT, TAB},
    },
    panel::{HEIGHT, NAMESPACE, WIDTH, server_buffer},
};
use crate::seat::BTN_LEFT;

/// The panel's top edge on the 800x600 output.
const TOP: i32 = 600 - HEIGHT;

/// A symbolic test icon (icon files come in E3).
struct Square;

impl IconSource for Square {
    fn raster(&self, px: u32, _scale: f32) -> Option<Rc<IconRaster>> {
        let inset = px / 4;
        let mask = (0..px * px)
            .map(|i| {
                let (x, y) = (i % px, i / px);
                let inside = (inset..px - inset).contains(&x) && (inset..px - inset).contains(&y);
                if inside { 255 } else { 0 }
            })
            .collect();
        Some(Rc::new(IconRaster::Mask(mask)))
    }
}

struct Ids {
    files: NodeId,
    terminal: NodeId,
    open: NodeId,
}

fn ui() -> (Ui, Ids) {
    let mut ui = Ui::new(
        Node::row()
            .padding(Insets::all(8))
            .spacing(8)
            .align(Align::Center)
            .width(Size::Flex(1))
            .height(Size::Flex(1)),
        Theme::dark(),
        Surface::Surface,
    );
    let root = ui.root();
    let apps = ui.add(root, Node::row().spacing(4).group().named("Apps"));
    let icon: Rc<dyn IconSource> = Rc::new(Square);
    let files = ui.add(apps, Node::icon_button(icon.clone(), "Files"));
    let terminal = ui.add(apps, Node::icon_button(icon, "Terminal"));
    let open = ui.add(root, Node::button("Open"));
    ui.add(root, Node::label("Sash test panel"));
    assert!(ui.issues().is_empty());
    (
        ui,
        Ids {
            files,
            terminal,
            open,
        },
    )
}

fn spec() -> PanelSpec {
    PanelSpec {
        layer: Layer::Top,
        anchor: Edges::BOTTOM_BAR,
        size: (0, HEIGHT as u32),
        exclusive_zone: HEIGHT,
        margin: 0,
        namespace: NAMESPACE.into(),
        keyboard: Keyboard::OnDemand,
    }
}

fn pump(panel: &mut Panel<Ui>, what: &str, done: impl Fn(&mut Panel<Ui>) -> bool) {
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

fn idle(panel: &mut Panel<Ui>, time: Duration) {
    let end = Instant::now() + time;
    while Instant::now() < end {
        panel
            .dispatch(Some(Duration::from_millis(5)))
            .expect("panel");
    }
}

/// Waits for the panel's events to hold `want`.
fn wait_for(panel: &mut Panel<Ui>, want: UiEvent) {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut seen = Vec::new();
    while !seen.contains(&want) {
        assert!(Instant::now() < deadline, "no {want:?}, saw {seen:?}");
        panel
            .dispatch(Some(Duration::from_millis(5)))
            .expect("panel");
        seen.extend(panel.view_mut().take_events());
    }
}

/// The centre of `id` on the output.
fn on_output(panel: &Panel<Ui>, id: NodeId) -> (f64, f64) {
    let r = panel.view().tree().get(id).expect("node").rect();
    ((r.x + r.w / 2) as f64, (TOP + r.y + r.h / 2) as f64)
}

fn press(server: &super::harness::Remote, key: u32) {
    server.on_server(move |h| {
        h.key(key, KeyState::Pressed);
        h.key(key, KeyState::Released);
    });
}

/// Points at `id` until the view sees it hovered (the panel binds its pointer after
/// the first roundtrip, so an earlier click could reach no pointer object), then
/// clicks it.
fn click(server: &super::harness::Remote, panel: &mut Panel<Ui>, id: NodeId) {
    let (x, y) = on_output(panel, id);
    let deadline = Instant::now() + Duration::from_secs(3);
    while panel.view().hovered() != Some(id) {
        assert!(
            Instant::now() < deadline,
            "the pointer never reached {id:?}"
        );
        server.on_server(move |h| h.pointer_to(x, y + 0.5));
        server.on_server(move |h| h.pointer_to(x, y));
        idle(panel, Duration::from_millis(10));
    }
    server.on_server(move |h| {
        h.button(BTN_LEFT, ButtonState::Pressed);
        h.button(BTN_LEFT, ButtonState::Released);
    });
}

/// The whole panel drawn on the CPU from the view's current state.
fn expected(panel: &mut Panel<Ui>) -> Vec<u8> {
    let mut pixels = vec![0; (WIDTH * HEIGHT * 4) as usize];
    let mut canvas = Canvas::new(
        &mut pixels,
        WIDTH as u32,
        HEIGHT as u32,
        UiRect::sized(WIDTH, HEIGHT),
    )
    .expect("canvas");
    panel.view_mut().draw(&mut canvas);
    pixels
}

fn settled(panel: &mut Panel<Ui>) {
    pump(panel, "the animation to end", |p| {
        !p.view().animating() && p.stats().frame_callbacks == p.stats().commits
    });
}

fn grown(r: UiRect, by: i32) -> UiRect {
    UiRect::new(r.x - by, r.y - by, r.w + 2 * by, r.h + 2 * by)
}

#[test]
fn a_click_activates_a_button_and_the_keyboard_navigates() {
    let mut harness = Harness::new();
    let stream = harness.server.state.privileged_connection_for_tests();
    harness.run_with_remote(move |server| {
        let (ui, ids) = ui();
        let mut panel = Panel::connect(stream, spec(), ui).expect("panel");
        pump(&mut panel, "the first commit", |p| p.stats().commits >= 1);
        click(&server, &mut panel, ids.open);
        wait_for(&mut panel, UiEvent::Activated(ids.open));
        assert_eq!(panel.view().focused(), Some(ids.open));
        assert!(!panel.view().focus_visible(), "a click shows no ring");
        // The click gave the on-demand panel the keyboard: Tab wraps to the group,
        // Right moves inside it, Enter activates.
        press(&server, TAB);
        pump(&mut panel, "Tab", |p| p.view().focused() == Some(ids.files));
        assert!(panel.view().focus_visible());
        press(&server, RIGHT);
        pump(&mut panel, "Right", |p| {
            p.view().focused() == Some(ids.terminal)
        });
        press(&server, ENTER);
        wait_for(&mut panel, UiEvent::Activated(ids.terminal));
        // When the ring has arrived, sash-wm holds exactly the CPU's full draw.
        settled(&mut panel);
        let want = expected(&mut panel);
        server.on_server(move |h| {
            h.run_until("sash-wm to hold the panel's pixels", |h| {
                server_buffer(h).is_some_and(|(_, _, _, px)| px == want)
            });
        });
    });
}

#[test]
fn an_animation_draws_only_while_it_runs_then_idle_is_zero_frames() {
    let mut harness = Harness::new();
    let stream = harness.server.state.privileged_connection_for_tests();
    harness.run_with_remote(move |server| {
        let (ui, ids) = ui();
        let mut panel = Panel::connect(stream, spec(), ui).expect("panel");
        pump(&mut panel, "the first commit", |p| p.stats().commits >= 1);
        click(&server, &mut panel, ids.files);
        wait_for(&mut panel, UiEvent::Activated(ids.files));
        press(&server, RIGHT);
        pump(&mut panel, "Right", |p| {
            p.view().focused() == Some(ids.terminal)
        });
        settled(&mut panel);
        idle(&mut panel, Duration::from_millis(100));
        let before = panel.stats().commits;
        let started = Instant::now();
        // The ring moves from Terminal back to Files over 180 ms.
        press(&server, crate::tests::harness::keys::LEFT);
        pump(&mut panel, "Left", |p| {
            p.view().focused() == Some(ids.files)
        });
        assert!(
            panel.view().animating(),
            "the ring follows the logical focus"
        );
        let rect = |id| panel.view().tree().get(id).expect("node").rect();
        let ring = grown(
            rect(ids.files).bounds(&rect(ids.terminal)),
            // The ring lies 3 px outside a widget; damage adds 4 for its edges.
            sash_ui::ui::RING_GAP + sash_ui::ui::RING_WIDTH + 4,
        );
        // Every commit of the animation damages only the ring's path.
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut seen = panel.stats().commits;
        while panel.view().animating() || panel.stats().frame_callbacks != panel.stats().commits {
            assert!(Instant::now() < deadline, "the animation never ended");
            panel
                .dispatch(Some(Duration::from_millis(5)))
                .expect("panel");
            if panel.stats().commits != seen {
                seen = panel.stats().commits;
                for r in &panel.stats().last_damage {
                    assert!(
                        ring.contains_rect(r),
                        "{r:?} outside the ring's path {ring:?}"
                    );
                }
            }
        }
        let took = started.elapsed();
        let during = panel.stats().commits - before;
        println!("ring animation: {during} commits in {took:?}, each inside {ring:?}");
        assert!(during >= 3, "drawn while it ran: {during} commits");
        // Then nothing: no commits, no compositor frames.
        let (tx, rx) = mpsc::channel();
        let sent = tx.clone();
        server.on_server(move |h| sent.send(h.frames).expect("frames"));
        let frames = rx.recv().expect("frames");
        let commits = panel.stats().commits;
        idle(&mut panel, Duration::from_millis(500));
        server.on_server(move |h| tx.send(h.frames).expect("frames"));
        assert_eq!(
            rx.recv().expect("frames"),
            frames,
            "compositor frames while idle"
        );
        assert_eq!(panel.stats().commits, commits, "panel commits while idle");
    });
}

#[test]
fn hovering_draws_only_the_hovered_button() {
    let mut harness = Harness::new();
    let stream = harness.server.state.privileged_connection_for_tests();
    harness.run_with_remote(move |server| {
        let (ui, ids) = ui();
        let mut panel = Panel::connect(stream, spec(), ui).expect("panel");
        pump(&mut panel, "the first frame", |p| {
            p.stats().frame_callbacks >= 1
        });
        let before = panel.stats().commits;
        let (x, y) = on_output(&panel, ids.open);
        server.on_server(move |h| h.pointer_to(x, y));
        pump(&mut panel, "the hover", |p| {
            p.view().hovered() == Some(ids.open)
        });
        settled(&mut panel);
        let open = grown(panel.view().tree().get(ids.open).expect("open").rect(), 4);
        let last = panel.stats().last_damage.clone();
        assert!(panel.stats().commits > before);
        for r in &last {
            assert!(open.contains_rect(r), "{r:?} outside {open:?}");
        }
        assert_eq!(panel.view().drawn_hover(ids.open), 1.0);
    });
}
