// SPDX-License-Identifier: Apache-2.0

//! Widgets (M17 E2): layout, label, icon, button, the focus model, accessible names,
//! keyboard navigation, animation on a fake clock, goldens at scales 1, 1.5 and 2.

mod common;

use std::{rc::Rc, time::Duration};

use proptest::prelude::*;
use sash_text::ClientText;
use sash_ui::{
    Canvas, Damage, IconRaster, IconSource, Key, Node, NodeId, Pointer, Rect, Role, Size, Surface,
    TextLevel, Theme, Ui, UiEvent,
    anim::ANIMATION,
    canvas::to_device,
    tree::{Align, Insets},
};

/// A symbolic test icon: an anti-aliased disc, drawn at any size (icon files come in E3).
struct Dot;

impl IconSource for Dot {
    fn raster(&self, px: u32, _scale: f32) -> Option<Rc<IconRaster>> {
        let c = px as f32 / 2.0;
        let r = px as f32 * 0.4;
        let mask = (0..px * px)
            .map(|i| {
                let (x, y) = ((i % px) as f32 + 0.5, (i / px) as f32 + 0.5);
                let d = ((x - c).powi(2) + (y - c).powi(2)).sqrt();
                ((r + 0.5 - d).clamp(0.0, 1.0) * 255.0).round() as u8
            })
            .collect();
        Some(Rc::new(IconRaster::Mask(mask)))
    }
}

fn dot() -> Rc<dyn IconSource> {
    Rc::new(Dot)
}

const W: i32 = 420;
const H: i32 = 48;

/// A dock-like row: a group of three icon buttons, a button with an icon, an accent
/// button, a secondary label taking the rest, a disabled button.
struct Dock {
    ui: Ui,
    files: NodeId,
    terminal: NodeId,
    settings: NodeId,
    open: NodeId,
    power: NodeId,
    label: NodeId,
    off: NodeId,
}

fn dock(theme: Theme) -> Dock {
    let mut ui = Ui::new(
        Node::row()
            .padding(Insets::all(8))
            .spacing(8)
            .align(Align::Center)
            .width(Size::Flex(1))
            .height(Size::Flex(1)),
        theme,
        Surface::Raised,
    );
    let root = ui.root();
    let apps = ui.add(root, Node::row().spacing(4).group().named("Apps"));
    let files = ui.add(apps, Node::icon_button(dot(), "Files"));
    let terminal = ui.add(apps, Node::icon_button(dot(), "Terminal"));
    let settings = ui.add(apps, Node::icon_button(dot(), "Settings"));
    let open = ui.add(root, Node::button("Open").with_icon(dot()));
    let power = ui.add(root, Node::button("Power off").accent());
    let label = ui.add(
        root,
        Node::label("12:00")
            .level(TextLevel::Secondary)
            .width(Size::Flex(1)),
    );
    let off = ui.add(root, Node::button("Off").disabled(true));
    ui.resize(W, H);
    ui.take_damage();
    Dock {
        ui,
        files,
        terminal,
        settings,
        open,
        power,
        label,
        off,
    }
}

fn ms(v: u64) -> Duration {
    Duration::from_millis(v)
}

fn rect(ui: &Ui, id: NodeId) -> Rect {
    ui.tree().get(id).expect("node").rect()
}

fn centre(ui: &Ui, id: NodeId) -> (f64, f64) {
    let r = rect(ui, id);
    ((r.x + r.w / 2) as f64, (r.y + r.h / 2) as f64)
}

/// Draws the whole surface at `scale`.
fn render(ui: &mut Ui, scale: f32) -> (Vec<u8>, u32, u32) {
    ui.set_scale(scale);
    let full = to_device(Rect::sized(W, H), scale);
    let (w, h) = (full.w as u32, full.h as u32);
    let mut pixels = vec![0; (w * h * 4) as usize];
    let mut canvas = Canvas::new(&mut pixels, w, h, full)
        .expect("canvas")
        .with_scale(scale);
    canvas.clear();
    ui.draw(&mut canvas);
    (pixels, w, h)
}

