// SPDX-License-Identifier: Apache-2.0

//! Property tests: random sequences of operations on random output sizes keep the
//! monitor layout's invariants (`MonitorLayout::check_invariants`), and bulk toggles
//! are lossless.

use proptest::prelude::*;

use super::{
    Direction, Edges, MIN_TILE_SIZE, Mode, MonitorLayout, NewWindow, Rect, SizeLimits, WindowId,
    ZoneId,
    gaps::Gaps,
    layers::{self, LayerId, LayerSurface, Level, Margins},
    monitor::Layer,
    resize_zone,
};

/// What the operations change besides the layout: window ids and the output's layer
/// surfaces (panels).
#[derive(Default)]
struct World {
    next: u64,
    panels: Vec<LayerSurface>,
    /// Ids and mapping order of panels.
    panel_serial: u64,
}

impl World {
    /// Applies the output and the current panels to the layout in one update, as the
    /// compositor does (`set_output_and_layers`).
    fn apply_geometry(&self, layout: &mut MonitorLayout, output: Rect) {
        let usable = layers::arrange(output, &self.panels).usable;
        layout.set_geometry(output, usable);
    }

    fn add_panel(&mut self, mut panel: LayerSurface) {
        self.panel_serial += 1;
        panel.id = LayerId(self.panel_serial);
        panel.mapped = self.panel_serial;
        self.panels.push(panel);
    }

    fn change_panel(&mut self, index: usize, panel: LayerSurface) {
        if self.panels.is_empty() {
            return;
        }
        let i = index % self.panels.len();
        let (id, mapped) = (self.panels[i].id, self.panels[i].mapped);
        self.panels[i] = LayerSurface {
            id,
            mapped,
            ..panel
        };
    }

    fn remove_panel(&mut self, index: usize) {
        if !self.panels.is_empty() {
            let i = index % self.panels.len();
            self.panels.remove(i);
        }
    }
}

/// A panel change.
#[derive(Clone, Debug)]
enum PanelOp {
    Add(LayerSurface),
    Change(usize, LayerSurface),
    Remove(usize),
}

impl PanelOp {
    fn apply(&self, world: &mut World) {
        match self {
            PanelOp::Add(panel) => world.add_panel(*panel),
            PanelOp::Change(i, panel) => world.change_panel(*i, *panel),
            PanelOp::Remove(i) => world.remove_panel(*i),
        }
    }
}

#[derive(Clone, Debug)]
enum Op {
    /// Opens a window; `forced` with an optional parent (index of an existing window).
    Open {
        forced: bool,
        parent: Option<usize>,
        fixed: Option<(i32, i32)>,
        /// Where the pointer is (rule A), if anywhere.
        pointer: Option<(i32, i32)>,
        /// It maps at once (focus moves to it); else focus stays until a later
        /// `Focus` (its map, M9e).
        mapped: bool,
        /// Server-side decorations negotiated (a bar while floating).
        server_side: bool,
    },
    /// The client negotiates (or drops) server-side decorations later.
    SetServerSide(usize, bool),
    /// Gives an unplaced floating window its size (the client's first buffer).
    Place(usize, (i32, i32)),
    Close(usize),
    Toggle(usize),
    FloatAll,
    TileAll,
    MoveBy(usize, i32, i32),
    ResizeBy(usize, i32, i32),
    /// A drag or client-chosen rectangle.
    SetRect(usize, Rect),
    Focus(usize),
    FocusDirection(Direction),
    Swap(Direction),
    SetForced(usize, bool, Option<usize>),
    SetArea(Rect),
    SetDefault(Mode),
    Snap(usize, u16),
    SnapKey(usize, Direction),
    /// mod+Y/U/B/N: one of the four quarters.
    SnapQuarter(usize, u8),
    Unsnap(usize),
    /// A drag pulls a snapped window out at this pointer position.
    UnsnapDrag(usize, i32, i32),
    Maximize(usize, bool),
    Fullscreen(usize, bool),
    /// mod+Tab: preview a window, then commit.
    Cycle(usize),
    /// mod+right-drag on a tiled window from a 3x3 cell by (dx, dy).
    TiledDrag(usize, (u8, u8), i32, i32),
    /// Panels appear, change or disappear.
    Panels(Vec<PanelOp>),
    /// An output resize (or mode change) and panel changes in the same frame: one
    /// update.
    ResizeWithPanels(Rect, Vec<PanelOp>),
    /// The gaps (M18a; the debug overrides in a real session): outer, inner.
    SetGaps(i32, i32),
    /// A move drag of a tile released at this point (owner, M19c); `home`: at a point
    /// inside its own visible rectangle instead (the coordinates pick it).
    DropDrag(usize, i32, i32, bool),
}

