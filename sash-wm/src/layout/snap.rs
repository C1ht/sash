// SPDX-License-Identifier: Apache-2.0

//! Snap zones as data, the keybind state table, and edge snapping. Plain geometry; the
//! monitor layout applies the results.
//!
//! A zone's rectangle is a span of a grid laid over the output: boundaries sit at
//! floor(length * k / n), so zones from the same grid cover the output exactly (the
//! first part gets the floor, the second the remainder).

use super::{Direction, Edges, Rect};

/// Corner trigger: the pointer within this many pixels of two output edges
/// (docs/design.md, "Interaction constants": 55).
pub const CORNER_TRIGGER: i32 = 55;
/// Edge trigger: the pointer within this many pixels of one output edge (8).
pub const EDGE_TRIGGER: i32 = 8;
/// Edge snapping threshold during mouse moves and resizes (13).
pub const EDGE_SNAP: i32 = 13;

/// Identifies a zone in the table. The settings protocol will edit the table; the
/// keybind table refers to the default zones by these ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ZoneId(pub u16);

impl ZoneId {
    pub const FULL: ZoneId = ZoneId(0);
    pub const LEFT: ZoneId = ZoneId(1);
    pub const RIGHT: ZoneId = ZoneId(2);
    pub const TOP_LEFT: ZoneId = ZoneId(3);
    pub const TOP_RIGHT: ZoneId = ZoneId(4);
    pub const BOTTOM_LEFT: ZoneId = ZoneId(5);
    pub const BOTTOM_RIGHT: ZoneId = ZoneId(6);
}

/// Where the pointer has to be during a drag to target a zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Top,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// A span `start..end` of `parts` equal parts of one axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u16,
    pub end: u16,
    pub parts: u16,
}