fn assert_named(ui: &Ui) {
    assert_eq!(ui.issues(), vec![], "interactive widgets without a name");
    for node in ui.semantics() {
        if node.focusable || node.role == Role::Button {
            assert!(node.name.is_some(), "{:?} has no accessible name", node.id);
        }
    }
}

#[test]
fn every_interactive_widget_in_the_test_trees_has_an_accessible_name() {
    assert_named(&dock(Theme::dark()).ui);
    assert_named(&dock(Theme::light()).ui);
}

#[test]
fn a_widget_without_an_accessible_name_fails() {
    for node in [
        Node::icon_button(dot(), ""),
        Node::button(""),
        Node::button("  "),
        Node::icon_button(dot(), ClientText::new(b"")),
    ] {
        let mut ui = Ui::new(Node::row(), Theme::dark(), Surface::Raised);
        let root = ui.root();
        let added = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let id = ui.add(root, node);
            (id, ui.issues())
        }));
        if cfg!(debug_assertions) {
            assert!(added.is_err(), "debug builds panic on a nameless widget");
        } else {
            let (id, issues) = added.expect("release builds report it");
            assert_eq!(issues, vec![sash_ui::tree::Issue::NoName(id)]);
        }
    }
}

#[test]
fn semantics_have_roles_names_states_and_actions() {
    let d = dock(Theme::dark());
    let sem = d.ui.semantics();
    let get = |id| sem.iter().find(|s| s.id == id).expect("in the semantics");
    assert_eq!(get(d.files).role, Role::Button);
    assert!(matches!(get(d.files).name, Some(sash_ui::Text::Own(n)) if n == "Files"));
    assert_eq!(get(d.files).actions, &[sash_ui::tree::Action::Activate]);
    assert!(get(d.off).disabled && !get(d.off).focusable);
    assert_eq!(get(d.label).role, Role::Label);
    assert_eq!(get(d.label).actions, &[]);
    // The group is announced with its name; the icons inside buttons are not separate.
    assert!(sem.iter().any(|s| s.role == Role::Group));
    assert!(sem.iter().all(|s| s.role != Role::Image));
}

#[test]
fn tab_follows_reading_order_and_wraps() {
    let mut d = dock(Theme::dark());
    let mut seen = Vec::new();
    for _ in 0..4 {
        d.ui.key(Key::Tab, ms(0));
        seen.push(d.ui.focused().expect("focus"));
    }
    // The group is one stop (entered at its first item); the disabled button is no stop.
    assert_eq!(seen, vec![d.files, d.open, d.power, d.files]);
    // Reading order is left to right here.
    let xs: Vec<i32> = seen[..3].iter().map(|&id| rect(&d.ui, id).x).collect();
    assert!(xs.windows(2).all(|w| w[0] < w[1]), "{xs:?}");
    d.ui.key(Key::BackTab, ms(0));
    assert_eq!(d.ui.focused(), Some(d.power));
    assert!(d.ui.focus_visible());
}

#[test]
fn arrows_move_inside_a_group_without_wrapping() {
    let mut d = dock(Theme::dark());
    d.ui.key(Key::Tab, ms(0));
    for (key, want) in [
        (Key::Left, d.files),
        (Key::Right, d.terminal),
        (Key::Right, d.settings),
        (Key::Right, d.settings),
        (Key::Up, d.settings),
        (Key::Home, d.files),
        (Key::End, d.settings),
    ] {
        d.ui.key(key, ms(0));
        assert_eq!(d.ui.focused(), Some(want), "{key:?}");
    }
    // Outside a group arrows move nothing.
    d.ui.set_focus(Some(d.open));
    d.ui.key(Key::Right, ms(0));
    assert_eq!(d.ui.focused(), Some(d.open));
}

#[test]
fn a_group_is_entered_where_it_was_left() {
    let mut d = dock(Theme::dark());
    d.ui.key(Key::Tab, ms(0));
    d.ui.key(Key::End, ms(0));
    d.ui.key(Key::Tab, ms(0));
    assert_eq!(d.ui.focused(), Some(d.open));
    d.ui.key(Key::BackTab, ms(0));
    assert_eq!(d.ui.focused(), Some(d.settings));
}

