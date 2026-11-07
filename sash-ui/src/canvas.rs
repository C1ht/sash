// SPDX-License-Identifier: Apache-2.0

//! Drawing into a buffer of premultiplied RGBA bytes (tiny-skia's layout, which is
//! wl_shm / DRM ABGR8888 in memory), clipped to one rectangle.
//!
//! A canvas draws only inside its clip. A redraw clears the clip to transparent first,
//! then the view draws its whole scene: what lies outside the clip is skipped, so the
//! cost follows the damage, and drawing the same region twice gives the same pixels.
//!
//! Coordinates here are buffer (device) pixels; widgets lay out in logical pixels and
//! convert with [`to_device`] at the canvas's scale. Anti-aliased shapes are drawn into
//! a scratch pixmap of their bounds inside the clip and then blended in, so their soft
//! edges never reach past the clip.

use sash_text::Image;
use tiny_skia::{FillRule, Paint, Path, PathBuilder, Pixmap, PixmapMut, PixmapPaint, Transform};

use crate::geometry::Rect;

/// A colour, straight (not premultiplied) RGBA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(hex: u32) -> Self {
        Self {
            r: (hex >> 16) as u8,
            g: (hex >> 8) as u8,
            b: hex as u8,
            a: 255,
        }
    }

    /// The same colour at `alpha` (0 to 1) of full opacity.
    pub fn with_alpha(self, alpha: f32) -> Self {
        Self {
            a: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
            ..self
        }
    }

    pub fn rgba(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }
}

pub struct Canvas<'a> {
    pixmap: PixmapMut<'a>,
    clip: Rect,
    scale: f32,
}

/// A logical rectangle in device pixels at `scale`: each edge is rounded on its own,
/// so rectangles that touch in logical pixels touch in device pixels too.
pub fn to_device(rect: Rect, scale: f32) -> Rect {
    let edge = |v: i32| (v as f32 * scale).round() as i32;
    let (x, y) = (edge(rect.x), edge(rect.y));
    Rect::new(x, y, edge(rect.right()) - x, edge(rect.bottom()) - y)
}

/// A rectangle with rounded corners, `radius` clamped to half the shorter side.
fn rounded(rect: Rect, radius: f32, pb: &mut PathBuilder) {
    let (x, y, w, h) = (rect.x as f32, rect.y as f32, rect.w as f32, rect.h as f32);
    let r = radius.clamp(0.0, w.min(h) / 2.0);
    // The cubic Bezier approximation of a quarter circle.
    let k = r * 0.552_284_8;
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
}

impl<'a> Canvas<'a> {
    /// A canvas over `pixels` (`width * height * 4` bytes, rows without padding),
    /// clipped to `clip` (cut to the buffer). None if the sizes do not match.
    pub fn new(pixels: &'a mut [u8], width: u32, height: u32, clip: Rect) -> Option<Self> {
        let pixmap = PixmapMut::from_bytes(pixels, width, height)?;
        let clip = clip.intersect(&Rect::sized(width as i32, height as i32));
        Some(Self {
            pixmap,
            clip,
            scale: 1.0,
        })
    }

