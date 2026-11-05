// SPDX-License-Identifier: Apache-2.0

//! sash-text: one line of text, shaped, measured and rasterised on the CPU.
//!
//! No Wayland and no UI code; sash-ui will build on it and sash-wm may use it directly.
//! The engine is swash (owner, after M14 step 1), behind this crate's own API:
//!
//! - [`ClientText`]: untrusted text (window titles, app ids), validated once: lossy
//!   UTF-8, no control characters, at most [`MAX_TEXT_BYTES`] bytes. It has no `Debug`
//!   or `Display`, so it cannot reach a log by accident.
//! - [`TextStack::line`]: shapes one line with Manrope and the fallback fonts
//!   ([`fonts::FALLBACK`]), at most [`MAX_GLYPHS`] glyphs, cut with an ellipsis to fit
//!   a width.
//! - [`TextStack::draw`]: rasterises a line to ARGB8888 at a scale factor, through a
//!   glyph cache of at most [`CACHE_BYTES`] (least recently used glyphs are evicted).
//!
//! Known limits (M14):
//! - Bidi: swash's shaper puts each run of a right-to-left script (Hebrew, Arabic) in
//!   visual order, so a Hebrew or Arabic title, also one with digits or after a Latin
//!   word, reads correctly; the Unicode bidi algorithm is not applied: the paragraph
//!   direction is always left-to-right, mirrored characters (brackets) are not
//!   mirrored, and neutral characters between runs of different directions keep their
//!   logical order. The Arabic result (letters joined, run in visual order) is
//!   unverified by a reader of Arabic.
//! - No IME, no vertical text; grayscale anti-aliasing only; COLRv1 colour glyphs are
//!   not drawn.
//!
//! Nothing here panics on any input (a panic inside swash is caught: the line becomes
//! an ellipsis, a glyph becomes empty).

mod cache;
pub mod fonts;
mod raster;
mod shape;
mod untrusted;

pub use raster::Image;
pub use shape::Line;
pub use untrusted::ClientText;

/// The glyph cache's size cap, over all sizes and scale factors (owner, after M14
/// step 1).
pub const CACHE_BYTES: usize = 4 * 1024 * 1024;
/// Client text is cut to this many bytes (on a character boundary) before anything
/// else (owner, after M14 step 1).
pub const MAX_TEXT_BYTES: usize = 1024;
/// A line has at most this many glyphs; more are cut with an ellipsis.
pub const MAX_GLYPHS: usize = 256;
/// Text size for the shell's UI text, in logical pixels at scale 1. A constant, to be
/// compared by eye at 13, 14 and 15 px on real hardware (owner); no setting.
pub const UI_SIZE: f32 = 13.0;
/// Sizes and scale factors are clamped to these (a 4x scale of a 256 px line is the
/// largest image).
pub const MAX_SIZE: f32 = 256.0;
pub const MAX_SCALE: f32 = 4.0;

/// How a line is set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// Font size in logical pixels.
    pub size: f32,
    /// Manrope's weight axis (200 to 800; 400 regular, 500 medium). The fallback
    /// fonts are static and ignore it.
    pub weight: u16,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            size: UI_SIZE,
            weight: 400,
        }
    }
}

impl Style {
    /// The size and weight, made finite and clamped (never NaN, never zero).
    fn sanitized(self) -> Self {
        let size = if self.size.is_finite() {
            self.size.clamp(1.0, MAX_SIZE)
        } else {
            UI_SIZE
        };
        Self {
            size,
            weight: self.weight.clamp(200, 800),
        }
    }
}

/// Fonts, shaping and rasterisation state, and the glyph cache. One per process.
pub struct TextStack {
    fonts: fonts::Fonts,
    shaper: swash::shape::ShapeContext,
    scaler: swash::scale::ScaleContext,
    cache: cache::GlyphCache,
}

impl Default for TextStack {
    fn default() -> Self {
        Self::new()
    }
}

impl TextStack {
    /// Manrope (always) and the fallback fonts found under /usr/share/fonts (each read
    /// when first needed).
    pub fn new() -> Self {
        Self::with_fonts(fonts::Fonts::system())
    }

    fn with_fonts(fonts: fonts::Fonts) -> Self {
        Self {
            fonts,
            shaper: swash::shape::ShapeContext::new(),
            scaler: swash::scale::ScaleContext::new(),
            cache: cache::GlyphCache::new(CACHE_BYTES),
        }
    }

    /// Shapes `text` as one line in `style`. With `max_width` (logical pixels) the line
    /// is cut at a cluster boundary and ends with an ellipsis if it does not fit; it is
    /// also cut (with an ellipsis) beyond [`MAX_GLYPHS`] or when `text` was cut.
    pub fn line(&mut self, text: &ClientText, style: Style, max_width: Option<f32>) -> Line {
        shape::line(
            &mut self.fonts,
            &mut self.shaper,
            text.as_str(),
            text.was_cut(),
            style.sanitized(),
            max_width,
        )
    }

    /// Rasterises `line` at `scale` in `color` (RGBA, straight alpha) to a premultiplied
    /// ARGB8888 image of the line's measured size. Colour glyphs (emoji) keep their
    /// own colours.
    pub fn draw(&mut self, line: &Line, scale: f32, color: [u8; 4]) -> Image {
        raster::draw(
            &mut self.fonts,
            &mut self.scaler,
            &mut self.cache,
            line,
            scale,
            color,
        )
    }

    /// Bytes held by the glyph cache now.
    pub fn cache_bytes(&self) -> usize {
        self.cache.bytes()
    }

    /// Which fonts are available: (name, loaded). Fallback fonts are loaded on first
    /// need, so "false" can also mean "not needed yet".
    pub fn fonts(&self) -> Vec<(&'static str, bool)> {
        self.fonts.status()
    }
}

#[cfg(test)]
mod tests;