#[test]
fn a_vertical_group_uses_up_and_down() {
    let mut ui = Ui::new(Node::column().group(), Theme::dark(), Surface::Raised);
    let root = ui.root();
    let a = ui.add(root, Node::button("A"));
    let b = ui.add(root, Node::button("B"));
    ui.resize(200, 200);
    ui.key(Key::Tab, ms(0));
    ui.key(Key::Right, ms(0));
    assert_eq!(ui.focused(), Some(a));
    ui.key(Key::Down, ms(0));
    assert_eq!(ui.focused(), Some(b));
    ui.key(Key::Down, ms(0));
    assert_eq!(ui.focused(), Some(b));
}

#[test]
fn every_control_is_reachable_and_activated_by_keyboard() {
    let mut d = dock(Theme::dark());
    let want: Vec<NodeId> =
        d.ui.semantics()
            .iter()
            .filter(|s| s.focusable)
            .map(|s| s.id)
            .collect();
    let mut reached = Vec::new();
    // Tab through every stop; inside a group, arrows through every item.
    for _ in 0..want.len() + 1 {
        d.ui.key(Key::Tab, ms(0));
        d.ui.key(Key::Home, ms(0));
        loop {
            let f = d.ui.focused().expect("focus");
            if !reached.contains(&f) {
                reached.push(f);
            }
            d.ui.key(Key::Enter, ms(0));
            assert_eq!(d.ui.take_events(), vec![UiEvent::Activated(f)]);
            d.ui.key(Key::Right, ms(0));
            if d.ui.focused() == Some(f) {
                break;
            }
        }
    }
    reached.sort();
    let mut want = want;
    want.sort();
    assert_eq!(reached, want);
    d.ui.key(Key::Space, ms(0));
    assert_eq!(d.ui.take_events().len(), 1, "Space activates too");
    d.ui.key(Key::Escape, ms(0));
    assert_eq!(d.ui.take_events(), vec![UiEvent::Escape]);
}

#[test]
fn focus_is_never_lost() {
    let mut d = dock(Theme::dark());
    d.ui.set_focus(Some(d.terminal));
    d.ui.remove(d.terminal);
    assert_eq!(d.ui.focused(), Some(d.settings), "the next one");
    d.ui.set_disabled(d.settings, true);
    assert_eq!(d.ui.focused(), Some(d.open), "disabled: the next one");
    d.ui.set_focus(Some(d.power));
    d.ui.set_disabled(d.power, true);
    assert_eq!(
        d.ui.focused(),
        Some(d.open),
        "the last enabled one: the previous"
    );
    // A whole subtree going away moves focus out of it.
    d.ui.set_focus(Some(d.files));
    let apps =
        d.ui.tree()
            .get(d.files)
            .and_then(|n| n.parent())
            .expect("group");
    d.ui.remove(apps);
    assert_eq!(d.ui.focused(), Some(d.open));
    d.ui.remove(d.open);
    assert_eq!(d.ui.focused(), None, "nothing focusable is left");
    assert_named(&d.ui);
}

#[test]
fn a_click_activates_and_focuses_without_the_ring() {
    let mut d = dock(Theme::dark());
    let (x, y) = centre(&d.ui, d.open);
    d.ui.pointer(Pointer::Motion { x, y }, ms(0));
    d.ui.pointer(Pointer::Press { x, y }, ms(10));
    assert_eq!(d.ui.pressed(), Some(d.open));
    assert_eq!(d.ui.focused(), Some(d.open));
    assert!(!d.ui.focus_visible());
    assert!(d.ui.take_events().is_empty(), "activation is on release");
    d.ui.pointer(Pointer::Release { x, y }, ms(20));
    assert_eq!(d.ui.take_events(), vec![UiEvent::Activated(d.open)]);
    // Then the keyboard shows the ring where focus is.
    d.ui.key(Key::Tab, ms(30));
    assert_eq!(d.ui.focused(), Some(d.power));
    assert!(d.ui.focus_visible());
}