/// Every tile's visible rectangle (owner, M19b: the minimum is measured on it).
fn visible_tiles(layout: &MonitorLayout) -> Vec<(WindowId, Rect)> {
    layout
        .tiled_rects()
        .into_iter()
        .map(|(w, cell)| (w, layout.inset(cell)))
        .collect()
}

/// A tiled resize never makes a tile's visible rectangle smaller than
/// min(MIN_TILE_SIZE, its size before) on either axis.
fn check_tile_minimum(before: &[(WindowId, Rect)], after: &[(WindowId, Rect)]) {
    for (w, old) in before {
        if let Some((_, new)) = after.iter().find(|(a, _)| a == w) {
            assert!(
                new.w >= old.w.min(MIN_TILE_SIZE.0),
                "{w:?} width {} -> {}",
                old.w,
                new.w
            );
            assert!(
                new.h >= old.h.min(MIN_TILE_SIZE.1),
                "{w:?} height {} -> {}",
                old.h,
                new.h
            );
        }
    }
}

fn direction() -> impl Strategy<Value = Direction> {
    prop_oneof![
        Just(Direction::Left),
        Just(Direction::Right),
        Just(Direction::Up),
        Just(Direction::Down)
    ]
}

fn area() -> impl Strategy<Value = Rect> {
    prop_oneof![
        4 => (-2000..2000i32, -2000..2000i32, 1..8000i32, 1..5000i32),
        1 => (0..2i32, 0..2i32, 1..6i32, 1..6i32),
    ]
    .prop_map(|(x, y, w, h)| Rect::new(x, y, w, h))
}

/// A layer surface; mostly reserving panels on one edge, sometimes anything.
fn panel() -> impl Strategy<Value = LayerSurface> {
    let edge = prop_oneof![
        Just("t"),
        Just("b"),
        Just("l"),
        Just("r"),
        Just("tlr"),
        Just("blr"),
        Just("ltb"),
        Just("rtb"),
        Just("tl"),
        Just("lr"),
        Just(""),
        Just("tlrb"),
    ];
    (
        edge,
        prop_oneof![Just(Level::Top), Just(Level::Bottom), Just(Level::Overlay)],
        (0u32..600, 0u32..600),
        prop_oneof![4 => 1i32..700, 1 => Just(0), 1 => Just(-1)],
        (-20i32..40, -20i32..40),
    )
        .prop_map(|(edge, level, size, exclusive, (m1, m2))| LayerSurface {
            id: LayerId(0),
            level,
            anchor: Edges {
                left: edge.contains('l'),
                right: edge.contains('r'),
                top: edge.contains('t'),
                bottom: edge.contains('b'),
            },
            margin: Margins {
                top: m1,
                right: m2,
                bottom: m1,
                left: m2,
            },
            size,
            exclusive,
            mapped: 0,
        })
}

fn panel_op() -> impl Strategy<Value = PanelOp> {
    prop_oneof![
        3 => panel().prop_map(PanelOp::Add),
        2 => (any::<usize>(), panel()).prop_map(|(i, p)| PanelOp::Change(i, p)),
        2 => any::<usize>().prop_map(PanelOp::Remove),
    ]
}

fn size() -> impl Strategy<Value = (i32, i32)> {
    (1..3000i32, 1..3000i32)
}

/// A gap for the property tests: none, the defaults (13 and 8), or anything up to 64.
fn gap() -> impl Strategy<Value = i32> {
    prop_oneof![Just(0i32), Just(8), Just(13), 1i32..65]
}

