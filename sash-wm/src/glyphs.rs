// SPDX-License-Identifier: Apache-2.0

//! The bar's glyphs (owner, M18b): 15 px at a 2 px stroke, drawn by sash-wm from simple
//! shapes (sash-wm never depends on sash-ui or an SVG renderer), anti-aliased at the
//! output scale. The shapes follow the proportions of the chrome icon set (Tabler, 24 px
//! grid: the "x" from 6 to 18, the "square" from 3 to 21 with a 2 px corner), scaled to
//! the 15 px box; the stroke is the bar's own 2 px.
//!
//! Coverage is sampled on a 5 x 5 grid per pixel, so the same glyph is the same image
//! everywhere (the goldens and the renderer share it).

use crate::layout::decor::{GLYPH, GLYPH_STROKE};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Glyph {
    Close,
    Maximize,
    /// The maximize button of a maximized window: two overlapping squares.
    Restore,
}

/// The glyph's side in physical pixels at `scale`: 15 rounded once (halves up).
pub fn size_px(scale: f64) -> i32 {
    crate::layout::gaps::physical_gap(GLYPH, scale)
}

/// Grid units: the 24 px icon grid mapped onto the 15 px box.
const UNIT: f64 = GLYPH as f64 / 24.0;

/// A rounded rectangle's signed distance (negative inside) in box units.
fn rounded_rect(px: f64, py: f64, (x0, y0, x1, y1): (f64, f64, f64, f64), r: f64) -> f64 {
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (hx, hy) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
    let qx = (px - cx).abs() - hx + r;
    let qy = (py - cy).abs() - hy + r;
    (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
}

/// Distance from a point to a segment.
fn segment(px: f64, py: f64, (ax, ay): (f64, f64), (bx, by): (f64, f64)) -> f64 {
    let (dx, dy) = (bx - ax, by - ay);
    let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    ((px - ax - t * dx).powi(2) + (py - ay - t * dy).powi(2)).sqrt()
}

/// Whether the point (box units) is on the glyph's stroke.
fn inked(glyph: Glyph, x: f64, y: f64) -> bool {
    let half = f64::from(GLYPH_STROKE) / 2.0;
    let g = |v: f64| v * UNIT;
    let corner = g(2.0);
    match glyph {
        Glyph::Close => {
            let (a, b) = (g(6.0), g(18.0));
            segment(x, y, (a, a), (b, b)) <= half || segment(x, y, (b, a), (a, b)) <= half
        }
        Glyph::Maximize => {
            rounded_rect(x, y, (g(3.0), g(3.0), g(21.0), g(21.0)), corner).abs() <= half
        }
        Glyph::Restore => {
            // The front square low left, the back one high right, hidden behind it.
            let front = (g(3.0), g(8.0), g(16.0), g(21.0));
            let back = (g(8.0), g(3.0), g(21.0), g(16.0));
            let f = rounded_rect(x, y, front, corner);
            f.abs() <= half || (rounded_rect(x, y, back, corner).abs() <= half && f > half)
        }
    }
}

/// The glyph's coverage at `scale`, row by row (0 to 255), `size_px(scale)` square.
pub fn coverage(glyph: Glyph, scale: f64) -> Vec<u8> {
    const SAMPLES: i32 = 5;
    let n = size_px(scale);
    // Box units per physical pixel.
    let unit = f64::from(GLYPH) / f64::from(n);
    let mut out = Vec::with_capacity((n * n) as usize);
    for py in 0..n {
        for px in 0..n {
            let mut hits = 0;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let x = (f64::from(px) + (f64::from(sx) + 0.5) / f64::from(SAMPLES)) * unit;
                    let y = (f64::from(py) + (f64::from(sy) + 0.5) / f64::from(SAMPLES)) * unit;
                    hits += i32::from(inked(glyph, x, y));
                }
            }
            out.push(((hits * 255 + 12) / (SAMPLES * SAMPLES)) as u8);
        }
    }
    out
}

/// The glyph in `ink` (straight alpha) as premultiplied ARGB8888 bytes (little endian:
/// B, G, R, A), `size_px(scale)` square.
pub fn argb(glyph: Glyph, scale: f64, ink: sash_theme::Color) -> Vec<u8> {
    let [r, g, b, a] = ink.rgba8();
    coverage(glyph, scale)
        .into_iter()
        .flat_map(|c| {
            let alpha = (u32::from(c) * u32::from(a) + 127) / 255;
            let pre = |v: u8| ((u32::from(v) * alpha + 127) / 255) as u8;
            [pre(b), pre(g), pre(r), alpha as u8]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyphs_are_15_px_rounded_once() {
        assert_eq!(size_px(1.0), 15);
        assert_eq!(size_px(1.25), 19);
        assert_eq!(size_px(1.5), 23);
        assert_eq!(size_px(2.0), 30);
    }

    /// The stroke is about 2 px wide: across the maximize square's left side at
    /// mid-height, the coverage adds up to the stroke width times the scale.
    #[test]
    fn the_stroke_is_2_px() {
        for scale in [1.0, 1.5, 2.0] {
            let n = size_px(scale) as usize;
            let cov = coverage(Glyph::Maximize, scale);
            let row = &cov[(n / 2) * n..(n / 2) * n + n / 2];
            let ink: f64 = row.iter().map(|&c| f64::from(c) / 255.0).sum();
            let want =
                f64::from(GLYPH_STROKE) * scale * f64::from(n as i32) / (f64::from(GLYPH) * scale);
            assert!((ink - want).abs() < 0.3, "x{scale}: {ink} vs {want}");
        }
    }

    /// Close and maximize are symmetric left to right and top to bottom (within one
    /// sample of 25: a sample exactly on the stroke's edge may round either way).
    #[test]
    fn close_and_maximize_are_symmetric() {
        for glyph in [Glyph::Close, Glyph::Maximize] {
            for scale in [1.0, 1.5, 2.0] {
                let n = size_px(scale) as usize;
                let c = coverage(glyph, scale);
                for y in 0..n {
                    for x in 0..n {
                        let a = c[y * n + x];
                        for b in [c[y * n + n - 1 - x], c[(n - 1 - y) * n + x]] {
                            assert!(a.abs_diff(b) <= 11, "{glyph:?} x{scale} at {x},{y}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn restore_hides_the_back_square_behind_the_front() {
        let n = size_px(1.0) as usize;
        let c = coverage(Glyph::Restore, 1.0);
        // Inside the front square (low left), away from its stroke: empty, although
        // the back square's left side passes there.
        let inside = |x: usize, y: usize| c[y * n + x];
        assert_eq!(inside(5, 10), 0);
        assert!(inside(13, 4) > 0, "the back square's right side shows");
    }
}