const fn span(start: u16, end: u16, parts: u16) -> Span {
    Span { start, end, parts }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Zone {
    pub id: ZoneId,
    pub trigger: Trigger,
    pub columns: Span,
    pub rows: Span,
    /// The whole-output zone: configured with the xdg maximized state.
    pub maximized: bool,
}

/// The seven default zones.
pub const DEFAULT_ZONES: [Zone; 7] = [
    Zone {
        id: ZoneId::FULL,
        trigger: Trigger::Top,
        columns: span(0, 1, 1),
        rows: span(0, 1, 1),
        maximized: true,
    },
    Zone {
        id: ZoneId::LEFT,
        trigger: Trigger::Left,
        columns: span(0, 1, 2),
        rows: span(0, 1, 1),
        maximized: false,
    },
    Zone {
        id: ZoneId::RIGHT,
        trigger: Trigger::Right,
        columns: span(1, 2, 2),
        rows: span(0, 1, 1),
        maximized: false,
    },
    Zone {
        id: ZoneId::TOP_LEFT,
        trigger: Trigger::TopLeft,
        columns: span(0, 1, 2),
        rows: span(0, 1, 2),
        maximized: false,
    },
    Zone {
        id: ZoneId::TOP_RIGHT,
        trigger: Trigger::TopRight,
        columns: span(1, 2, 2),
        rows: span(0, 1, 2),
        maximized: false,
    },
    Zone {
        id: ZoneId::BOTTOM_LEFT,
        trigger: Trigger::BottomLeft,
        columns: span(0, 1, 2),
        rows: span(1, 2, 2),
        maximized: false,
    },
    Zone {
        id: ZoneId::BOTTOM_RIGHT,
        trigger: Trigger::BottomRight,
        columns: span(1, 2, 2),
        rows: span(1, 2, 2),
        maximized: false,
    },
];

/// Grid boundary k of n along a length starting at `start`.
fn boundary(start: i32, length: i32, k: u16, n: u16) -> i32 {
    start + (length as i64 * k as i64 / n.max(1) as i64) as i32
}

impl Zone {
    /// The zone's rectangle on an output covering `area`.
    pub fn rect(&self, area: Rect) -> Rect {
        let x0 = boundary(area.x, area.w, self.columns.start, self.columns.parts);
        let x1 = boundary(area.x, area.w, self.columns.end, self.columns.parts);
        let y0 = boundary(area.y, area.h, self.rows.start, self.rows.parts);
        let y1 = boundary(area.y, area.h, self.rows.end, self.rows.parts);
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// Which output edges the zone touches (for the xdg tiled states).
    pub fn output_edges(&self) -> Edges {
        Edges {
            left: self.columns.start == 0,
            right: self.columns.end == self.columns.parts,
            top: self.rows.start == 0,
            bottom: self.rows.end == self.rows.parts,
        }
    }
}

/// The zone table of a monitor.
#[derive(Clone, Debug, PartialEq)]
pub struct Zones {
    zones: Vec<Zone>,
}

impl Default for Zones {
    fn default() -> Self {
        Self {
            zones: DEFAULT_ZONES.to_vec(),
        }
    }
}

impl Zones {
    pub fn get(&self, id: ZoneId) -> Option<&Zone> {
        self.zones.iter().find(|z| z.id == id)
    }

    /// The zone the pointer at (x, y) targets during a drag, if any. Corners (within
    /// CORNER_TRIGGER of two edges) win over edges (within EDGE_TRIGGER of one edge).
    pub fn at(&self, area: Rect, x: i32, y: i32) -> Option<&Zone> {
        if !area.contains(x, y) {
            return None;
        }
        let (left, right) = (x - area.x, area.right() - 1 - x);
        let (top, bottom) = (y - area.y, area.bottom() - 1 - y);
        let near = |d: i32, limit: i32| d < limit;
        let corner = match (
            near(left, CORNER_TRIGGER),
            near(right, CORNER_TRIGGER),
            near(top, CORNER_TRIGGER),
            near(bottom, CORNER_TRIGGER),
        ) {
            // On outputs narrower than two corner bands, the nearer side wins.
            (true, true, ..) | (.., true, true) => None,
            (true, false, true, false) => Some(Trigger::TopLeft),
            (false, true, true, false) => Some(Trigger::TopRight),
            (true, false, false, true) => Some(Trigger::BottomLeft),
            (false, true, false, true) => Some(Trigger::BottomRight),
            _ => None,
        };
        let edge = || {
            if near(top, EDGE_TRIGGER) {
                Some(Trigger::Top)
            } else if near(left, EDGE_TRIGGER) {
                Some(Trigger::Left)
            } else if near(right, EDGE_TRIGGER) {
                Some(Trigger::Right)
            } else {
                None
            }
        };
        let trigger = corner.or_else(edge)?;
        self.zones.iter().find(|z| z.trigger == trigger)
    }
}

/// What a snap key does to a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapStep {
    To(ZoneId),
    /// Back to the pre-snap rectangle (or, for a maximized tiled window, its tile).
    Restore,
    Nothing,
}

/// The mod+arrow state table (docs/plan.md). `from` is the window's zone, None when it
/// is not snapped.
pub fn snap_step(from: Option<ZoneId>, key: Direction) -> SnapStep {
    use Direction::*;
    use SnapStep::*;
    let z = |id| To(id);
    match (from, key) {
        (None, Left) => z(ZoneId::LEFT),
        (None, Right) => z(ZoneId::RIGHT),
        (None, Up) => z(ZoneId::FULL),
        (None, Down) => Nothing,
        (Some(ZoneId::LEFT), Left) => z(ZoneId::BOTTOM_LEFT),
        (Some(ZoneId::LEFT), Right) => z(ZoneId::RIGHT),
        (Some(ZoneId::LEFT), Up) => z(ZoneId::TOP_LEFT),
        (Some(ZoneId::LEFT), Down) => Restore,
        (Some(ZoneId::RIGHT), Left) => z(ZoneId::LEFT),
        (Some(ZoneId::RIGHT), Right) => z(ZoneId::BOTTOM_RIGHT),
        (Some(ZoneId::RIGHT), Up) => z(ZoneId::TOP_RIGHT),
        (Some(ZoneId::RIGHT), Down) => Restore,
        (Some(ZoneId::TOP_LEFT), Left) => Nothing,
        (Some(ZoneId::TOP_LEFT), Right) => z(ZoneId::TOP_RIGHT),
        (Some(ZoneId::TOP_LEFT), Up) => z(ZoneId::FULL),
        (Some(ZoneId::TOP_LEFT), Down) => z(ZoneId::LEFT),
        (Some(ZoneId::TOP_RIGHT), Left) => z(ZoneId::TOP_LEFT),
        (Some(ZoneId::TOP_RIGHT), Right) => Nothing,
        (Some(ZoneId::TOP_RIGHT), Up) => z(ZoneId::FULL),
        (Some(ZoneId::TOP_RIGHT), Down) => z(ZoneId::RIGHT),
        (Some(ZoneId::BOTTOM_LEFT), Left) => Nothing,
        (Some(ZoneId::BOTTOM_LEFT), Right) => z(ZoneId::BOTTOM_RIGHT),
        (Some(ZoneId::BOTTOM_LEFT), Up) => z(ZoneId::TOP_LEFT),
        (Some(ZoneId::BOTTOM_LEFT), Down) => z(ZoneId::LEFT),
        (Some(ZoneId::BOTTOM_RIGHT), Left) => z(ZoneId::BOTTOM_LEFT),
        (Some(ZoneId::BOTTOM_RIGHT), Right) => Nothing,
        (Some(ZoneId::BOTTOM_RIGHT), Up) => z(ZoneId::TOP_RIGHT),
        (Some(ZoneId::BOTTOM_RIGHT), Down) => z(ZoneId::RIGHT),
        (Some(ZoneId::FULL), Left) => z(ZoneId::LEFT),
        (Some(ZoneId::FULL), Right) => z(ZoneId::RIGHT),
        (Some(ZoneId::FULL), Up) => Nothing,
        (Some(ZoneId::FULL), Down) => Restore,
        // A zone the keybind table does not know (edited table): Down restores.
        (Some(_), Down) => Restore,
        (Some(_), _) => Nothing,
    }
}

/// Snaps the moving edges of `rect` to the output edges and to the edges of `others`
/// within EDGE_SNAP pixels, nearest first, per axis. A window edge only counts if the
/// two rectangles overlap on the other axis. `moving` says which edges move: all four
/// for a move (the size is kept), the dragged ones for a resize.
pub fn snap_to_edges(rect: Rect, moving: Edges, area: Rect, others: &[Rect]) -> Rect {
    let overlap = |a0: i32, a1: i32, b0: i32, b1: i32| a0 < b1 && b0 < a1;
    // Candidate positions for vertical edges (x) and horizontal edges (y).
    let mut xs = vec![area.x, area.right()];
    let mut ys = vec![area.y, area.bottom()];
    for o in others {
        if overlap(rect.y, rect.bottom(), o.y, o.bottom()) {
            xs.extend([o.x, o.right()]);
        }
        if overlap(rect.x, rect.right(), o.x, o.right()) {
            ys.extend([o.y, o.bottom()]);
        }
    }
    // The smallest correction (by absolute value) that brings one of `edges` onto a
    // target, if within the threshold.
    let best = |edges: &[i32], targets: &[i32]| -> Option<i32> {
        edges
            .iter()
            .flat_map(|e| targets.iter().map(move |t| t - e))
            .filter(|d| d.abs() <= EDGE_SNAP)
            .min_by_key(|d| (d.abs(), *d))
    };
    let mut out = rect;
    let is_move = moving == Edges::ALL;
    if is_move {
        if let Some(dx) = best(&[rect.x, rect.right()], &xs) {
            out.x += dx;
        }
        if let Some(dy) = best(&[rect.y, rect.bottom()], &ys) {
            out.y += dy;
        }
        return out;
    }
    if moving.left
        && let Some(dx) = best(&[rect.x], &xs)
    {
        out.x += dx;
        out.w -= dx;
    } else if moving.right
        && let Some(dx) = best(&[rect.right()], &xs)
    {
        out.w += dx;
    }
    if moving.top
        && let Some(dy) = best(&[rect.y], &ys)
    {
        out.y += dy;
        out.h -= dy;
    } else if moving.bottom
        && let Some(dy) = best(&[rect.bottom()], &ys)
    {
        out.h += dy;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0, 0, 800, 600);

    fn zone(id: ZoneId) -> Zone {
        *Zones::default().get(id).expect("default zone")
    }

    #[test]
    fn default_zone_rectangles() {
        assert_eq!(zone(ZoneId::FULL).rect(AREA), AREA);
        assert_eq!(zone(ZoneId::LEFT).rect(AREA), Rect::new(0, 0, 400, 600));
        assert_eq!(zone(ZoneId::RIGHT).rect(AREA), Rect::new(400, 0, 400, 600));
        assert_eq!(zone(ZoneId::TOP_LEFT).rect(AREA), Rect::new(0, 0, 400, 300));
        assert_eq!(
            zone(ZoneId::BOTTOM_RIGHT).rect(AREA),
            Rect::new(400, 300, 400, 300)
        );
    }

    /// Halves and quadrants cover the area exactly for odd, tiny and offset areas.
    #[test]
    fn halves_and_quadrants_cover_exactly() {
        let areas = [
            AREA,
            Rect::new(0, 0, 801, 601),
            Rect::new(1920, -100, 333, 7),
            Rect::new(0, 0, 1, 1),
            Rect::new(0, 0, 3, 2),
        ];
        let groups: [&[ZoneId]; 2] = [
            &[ZoneId::LEFT, ZoneId::RIGHT],
            &[
                ZoneId::TOP_LEFT,
                ZoneId::TOP_RIGHT,
                ZoneId::BOTTOM_LEFT,
                ZoneId::BOTTOM_RIGHT,
            ],
        ];
        for area in areas {
            for group in groups {
                let rects: Vec<Rect> = group.iter().map(|id| zone(*id).rect(area)).collect();
                let covered: i64 = rects.iter().map(|r| r.area()).sum();
                assert_eq!(covered, area.area(), "{area:?} {group:?}");
                for (i, a) in rects.iter().enumerate() {
                    assert!(a.w >= 0 && a.h >= 0);
                    assert!(
                        a.x >= area.x
                            && a.right() <= area.right()
                            && a.y >= area.y
                            && a.bottom() <= area.bottom()
                    );
                    for b in &rects[i + 1..] {
                        assert!(!a.overlaps(b), "{a:?} overlaps {b:?}");
                    }
                }
            }
        }
        // Odd sizes: the second half gets the remainder.
        let odd = Rect::new(0, 0, 801, 601);
        assert_eq!(zone(ZoneId::LEFT).rect(odd).w, 400);
        assert_eq!(zone(ZoneId::RIGHT).rect(odd).w, 401);
        assert_eq!(
            zone(ZoneId::BOTTOM_RIGHT).rect(odd),
            Rect::new(400, 300, 401, 301)
        );
    }

    #[test]
    fn point_to_zone_with_corner_priority() {
        let zones = Zones::default();
        let at = |x, y| zones.at(AREA, x, y).map(|z| z.id);
        assert_eq!(at(400, 0), Some(ZoneId::FULL));
        assert_eq!(at(400, 7), Some(ZoneId::FULL));
        assert_eq!(
            at(400, 8),
            None,
            "8 px from the top is outside the edge band"
        );
        assert_eq!(at(0, 300), Some(ZoneId::LEFT));
        assert_eq!(at(799, 300), Some(ZoneId::RIGHT));
        assert_eq!(at(792, 300), Some(ZoneId::RIGHT));
        assert_eq!(at(791, 300), None);
        assert_eq!(at(400, 599), None, "no bottom zone");
        // Corners: within 55 px of two edges, even inside an edge band.
        assert_eq!(at(0, 0), Some(ZoneId::TOP_LEFT));
        assert_eq!(at(54, 54), Some(ZoneId::TOP_LEFT));
        assert_eq!(
            at(55, 3),
            Some(ZoneId::FULL),
            "55 px from the left: top edge, not corner"
        );
        assert_eq!(at(5, 54), Some(ZoneId::TOP_LEFT));
        assert_eq!(at(5, 55), Some(ZoneId::LEFT));
        assert_eq!(at(799, 0), Some(ZoneId::TOP_RIGHT));
        assert_eq!(at(0, 599), Some(ZoneId::BOTTOM_LEFT));
        assert_eq!(at(799, 599), Some(ZoneId::BOTTOM_RIGHT));
        assert_eq!(at(746, 546), Some(ZoneId::BOTTOM_RIGHT));
        assert_eq!(at(744, 546), None, "55 px from the right: no corner");
        assert_eq!(at(400, 300), None);
        assert_eq!(at(-1, 300), None, "outside the output");
    }

    #[test]
    fn tiny_output_zones() {
        let tiny = Rect::new(0, 0, 3, 2);
        let zones = Zones::default();
        // Every point is near every edge; corner bands overlap, so edges decide.
        assert_eq!(zones.at(tiny, 1, 0).map(|z| z.id), Some(ZoneId::FULL));
        for id in [ZoneId::LEFT, ZoneId::TOP_LEFT, ZoneId::BOTTOM_RIGHT] {
            let r = zone(id).rect(tiny);
            assert!(
                r.w >= 0 && r.h >= 0 && r.right() <= 3 && r.bottom() <= 2,
                "{id:?} {r:?}"
            );
        }
    }

    #[test]
    fn keybind_state_table() {
        use Direction::*;
        let all = [
            None,
            Some(ZoneId::LEFT),
            Some(ZoneId::RIGHT),
            Some(ZoneId::TOP_LEFT),
            Some(ZoneId::TOP_RIGHT),
            Some(ZoneId::BOTTOM_LEFT),
            Some(ZoneId::BOTTOM_RIGHT),
            Some(ZoneId::FULL),
        ];
        // Every zone is reachable from unsnapped.
        let mut reached = std::collections::BTreeSet::new();
        let mut frontier = vec![None];
        while let Some(state) = frontier.pop() {
            for key in [Left, Right, Up, Down] {
                if let SnapStep::To(next) = snap_step(state, key)
                    && reached.insert(next)
                {
                    frontier.push(Some(next));
                }
            }
        }
        assert_eq!(
            reached.len(),
            7,
            "every default zone is reachable: {reached:?}"
        );
        // Repeated Down always ends at restore within two presses.
        for start in all.iter().copied().filter(Option::is_some) {
            let mut state = start;
            let mut restored = false;
            for _ in 0..3 {
                match snap_step(state, Down) {
                    SnapStep::Restore => {
                        restored = true;
                        break;
                    }
                    SnapStep::To(next) => state = Some(next),
                    SnapStep::Nothing => break,
                }
            }
            assert!(restored, "Down from {start:?} does not end at restore");
        }
        assert_eq!(snap_step(None, Down), SnapStep::Nothing);
        assert_eq!(
            snap_step(Some(ZoneId::LEFT), Left),
            SnapStep::To(ZoneId::BOTTOM_LEFT)
        );
        assert_eq!(
            snap_step(Some(ZoneId::LEFT), Up),
            SnapStep::To(ZoneId::TOP_LEFT)
        );
        assert_eq!(
            snap_step(Some(ZoneId::TOP_LEFT), Up),
            SnapStep::To(ZoneId::FULL)
        );
        assert_eq!(
            snap_step(Some(ZoneId::FULL), Left),
            SnapStep::To(ZoneId::LEFT)
        );
        assert_eq!(
            snap_step(Some(ZoneId::BOTTOM_RIGHT), Up),
            SnapStep::To(ZoneId::TOP_RIGHT)
        );
    }

    #[test]
    fn edge_snapping() {
        let none: &[Rect] = &[];
        // Move: within 13 px of the left output edge snaps; 14 px does not.
        let r = Rect::new(13, 100, 200, 100);
        assert_eq!(
            snap_to_edges(r, Edges::ALL, AREA, none),
            Rect::new(0, 100, 200, 100)
        );
        let r = Rect::new(14, 100, 200, 100);
        assert_eq!(snap_to_edges(r, Edges::ALL, AREA, none), r);
        // Outside the output: the right edge at 810 snaps back to 800.
        let r = Rect::new(610, 100, 200, 100);
        assert_eq!(
            snap_to_edges(r, Edges::ALL, AREA, none),
            Rect::new(600, 100, 200, 100)
        );
        // Nearest wins: left edge 5 px from a window edge, right edge 3 px from the output edge.
        let other = Rect::new(0, 0, 395, 600);
        let r = Rect::new(400, 100, 403, 100);
        assert_eq!(
            snap_to_edges(r, Edges::ALL, AREA, &[other]),
            Rect::new(397, 100, 403, 100)
        );
        // A window that does not overlap vertically is not a target.
        let far = Rect::new(0, 400, 395, 100);
        let r = Rect::new(400, 100, 200, 100);
        assert_eq!(snap_to_edges(r, Edges::ALL, AREA, &[far]), r);
        // Resize: only the dragged edge moves; the other edges stay.
        let r = Rect::new(100, 100, 290, 200);
        let right = Edges {
            right: true,
            ..Default::default()
        };
        assert_eq!(
            snap_to_edges(r, right, AREA, &[Rect::new(400, 0, 100, 600)]),
            Rect::new(100, 100, 300, 200)
        );
        let left = Edges {
            left: true,
            ..Default::default()
        };
        assert_eq!(
            snap_to_edges(Rect::new(9, 100, 200, 200), left, AREA, none),
            Rect::new(0, 100, 209, 200)
        );
    }

    #[test]
    fn zone_output_edges() {
        let e = zone(ZoneId::LEFT).output_edges();
        assert_eq!(
            (e.left, e.right, e.top, e.bottom),
            (true, false, true, true)
        );
        let e = zone(ZoneId::BOTTOM_RIGHT).output_edges();
        assert_eq!(
            (e.left, e.right, e.top, e.bottom),
            (false, true, false, true)
        );
    }
}
