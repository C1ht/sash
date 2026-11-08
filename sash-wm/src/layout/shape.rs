// SPDX-License-Identifier: Apache-2.0

//! Rounded corners (M15b step 2) as plain data: a window's shape (its visible
//! rectangle with a radius per corner), its signed distance, the coverage of a pixel
//! and of the 1 px border ring. No Smithay types; the shaders in `rounded.rs` compute
//! exactly these formulas (the CPU reference for the goldens and the GPU tests).
//!
//! Distances are in physical pixels: a logical shape is scaled first. A pixel is
//! sampled at its centre; coverage is `clamp(0.5 - distance, 0, 1)`, one pixel of
//! anti-aliasing across the edge.

#[cfg(test)]
use super::Edges;
#[cfg(test)]
use super::Rect;

/// The window corner radius (owner, M18a: 13, was 10): a constant; debug builds may
/// override it.
pub const RADIUS: i32 = 13;

/// A rectangle with a radius per corner, in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Top-left, top-right, bottom-right, bottom-left.
    pub radii: [f32; 4],
}

impl Shape {
    /// The shape of a logical rectangle at `scale` (each edge rounded to a whole
    /// physical pixel), `radius` logical pixels on the corners that are rounded (never
    /// more than half the shorter side).
    #[cfg(test)]
    pub fn new(rect: Rect, corners: [bool; 4], radius: i32, scale: f32) -> Self {
        // Each edge on a whole physical pixel, rounded on its own (owner, M18a), so
        // neighbours keep the exact gap and the outline is crisp at 1.5.
        let edge = |v: i32| super::gaps::physical_edge(v, f64::from(scale)) as f32;
        let (x, y) = (edge(rect.x), edge(rect.y));
        let (w, h) = (edge(rect.right()) - x, edge(rect.bottom()) - y);
        let r = (radius as f32 * scale).min(w / 2.0).min(h / 2.0).max(0.0);
        Self {
            x,
            y,
            w,
            h,
            radii: corners.map(|on| if on { r } else { 0.0 }),
        }
    }

    /// The shape of a physical rectangle (a window's frame), `radius` logical pixels
    /// on every corner (never more than half the shorter side).
    pub fn from_physical(
        rect: smithay::utils::Rectangle<i32, smithay::utils::Physical>,
        radius: i32,
        scale: f32,
    ) -> Self {
        let (w, h) = (rect.size.w as f32, rect.size.h as f32);
        let r = (radius as f32 * scale).min(w / 2.0).min(h / 2.0).max(0.0);
        Self {
            x: rect.loc.x as f32,
            y: rect.loc.y as f32,
            w,
            h,
            radii: [r; 4],
        }
    }

    pub fn is_square(&self) -> bool {
        self.radii.iter().all(|r| *r == 0.0)
    }

    /// Signed distance from (px, py) to the edge: negative inside.
    pub fn distance(&self, px: f32, py: f32) -> f32 {
        let (cx, cy) = (self.x + self.w * 0.5, self.y + self.h * 0.5);
        let (dx, dy) = (px - cx, py - cy);
        let r = if dx < 0.0 {
            if dy < 0.0 {
                self.radii[0]
            } else {
                self.radii[3]
            }
        } else if dy < 0.0 {
            self.radii[1]
        } else {
            self.radii[2]
        };
        let qx = dx.abs() - self.w * 0.5 + r;
        let qy = dy.abs() - self.h * 0.5 + r;
        let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
        outside + qx.max(qy).min(0.0) - r
    }

    /// How much of the pixel whose centre is (px, py) lies inside.
    #[cfg(test)]
    pub fn coverage(&self, px: f32, py: f32) -> f32 {
        (0.5 - self.distance(px, py)).clamp(0.0, 1.0)
    }

    /// Coverage of the border ring `width` pixels wide along the inside of the edge,
    /// drawn only along `edges` (a corner arc if either of its edges has a border; a
    /// straight part by the edge it is nearest to).
    #[cfg(test)]
    pub fn ring_coverage(&self, px: f32, py: f32, width: f32, edges: Edges) -> f32 {
        let d = self.distance(px, py);
        let ring = (0.5 - d).clamp(0.0, 1.0) - (0.5 - (d + width)).clamp(0.0, 1.0);
        if ring <= 0.0 {
            return 0.0;
        }
        let (left, top) = (px - self.x, py - self.y);
        let (right, bottom) = (self.x + self.w - px, self.y + self.h - py);
        let in_corner = |r: f32, a: f32, b: f32| r > 0.0 && a < r && b < r;
        let drawn = if in_corner(self.radii[0], left, top) {
            edges.left || edges.top
        } else if in_corner(self.radii[1], right, top) {
            edges.right || edges.top
        } else if in_corner(self.radii[2], right, bottom) {
            edges.right || edges.bottom
        } else if in_corner(self.radii[3], left, bottom) {
            edges.left || edges.bottom
        } else {
            let nearest = left.min(top).min(right).min(bottom);
            (nearest == left && edges.left)
                || (nearest == top && edges.top)
                || (nearest == right && edges.right)
                || (nearest == bottom && edges.bottom)
        };
        if drawn { ring } else { 0.0 }
    }

