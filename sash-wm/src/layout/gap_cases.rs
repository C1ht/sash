// SPDX-License-Identifier: Apache-2.0

//! The M18a gap cases (owner): one to four columns, a column split top and bottom, a
//! panel on each edge, maximize, fullscreen, every snap zone and its preview, both
//! ways to a quarter. Exactly 8 px between neighbours, 13 px to the usable area's
//! edge, nothing overlapping.

use super::{
    MonitorLayout, NewWindow, Rect, WindowId, ZoneId,
    gaps::{self, Gaps},
};

const OUTPUT: Rect = Rect::new(0, 0, 2000, 900);

fn id(n: u64) -> WindowId {
    WindowId(n)
}

fn gapped(area: Rect) -> MonitorLayout {
    let mut layout = MonitorLayout::new(area);
    layout.set_gaps(Gaps::DEFAULT);
    layout
}

/// Opens window `n`, splitting window `at` (or the focused one).
fn open(layout: &mut MonitorLayout, n: u64, at: Option<u64>) {
    if let Some(at) = at {
        layout.focus(id(at));
    }
    layout.open(id(n), NewWindow::default());
}

fn shown(layout: &MonitorLayout, n: u64) -> Rect {
    layout.shown_rect(id(n)).expect("shown")
}

/// Every pair of shown rectangles is disjoint.
fn assert_apart(layout: &MonitorLayout) {
    let rects: Vec<Rect> = layout.stacking().into_iter().map(|(_, r, _)| r).collect();
    for (i, a) in rects.iter().enumerate() {
        for b in &rects[i + 1..] {
            let apart =
                a.right() <= b.x || b.right() <= a.x || a.bottom() <= b.y || b.bottom() <= a.y;
            assert!(apart, "{a:?} overlaps {b:?}");
        }
    }
}

/// The distance of `r` from each edge of `area`: left, top, right, bottom.
fn margins(r: Rect, area: Rect) -> (i32, i32, i32, i32) {
    (
        r.x - area.x,
        r.y - area.y,
        area.right() - r.right(),
        area.bottom() - r.bottom(),
    )
}

#[test]
fn one_to_four_columns() {
    // Wide enough that each new column still splits side by side.
    let wide = Rect::new(0, 0, 4000, 900);
    for columns in 1..=4u64 {
        let mut layout = gapped(wide);
        open(&mut layout, 1, None);
        for n in 2..=columns {
            // The newest column is the widest: splitting it again is side by side.
            open(&mut layout, n, Some(n - 1));
        }
        let mut rects: Vec<Rect> = (1..=columns).map(|n| shown(&layout, n)).collect();
        rects.sort_by_key(|r| r.x);
        assert_eq!(rects[0].x, 13, "{columns}: 13 to the left edge");
        assert_eq!(wide.right() - rects.last().unwrap().right(), 13);
        for r in &rects {
            assert_eq!(
                (r.y, wide.bottom() - r.bottom()),
                (13, 13),
                "{columns}: {r:?}"
            );
        }
        for pair in rects.windows(2) {
            assert_eq!(pair[1].x - pair[0].right(), 8, "{columns}: {pair:?}");
        }
        assert_apart(&layout);
        layout.check_invariants();
    }
}

#[test]
fn a_column_split_top_and_bottom() {
    // 1000 x 900: the right column (500 x 900) is taller than wide, so it splits.
    let area = Rect::new(0, 0, 1000, 900);
    let mut layout = gapped(area);
    open(&mut layout, 1, None);
    open(&mut layout, 2, Some(1));
    open(&mut layout, 3, Some(2));
    let (left, top, bottom) = (shown(&layout, 1), shown(&layout, 2), shown(&layout, 3));
    assert_eq!(margins(left, area), (13, 13, 1000 - left.right(), 13));
    assert_eq!(top.x - left.right(), 8);
    assert_eq!(bottom.y - top.bottom(), 8, "top / bottom");
    assert_eq!((top.x, top.w), (bottom.x, bottom.w));
    assert_eq!(top.y - area.y, 13);
    assert_eq!(area.bottom() - bottom.bottom(), 13);
    assert_eq!(area.right() - top.right(), 13);
    assert_apart(&layout);
    layout.check_invariants();
}