fn op() -> impl Strategy<Value = Op> {
    let index = any::<usize>();
    prop_oneof![
        4 => (
            any::<bool>(),
            prop::option::of(any::<usize>()),
            prop::option::of(size()),
            prop::option::of((-500..4500i32, -500..4500i32)),
            any::<bool>(),
            any::<bool>(),
        )
            .prop_map(|(forced, parent, fixed, pointer, mapped, server_side)| Op::Open {
                forced,
                parent,
                fixed,
                pointer,
                mapped,
                server_side,
            }),
        1 => (index, any::<bool>()).prop_map(|(i, s)| Op::SetServerSide(i, s)),
        3 => (index, size()).prop_map(|(i, s)| Op::Place(i, s)),
        2 => index.prop_map(Op::Close),
        2 => index.prop_map(Op::Toggle),
        1 => Just(Op::FloatAll),
        1 => Just(Op::TileAll),
        2 => (index, -3000..3000i32, -3000..3000i32).prop_map(|(i, x, y)| Op::MoveBy(i, x, y)),
        2 => (index, -2000..2000i32, -2000..2000i32).prop_map(|(i, w, h)| Op::ResizeBy(i, w, h)),
        2 => (index, -5000..5000i32, -5000..5000i32, -100..4000i32, -100..4000i32)
            .prop_map(|(i, x, y, w, h)| Op::SetRect(i, Rect::new(x, y, w.max(0), h.max(0)))),
        2 => index.prop_map(Op::Focus),
        2 => direction().prop_map(Op::FocusDirection),
        1 => direction().prop_map(Op::Swap),
        1 => (index, any::<bool>(), prop::option::of(any::<usize>()))
            .prop_map(|(i, f, p)| Op::SetForced(i, f, p)),
        1 => area().prop_map(Op::SetArea),
        3 => prop::collection::vec(panel_op(), 1..4).prop_map(Op::Panels),
        1 => (area(), prop::collection::vec(panel_op(), 1..4))
            .prop_map(|(a, p)| Op::ResizeWithPanels(a, p)),
        1 => prop_oneof![Just(Mode::Tiled), Just(Mode::Floating)].prop_map(Op::SetDefault),
        1 => (gap(), gap()).prop_map(|(o, i)| Op::SetGaps(o, i)),
        3 => (index, 0..8u16).prop_map(|(i, z)| Op::Snap(i, z)),
        3 => (index, direction()).prop_map(|(i, d)| Op::SnapKey(i, d)),
        3 => (index, 0..4u8).prop_map(|(i, q)| Op::SnapQuarter(i, q)),
        1 => index.prop_map(Op::Unsnap),
        1 => (index, -500..3000i32, -500..3000i32).prop_map(|(i, x, y)| Op::UnsnapDrag(i, x, y)),
        2 => (index, any::<bool>()).prop_map(|(i, m)| Op::Maximize(i, m)),
        2 => (index, any::<bool>()).prop_map(|(i, f)| Op::Fullscreen(i, f)),
        1 => index.prop_map(Op::Cycle),
        3 => (index, (0..3u8, 0..3u8), -2000..2000i32, -2000..2000i32)
            .prop_map(|(i, cell, dx, dy)| Op::TiledDrag(i, cell, dx, dy)),
        3 => (index, -200..4000i32, -200..3000i32, any::<bool>())
            .prop_map(|(i, x, y, home)| Op::DropDrag(i, x, y, home)),
    ]
}

fn pick(layout: &MonitorLayout, index: usize) -> Option<WindowId> {
    let windows: Vec<WindowId> = layout.windows().collect();
    (!windows.is_empty()).then(|| windows[index % windows.len()])
}

