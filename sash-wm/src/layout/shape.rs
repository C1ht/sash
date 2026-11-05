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
use super::Rect;

/// The corner radius (owner, M15b): a constant; debug builds may override it.
pub const RADIUS: i32 = 10;

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

/// Which corners of `rect` are rounded (owner, after M15b step 1): every corner
/// unless both of its edges lie on the boundary of `area` (the usable area).
/// Top-left, top-right, bottom-right, bottom-left.
pub fn rounded_corners(rect: Rect, area: Rect) -> [bool; 4] {
    let left = rect.x <= area.x;
    let top = rect.y <= area.y;
    let right = rect.right() >= area.right();
    let bottom = rect.bottom() >= area.bottom();
    [
        !(left && top),
        !(right && top),
        !(right && bottom),
        !(left && bottom),
    ]
}

impl Shape {
    /// The shape of a logical rectangle at `scale`, `radius` logical pixels on the
    /// corners that are rounded (never more than half the shorter side).
    pub fn new(rect: Rect, corners: [bool; 4], radius: i32, scale: f32) -> Self {
        let (w, h) = (rect.w as f32 * scale, rect.h as f32 * scale);
        let r = (radius as f32 * scale).min(w / 2.0).min(h / 2.0).max(0.0);
        Self {
            x: rect.x as f32 * scale,
            y: rect.y as f32 * scale,
            w,
            h,
            radii: corners.map(|on| if on { r } else { 0.0 }),
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

    const AREA: Rect = Rect::new(0, 0, 800, 600);

    #[test]
    fn the_corner_rule_at_every_output_corner_and_edge() {
        // Tiles in the four corners: the corner on both output edges is square.
        assert_eq!(
            rounded_corners(Rect::new(0, 0, 400, 300), AREA),
            [false, true, true, true]
        );
        assert_eq!(
            rounded_corners(Rect::new(400, 0, 400, 300), AREA),
            [true, false, true, true]
        );
        assert_eq!(
            rounded_corners(Rect::new(400, 300, 400, 300), AREA),
            [true, true, false, true]
        );
        assert_eq!(
            rounded_corners(Rect::new(0, 300, 400, 300), AREA),
            [true, true, true, false]
        );
        // A half: two corners on the boundary.
        assert_eq!(
            rounded_corners(Rect::new(0, 0, 400, 600), AREA),
            [false, true, true, false]
        );
        // A window touching one edge only: all rounded.
        assert_eq!(
            rounded_corners(Rect::new(100, 0, 400, 300), AREA),
            [true; 4]
        );
        // Maximized (the whole area): square.
        assert_eq!(rounded_corners(AREA, AREA), [false; 4]);
        // Floating in the middle: all rounded.
        assert_eq!(
            rounded_corners(Rect::new(100, 100, 300, 200), AREA),
            [true; 4]
        );
        // Against a top panel (the usable area starts at 30): square there.
        let usable = Rect::new(0, 30, 800, 570);
        assert_eq!(
            rounded_corners(Rect::new(0, 30, 400, 570), usable),
            [false, true, true, false]
        );
    }

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
