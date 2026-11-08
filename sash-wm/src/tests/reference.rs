// SPDX-License-Identifier: Apache-2.0

//! The CPU reference of a window's decorations (M18b): a window composed the way a
//! frame draws it, in the same order and placed by the same frame: the client's buffer
//! (masked by the rounded shape), then `decorations::pieces` (the dim, the bar, hovered
//! squares, glyphs), the title, and the outline on top. The goldens are made from it,
//! and the GPU tests compare the real shaders with it.

use sash_text::{ClientText, TextStack};
use sash_theme::{Color, Theme};
use smithay::utils::{Physical, Rectangle};

use crate::{
    decorations::{Look, Piece, centred_px, pieces},
    glyphs,
    layout::{
        Rect,
        decor::{BORDER, Decoration, HOVER_RADIUS, HOVER_SQUARE},
        gaps::{Frame, physical_gap},
        shape::{RADIUS, Shape},
    },
    titles,
};

/// One window to compose.
pub struct Window<'a> {
    pub outer: Rect,
    pub decoration: Decoration,
    pub look: Look,
    /// The title, if any (shaped and drawn as the bar does).
    pub title: Option<&'a str>,
    /// What the stand-in client draws: a colour per client pixel (physical, relative to
    /// the client's top-left corner).
    pub client: &'a dyn Fn(i32, i32) -> [f32; 3],
}

/// A picture: straight RGB in 0..1 per physical pixel.
pub struct Picture {
    pub w: i32,
    pub h: i32,
    /// Where the picture's top-left pixel is in output pixels.
    pub origin: (i32, i32),
    pub rgb: Vec<[f32; 3]>,
}

impl Picture {
    /// A picture of `area` (physical pixels) filled with `background`.
    pub fn new(area: Rectangle<i32, Physical>, background: Color) -> Self {
        let bg = rgb(background);
        Self {
            w: area.size.w,
            h: area.size.h,
            origin: (area.loc.x, area.loc.y),
            rgb: vec![bg; (area.size.w * area.size.h) as usize],
        }
    }

    /// Blends `color` at `coverage` (0..1, times the colour's alpha) over the pixel at
    /// output pixel (x, y).
    fn blend(&mut self, x: i32, y: i32, color: [f32; 3], alpha: f32) {
        let (px, py) = (x - self.origin.0, y - self.origin.1);
        if alpha <= 0.0 || px < 0 || py < 0 || px >= self.w || py >= self.h {
            return;
        }
        let p = &mut self.rgb[(py * self.w + px) as usize];
        for i in 0..3 {
            p[i] = color[i] * alpha + p[i] * (1.0 - alpha);
        }
    }

    /// 8-bit RGB rows.
    pub fn bytes(&self) -> Vec<u8> {
        self.rgb
            .iter()
            .flat_map(|p| p.map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8))
            .collect()
    }

    pub fn at(&self, x: i32, y: i32) -> [u8; 3] {
        let p = self.rgb[((y - self.origin.1) * self.w + x - self.origin.0) as usize];
        p.map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8)
    }
}

fn rgb(c: Color) -> [f32; 3] {
    [c.r(), c.g(), c.b()].map(|v| f32::from(v) / 255.0)
}

fn alpha(c: Color) -> f32 {
    f32::from(c.alpha) / 100.0
}