fn apply(layout: &mut MonitorLayout, world: &mut World, op: &Op) {
    let next = &mut world.next;
    match op.clone() {
        Op::Open {
            forced,
            parent,
            fixed,
            pointer,
            mapped,
            server_side,
        } => {
            *next += 1;
            let limits = fixed
                .map(|size| SizeLimits {
                    min: size,
                    max: size,
                })
                .unwrap_or_default();
            let new = NewWindow {
                forced: forced || limits.is_fixed(),
                parent: parent.and_then(|p| pick(layout, p)),
                limits,
                server_side,
                pointer,
            };
            // Placement (owner, M19b): a new tiled window splits the tile under the
            // pointer, else the focused tile; if neither axis fits it floats with its
            // placeholder there. Floating ones go next to the focused window.
            let wants_tile = layout.default_mode() == Mode::Tiled && !new.forced;
            let under = pointer.and_then(|(x, y)| layout.tile_at(x, y));
            let target = under.or_else(|| layout.focused_tile());
            let focused = layout.focused();
            let window = WindowId(*next);
            if mapped {
                layout.open(window, new);
            } else {
                layout.open_unfocused(window, new);
            }
            let tiled = layout.mode(window) == Some(Mode::Tiled);
            let expected = if wants_tile {
                target.or(focused)
            } else {
                focused
            };
            if let Some(target) = expected {
                assert_eq!(
                    layout.tree().sibling_leaf(window),
                    Some(target),
                    "the new window splits the right leaf"
                );
            }
            if tiled {
                assert!(
                    layout.holds_minimum(window, true),
                    "the new tile holds the minimum"
                );
            }
            if mapped {
                assert_eq!(
                    layout.focused(),
                    Some(WindowId(*next)),
                    "a new window gets focus when it maps"
                );
            } else {
                assert_eq!(layout.focused(), focused, "focus stays until the map");
            }
        }
        Op::Place(i, size) => {
            if let Some(w) = pick(layout, i) {
                layout.place(w, size);
            }
        }
        Op::Close(i) => {
            if let Some(w) = pick(layout, i) {
                assert!(layout.close(w));
                assert!(!layout.contains(w));
            }
        }
        Op::Toggle(i) => {
            if let Some(w) = pick(layout, i) {
                let forced = layout.is_forced(w);
                let floating = layout.mode(w) == Some(Mode::Floating);
                let toggled = layout.toggle(w);
                if forced {
                    assert!(!toggled, "a forced window does not toggle");
                } else if !toggled {
                    // Only tiling can be refused: when the tiles would not fit.
                    assert!(floating, "floating always works");
                    assert_eq!(layout.mode(w), Some(Mode::Floating));
                }
            }
        }
        Op::FloatAll => layout.float_all(),
        Op::TileAll => {
            layout.tile_all();
            for w in layout.windows().collect::<Vec<_>>() {
                if layout.is_forced(w) {
                    assert_eq!(layout.mode(w), Some(Mode::Floating));
                } else if layout.mode(w) == Some(Mode::Floating) {
                    // Left floating only because tiling it would not fit.
                    let mut probe = layout.clone();
                    assert!(!probe.toggle(w), "{w:?} would fit, yet stayed floating");
                }
            }
        }
        Op::MoveBy(i, dx, dy) => {
            if let Some(w) = pick(layout, i) {
                layout.move_by(w, dx, dy);
            }
        }
        Op::ResizeBy(i, dw, dh) => {
            if let Some(w) = pick(layout, i) {
                let tiled = layout.mode(w) == Some(Mode::Tiled);
                let before = visible_tiles(layout);
                let result = layout.resize_by(w, dw, dh);
                if tiled {
                    check_tile_minimum(&before, &visible_tiles(layout));
                } else if let Some(r) = result {
                    let limits = layout.limits(w);
                    assert_eq!(limits.clamp((r.w, r.h)), (r.w, r.h), "size within limits");
                }
            }
        }
        Op::TiledDrag(i, (cx, cy), dx, dy) => {
            if let Some(w) = pick(layout, i)
                && let Some(rect) = layout.rect_of(w)
            {
                // The pointer in cell (cx, cy) of the window's 3x3 grid.
                let x = rect.x + rect.w * (2 * cx as i32 + 1) / 6;
                let y = rect.y + rect.h * (2 * cy as i32 + 1) / 6;
                let edges = resize_zone(rect, x, y);
                if let Some(resize) = layout.begin_tiled_resize(w, edges) {
                    let before = visible_tiles(layout);
                    // A drag in several motions, measured from the press.
                    for step in 1..=3 {
                        layout.drag_tiled_resize(&resize, dx * step / 3, dy * step / 3);
                        layout.check_invariants();
                    }
                    check_tile_minimum(&before, &visible_tiles(layout));
                }
            }
        }
        Op::SetRect(i, rect) => {
            if let Some(w) = pick(layout, i)
                && layout.mode(w) == Some(Mode::Floating)
            {
                layout.set_floating_rect(w, rect);
            }
        }
        Op::Focus(i) => {
            if let Some(w) = pick(layout, i) {
                layout.focus(w);
                assert_eq!(layout.recent_first().first(), Some(&w));
                if layout.mode(w) == Some(Mode::Floating) && layout.is_placed(w) {
                    let top = layout.floating_rects().last().map(|(t, _)| *t);
                    assert_eq!(top, Some(w), "focusing raises");
                }
            }
        }
        Op::FocusDirection(d) => {
            if let Some(f) = layout.focused()
                && let Some(n) = layout.neighbour(f, d)
            {
                assert_ne!(n, f);
                layout.focus(n);
            }
        }
        Op::Swap(d) => {
            let before = layout.focused();
            layout.swap_focused(d);
            assert_eq!(layout.focused(), before, "focus follows the swapped window");
        }
        Op::SetForced(i, forced, parent) => {
            if let Some(w) = pick(layout, i) {
                let parent = parent.and_then(|p| pick(layout, p));
                let fixed = layout.limits(w).is_fixed();
                layout.set_forced(w, forced || fixed, parent);
            }
        }
        // An output resize keeps the panels.
        Op::SetArea(area) => world.apply_geometry(layout, area),
        Op::Panels(changes) => {
            for change in &changes {
                change.apply(world);
            }
            let output = layout.output();
            world.apply_geometry(layout, output);
        }
        Op::SetGaps(outer, inner) => layout.set_gaps(Gaps::new(outer, inner)),
        Op::ResizeWithPanels(area, changes) => {
            for change in &changes {
                change.apply(world);
            }
            world.apply_geometry(layout, area);
        }
        Op::SetDefault(mode) => layout.set_default_mode(mode),
        Op::Snap(i, zone) => {
            if let Some(w) = pick(layout, i)
                && layout.snap(w, ZoneId(zone))
            {
                assert_eq!(layout.snapped_zone(w), Some(ZoneId(zone)));
                assert_eq!(layout.rect_of(w), layout.zone_rect(ZoneId(zone)));
            }
        }
        Op::SnapKey(i, d) => {
            if let Some(w) = pick(layout, i) {
                layout.snap_key(w, d);
            }
        }
        Op::SnapQuarter(i, q) => {
            if let Some(w) = pick(layout, i) {
                let zone = [
                    ZoneId::TOP_LEFT,
                    ZoneId::TOP_RIGHT,
                    ZoneId::BOTTOM_LEFT,
                    ZoneId::BOTTOM_RIGHT,
                ][q as usize];
                let fullscreen = layout.fullscreen() == Some(w);
                let fixed = layout.limits(w).is_fixed();
                // A floating window that has no rectangle yet (no first buffer).
                let unplaced = layout.rect_of(w).is_none();
                if layout.snap_quarter(w, zone) {
                    assert_eq!(layout.snapped_zone(w), Some(zone));
                    assert_eq!(layout.rect_of(w), layout.zone_rect(zone));
                } else {
                    assert!(
                        fullscreen || fixed || unplaced || layout.snapped_zone(w) == Some(zone),
                        "a quarter chord did nothing for no reason"
                    );
                }
            }
        }
        Op::DropDrag(i, x, y, home) => {
            if let Some(w) = pick(layout, i)
                && layout.mode(w) == Some(Mode::Tiled)
                && layout.default_mode() == Mode::Tiled
            {
                let tree_before = layout.tree().outline();
                assert!(layout.float_in_place(w));
                let origin = layout.rect_of(w).expect("floated in place");
                let at = if home && origin.w > 0 && origin.h > 0 {
                    (
                        origin.x + x.rem_euclid(origin.w),
                        origin.y + y.rem_euclid(origin.h),
                    )
                } else {
                    (x, y)
                };
                let preview = layout.drop_preview(w, Some(at), origin);
                let dropped = layout.drop_tile(w, Some(at), origin);
                // The preview comes from the same code as the release.
                assert_eq!(dropped, preview.is_some());
                if dropped {
                    assert_eq!(layout.mode(w), Some(Mode::Tiled));
                    let cell = layout.tree().rect_of(w).expect("tiled");
                    assert_eq!(preview.map(|p| p.logical()), Some(layout.inset(cell)));
                    assert!(layout.holds_minimum(w, true));
                    let inside = (origin.x..origin.x + origin.w).contains(&at.0)
                        && (origin.y..origin.y + origin.h).contains(&at.1);
                    if inside {
                        assert_eq!(layout.tree().outline(), tree_before, "back in its leaf");
                    }
                } else {
                    assert_eq!(layout.mode(w), Some(Mode::Floating), "stays floating");
                }
            }
        }
        Op::Unsnap(i) => {
            if let Some(w) = pick(layout, i) {
                layout.unsnap_restore(w);
                assert_eq!(layout.snapped_zone(w), None);
            }
        }
        Op::UnsnapDrag(i, x, y) => {
            if let Some(w) = pick(layout, i) {
                layout.unsnap_for_drag(w, (x, y), (y, x));
            }
        }
        Op::SetServerSide(i, on) => {
            if let Some(w) = pick(layout, i) {
                layout.set_server_side(w, on);
            }
        }
        Op::Maximize(i, on) => {
            if let Some(w) = pick(layout, i) {
                layout.set_maximized(w, on);
            }
        }
        Op::Fullscreen(i, on) => {
            if let Some(w) = pick(layout, i) {
                layout.set_fullscreen(w, on);
                if on {
                    assert_eq!(layout.fullscreen(), Some(w));
                }
            }
        }
        Op::Cycle(i) => {
            if let Some(w) = pick(layout, i) {
                layout.preview_focus(w);
                layout.commit_focus();
                if let Some(f) = layout.fullscreen() {
                    assert_eq!(f, w, "committing focus elsewhere ends fullscreen");
                }
            }
        }
    }
    let fullscreen_layers = layout
        .stacking()
        .iter()
        .filter(|(_, _, l)| *l == Layer::Fullscreen)
        .count();
    assert!(fullscreen_layers <= 1, "more than one fullscreen window");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1500))]

    #[test]
    fn random_operations_keep_invariants(
        area in area(),
        outer in gap(),
        inner in gap(),
        ops in prop::collection::vec(op(), 1..80),
    ) {
        let mut layout = MonitorLayout::new(area);
        layout.set_gaps(Gaps::new(outer, inner));
        let mut world = World::default();
        for op in &ops {
            apply(&mut layout, &mut world, op);
            layout.check_invariants();
        }
    }

    /// From any state: tile all, then float all, any moves, resizes and output resizes,
    /// then tile all again gives the identical tree (with the final area).
    #[test]
    fn float_all_then_tile_all_is_lossless(
        area in area(),
        setup in prop::collection::vec(op(), 0..40),
        between in prop::collection::vec(prop_oneof![
            (any::<usize>(), -3000..3000i32, -3000..3000i32).prop_map(|(i, x, y)| Op::MoveBy(i, x, y)),
            (any::<usize>(), -2000..2000i32, -2000..2000i32).prop_map(|(i, w, h)| Op::ResizeBy(i, w, h)),
            any::<usize>().prop_map(Op::Focus),
            area().prop_map(Op::SetArea),
            prop::collection::vec(panel_op(), 1..4).prop_map(Op::Panels),
        ], 0..20),
    ) {
        let mut layout = MonitorLayout::new(area);
        let mut world = World::default();
        for op in &setup {
            apply(&mut layout, &mut world, op);
        }
        layout.tile_all();
        let mut expected = layout.tree().clone();
        layout.float_all();
        for op in &between {
            apply(&mut layout, &mut world, op);
            layout.check_invariants();
        }
        layout.tile_all();
        use crate::layout::Layout;
        expected.set_area(layout.area());
        // Owner, M19b: a window whose tile would not hold the minimum in the area as it
        // is now stays floating; the tree is otherwise identical, and each such window
        // would still not fit.
        let windows: Vec<WindowId> = layout.windows().collect();
        for w in windows {
            if expected.mode_of(w) == Some(Mode::Tiled) && layout.mode(w) == Some(Mode::Floating) {
                expected.set_mode(w, Mode::Floating);
                let mut probe = layout.clone();
                prop_assert!(!probe.toggle(w), "{:?} would fit, yet stayed floating", w);
            }
            // And the other way: one that did not fit before tiles now that it does.
            if expected.mode_of(w) == Some(Mode::Floating)
                && layout.mode(w) == Some(Mode::Tiled)
                && !layout.is_forced(w)
            {
                expected.set_mode(w, Mode::Tiled);
            }
        }
        prop_assert_eq!(layout.tree(), &expected);
        prop_assert_eq!(layout.tiled_rects(), expected.rects());
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// Windows already floating when a top panel appears are moved down just enough
    /// to keep their top edge inside the usable area (size kept); when the panel goes
    /// away they stay where they were moved (owner, after M13b step 1).
    #[test]
    fn floating_windows_under_a_new_top_panel(
        area in (0..2i32, 0..2i32, 800..5000i32, 600..3000i32)
            .prop_map(|(x, y, w, h)| Rect::new(x, y, w, h)),
        setup in prop::collection::vec(op(), 0..40),
        height in 1i32..300,
    ) {
        let mut layout = MonitorLayout::new(area);
        let mut world = World::default();
        for op in &setup {
            if matches!(op, Op::SetArea(_) | Op::Panels(_) | Op::ResizeWithPanels(..)) {
                continue;
            }
            apply(&mut layout, &mut world, op);
        }
        let plain: Vec<(WindowId, Rect)> = layout
            .floating_rects()
            .into_iter()
            .filter(|(w, _)| layout.snapped_zone(*w).is_none())
            .collect();
        world.add_panel(LayerSurface {
            id: LayerId(0),
            level: Level::Top,
            anchor: Edges { left: true, right: true, top: true, bottom: false },
            margin: Margins::default(),
            size: (0, height as u32),
            exclusive: height,
            mapped: 0,
        });
        world.apply_geometry(&mut layout, area);
        layout.check_invariants();
        let usable = layout.area();
        let reserved = usable.y - area.y;
        let mut moved = Vec::new();
        for (w, before) in &plain {
            let after = layout.rect_of(*w).expect("still floating");
            prop_assert_eq!((after.w, after.h), (before.w, before.h), "size kept");
            prop_assert!(after.y >= usable.y, "{:?} above the usable area", after);
            if before.y >= usable.y {
                prop_assert_eq!(after.y, before.y, "a window below the panel moved");
            } else {
                prop_assert!(reserved > 0);
            }
            moved.push((*w, after));
        }
        world.remove_panel(0);
        world.apply_geometry(&mut layout, area);
        layout.check_invariants();
        for (w, at) in moved {
            prop_assert_eq!(layout.rect_of(w), Some(at), "moved back after the panel went");
        }
    }

    /// An output resize and panel changes in the same frame: one update gives a state
    /// that keeps every invariant and depends only on the final output and panels, not
    /// on the order the panels were listed in.
    #[test]
    fn a_resize_and_panel_changes_together_are_one_update(
        area in area(),
        setup in prop::collection::vec(op(), 0..40),
        output in area(),
        changes in prop::collection::vec(panel_op(), 1..6),
    ) {
        let mut layout = MonitorLayout::new(area);
        let mut world = World::default();
        for op in &setup {
            apply(&mut layout, &mut world, op);
        }
        for change in &changes {
            change.apply(&mut world);
        }
        let mut reversed = layout.clone();
        let mut together = layout.clone();
        world.apply_geometry(&mut together, output);
        together.check_invariants();
        prop_assert_eq!(together.output(), output);
        prop_assert_eq!(together.area(), layers::arrange(output, &world.panels).usable);
        world.panels.reverse();
        world.apply_geometry(&mut reversed, output);
        prop_assert_eq!(format!("{together:?}"), format!("{reversed:?}"));
    }
}