#[test]
fn a_release_elsewhere_or_on_a_disabled_button_does_nothing() {
    let mut d = dock(Theme::dark());
    let (x, y) = centre(&d.ui, d.open);
    let (x2, y2) = centre(&d.ui, d.power);
    d.ui.pointer(Pointer::Press { x, y }, ms(0));
    d.ui.pointer(Pointer::Release { x: x2, y: y2 }, ms(10));
    assert!(d.ui.take_events().is_empty());
    let (x, y) = centre(&d.ui, d.off);
    d.ui.pointer(Pointer::Press { x, y }, ms(20));
    d.ui.pointer(Pointer::Release { x, y }, ms(30));
    assert!(d.ui.take_events().is_empty());
    assert_eq!(d.ui.hovered(), None);
    assert_ne!(d.ui.focused(), Some(d.off));
}

#[test]
fn logical_state_changes_before_any_frame() {
    let mut d = dock(Theme::dark());
    d.ui.key(Key::Tab, ms(0));
    d.ui.advance(ms(1000));
    let at_files = d.ui.drawn_ring().expect("ring");
    d.ui.key(Key::Right, ms(1000));
    // Focus moved at once; the ring is still drawn where it was.
    assert_eq!(d.ui.focused(), Some(d.terminal));
    assert_eq!(d.ui.drawn_ring(), Some(at_files));
    assert_ne!(d.ui.ring_target(), Some(at_files));
    assert!(d.ui.animating());
    // Input acts on the new state, with no frame in between.
    d.ui.key(Key::Enter, ms(1000));
    assert_eq!(d.ui.take_events(), vec![UiEvent::Activated(d.terminal)]);
    let sem = d.ui.semantics();
    assert!(sem.iter().any(|s| s.id == d.terminal && s.focused));
    // Hover and press are logical at once too.
    let (x, y) = centre(&d.ui, d.open);
    d.ui.pointer(Pointer::Motion { x, y }, ms(1000));
    assert_eq!(d.ui.hovered(), Some(d.open));
    assert_eq!(d.ui.drawn_hover(d.open), 0.0);
    d.ui.advance(ms(1000) + ANIMATION);
    assert_eq!(d.ui.drawn_hover(d.open), 1.0);
    assert_eq!(d.ui.drawn_ring(), d.ui.ring_target());
}

#[test]
fn an_interrupted_animation_starts_from_where_it_is_drawn() {
    let mut d = dock(Theme::dark());
    d.ui.key(Key::Tab, ms(0));
    d.ui.advance(ms(500));
    let a = d.ui.drawn_ring().expect("ring");
    d.ui.key(Key::Right, ms(500));
    let b = d.ui.ring_target().expect("target");
    d.ui.advance(ms(590));
    let mid = d.ui.drawn_ring().expect("ring");
    assert!(mid.x > a.x && mid.x < b.x, "half-way: {a:?} {mid:?} {b:?}");
    // A new change half-way: the ring heads for the third button from where it is.
    d.ui.key(Key::Right, ms(590));
    assert_eq!(d.ui.focused(), Some(d.settings));
    assert_eq!(d.ui.drawn_ring(), Some(mid), "no jump");
    let c = d.ui.ring_target().expect("target");
    assert!(c.x > b.x);
    d.ui.advance(ms(600));
    let next = d.ui.drawn_ring().expect("ring");
    assert!(next.x >= mid.x && next.x <= c.x);
    // It ends a full animation after the interruption, not after the first change.
    d.ui.advance(ms(590) + ANIMATION - ms(1));
    assert!(d.ui.animating());
    d.ui.advance(ms(590) + ANIMATION);
    assert_eq!(d.ui.drawn_ring(), Some(c));
    assert!(!d.ui.animating());
}

#[test]
fn frames_stop_when_the_animation_ends() {
    let mut d = dock(Theme::dark());
    d.ui.key(Key::Tab, ms(0));
    d.ui.advance(ms(500));
    d.ui.take_damage();
    d.ui.key(Key::Right, ms(500));
    assert!(!d.ui.take_damage().is_empty());
    let mut frames = 0;
    let mut t = 500;
    while d.ui.animating() {
        t += 16;
        d.ui.advance(ms(t));
        assert!(!d.ui.take_damage().is_empty(), "a running animation draws");
        frames += 1;
    }
    assert_eq!(frames, 12, "180 ms at 16 ms per frame");
    for _ in 0..10 {
        t += 16;
        d.ui.advance(ms(t));
        assert!(d.ui.take_damage().is_empty(), "idle: nothing to draw");
    }
}