    /// The same canvas for a scene laid out at `scale` (device pixels per logical
    /// pixel; finite and positive, else 1).
    pub fn with_scale(mut self, scale: f32) -> Self {
        self.scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        self
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn clip(&self) -> Rect {
        self.clip
    }

    pub fn size(&self) -> (u32, u32) {
        (self.pixmap.width(), self.pixmap.height())
    }

    /// Sets the clip to transparent.
    pub fn clear(&mut self) {
        let clip = self.clip;
        if clip.is_empty() {
            return;
        }
        let stride = self.pixmap.width() as usize * 4;
        let data = self.pixmap.data_mut();
        for y in clip.y..clip.bottom() {
            let start = y as usize * stride + clip.x as usize * 4;
            data[start..start + clip.w as usize * 4].fill(0);
        }
    }

    /// Fills `rect` (inside the clip) with `color`, blended over what is there.
    pub fn fill(&mut self, rect: Rect, color: Color) {
        let rect = rect.intersect(&self.clip);
        if rect.is_empty() || color.a == 0 {
            return;
        }
        let Some(area) =
            tiny_skia::Rect::from_xywh(rect.x as f32, rect.y as f32, rect.w as f32, rect.h as f32)
        else {
            return;
        };
        let mut paint = Paint::default();
        paint.set_color_rgba8(color.r, color.g, color.b, color.a);
        // Whole pixels: no anti-aliasing, so nothing bleeds past the clip.
        paint.anti_alias = false;
        self.pixmap
            .fill_rect(area, &paint, Transform::identity(), None);
    }

    /// Fills `rect` with rounded corners of `radius`, anti-aliased.
    pub fn fill_rounded(&mut self, rect: Rect, radius: f32, color: Color) {
        let mut pb = PathBuilder::new();
        rounded(rect, radius, &mut pb);
        if let Some(path) = pb.finish() {
            self.fill_path(rect, &path, FillRule::Winding, color);
        }
    }

    /// A ring `width` pixels wide along the inside of `rect`, its outer corners
    /// rounded by `radius` (the inner ones by `radius - width`), anti-aliased.
    pub fn ring(&mut self, rect: Rect, radius: f32, width: i32, color: Color) {
        let inner = Rect::new(
            rect.x + width,
            rect.y + width,
            rect.w - 2 * width,
            rect.h - 2 * width,
        );
        let mut pb = PathBuilder::new();
        rounded(rect, radius, &mut pb);
        if !inner.is_empty() {
            rounded(inner, (radius - width as f32).max(0.0), &mut pb);
        }
        if let Some(path) = pb.finish() {
            self.fill_path(rect, &path, FillRule::EvenOdd, color);
        }
    }

    fn fill_path(&mut self, bounds: Rect, path: &Path, rule: FillRule, color: Color) {
        let area = bounds.intersect(&self.clip);
        if area.is_empty() || color.a == 0 {
            return;
        }
        let Some(mut scratch) = Pixmap::new(area.w as u32, area.h as u32) else {
            return;
        };
        let mut paint = Paint::default();
        paint.set_color_rgba8(color.r, color.g, color.b, color.a);
        paint.anti_alias = true;
        let shift = Transform::from_translate(-area.x as f32, -area.y as f32);
        scratch.fill_path(path, &paint, rule, shift, None);
        self.pixmap.draw_pixmap(
            area.x,
            area.y,
            scratch.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }

    /// Blends an alpha mask (`size` x `size` bytes, row by row) in `color` with its
    /// top-left corner at (x, y): a symbolic icon recoloured.
    pub fn mask(&mut self, alpha: &[u8], size: u32, x: i32, y: i32, color: Color) {
        let area = Rect::new(x, y, size as i32, size as i32).intersect(&self.clip);
        if area.is_empty() || alpha.len() < (size * size) as usize {
            return;
        }
        let stride = self.pixmap.width() as usize * 4;
        let data = self.pixmap.data_mut();
        let ca = u32::from(color.a);
        for py in area.y..area.bottom() {
            for px in area.x..area.right() {
                let m = u32::from(alpha[((py - y) as u32 * size + (px - x) as u32) as usize]);
                // Source alpha: the mask times the colour's alpha.
                let a = (m * ca + 127) / 255;
                if a == 0 {
                    continue;
                }
                let d = py as usize * stride + px as usize * 4;
                let keep = 255 - a;
                for (i, c) in [color.r, color.g, color.b].into_iter().enumerate() {
                    let src = (u32::from(c) * a + 127) / 255;
                    let dst = u32::from(data[d + i]);
                    data[d + i] = (src + (dst * keep + 127) / 255).min(255) as u8;
                }
                let dst = u32::from(data[d + 3]);
                data[d + 3] = (a + (dst * keep + 127) / 255).min(255) as u8;
            }
        }
    }

    /// Blends a premultiplied RGBA image (`size` x `size`) with its top-left corner at
    /// (x, y): a full-colour icon.
    pub fn image(&mut self, rgba: &[u8], size: u32, x: i32, y: i32) {
        let area = Rect::new(x, y, size as i32, size as i32).intersect(&self.clip);
        if area.is_empty() || rgba.len() < (size * size * 4) as usize {
            return;
        }
        let stride = self.pixmap.width() as usize * 4;
        let data = self.pixmap.data_mut();
        for py in area.y..area.bottom() {
            for px in area.x..area.right() {
                let s = (((py - y) as u32 * size + (px - x) as u32) * 4) as usize;
                let a = u32::from(rgba[s + 3]);
                if a == 0 {
                    continue;
                }
                let d = py as usize * stride + px as usize * 4;
                let keep = 255 - a;
                for i in 0..4 {
                    let dst = u32::from(data[d + i]);
                    data[d + i] =
                        (u32::from(rgba[s + i]) + (dst * keep + 127) / 255).min(255) as u8;
                }
            }
        }
    }

    /// Blends sash-text's image (premultiplied BGRA) over the canvas with the image's
    /// line-box origin at (x, y).
    pub fn text(&mut self, image: &Image, x: i32, y: i32) {
        let left = x - image.origin.0 as i32;
        let top = y - image.origin.1 as i32;
        let area =
            Rect::new(left, top, image.width as i32, image.height as i32).intersect(&self.clip);
        if area.is_empty() {
            return;
        }
        let stride = self.pixmap.width() as usize * 4;
        let data = self.pixmap.data_mut();
        for py in area.y..area.bottom() {
            let src_row = (py - top) as usize * image.stride as usize;
            let dst_row = py as usize * stride;
            for px in area.x..area.right() {
                let s = src_row + (px - left) as usize * 4;
                let [b, g, r, a] = [
                    image.pixels[s],
                    image.pixels[s + 1],
                    image.pixels[s + 2],
                    image.pixels[s + 3],
                ];
                if a == 0 {
                    continue;
                }
                let d = dst_row + px as usize * 4;
                let keep = 255 - u32::from(a);
                for (i, src) in [r, g, b, a].into_iter().enumerate() {
                    let dst = u32::from(data[d + i]);
                    data[d + i] = (u32::from(src) + (dst * keep + 127) / 255).min(255) as u8;
                }
            }
        }
    }
}