/// The gaps in physical pixels (owner, M18a; each gap rounded once per scale after
/// M18a) at scales 1, 1.25, 1.5, 1.75 and 2, after every operation: tiles never
/// overlap; neighbours are exactly the inner gap rounded once apart (12 at 1.5); every
/// tile on the usable area's edge is exactly the outer gap rounded once from it (20 at
/// 1.5, on all four sides).
pub(crate) fn check_physical(layout: &MonitorLayout) {
    use super::gaps::{physical_edge, physical_gap};
    let Gaps { outer, inner } = layout.gaps();
    let area = layout.area();
    let room = 2 * outer.max(inner) + 1;
    let tiles: Vec<(WindowId, Rect)> = layout
        .stacking()
        .into_iter()
        .filter(|(_, _, l)| *l == Layer::Tiled)
        .filter_map(|(w, _, _)| Some((w, layout.cell(w)?)))
        .collect();
    for scale in SCALES {
        let p = |w: WindowId| layout.frame(w, scale).expect("frame").physical;
        let edge = |v: i32| physical_edge(v, scale);
        let (outer_px, inner_px) = (physical_gap(outer, scale), physical_gap(inner, scale));
        for (i, &(wa, ca)) in tiles.iter().enumerate() {
            let a = p(wa);
            let big = |r: Rect| r.w > 2 * room && r.h > 2 * room;
            if big(ca) {
                for (on, m) in [
                    (ca.x <= area.x, a.loc.x - edge(area.x)),
                    (ca.y <= area.y, a.loc.y - edge(area.y)),
                    (
                        ca.right() >= area.right(),
                        edge(area.right()) - (a.loc.x + a.size.w),
                    ),
                    (
                        ca.bottom() >= area.bottom(),
                        edge(area.bottom()) - (a.loc.y + a.size.h),
                    ),
                ] {
                    if on {
                        assert_eq!(m, outer_px, "x{scale}: outer gap of {ca:?}");
                    }
                }
            }
            for &(wb, cb) in &tiles[i + 1..] {
                let b = p(wb);
                let apart = a.loc.x + a.size.w <= b.loc.x
                    || b.loc.x + b.size.w <= a.loc.x
                    || a.loc.y + a.size.h <= b.loc.y
                    || b.loc.y + b.size.h <= a.loc.y;
                assert!(apart, "x{scale}: {ca:?} and {cb:?} overlap");
                if !big(ca) || !big(cb) {
                    continue;
                }
                for (first, second, gap) in [
                    (ca, cb, b.loc.x - (a.loc.x + a.size.w)),
                    (cb, ca, a.loc.x - (b.loc.x + b.size.w)),
                ] {
                    if first.right() == second.x
                        && first.y < second.bottom()
                        && second.y < first.bottom()
                    {
                        assert_eq!(gap, inner_px, "x{scale}: {first:?} | {second:?}");
                    }
                }
                for (first, second, gap) in [
                    (ca, cb, b.loc.y - (a.loc.y + a.size.h)),
                    (cb, ca, a.loc.y - (b.loc.y + b.size.h)),
                ] {
                    if first.bottom() == second.y
                        && first.x < second.right()
                        && second.x < first.right()
                    {
                        assert_eq!(gap, inner_px, "x{scale}: {first:?} / {second:?}");
                    }
                }
            }
        }
    }
}

