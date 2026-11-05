// SPDX-License-Identifier: Apache-2.0

//! Layer surfaces (wlr-layer-shell) as plain data: where each one goes on its output,
//! the usable area left for windows, and the stacking order. No Smithay or Wayland
//! types; the protocol code (M13b step 3) fills `LayerSurface` from a surface's
//! committed state.
//!
//! Arrangement, per the protocol text (wlr-layer-shell-unstable-v1, v4):
//! - Anchors pick where the surface sits: one edge centers it on that edge, two
//!   perpendicular edges put it in that corner, two opposite edges center it between
//!   them (and stretch it if its size on that axis is 0), none centers it.
//! - Margins apply on anchored edges only.
//! - A positive exclusive zone reserves that distance from one edge, and only when the
//!   surface is anchored to exactly one edge, or to one edge and both perpendicular
//!   ones (that edge). A corner, two opposite edges, all four or none reserve nothing.
//!   The zone includes the margin: the reservation is the zone itself.
//! - Zone 0: placed inside the usable area. Zone -1 (any negative): placed against the
//!   output's edges, ignoring every reservation.
//!
//! sash-wm's rules (owner, after M12 and M13a):
//! - Reservations apply in the order the surfaces were mapped, each inside the area the
//!   earlier ones left. One that would reserve more than half the output on an axis, or
//!   leave a usable area under `MIN_USABLE` on the axis it shrinks, is ignored (the
//!   caller logs it); so a panel already there never moves because a new one arrived.
//! - Sizes are never negative; a requested size larger than the room is cut to it.

use super::{Edges, Rect};

/// The smallest usable area reservations may leave (owner, after M12).
pub const MIN_USABLE: (i32, i32) = (640, 360);

/// A layer-shell layer, bottom to top.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Background,
    Bottom,
    Top,
    Overlay,
}

/// Identifies a layer surface. Assigned by the compositor, never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LayerId(pub u64);

/// Margins from the anchored edges, in logical pixels (may be negative).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Margins {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

/// What the arrangement needs from a layer surface's committed state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerSurface {
    pub id: LayerId,
    pub level: Level,
    pub anchor: Edges,
    pub margin: Margins,
    /// The requested size; 0 on an axis: the compositor decides (stretch between the
    /// anchors).
    pub size: (u32, u32),
    /// Positive: reserve; 0: stay out of reservations; negative: ignore them.
    pub exclusive: i32,
    /// When it was mapped: earlier surfaces reserve first and stack lower.
    pub mapped: u64,
}

/// An output's layer surfaces, placed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Arrangement {
    /// The output minus the accepted reservations.
    pub usable: Rect,
    /// Every surface's rectangle (the size to configure and where to draw it), in
    /// mapping order.
    pub placed: Vec<(LayerId, Rect)>,
    /// Surfaces whose reservation would have broken the cap, in mapping order.
    pub ignored: Vec<LayerId>,
}

impl Arrangement {
    pub fn rect_of(&self, id: LayerId) -> Option<Rect> {
        self.placed
            .iter()
            .find(|(placed, _)| *placed == id)
            .map(|(_, rect)| *rect)
    }
}