    /// Whether the logical pixel (x, y) is in the shape (its centre at coverage over
    /// one half): hit-testing; a cut-off corner falls through.
    pub fn contains_logical(&self, x: i32, y: i32, scale: f32) -> bool {
        self.distance((x as f32 + 0.5) * scale, (y as f32 + 0.5) * scale) <= 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_and_coverage() {
        let s = Shape::new(Rect::new(0, 0, 100, 60), [true; 4], 10, 1.0);
        assert!(s.distance(50.0, 30.0) < -29.0, "deep inside");
        assert!(
            (s.distance(50.5, 0.5) + 0.5).abs() < 1e-4,
            "half a pixel inside the top"
        );
        // The corner: the centre of the arc is (10, 10); its pixel (0, 0) is outside.
        assert!(s.coverage(0.5, 0.5) == 0.0);
        assert!(
            s.coverage(10.5, 0.5) > 0.99,
            "on the straight edge past the arc"
        );
        assert!(
            s.coverage(3.5, 2.5) > 0.0 && s.coverage(3.5, 2.5) < 1.0,
            "anti-aliased arc"
        );
        // Square corners cover the corner pixel fully.
        let sq = Shape::new(Rect::new(0, 0, 100, 60), [false; 4], 10, 1.0);
        assert_eq!(sq.coverage(0.5, 0.5), 1.0);
        assert!(sq.is_square());
    }

    #[test]
    fn the_radius_never_exceeds_half_the_shorter_side() {
        let s = Shape::new(Rect::new(0, 0, 12, 8), [true; 4], 10, 2.0);
        assert_eq!(s.radii, [8.0; 4]);
    }

    #[test]
    fn hit_testing_lets_cut_off_corners_fall_through() {
        let s = Shape::new(Rect::new(100, 100, 300, 200), [true; 4], 10, 1.0);
        assert!(!s.contains_logical(100, 100, 1.0), "the corner pixel");
        assert!(!s.contains_logical(101, 101, 1.0));
        assert!(s.contains_logical(104, 104, 1.0), "inside the arc");
        assert!(s.contains_logical(110, 100, 1.0), "the straight edge");
        assert!(s.contains_logical(250, 200, 1.0));
    }

    /// A rounded window composed by the CPU reference, as a frame draws it: the content
    /// masked by the shape, the bar clipped by it (if `bar`), the 1 px ring on top, over
    /// the background. RGB rows.
    pub fn picture(w: i32, h: i32, scale: f32, bar: bool) -> (u32, u32, Vec<u8>) {
        let pad = 4;
        let rect = Rect::new(pad, pad, w, h);
        let shape = Shape::new(rect, [true; 4], RADIUS, scale);
        let (pw, ph) = (
            ((w + 2 * pad) as f32 * scale).ceil() as u32,
            ((h + 2 * pad) as f32 * scale).ceil() as u32,
        );
        let background = [26.0, 26.0, 26.0];
        let content = [0x40 as f32, 0x60 as f32, 0x20 as f32];
        let bar_fill = [0x2C as f32, 0x2C as f32, 0x2E as f32];
        let ring = [0x0A as f32, 0x84 as f32, 0xFF as f32];
        let mut out = Vec::with_capacity((pw * ph * 3) as usize);
        for y in 0..ph {
            for x in 0..pw {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let inside = shape.coverage(px, py);
                let in_bar = bar && py < (pad as f32 + 1.0 + 32.0) * scale;
                let fill = if in_bar { bar_fill } else { content };
                let ring_cov = shape.ring_coverage(px, py, scale, Edges::ALL);
                let mut c = [0.0f32; 3];
                for i in 0..3 {
                    let under = background[i] * (1.0 - inside) + fill[i] * inside;
                    c[i] = under * (1.0 - ring_cov) + ring[i] * ring_cov;
                }
                out.extend(c.map(|v| v.round().clamp(0.0, 255.0) as u8));
            }
        }
        (pw, ph, out)
    }

    /// Three tiles (1 | 2 / 3) of an 800 x 480 output laid out with the M18a gaps and
    /// radius, drawn with the CPU reference: every corner rounded, the ring on every
    /// edge, the background in the gaps (owner, M18a). 800 x 480 since M19b: at the
    /// former 400 x 240 tiles 2 and 3 would be under the minimum tile.
    pub fn tiles_picture(scale: f32) -> (u32, u32, Vec<u8>) {
        use super::super::{MonitorLayout, NewWindow, WindowId, gaps::Gaps};
        let mut layout = MonitorLayout::new(Rect::new(0, 0, 800, 480));
        for n in 1..=3 {
            layout.open(WindowId(n), NewWindow::default());
            layout.set_server_side(WindowId(n), true);
        }
        layout.set_gaps(Gaps::DEFAULT);
        layout.set_radius(RADIUS);
        let shapes: Vec<Shape> = (1..=3)
            .filter_map(|n| layout.shape(WindowId(n), scale))
            .collect();
        assert_eq!(shapes.len(), 3);
        let (pw, ph) = (
            (800.0 * scale).round() as u32,
            (480.0 * scale).round() as u32,
        );
        let background = [26.0, 26.0, 26.0];
        let content = [0x40 as f32, 0x60 as f32, 0x20 as f32];
        let ring = [0x0A as f32, 0x84 as f32, 0xFF as f32];
        let mut out = Vec::with_capacity((pw * ph * 3) as usize);
        for y in 0..ph {
            for x in 0..pw {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let mut c = background;
                for shape in &shapes {
                    let inside = shape.coverage(px, py);
                    let ring_cov = shape.ring_coverage(px, py, scale, Edges::ALL);
                    for i in 0..3 {
                        let under = c[i] * (1.0 - inside) + content[i] * inside;
                        c[i] = under * (1.0 - ring_cov) + ring[i] * ring_cov;
                    }
                }
                out.extend(c.map(|v| v.round().clamp(0.0, 255.0) as u8));
            }
        }
        (pw, ph, out)
    }

    fn golden(name: &str, (w, h, rgb): (u32, u32, Vec<u8>)) {
        let dir =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/tests/fixtures/rounded");
        let path = dir.join(format!("{name}.png"));
        if std::env::var_os("SASH_TEXT_BLESS").is_some() {
            std::fs::create_dir_all(&dir).unwrap();
            let file = std::fs::File::create(&path).unwrap();
            let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&rgb)
                .unwrap();
            return;
        }
        let decoder =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut data = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut data).unwrap();
        assert_eq!((info.width, info.height), (w, h), "{name}: size");
        // The reference is exact arithmetic: allowed only 1 per channel (rounding of
        // floating point between CPUs).
        let worst = rgb
            .iter()
            .zip(&data)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert!(worst <= 1, "{name}: differs by {worst}");
    }

    /// Goldens of the CPU reference at scales 1, 1.5 and 2 (a window with a bar).
    #[test]
    fn goldens_at_three_scales() {
        for (scale, tag) in [(1.0, "1"), (1.5, "1.5"), (2.0, "2")] {
            golden(&format!("window-x{tag}"), picture(120, 80, scale, true));
            golden(&format!("tiles-x{tag}"), tiles_picture(scale));
        }
    }

    /// Without a bar the four corners are mirror images of each other.
    #[test]
    fn four_corner_symmetry() {
        for scale in [1.0f32, 1.5, 2.0] {
            let (w, h, rgb) = picture(120, 80, scale, false);
            let at = |x: u32, y: u32| {
                let i = ((y * w + x) * 3) as usize;
                [rgb[i], rgb[i + 1], rgb[i + 2]]
            };
            for y in 0..h {
                for x in 0..w {
                    assert_eq!(
                        at(x, y),
                        at(w - 1 - x, y),
                        "x{scale}: left/right at {x},{y}"
                    );
                    assert_eq!(
                        at(x, y),
                        at(x, h - 1 - y),
                        "x{scale}: top/bottom at {x},{y}"
                    );
                }
            }
        }
    }

    /// The ring is continuous: every pixel the shape's edge passes through (partly
    /// covered, or fully covered within a pixel of the edge) carries ring colour; no
    /// seams where the arcs meet the straight edges.
    #[test]
    fn the_border_ring_is_continuous() {
        for scale in [1.0f32, 1.5, 2.0] {
            let shape = Shape::new(Rect::new(4, 4, 120, 80), [true; 4], RADIUS, scale);
            let (pw, ph) = ((128.0 * scale) as u32, (88.0 * scale) as u32);
            for y in 0..ph {
                for x in 0..pw {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let d = shape.distance(px, py);
                    if (-0.5..=0.0).contains(&d) {
                        let ring = shape.ring_coverage(px, py, scale, Edges::ALL);
                        assert!(
                            ring > 0.45,
                            "x{scale}: seam at {x},{y} (d {d}, ring {ring})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_ring_follows_only_bordered_edges() {
        let shape = Shape::new(
            Rect::new(0, 0, 100, 60),
            [false, true, true, false],
            RADIUS,
            1.0,
        );
        let left_only = Edges {
            left: false,
            ..Edges::ALL
        };
        assert_eq!(
            shape.ring_coverage(0.5, 30.5, 1.0, left_only),
            0.0,
            "no border on the left"
        );
        assert!(
            shape.ring_coverage(99.5, 30.5, 1.0, left_only) > 0.9,
            "border on the right"
        );
    }
}