#[test]
fn damage_covers_only_the_changed_widget() {
    let mut d = dock(Theme::dark());
    let (x, y) = centre(&d.ui, d.open);
    d.ui.pointer(Pointer::Motion { x, y }, ms(0));
    let damage = d.ui.take_damage();
    let bounds = damage.bounds();
    let open = rect(&d.ui, d.open);
    // The widget plus the ring (3 px off, 2 px wide) and 1 px of anti-aliasing,
    // nothing else.
    let m = sash_ui::ui::RING_GAP + sash_ui::ui::RING_WIDTH + 1;
    let grown = Rect::new(open.x - m, open.y - m, open.w + 2 * m, open.h + 2 * m);
    assert!(grown.contains_rect(&bounds), "{bounds:?} in {grown:?}");
    for other in [d.files, d.terminal, d.settings, d.label, d.off] {
        assert!(
            rect(&d.ui, other).intersect(&bounds).is_empty(),
            "{other:?} untouched"
        );
    }
}

#[test]
fn client_text_never_prints() {
    const MARKER: &str = "untrusted-title-marker-4711";
    let mut ui = Ui::new(Node::row(), Theme::dark(), Surface::Raised);
    let root = ui.root();
    let label = ui.add(root, Node::label(ClientText::from(MARKER)));
    ui.add(
        root,
        Node::icon_button(dot(), ClientText::from(MARKER)).named(ClientText::from(MARKER)),
    );
    ui.resize(400, 40);
    let printed = format!(
        "{:?} {:?} {:?}",
        ui.tree(),
        ui.semantics(),
        ui.tree().get(label)
    );
    assert!(!printed.contains(MARKER), "client text was printed");
    // It is drawn, through sash-text.
    let mut pixels = vec![0; 400 * 40 * 4];
    let mut canvas = Canvas::new(&mut pixels, 400, 40, Rect::sized(400, 40)).expect("canvas");
    ui.draw(&mut canvas);
    assert!(pixels.chunks(4).any(|p| p[0] > 200), "the label shows");
}

#[test]
fn the_focus_ring_adds_a_shape_outside_the_widget() {
    let mut plain = dock(Theme::dark());
    let mut ringed = dock(Theme::dark());
    ringed.ui.set_focus(Some(ringed.open));
    for scale in [1.0, 1.5, 2.0] {
        let (a, w, _) = render(&mut plain.ui, scale);
        let (b, _, _) = render(&mut ringed.ui, scale);
        let widget = to_device(rect(&ringed.ui, ringed.open), scale);
        let ring = to_device(ringed.ui.ring_target().expect("ring"), scale);
        // The widget's corners are rounded: outside its arcs is background, which the
        // ring's inner arc may cover (it keeps the 1 px gap to the arc).
        let r = (widget.w.min(widget.h) as f32 * sash_ui::ui::RADIUS_FRACTION).ceil() as i32;
        let in_corner = |x: i32, y: i32| {
            (x < widget.x + r || x >= widget.right() - r)
                && (y < widget.y + r || y >= widget.bottom() - r)
        };
        let mut changed = 0;
        for (i, (pa, pb)) in a.chunks(4).zip(b.chunks(4)).enumerate() {
            if pa == pb {
                continue;
            }
            let (x, y) = ((i as u32 % w) as i32, (i as u32 / w) as i32);
            assert!(
                !widget.contains(x, y) || in_corner(x, y),
                "scale {scale}: the widget itself changed at ({x}, {y})"
            );
            assert!(
                ring.contains(x, y),
                "scale {scale}: a change outside the ring"
            );
            changed += 1;
        }
        // At least the ring's straight sides, 2 px (scaled) wide.
        let min = (2.0 * scale) as i32 * (widget.w + widget.h);
        assert!(changed >= min, "scale {scale}: {changed} < {min}");
    }
}

