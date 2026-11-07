// SPDX-License-Identifier: Apache-2.0

//! Damage: the parts of a surface that must be drawn again.
//!
//! A region is a short list of rectangles inside the surface. Adding a rectangle that
//! another one holds changes nothing; past `MAX_RECTS` the list collapses to its
//! bounding box (more pixels drawn, never fewer). Rectangles may overlap: drawing is
//! idempotent (every pixel of the region is drawn from the scene, not blended onto its
//! previous value), so an overlap costs time only.

use crate::geometry::Rect;

/// Rectangles kept before a region collapses to its bounds.
pub const MAX_RECTS: usize = 8;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Damage {
    rects: Vec<Rect>,
}

impl Damage {
    /// The whole of `area`.
    pub fn full(area: Rect) -> Self {
        let mut damage = Self::default();
        damage.add(area);
        damage
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    pub fn rects(&self) -> &[Rect] {
        &self.rects
    }

    /// Adds `rect` (empty rectangles are dropped).
    pub fn add(&mut self, rect: Rect) {
        if rect.is_empty() || self.rects.iter().any(|r| r.contains_rect(&rect)) {
            return;
        }
        self.rects.retain(|r| !rect.contains_rect(r));
        self.rects.push(rect);
        if self.rects.len() > MAX_RECTS {
            let bounds = self.bounds();
            self.rects = vec![bounds];
        }
    }

    pub fn add_all(&mut self, other: &Damage) {
        for rect in &other.rects {
            self.add(*rect);
        }
    }

    /// Keeps only what lies inside `area`.
    pub fn clip(&mut self, area: Rect) {
        let rects = std::mem::take(&mut self.rects);
        for rect in rects {
            self.add(rect.intersect(&area));
        }
    }

    pub fn clear(&mut self) {
        self.rects.clear();
    }

    pub fn bounds(&self) -> Rect {
        self.rects
            .iter()
            .fold(Rect::default(), |acc, rect| acc.bounds(rect))
    }

    /// Pixels drawn for this region (overlaps counted twice, as they are drawn twice).
    pub fn area(&self) -> u64 {
        self.rects.iter().map(Rect::area).sum()
    }

    /// Whether the pixel (x, y) is in the region.
    pub fn contains(&self, x: i32, y: i32) -> bool {
        self.rects.iter().any(|r| r.contains(x, y))
    }
}
