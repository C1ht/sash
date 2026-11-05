// SPDX-License-Identifier: Apache-2.0

//! The glyph cache: rasterised glyphs by font, glyph, pixel size and weight, with a
//! byte cap over all sizes and scale factors and least-recently-used eviction.
//!
//! A glyph in use is never freed: images are shared (`Rc`), so one a caller still holds
//! outlives its eviction; and every glyph of the line being drawn is pinned (marked
//! with the current draw's generation) until that draw is over, so drawing a line can
//! never evict its own glyphs.

use std::{collections::HashMap, rc::Rc};

/// What identifies a rasterised glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    pub font: u8,
    pub id: u16,
    /// Pixel size in 1/64 px.
    pub size: u32,
    pub weight: u16,
}

/// A rasterised glyph: an alpha mask, or premultiplied RGBA for colour glyphs.
#[derive(Debug, Default)]
pub(crate) struct GlyphImage {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub color: bool,
    pub data: Vec<u8>,
}

/// Bytes counted per entry besides its pixels (key, map slot, Rc header).
const ENTRY_OVERHEAD: usize = 64;

struct Entry {
    image: Rc<GlyphImage>,
    /// The draw that last used it.
    generation: u64,
    /// Least recently used first.
    stamp: u64,
}

pub(crate) struct GlyphCache {
    map: HashMap<Key, Entry>,
    cap: usize,
    bytes: usize,
    generation: u64,
    clock: u64,
}

fn cost(image: &GlyphImage) -> usize {
    image.data.len() + ENTRY_OVERHEAD
}

impl GlyphCache {
    pub fn new(cap: usize) -> Self {
        Self {
            map: HashMap::new(),
            cap,
            bytes: 0,
            generation: 0,
            clock: 0,
        }
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Starts a draw: glyphs looked up from now on are pinned until `end_draw`.
    pub fn begin_draw(&mut self) {
        self.generation += 1;
    }

    /// The glyph for `key`, rasterised by `render` if it is not cached.
    pub fn get(&mut self, key: Key, render: impl FnOnce() -> GlyphImage) -> Rc<GlyphImage> {
        self.clock += 1;
        let (generation, clock) = (self.generation, self.clock);
        if let Some(entry) = self.map.get_mut(&key) {
            entry.generation = generation;
            entry.stamp = clock;
            return entry.image.clone();
        }
        let image = Rc::new(render());
        self.bytes += cost(&image);
        self.map.insert(
            key,
            Entry {
                image: image.clone(),
                generation,
                stamp: clock,
            },
        );
        self.evict();
        image
    }

    /// Ends a draw: its glyphs may be evicted again.
    pub fn end_draw(&mut self) {
        // A new generation unpins the draw's glyphs.
        self.generation += 1;
        self.evict();
    }

    /// Evicts the least recently used glyphs not pinned by the current draw until the
    /// cache is within its cap (a single line larger than the cap stays until its draw
    /// ends).
    fn evict(&mut self) {
        if self.bytes <= self.cap {
            return;
        }
        let generation = self.generation;
        let mut candidates: Vec<(u64, Key)> = self
            .map
            .iter()
            .filter(|(_, e)| e.generation != generation)
            .map(|(k, e)| (e.stamp, *k))
            .collect();
        candidates.sort_unstable_by_key(|(stamp, _)| *stamp);
        for (_, key) in candidates {
            if self.bytes <= self.cap {
                break;
            }
            if let Some(entry) = self.map.remove(&key) {
                self.bytes -= cost(&entry.image);
            }
        }
    }

    #[cfg(test)]
    pub fn contains(&self, key: &Key) -> bool {
        self.map.contains_key(key)
    }
}
