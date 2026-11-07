// SPDX-License-Identifier: Apache-2.0

//! Icons (M17 E3): the first-party Tabler set for chrome, theme icons looked up per the
//! freedesktop Icon Theme Specification, SVG through resvg with limits for untrusted
//! files, PNG, and a byte-capped LRU cache.
//!
//! Every icon file is untrusted (it comes from a theme or an app), and so is every
//! icon name: names never become paths, and failures give the fallback icon, never a
//! panic. Nothing here logs; names and file contents never reach a log or the
//! recorder.

use std::rc::Rc;

pub mod cache;
pub mod index;
pub mod loader;
pub mod lookup;
pub mod name;
pub mod png;
pub mod svg;
pub mod symbols;

pub use loader::{IconLoader, SymbolIcon, ThemedIcon};
pub use name::{IconName, IconPath};
pub use symbols::Symbol;

/// A square raster, `px` x `px` for the size it was asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IconRaster {
    /// One alpha byte per pixel: a symbolic icon, drawn in a theme colour.
    Mask(Vec<u8>),
    /// Premultiplied RGBA: a full-colour icon, drawn as it is.
    Color(Vec<u8>),
}

impl IconRaster {
    pub fn bytes(&self) -> usize {
        match self {
            IconRaster::Mask(v) | IconRaster::Color(v) => v.len(),
        }
    }
}

pub trait IconSource {
    /// The icon at `px` x `px` device pixels, for an output at `scale`; None if it
    /// cannot be drawn (nothing is shown).
    fn raster(&self, px: u32, scale: f32) -> Option<Rc<IconRaster>>;
}

/// The largest raster an icon is drawn at (device pixels per side).
pub const MAX_ICON_PX: u32 = 512;