/// Composes `window` into `picture` with `theme` at `frame`'s scale, rounded by
/// `RADIUS` when `window.look.rounded`.
pub fn compose(
    picture: &mut Picture,
    stack: &mut TextStack,
    theme: &Theme,
    frame: &Frame,
    window: &Window<'_>,
) {
    let scale = frame.scale;
    let shape = window
        .look
        .rounded
        .then(|| Shape::from_physical(frame.physical, RADIUS, scale as f32));
    let cover = |x: i32, y: i32| {
        shape
            .as_ref()
            .map_or(1.0, |s| s.coverage(x as f32 + 0.5, y as f32 + 0.5))
    };
    let p = frame.physical;
    let pixels = |r: Rectangle<i32, Physical>| {
        (r.loc.y..r.loc.y + r.size.h)
            .flat_map(move |y| (r.loc.x..r.loc.x + r.size.w).map(move |x| (x, y)))
    };
    // The client's buffer: at the frame's point for the client's corner, its own size.
    let inner = window.decoration.inner(window.outer);
    let at = frame.point(inner.x, inner.y);
    let client = Rectangle::<i32, Physical>::new(
        at,
        (
            (f64::from(inner.w) * scale).round() as i32,
            (f64::from(inner.h) * scale).round() as i32,
        )
            .into(),
    );
    for (x, y) in pixels(client) {
        let c = (window.client)(x - at.x, y - at.y);
        picture.blend(x, y, c, cover(x, y));
    }
    let all = pieces(theme, window.outer, window.decoration, window.look);
    let mut title_done = false;
    for piece in all {
        if matches!(piece, Piece::Ring(..) | Piece::Outline(..)) && !title_done {
            title_done = true;
            draw_title(picture, stack, theme, frame, window);
        }
        match piece {
            Piece::Fill(r, c) | Piece::Outline(r, c) => {
                for (x, y) in pixels(frame.map(r)) {
                    picture.blend(x, y, rgb(c), alpha(c));
                }
            }
            Piece::ShapeFill(r, c) => {
                for (x, y) in pixels(frame.map(r)) {
                    picture.blend(x, y, rgb(c), alpha(c) * cover(x, y));
                }
            }
            Piece::Square(area, c) => {
                let square = centred_px(frame.map(area), physical_gap(HOVER_SQUARE, scale));
                if window.look.rounded {
                    let s = Shape::from_physical(square, HOVER_RADIUS, scale as f32);
                    for (x, y) in pixels(square) {
                        let k = s.coverage(x as f32 + 0.5, y as f32 + 0.5);
                        picture.blend(x, y, rgb(c), alpha(c) * k);
                    }
                } else {
                    for (x, y) in pixels(square) {
                        picture.blend(x, y, rgb(c), alpha(c));
                    }
                }
            }
            Piece::Glyph(glyph, area, ink) => {
                let n = glyphs::size_px(scale);
                let at = centred_px(frame.map(area), n);
                let image = glyphs::argb(glyph, scale, ink);
                for (i, px) in image.chunks(4).enumerate() {
                    let (x, y) = (at.loc.x + i as i32 % n, at.loc.y + i as i32 / n);
                    let a = f32::from(px[3]) / 255.0;
                    if a > 0.0 {
                        // Premultiplied: back to straight for the blend.
                        let c = [px[2], px[1], px[0]].map(|v| f32::from(v) / 255.0 / a);
                        picture.blend(x, y, c, a);
                    }
                }
            }
            Piece::Ring(edges, c) => {
                let s = shape.as_ref().expect("a ring needs the shape");
                let width = BORDER as f32 * scale as f32;
                for (x, y) in pixels(p) {
                    let k = s.ring_coverage(x as f32 + 0.5, y as f32 + 0.5, width, edges);
                    picture.blend(x, y, rgb(c), alpha(c) * k);
                }
            }
        }
    }
    if !title_done {
        draw_title(picture, stack, theme, frame, window);
    }
}

/// The title where `Sash::bar_title` puts it.
fn draw_title(
    picture: &mut Picture,
    stack: &mut TextStack,
    theme: &Theme,
    frame: &Frame,
    window: &Window<'_>,
) {
    let (Some(text), Some(area), Some(bar)) = (
        window.title,
        window.decoration.title_rect(window.outer),
        window.decoration.bar_rect(window.outer),
    ) else {
        return;
    };
    if text.is_empty() {
        return;
    }
    let focused = window.look.focused;
    let line = titles::shape(stack, &ClientText::from(text), focused);
    let (image, width) = titles::draw(
        stack,
        &line,
        theme.bar_ink(focused),
        area.w,
        frame.scale as f32,
    );
    let at = titles::title_box((width, line.height()), frame.map(bar), frame.scale);
    let (x0, y0) = (
        at.loc.x - image.origin.0 as i32,
        at.loc.y - image.origin.1 as i32,
    );
    for y in 0..image.height {
        for x in 0..image.width {
            let (a, r, g, b) = image.argb(x, y);
            if a == 0 {
                continue;
            }
            let a = f32::from(a) / 255.0;
            let c = [r, g, b].map(|v| (f32::from(v) / 255.0 / a).min(1.0));
            picture.blend(x0 + x as i32, y0 + y as i32, c, a);
        }
    }
}

/// Where a title's line box is in its bar (tests of centring and overlap).
pub fn title_box(
    stack: &mut TextStack,
    frame: &Frame,
    outer: Rect,
    text: &str,
    focused: bool,
) -> Option<Rectangle<i32, Physical>> {
    let d = Decoration::FULL;
    let (area, bar) = (d.title_rect(outer)?, d.bar_rect(outer)?);
    let line = titles::shape(stack, &ClientText::from(text), focused);
    let fitted = line.fit(Some(area.w as f32));
    (!text.is_empty()).then(|| {
        titles::title_box(
            (fitted.width(), fitted.height()),
            frame.map(bar),
            frame.scale,
        )
    })
}
