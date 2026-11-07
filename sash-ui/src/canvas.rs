// SPDX-License-Identifier: Apache-2.0

//! Drawing into a buffer of premultiplied RGBA bytes (tiny-skia's layout, which is
//! wl_shm / DRM ABGR8888 in memory), clipped to one rectangle.
//!
//! A canvas draws only inside its clip. A redraw clears the clip to transparent first,
//! then the view draws its whole scene: what lies outside the clip is skipped, so the
//! cost follows the damage, and drawing the same region twice gives the same pixels.

use sash_text::Image;
use tiny_skia::{Paint, PixmapMut, Transform};

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
}

impl<'a> Canvas<'a> {
    /// A canvas over `pixels` (`width * height * 4` bytes, rows without padding),
    /// clipped to `clip` (cut to the buffer). None if the sizes do not match.
    pub fn new(pixels: &'a mut [u8], width: u32, height: u32, clip: Rect) -> Option<Self> {
        let pixmap = PixmapMut::from_bytes(pixels, width, height)?;
        let clip = clip.intersect(&Rect::sized(width as i32, height as i32));
        Some(Self { pixmap, clip })
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
