// SPDX-License-Identifier: Apache-2.0

//! Icon sources. The icon widget asks its source for a raster at the device size it
//! is drawn at. Loading icon files (resvg for SVG, the icon theme, the first-party
//! symbolic set) is E3; until then the only sources are the ones tests build.

/// A square raster, `px` x `px` for the size it was asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IconRaster {
    /// One alpha byte per pixel: a symbolic icon, drawn in a theme colour.
    Mask(Vec<u8>),
    /// Premultiplied RGBA: a full-colour icon, drawn as it is.
    Color(Vec<u8>),
}

pub trait IconSource {
    /// The icon at `px` x `px` device pixels; None if it cannot be drawn (nothing is
    /// shown).
    fn raster(&self, px: u32) -> Option<IconRaster>;
}
