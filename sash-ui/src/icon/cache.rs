// SPDX-License-Identifier: Apache-2.0

//! Rasterised icons, kept up to a byte cap and evicted least recently used first.
//! An icon still held elsewhere (a widget drawing it holds its `Rc`) is never evicted:
//! while only such icons are left, the cache may stay above its cap until they are
//! let go.

use std::{collections::HashMap, hash::Hash, rc::Rc};

use super::IconRaster;

pub struct IconCache<K> {
    cap: usize,
    bytes: usize,
    tick: u64,
    entries: HashMap<K, (Rc<IconRaster>, u64)>,
}

impl<K: Hash + Eq + Clone> IconCache<K> {
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            bytes: 0,
            tick: 0,
            entries: HashMap::new(),
        }
    }

    pub fn get(&mut self, key: &K) -> Option<Rc<IconRaster>> {
        self.tick += 1;
        let tick = self.tick;
        self.entries.get_mut(key).map(|(raster, used)| {
            *used = tick;
            raster.clone()
        })
    }

    pub fn insert(&mut self, key: K, raster: Rc<IconRaster>) {
        self.tick += 1;
        self.bytes += raster.bytes();
        if let Some((old, _)) = self.entries.insert(key, (raster, self.tick)) {
            self.bytes -= old.bytes();
        }
        self.evict();
    }

    fn evict(&mut self) {
        while self.bytes > self.cap {
            let victim = self
                .entries
                .iter()
                .filter(|(_, (raster, _))| Rc::strong_count(raster) == 1)
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| k.clone());
            let Some(key) = victim else { break };
            if let Some((raster, _)) = self.entries.remove(&key) {
                self.bytes -= raster.bytes();
            }
        }
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }
}