const SCALES: [f64; 5] = [1.0, 1.25, 1.5, 1.75, 2.0];

fn physical_run(area: Rect, ops: &[Op]) {
    let mut layout = MonitorLayout::new(area);
    layout.set_gaps(Gaps::DEFAULT);
    let mut world = World::default();
    for op in ops {
        if matches!(op, Op::SetGaps(..)) {
            continue;
        }
        apply(&mut layout, &mut world, op);
        layout.check_invariants();
        check_physical(&layout);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    #[test]
    fn physical_gaps_at_five_scales(
        area in area(),
        ops in prop::collection::vec(op(), 1..60),
    ) {
        physical_run(area, &ops);
    }
}

/// The long run of the physical gap property (owner, M18a). Ignored by default:
/// `cargo test --release -p sash-wm -- --ignored --nocapture long_physical_gaps`.
#[test]
#[ignore]
fn long_physical_gaps() {
    let cases = 200_000;
    let started = std::time::Instant::now();
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases,
        ..ProptestConfig::default()
    });
    runner
        .run(
            &(area(), prop::collection::vec(op(), 1..80)),
            |(area, ops)| {
                physical_run(area, &ops);
                Ok(())
            },
        )
        .expect("the physical gap property");
    println!(
        "long physical gaps: {cases} cases in {:?}",
        started.elapsed()
    );
}