#[test]
fn a_panel_on_each_edge_keeps_windows_13_px_away() {
    for (edge, usable) in [
        ("top", Rect::new(0, 50, 2000, 850)),
        ("bottom", Rect::new(0, 0, 2000, 790)),
        ("left", Rect::new(110, 0, 1890, 900)),
        ("right", Rect::new(0, 0, 1890, 900)),
    ] {
        let mut layout = gapped(OUTPUT);
        open(&mut layout, 1, None);
        open(&mut layout, 2, Some(1));
        layout.set_geometry(OUTPUT, usable);
        // 1 | 2: both touch a top or bottom panel, the left or right tile a side one.
        let next_to: &[u64] = match edge {
            "left" => &[1],
            "right" => &[2],
            _ => &[1, 2],
        };
        for &n in next_to {
            let (l, t, r, b) = margins(shown(&layout, n), usable);
            let near = match edge {
                "top" => t,
                "bottom" => b,
                "left" => l,
                _ => r,
            };
            assert_eq!(near, 13, "{edge} panel: window {n} to the panel");
        }
        // The dock zone rule (docs/design.md): a 110 px zone puts windows 123 px from
        // that screen edge.
        if edge == "left" {
            assert_eq!(shown(&layout, 1).x, 123);
        }
        assert_apart(&layout);
        layout.check_invariants();
    }
}

#[test]
fn maximize_is_the_usable_area_minus_13_and_fullscreen_has_no_gap() {
    let usable = Rect::new(0, 0, 2000, 790);
    let mut layout = gapped(OUTPUT);
    open(&mut layout, 1, None);
    open(&mut layout, 2, Some(1));
    layout.set_geometry(OUTPUT, usable);
    assert!(layout.set_maximized(id(1), true));
    assert_eq!(shown(&layout, 1), Rect::new(13, 13, 2000 - 26, 790 - 26));
    let size = layout.presentation(id(1)).and_then(|p| p.size);
    assert_eq!(
        size,
        Some((2000 - 26 - 2, 790 - 26 - 2)),
        "minus the border"
    );
    assert!(layout.set_fullscreen(id(2), true));
    assert_eq!(shown(&layout, 2), OUTPUT, "fullscreen: the whole output");
    layout.check_invariants();
}

#[test]
fn every_snap_zone_and_its_preview_use_the_gaps() {
    let area = Rect::new(0, 0, 1000, 800);
    let mut layout = gapped(area);
    open(&mut layout, 1, None);
    let expected = [
        (ZoneId::FULL, Rect::new(13, 13, 974, 774)),
        (ZoneId::LEFT, Rect::new(13, 13, 483, 774)),
        (ZoneId::RIGHT, Rect::new(504, 13, 483, 774)),
        (ZoneId::TOP_LEFT, Rect::new(13, 13, 483, 383)),
        (ZoneId::TOP_RIGHT, Rect::new(504, 13, 483, 383)),
        (ZoneId::BOTTOM_LEFT, Rect::new(13, 404, 483, 383)),
        (ZoneId::BOTTOM_RIGHT, Rect::new(504, 404, 483, 383)),
    ];
    for (zone, want) in expected {
        // The preview shows exactly what the snapped window will show.
        assert_eq!(
            layout.zone_visible_rect(zone),
            Some(want),
            "{zone:?} preview"
        );
        assert!(layout.snap(id(1), zone) || layout.snapped_zone(id(1)) == Some(zone));
        assert_eq!(shown(&layout, 1), want, "{zone:?} snapped");
        layout.check_invariants();
    }
    // A quarter by its chord (mod+Y/U/B/N) is the same as by a drag.
    for (zone, want) in expected.into_iter().skip(3) {
        let mut chord = gapped(area);
        open(&mut chord, 1, None);
        assert!(chord.snap_quarter(id(1), zone));
        assert_eq!(shown(&chord, 1), want, "{zone:?} by chord");
    }
    // Halves meet with exactly the inner gap.
    let left = layout.zone_visible_rect(ZoneId::LEFT).unwrap();
    let right = layout.zone_visible_rect(ZoneId::RIGHT).unwrap();
    assert_eq!(right.x - left.right(), 8);
}

#[test]
fn the_one_function_is_what_the_layout_shows() {
    // Nothing else computes a gap: every gapped rectangle equals gaps::visible of its
    // cell.
    let mut layout = gapped(OUTPUT);
    for n in 1..=5 {
        open(&mut layout, n, Some(n.max(2) - 1));
    }
    assert!(layout.snap(id(5), ZoneId::RIGHT));
    for n in 1..=5 {
        let cell = layout.cell(id(n)).expect("cell");
        assert_eq!(
            shown(&layout, n),
            gaps::visible(cell, layout.area(), Gaps::DEFAULT),
            "window {n}"
        );
    }
}