#[test]
fn disabled_text_uses_the_secondary_token() {
    let mut d = dock(Theme::dark());
    let (pixels, w, _) = render(&mut d.ui, 1.0);
    let brightest = |r: Rect| {
        let mut max = 0u8;
        for y in r.y..r.bottom() {
            for x in r.x..r.right() {
                let i = ((y as u32 * w + x as u32) * 4) as usize;
                max = max.max(pixels[i]);
            }
        }
        max
    };
    let on = brightest(rect(&d.ui, d.open));
    let off = brightest(rect(&d.ui, d.off));
    // White at 92% over raised #22282C is about 237; at 62% (secondary) about 171.
    assert!(on > 220, "enabled text {on}");
    assert!((150..190).contains(&off), "disabled text {off}");
}

#[test]
fn goldens_at_scales_1_1_5_and_2() {
    for (theme, name) in [(Theme::dark(), "dark"), (Theme::light(), "light")] {
        let mut d = dock(theme);
        // The ring on Terminal, Open hovered: every state in one image.
        d.ui.key(Key::Tab, ms(0));
        d.ui.key(Key::Right, ms(0));
        let (x, y) = centre(&d.ui, d.open);
        d.ui.pointer(Pointer::Motion { x, y }, ms(0));
        d.ui.advance(ANIMATION);
        assert_named(&d.ui);
        let scales: &[f32] = if name == "dark" {
            &[1.0, 1.5, 2.0]
        } else {
            &[1.0]
        };
        for &scale in scales {
            let (pixels, w, h) = render(&mut d.ui, scale);
            let tag = format!("{scale}").replace('.', "_");
            common::golden(&format!("widgets-{name}-x{tag}"), &pixels, w, h);
        }
    }
}

#[test]
fn a_redraw_of_any_damage_matches_a_full_draw() {
    let mut d = dock(Theme::dark());
    let (x, y) = centre(&d.ui, d.open);
    d.ui.key(Key::Tab, ms(0));
    let (before, w, h) = render(&mut d.ui, 1.0);
    d.ui.take_damage();
    // Changes: hover, the ring moving half-way, a press.
    d.ui.pointer(Pointer::Motion { x, y }, ms(10));
    d.ui.key(Key::Right, ms(10));
    d.ui.advance(ms(100));
    let (px, py) = centre(&d.ui, d.power);
    d.ui.pointer(Pointer::Press { x: px, y: py }, ms(100));
    let damage: Damage = d.ui.take_damage();
    let mut partial = before;
    for r in damage.rects() {
        let mut canvas = Canvas::new(&mut partial, w, h, *r).expect("canvas");
        canvas.clear();
        d.ui.draw(&mut canvas);
    }
    let (full, _, _) = render(&mut d.ui, 1.0);
    assert!(partial == full, "redrawing the damage gives the full draw");
}

fn row_of(sizes: &[(Size, i32)], width: i32, spacing: i32) -> (Ui, Vec<NodeId>) {
    let mut ui = Ui::new(
        Node::row()
            .spacing(spacing)
            .padding(Insets::all(3))
            .width(Size::Flex(1)),
        Theme::dark(),
        Surface::Raised,
    );
    let root = ui.root();
    let ids = sizes
        .iter()
        .map(|&(size, h)| ui.add(root, Node::stack().width(size).height(Size::Fixed(h))))
        .collect();
    ui.resize(width, 40);
    (ui, ids)
}

#[test]
fn flex_children_share_the_rest_exactly() {
    let (ui, ids) = row_of(
        &[
            (Size::Fixed(10), 5),
            (Size::Flex(1), 5),
            (Size::Flex(2), 5),
            (Size::Flex(1), 5),
        ],
        103,
        2,
    );
    let rects: Vec<Rect> = ids.iter().map(|&id| rect(&ui, id)).collect();
    // 103 - 6 padding - 10 fixed - 6 spacing = 81 for weights 1:2:1 -> 20/40/20 + 1.
    assert_eq!(
        rects.iter().map(|r| r.w).collect::<Vec<_>>(),
        vec![10, 21, 40, 20]
    );
    assert_eq!(rects[0].x, 3);
    assert_eq!(rects[3].right(), 100);
}