/// The edge a surface reserves from, if any (see the module docs).
pub fn reserved_edge(anchor: Edges) -> Option<Edge> {
    let Edges {
        left,
        right,
        top,
        bottom,
    } = anchor;
    match (left, right, top, bottom) {
        (true, false, false, false) | (true, false, true, true) => Some(Edge::Left),
        (false, true, false, false) | (false, true, true, true) => Some(Edge::Right),
        (false, false, true, false) | (true, true, true, false) => Some(Edge::Top),
        (false, false, false, true) | (true, true, false, true) => Some(Edge::Bottom),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

/// Places every surface of an output and computes its usable area.
pub fn arrange(output: Rect, surfaces: &[LayerSurface]) -> Arrangement {
    let mut order: Vec<&LayerSurface> = surfaces.iter().collect();
    order.sort_by_key(|surface| (surface.mapped, surface.id));

    // Reservations first, in mapping order, each inside what the earlier ones left.
    let mut usable = output;
    let mut ignored = Vec::new();
    let mut reserving_bounds = Vec::new();
    for surface in &order {
        if surface.exclusive <= 0 {
            continue;
        }
        let Some(edge) = reserved_edge(surface.anchor) else {
            continue;
        };
        reserving_bounds.push((surface.id, usable));
        let shrunk = shrink(usable, edge, surface.exclusive);
        if within_cap(output, usable, shrunk) {
            usable = shrunk;
        } else {
            ignored.push(surface.id);
        }
    }

    let placed = order
        .iter()
        .map(|surface| {
            let bounds = if surface.exclusive < 0 {
                output
            } else {
                reserving_bounds
                    .iter()
                    .find(|(id, _)| *id == surface.id)
                    .map_or(usable, |(_, bounds)| *bounds)
            };
            (surface.id, place(bounds, surface))
        })
        .collect();

    Arrangement {
        usable,
        placed,
        ignored,
    }
}

/// `area` with `amount` taken from `edge` (never below zero size).
fn shrink(area: Rect, edge: Edge, amount: i32) -> Rect {
    let Rect { x, y, w, h } = area;
    match edge {
        Edge::Top => {
            let amount = amount.min(h);
            Rect::new(x, y + amount, w, h - amount)
        }
        Edge::Bottom => Rect::new(x, y, w, h - amount.min(h)),
        Edge::Left => {
            let amount = amount.min(w);
            Rect::new(x + amount, y, w - amount, h)
        }
        Edge::Right => Rect::new(x, y, w - amount.min(w), h),
    }
}

/// Whether going from `before` to `after` keeps the cap: at most half the output
/// reserved per axis, and an axis that shrinks stays at least `MIN_USABLE`.
fn within_cap(output: Rect, before: Rect, after: Rect) -> bool {
    let reserved_x = output.w - after.w;
    let reserved_y = output.h - after.h;
    let width_ok = after.w == before.w || after.w >= MIN_USABLE.0;
    let height_ok = after.h == before.h || after.h >= MIN_USABLE.1;
    reserved_x <= output.w / 2 && reserved_y <= output.h / 2 && width_ok && height_ok
}

/// A surface's rectangle inside `bounds`.
fn place(bounds: Rect, surface: &LayerSurface) -> Rect {
    let (x, w) = place_axis(
        bounds.x,
        bounds.w,
        surface.anchor.left,
        surface.anchor.right,
        surface.margin.left,
        surface.margin.right,
        surface.size.0,
    );
    let (y, h) = place_axis(
        bounds.y,
        bounds.h,
        surface.anchor.top,
        surface.anchor.bottom,
        surface.margin.top,
        surface.margin.bottom,
        surface.size.1,
    );
    Rect::new(x, y, w, h)
}

/// One axis of `place`: the start and length inside `start..start + length`.
fn place_axis(
    start: i32,
    length: i32,
    at_start: bool,
    at_end: bool,
    margin_start: i32,
    margin_end: i32,
    size: u32,
) -> (i32, i32) {
    let margin_start = if at_start { margin_start } else { 0 };
    let margin_end = if at_end { margin_end } else { 0 };
    let room = length
        .saturating_sub(margin_start)
        .saturating_sub(margin_end)
        .max(0);
    let size = if size == 0 {
        // Only valid between two anchors (the protocol code refuses it otherwise);
        // stretching keeps it well defined either way.
        room
    } else {
        i32::try_from(size).unwrap_or(i32::MAX).min(room)
    };
    let offset = match (at_start, at_end) {
        (true, false) => margin_start,
        (false, true) => margin_start + room - size,
        // Between both anchors, or neither: centered in the room.
        _ => margin_start + (room - size) / 2,
    };
    (start.saturating_add(offset), size)
}

/// What draws where, bottom to top (owner, after M13a). Popups of a layer surface are
/// directly above its layer; a fullscreen window is above the top layer and below the
/// overlay layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stratum {
    Layer(Level),
    LayerPopups(Level),
    /// Tiled windows, their popups, floating and maximized windows with their popups
    /// and bars, the snap preview: ordered among themselves as before layer-shell.
    Windows,
    Fullscreen,
    DragIcon,
    Cursor,
}

/// Every stratum, bottom to top.
pub const STACKING: [Stratum; 12] = [
    Stratum::Layer(Level::Background),
    Stratum::LayerPopups(Level::Background),
    Stratum::Layer(Level::Bottom),
    Stratum::LayerPopups(Level::Bottom),
    Stratum::Windows,
    Stratum::Layer(Level::Top),
    Stratum::LayerPopups(Level::Top),
    Stratum::Fullscreen,
    Stratum::Layer(Level::Overlay),
    Stratum::LayerPopups(Level::Overlay),
    Stratum::DragIcon,
    Stratum::Cursor,
];

/// Position of `stratum` in `STACKING` (lower draws first).
pub fn depth(stratum: Stratum) -> usize {
    STACKING
        .iter()
        .position(|s| *s == stratum)
        .expect("every stratum is in STACKING")
}

/// The surfaces of one level, bottom to top: in mapping order (a surface mapped later
/// is above), ties by id.
pub fn stack(surfaces: &[LayerSurface], level: Level) -> Vec<LayerId> {
    let mut on_level: Vec<&LayerSurface> = surfaces.iter().filter(|s| s.level == level).collect();
    on_level.sort_by_key(|s| (s.mapped, s.id));
    on_level.iter().map(|s| s.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: Rect = Rect::new(0, 0, 1920, 1080);

    fn anchors(spec: &str) -> Edges {
        Edges {
            left: spec.contains('l'),
            right: spec.contains('r'),
            top: spec.contains('t'),
            bottom: spec.contains('b'),
        }
    }

    fn surface(id: u64, anchor: &str, size: (u32, u32), exclusive: i32) -> LayerSurface {
        LayerSurface {
            id: LayerId(id),
            level: Level::Top,
            anchor: anchors(anchor),
            margin: Margins::default(),
            size,
            exclusive,
            mapped: id,
        }
    }

    #[test]
    fn a_top_panel_reserves_its_zone_and_stretches_between_its_anchors() {
        let panel = surface(1, "tlr", (0, 30), 30);
        let a = arrange(OUTPUT, &[panel]);
        assert_eq!(a.usable, Rect::new(0, 30, 1920, 1050));
        assert_eq!(a.rect_of(LayerId(1)), Some(Rect::new(0, 0, 1920, 30)));
        assert!(a.ignored.is_empty());
    }

    #[test]
    fn each_single_edge_and_three_edge_anchor_reserves_that_edge() {
        let cases = [
            ("t", Rect::new(0, 40, 1920, 1040)),
            ("b", Rect::new(0, 0, 1920, 1040)),
            ("l", Rect::new(40, 0, 1880, 1080)),
            ("r", Rect::new(0, 0, 1880, 1080)),
            ("tlr", Rect::new(0, 40, 1920, 1040)),
            ("blr", Rect::new(0, 0, 1920, 1040)),
            ("ltb", Rect::new(40, 0, 1880, 1080)),
            ("rtb", Rect::new(0, 0, 1880, 1080)),
        ];
        for (anchor, usable) in cases {
            let a = arrange(OUTPUT, &[surface(1, anchor, (100, 40), 40)]);
            assert_eq!(a.usable, usable, "anchor {anchor}");
        }
    }

    #[test]
    fn corners_opposite_edges_all_edges_and_none_reserve_nothing() {
        for anchor in ["", "tl", "tr", "bl", "br", "tb", "lr", "tlrb"] {
            let a = arrange(OUTPUT, &[surface(1, anchor, (100, 40), 40)]);
            assert_eq!(a.usable, OUTPUT, "anchor {anchor:?}");
            assert!(a.ignored.is_empty());
        }
    }

    #[test]
    fn the_zone_includes_the_margin() {
        let mut panel = surface(1, "tlr", (0, 30), 40);
        panel.margin = Margins {
            top: 10,
            right: 5,
            bottom: 99,
            left: 5,
        };
        let a = arrange(OUTPUT, &[panel]);
        assert_eq!(a.usable, Rect::new(0, 40, 1920, 1040));
        // Margins on anchored edges only (bottom is not anchored).
        assert_eq!(a.rect_of(LayerId(1)), Some(Rect::new(5, 10, 1910, 30)));
    }

    #[test]
    fn anchors_place_in_corners_on_edges_and_in_the_center() {
        let size = (200, 100);
        let cases = [
            ("tl", (0, 0)),
            ("tr", (1720, 0)),
            ("bl", (0, 980)),
            ("br", (1720, 980)),
            ("t", (860, 0)),
            ("b", (860, 980)),
            ("l", (0, 490)),
            ("r", (1720, 490)),
            ("", (860, 490)),
            ("lr", (860, 490)),
            ("tb", (860, 490)),
        ];
        for (anchor, (x, y)) in cases {
            let a = arrange(OUTPUT, &[surface(1, anchor, size, 0)]);
            assert_eq!(
                a.rect_of(LayerId(1)),
                Some(Rect::new(x, y, 200, 100)),
                "anchor {anchor:?}"
            );
        }
    }

    #[test]
    fn zone_zero_avoids_reservations_and_minus_one_ignores_them() {
        let panel = surface(1, "tlr", (0, 30), 30);
        let notification = surface(2, "tr", (300, 80), 0);
        let wallpaper = surface(3, "tlrb", (0, 0), -1);
        let a = arrange(OUTPUT, &[panel, notification, wallpaper]);
        assert_eq!(a.rect_of(LayerId(2)), Some(Rect::new(1620, 30, 300, 80)));
        assert_eq!(a.rect_of(LayerId(3)), Some(OUTPUT));
    }

    #[test]
    fn later_reservations_are_placed_inside_what_earlier_ones_left() {
        let top = surface(1, "tlr", (0, 30), 30);
        let dock = surface(2, "ltb", (64, 0), 64);
        let a = arrange(OUTPUT, &[dock, top]);
        assert_eq!(a.rect_of(LayerId(1)), Some(Rect::new(0, 0, 1920, 30)));
        assert_eq!(a.rect_of(LayerId(2)), Some(Rect::new(0, 30, 64, 1050)));
        assert_eq!(a.usable, Rect::new(64, 30, 1856, 1050));
    }

    #[test]
    fn a_reservation_beyond_half_the_output_is_ignored_and_later_ones_still_count() {
        let first = surface(1, "t", (100, 400), 400);
        let second = surface(2, "b", (100, 200), 200);
        let third = surface(3, "b", (100, 100), 100);
        let a = arrange(OUTPUT, &[first, second, third]);
        // 400 + 200 > 540; 400 + 100 <= 540.
        assert_eq!(a.ignored, vec![LayerId(2)]);
        assert_eq!(a.usable, Rect::new(0, 400, 1920, 580));
    }

    #[test]
    fn a_reservation_leaving_less_than_the_minimum_is_ignored() {
        let output = Rect::new(0, 0, 800, 400);
        let a = arrange(output, &[surface(1, "t", (100, 50), 50)]);
        assert_eq!(a.ignored, vec![LayerId(1)]);
        assert_eq!(a.usable, output);
        // The other axis is free to shrink while it stays at least 640.
        let a = arrange(output, &[surface(1, "l", (100, 50), 160)]);
        assert_eq!(a.usable, Rect::new(160, 0, 640, 400));
        let a = arrange(output, &[surface(1, "l", (100, 50), 161)]);
        assert_eq!(a.usable, output);
    }

    #[test]
    fn an_output_below_the_minimum_takes_no_reservation() {
        let output = Rect::new(0, 0, 600, 300);
        let a = arrange(
            output,
            &[surface(1, "t", (0, 10), 10), surface(2, "l", (10, 0), 10)],
        );
        assert_eq!(a.usable, output);
        assert_eq!(a.ignored, vec![LayerId(1), LayerId(2)]);
    }

    #[test]
    fn sizes_are_cut_to_the_room_and_never_negative() {
        let mut big = surface(1, "tl", (u32::MAX, 5000), 0);
        big.margin = Margins {
            top: 3000,
            right: 0,
            bottom: 0,
            left: -40,
        };
        let a = arrange(OUTPUT, &[big]);
        let rect = a.rect_of(LayerId(1)).expect("placed");
        assert_eq!(rect, Rect::new(-40, 3000, 1960, 0));
    }

    #[test]
    fn the_input_order_does_not_matter() {
        let surfaces = [
            surface(1, "tlr", (0, 30), 30),
            surface(2, "b", (500, 50), 50),
            surface(3, "", (100, 100), 0),
        ];
        let mut reversed = surfaces;
        reversed.reverse();
        assert_eq!(arrange(OUTPUT, &surfaces), arrange(OUTPUT, &reversed));
    }

    #[test]
    fn the_stacking_order_is_the_owners() {
        use Level::*;
        let order = |a, b| assert!(depth(a) < depth(b), "{a:?} below {b:?}");
        order(Stratum::Layer(Background), Stratum::Layer(Bottom));
        order(Stratum::Layer(Bottom), Stratum::Windows);
        order(Stratum::Windows, Stratum::Layer(Top));
        order(Stratum::Layer(Top), Stratum::Fullscreen);
        order(Stratum::Fullscreen, Stratum::Layer(Overlay));
        order(Stratum::Layer(Overlay), Stratum::DragIcon);
        order(Stratum::DragIcon, Stratum::Cursor);
        for level in [Background, Bottom, Top, Overlay] {
            assert_eq!(
                depth(Stratum::LayerPopups(level)),
                depth(Stratum::Layer(level)) + 1,
                "popups of {level:?} directly above it"
            );
        }
    }

    #[test]
    fn within_a_level_later_mapped_surfaces_are_above() {
        let mut a = surface(5, "t", (10, 10), 0);
        a.mapped = 9;
        let b = surface(7, "t", (10, 10), 0);
        let mut c = surface(6, "t", (10, 10), 0);
        c.level = Level::Overlay;
        assert_eq!(stack(&[a, b, c], Level::Top), vec![LayerId(7), LayerId(5)]);
        assert_eq!(stack(&[a, b, c], Level::Overlay), vec![LayerId(6)]);
    }

    mod props {
        use proptest::prelude::*;

        use super::super::*;

        fn level() -> impl Strategy<Value = Level> {
            prop_oneof![
                Just(Level::Background),
                Just(Level::Bottom),
                Just(Level::Top),
                Just(Level::Overlay),
            ]
        }

        fn layer_surface() -> impl Strategy<Value = LayerSurface> {
            (
                level(),
                0u8..16,
                prop::array::uniform4(-50i32..300),
                (
                    prop_oneof![Just(0u32), 1u32..4000],
                    prop_oneof![Just(0u32), 1u32..3000],
                ),
                prop_oneof![Just(-1i32), Just(0), -5i32..0, 1i32..1500],
                0u64..64,
            )
                .prop_map(|(level, anchor, [t, r, b, l], size, exclusive, mapped)| {
                    LayerSurface {
                        id: LayerId(0),
                        level,
                        anchor: Edges {
                            left: anchor & 1 != 0,
                            right: anchor & 2 != 0,
                            top: anchor & 4 != 0,
                            bottom: anchor & 8 != 0,
                        },
                        margin: Margins {
                            top: t,
                            right: r,
                            bottom: b,
                            left: l,
                        },
                        size,
                        exclusive,
                        mapped,
                    }
                })
        }

        fn surfaces() -> impl Strategy<Value = Vec<LayerSurface>> {
            prop::collection::vec(layer_surface(), 0..12).prop_map(|mut list| {
                for (i, s) in list.iter_mut().enumerate() {
                    s.id = LayerId(i as u64);
                }
                list
            })
        }

        fn output() -> impl Strategy<Value = Rect> {
            (-100i32..100, -100i32..100, 1i32..6000, 1i32..3000)
                .prop_map(|(x, y, w, h)| Rect::new(x, y, w, h))
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(3000))]

            /// Random panels never break the cap, and nothing is ever negative.
            #[test]
            fn the_cap_holds_and_nothing_is_negative(output in output(), list in surfaces()) {
                let a = arrange(output, &list);
                let u = a.usable;
                prop_assert!(u.w >= 0 && u.h >= 0);
                prop_assert!(u.x >= output.x && u.y >= output.y);
                prop_assert!(u.right() <= output.right() && u.bottom() <= output.bottom());
                prop_assert!(output.w - u.w <= output.w / 2);
                prop_assert!(output.h - u.h <= output.h / 2);
                prop_assert!(u.w == output.w || u.w >= MIN_USABLE.0);
                prop_assert!(u.h == output.h || u.h >= MIN_USABLE.1);
                prop_assert_eq!(a.placed.len(), list.len());
                for (_, rect) in &a.placed {
                    prop_assert!(rect.w >= 0 && rect.h >= 0, "{:?}", rect);
                }
            }

            /// Only reserving anchors reserve; ignored surfaces are reserving ones.
            #[test]
            fn only_reserving_surfaces_change_the_usable_area(output in output(), list in surfaces()) {
                let reserving: Vec<LayerSurface> = list
                    .iter()
                    .copied()
                    .filter(|s| s.exclusive > 0 && reserved_edge(s.anchor).is_some())
                    .collect();
                let a = arrange(output, &list);
                prop_assert_eq!(a.usable, arrange(output, &reserving).usable);
                for id in &a.ignored {
                    prop_assert!(reserving.iter().any(|s| s.id == *id));
                }
            }

            /// The order surfaces are given in does not matter; the mapping order does.
            #[test]
            fn arrangement_is_deterministic(output in output(), list in surfaces(), seed in any::<u64>()) {
                let mut shuffled = list.clone();
                let n = shuffled.len();
                if n > 1 {
                    for i in 0..n {
                        let j = (seed.wrapping_mul(i as u64 + 1) % n as u64) as usize;
                        shuffled.swap(i, j);
                    }
                }
                prop_assert_eq!(arrange(output, &list), arrange(output, &shuffled));
            }

            /// A surface mapped after all others never moves an earlier reserving
            /// surface, nor changes which earlier reservations are accepted.
            #[test]
            fn a_new_panel_never_moves_an_old_one(
                output in output(),
                list in surfaces(),
                new in layer_surface(),
            ) {
                let before = arrange(output, &list);
                let mut more = list.clone();
                let mut new = new;
                new.id = LayerId(list.len() as u64);
                new.mapped = 1000;
                more.push(new);
                let after = arrange(output, &more);
                for s in list.iter().filter(|s| s.exclusive > 0 && reserved_edge(s.anchor).is_some()) {
                    prop_assert_eq!(before.rect_of(s.id), after.rect_of(s.id));
                }
                prop_assert!(after.ignored.starts_with(&before.ignored));
                prop_assert!(after.ignored.len() <= before.ignored.len() + 1);
            }

            /// Stacking within a level is stable: removing a surface keeps the order of
            /// the others, and every surface is on exactly its own level.
            #[test]
            fn stacking_is_stable(list in surfaces(), remove in any::<prop::sample::Index>()) {
                for level in [Level::Background, Level::Bottom, Level::Top, Level::Overlay] {
                    let full = stack(&list, level);
                    prop_assert_eq!(full.len(), list.iter().filter(|s| s.level == level).count());
                    if list.is_empty() {
                        continue;
                    }
                    let gone = list[remove.index(list.len())].id;
                    let fewer: Vec<LayerSurface> = list.iter().copied().filter(|s| s.id != gone).collect();
                    let expected: Vec<LayerId> = full.iter().copied().filter(|id| *id != gone).collect();
                    prop_assert_eq!(stack(&fewer, level), expected);
                }
            }
        }
    }
}
