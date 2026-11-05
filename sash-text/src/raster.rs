// SPDX-License-Identifier: Apache-2.0

//! Rasterisation: a line's glyphs through the cache into one ARGB8888 image.

use swash::{
    scale::{Render, ScaleContext, Source, StrikeWith, image::Content},
    zeno::Format,
};

use crate::{
    MAX_SCALE,
    cache::{GlyphCache, GlyphImage, Key},
    fonts::Fonts,
    shape::Line,
};

/// A premultiplied ARGB8888 image (Wayland's and DRM's format: bytes B, G, R, A in
/// memory), `stride` bytes per row. It covers the line's advance box (its measured
/// width and height) and all its ink, which can reach past the box (a "j" at the start
/// of a line); `origin` is where the box's top-left corner is in the image.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub origin: (u32, u32),
    pub pixels: Vec<u8>,
}

impl Image {
    /// The pixel at (x, y) as (a, r, g, b), premultiplied.
    pub fn argb(&self, x: u32, y: u32) -> (u8, u8, u8, u8) {
        let i = (y * self.stride + x * 4) as usize;
        let p = &self.pixels[i..i + 4];
        (p[3], p[2], p[1], p[0])
    }
}

/// The image size of `line` at `scale`: its advance width and height, rounded up.
pub(crate) fn image_size(line: &Line, scale: f32) -> (u32, u32) {
    (
        (line.width() * scale).ceil().max(0.0) as u32,
        (line.height() * scale).ceil().max(0.0) as u32,
    )
}

pub(crate) fn sanitize_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(0.25, MAX_SCALE)
    } else {
        1.0
    }
}

pub(crate) fn draw(
    fonts: &mut Fonts,
    scaler: &mut ScaleContext,
    cache: &mut GlyphCache,
    line: &Line,
    scale: f32,
    color: [u8; 4],
) -> Image {
    let scale = sanitize_scale(scale);
    let (box_w, box_h) = image_size(line, scale);
    let px = line.style.size * scale;
    let baseline = (line.ascent() * scale).round() as i32;
    // Every glyph first (pinned in the cache for this draw), to size the image.
    cache.begin_draw();
    let placed: Vec<_> = line
        .glyphs
        .iter()
        .map(|glyph| {
            let key = Key {
                font: glyph.font,
                id: glyph.id,
                size: (px * 64.0).round() as u32,
                weight: line.style.weight,
            };
            let rendered = cache.get(key, || rasterise(fonts, scaler, key, px));
            let x = (glyph.x * scale).round() as i32 + rendered.left;
            let y = baseline + (glyph.y * scale).round() as i32 - rendered.top;
            (rendered, x, y)
        })
        .collect();
    let (mut x0, mut y0, mut x1, mut y1) = (0, 0, box_w as i32, box_h as i32);
    for (g, x, y) in &placed {
        if g.width > 0 && g.height > 0 {
            x0 = x0.min(*x);
            y0 = y0.min(*y);
            x1 = x1.max(x + g.width as i32);
            y1 = y1.max(y + g.height as i32);
        }
    }
    let (width, height) = ((x1 - x0) as u32, (y1 - y0) as u32);
    let stride = width * 4;
    let mut image = Image {
        width,
        height,
        stride,
        origin: ((-x0) as u32, (-y0) as u32),
        pixels: vec![0; (stride * height) as usize],
    };
    for (g, x, y) in &placed {
        blit(&mut image, g, x - x0, y - y0, color);
    }
    drop(placed);
    cache.end_draw();
    image
}

/// One glyph at `px` pixels; an empty image if the font or glyph has none.
fn rasterise(fonts: &mut Fonts, scaler: &mut ScaleContext, key: Key, px: f32) -> GlyphImage {
    let Some(font) = fonts.loaded(usize::from(key.font)) else {
        return GlyphImage::default();
    };
    let mut builder = scaler.builder(font.font()).size(px).hint(true);
    if font.variable {
        builder = builder.variations(&[("wght", f32::from(key.weight))]);
    }
    let mut scaler = builder.build();
    // Fonts are trusted, glyph ids come from shaping; still, a panic inside swash
    // gives an empty glyph rather than taking the caller down.
    let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        Render::new(&[
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ])
        .format(Format::Alpha)
        .render(&mut scaler, key.id)
    }));
    let Ok(Some(rendered)) = rendered else {
        return GlyphImage::default();
    };
    let p = rendered.placement;
    let color = rendered.content == Content::Color;
    let mut data = rendered.data;
    if color {
        // Straight RGBA to premultiplied.
        for px in data.as_chunks_mut::<4>().0 {
            let a = u16::from(px[3]);
            for c in &mut px[..3] {
                *c = ((u16::from(*c) * a + 127) / 255) as u8;
            }
        }
    }
    let expected = (p.width * p.height) as usize * if color { 4 } else { 1 };
    if data.len() != expected || rendered.content == Content::SubpixelMask {
        return GlyphImage::default();
    }
    GlyphImage {
        left: p.left,
        top: p.top,
        width: p.width,
        height: p.height,
        color,
        data,
    }
}

/// Composites `glyph` at (x, y) over `image` (source over), clipped to the image.
fn blit(image: &mut Image, glyph: &GlyphImage, x: i32, y: i32, color: [u8; 4]) {
    let [r, g, b, a] = color.map(u32::from);
    for gy in 0..glyph.height as i32 {
        let iy = y + gy;
        if iy < 0 || iy >= image.height as i32 {
            continue;
        }
        for gx in 0..glyph.width as i32 {
            let ix = x + gx;
            if ix < 0 || ix >= image.width as i32 {
                continue;
            }
            let src = (gy as u32 * glyph.width + gx as u32) as usize;
            // Premultiplied source (a, r, g, b).
            let (sa, sr, sg, sb) = if glyph.color {
                let p = &glyph.data[src * 4..src * 4 + 4];
                (
                    u32::from(p[3]),
                    u32::from(p[0]),
                    u32::from(p[1]),
                    u32::from(p[2]),
                )
            } else {
                let cov = u32::from(glyph.data[src]);
                let sa = (cov * a + 127) / 255;
                (
                    sa,
                    (r * sa + 127) / 255,
                    (g * sa + 127) / 255,
                    (b * sa + 127) / 255,
                )
            };
            if sa == 0 {
                continue;
            }
            let i = (iy as u32 * image.stride + ix as u32 * 4) as usize;
            let d = &mut image.pixels[i..i + 4];
            let keep = 255 - sa;
            let over = |s: u32, d: u8| (s + (u32::from(d) * keep + 127) / 255).min(255) as u8;
            d[0] = over(sb, d[0]);
            d[1] = over(sg, d[1]);
            d[2] = over(sr, d[2]);
            d[3] = over(sa, d[3]);
        }
    }
}