#[test]
fn alignment_and_stacks() {
    let mut ui = Ui::new(
        Node::column().align(Align::Center).width(Size::Flex(1)),
        Theme::dark(),
        Surface::Raised,
    );
    let root = ui.root();
    let a = ui.add(
        root,
        Node::stack().width(Size::Fixed(20)).height(Size::Fixed(10)),
    );
    let s = ui.add(
        root,
        Node::stack()
            .align(Align::End)
            .width(Size::Fixed(50))
            .height(Size::Fixed(30)),
    );
    let inner = ui.add(
        s,
        Node::stack().width(Size::Fixed(10)).height(Size::Fixed(10)),
    );
    ui.resize(100, 100);
    assert_eq!(rect(&ui, a), Rect::new(40, 0, 20, 10));
    assert_eq!(rect(&ui, s), Rect::new(25, 10, 50, 30));
    assert_eq!(rect(&ui, inner), Rect::new(65, 30, 10, 10));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn layout_keeps_children_inside_and_apart(
        kids in proptest::collection::vec((0u8..3, 0i32..80, 0u16..4, 0i32..60), 1..8),
        width in 0i32..400,
        spacing in 0i32..10,
    ) {
        let sizes: Vec<(Size, i32)> = kids
            .iter()
            .map(|&(kind, v, w, h)| {
                let size = match kind {
                    0 => Size::Fixed(v),
                    1 => Size::Flex(w),
                    _ => Size::Content,
                };
                (size, h)
            })
            .collect();
        let (ui, ids) = row_of(&sizes, width, spacing);
        let parent = rect(&ui, ui.root());
        let inner = Rect::new(parent.x + 3, parent.y + 3, (parent.w - 6).max(0), (parent.h - 6).max(0));
        let rects: Vec<Rect> = ids.iter().map(|&id| rect(&ui, id)).collect();
        for r in &rects {
            prop_assert!(r.is_empty() || inner.contains_rect(r), "{r:?} outside {inner:?}");
        }
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                prop_assert!(a.intersect(b).is_empty(), "{a:?} overlaps {b:?}");
            }
        }
        // With a flexible child and room to spare, the children cover the row exactly.
        let fixed: i32 = sizes.iter().map(|(s, _)| if let Size::Fixed(v) = s { *v } else { 0 }).sum();
        let gaps = spacing * (rects.len() as i32 - 1);
        let flexible = sizes.iter().any(|(s, _)| matches!(s, Size::Flex(w) if *w > 0));
        if flexible && fixed + gaps <= inner.w {
            let covered: i32 = rects.iter().map(|r| r.w).sum::<i32>() + gaps;
            prop_assert_eq!(covered, inner.w);
        }
        // Deterministic.
        let (again, ids2) = row_of(&sizes, width, spacing);
        let rects2: Vec<Rect> = ids2.iter().map(|&id| rect(&again, id)).collect();
        prop_assert_eq!(rects, rects2);
    }

    #[test]
    fn neighbours_stay_adjacent_at_any_scale(
        x in -100i32..100, y in -100i32..100, w in 0i32..200, w2 in 0i32..200, h in 0i32..100,
        scale in prop::sample::select(vec![1.0f32, 1.25, 1.5, 1.75, 2.0, 3.0]),
    ) {
        let a = to_device(Rect::new(x, y, w, h), scale);
        let b = to_device(Rect::new(x + w, y, w2, h), scale);
        prop_assert_eq!(a.right(), b.x);
        prop_assert_eq!(a.y, b.y);
        prop_assert_eq!(a.h, b.h);
    }
}

#[test]
fn a_filled_button_has_an_outline_of_at_least_3_to_1() {
    // Owner, after E2: never only a fill (the accent fill is under 3:1 on dark).
    for theme in [Theme::dark(), Theme::light()] {
        let mut d = dock(theme);
        let (pixels, w, _) = render(&mut d.ui, 1.0);
        let r = rect(&d.ui, d.power);
        // The middle of the top edge: the outline's pixel.
        let (x, y) = (r.x + r.w / 2, r.y);
        let i = ((y as u32 * w + x as u32) * 4) as usize;
        let edge = sash_ui::Color {
            r: pixels[i],
            g: pixels[i + 1],
            b: pixels[i + 2],
            a: 255,
        };
        let surface = theme.surface_color(Surface::Raised);
        let ratio = sash_ui::theme::contrast(edge, surface);
        assert_eq!(edge, theme.accent(), "the outline is the accent");
        assert!(ratio >= 3.0, "{ratio}");
    }
}
