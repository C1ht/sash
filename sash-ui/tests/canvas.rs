// SPDX-License-Identifier: Apache-2.0

//! The canvas (M17 E1): a rectangle and text drawn on the CPU, compared with golden
//! images; drawing stays inside the clip; a region drawn twice or in pieces gives the
//! same pixels as one full draw.

mod common;

use proptest::prelude::*;
use sash_text::{ClientText, Image, Style, TextStack};
use sash_ui::{Canvas, Color, Damage, Rect};

const W: u32 = 320;
const H: u32 = 48;

/// The E1 scene: the dark bar fill, an accent square and one line of Manrope.
struct Scene {
    text: Image,
    square: Rect,
}

impl Scene {
    fn new() -> Self {
        let mut stack = TextStack::new();
        let line = stack.line(
            &ClientText::new("Sash panel 13 px: æøå".as_bytes()),
            Style {
                size: sash_text::UI_SIZE,
                weight: 400,
            },
            None,
        );
        let text = stack.draw(&line, 1.0, Color::rgb(0xFFFFFF).with_alpha(0.92).rgba());
        Self {
            text,
            square: Rect::new(W as i32 - 40, 8, 32, 32),
        }
    }

    fn draw(&self, canvas: &mut Canvas) {
        canvas.fill(Rect::sized(W as i32, H as i32), Color::rgb(0x2C2C2E));
        canvas.fill(self.square, Color::rgb(0x0A84FF));
        canvas.text(&self.text, 12, 16);
    }
}

fn render(scene: &Scene, damage: &Damage, pixels: &mut [u8]) {
    for rect in damage.rects() {
        let mut canvas = Canvas::new(pixels, W, H, *rect).expect("canvas");
        canvas.clear();
        scene.draw(&mut canvas);
    }
}

fn full() -> Vec<u8> {
    let mut pixels = vec![0; (W * H * 4) as usize];
    render(
        &Scene::new(),
        &Damage::full(Rect::sized(W as i32, H as i32)),
        &mut pixels,
    );
    pixels
}

fn golden(name: &str, pixels: &[u8]) {
    common::golden(name, pixels, W, H);
}

#[test]
fn a_rectangle_and_text_match_the_golden() {
    golden("panel-e1", &full());
}

#[test]
fn the_rectangles_are_exact() {
    let pixels = full();
    let at = |x: u32, y: u32| {
        let i = ((y * W + x) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    };
    // RGBA in memory (wl_shm ABGR8888), opaque, so premultiplied equals straight.
    assert_eq!(at(2, 2), [0x2C, 0x2C, 0x2E, 0xFF]);
    assert_eq!(at(W - 40, 8), [0x0A, 0x84, 0xFF, 0xFF]);
    assert_eq!(at(W - 9, 39), [0x0A, 0x84, 0xFF, 0xFF]);
    assert_eq!(at(W - 8, 39), [0x2C, 0x2C, 0x2E, 0xFF]);
    assert_eq!(at(W - 41, 8), [0x2C, 0x2C, 0x2E, 0xFF]);
}

#[test]
fn text_is_drawn_where_asked() {
    let pixels = full();
    let lit = (0..H).any(|y| {
        (12..100).any(|x| {
            let i = ((y * W + x) * 4) as usize;
            pixels[i] > 0x80
        })
    });
    assert!(lit, "no text pixels near x 12");
}

#[test]
fn nothing_is_drawn_outside_the_clip() {
    let scene = Scene::new();
    let mut pixels = vec![7; (W * H * 4) as usize];
    let clip = Rect::new(30, 10, 50, 20);
    let mut canvas = Canvas::new(&mut pixels, W, H, clip).expect("canvas");
    canvas.clear();
    scene.draw(&mut canvas);
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            let i = ((y as u32 * W + x as u32) * 4) as usize;
            if !clip.contains(x, y) {
                assert_eq!(&pixels[i..i + 4], &[7, 7, 7, 7], "({x}, {y}) touched");
            }
        }
    }
}

#[test]
fn a_clip_past_the_buffer_is_cut() {
    let mut pixels = vec![0; (W * H * 4) as usize];
    let canvas =
        Canvas::new(&mut pixels, W, H, Rect::new(-10, -10, 10_000, 10_000)).expect("canvas");
    assert_eq!(canvas.clip(), Rect::sized(W as i32, H as i32));
    assert!(Canvas::new(&mut pixels, W + 1, H, Rect::sized(1, 1)).is_none());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Any damage, drawn over a buffer that held a full draw, leaves it equal to a
    /// full draw: drawing is idempotent per pixel (overlaps and pieces included).
    #[test]
    fn redrawing_any_damage_keeps_the_pixels(
        rects in proptest::collection::vec((-20i32..340, -20i32..60, 0i32..200, 0i32..60), 1..12)
    ) {
        let scene = Scene::new();
        let mut pixels = full();
        let mut damage = Damage::default();
        for (x, y, w, h) in rects {
            damage.add(Rect::new(x, y, w, h));
        }
        damage.clip(Rect::sized(W as i32, H as i32));
        render(&scene, &damage, &mut pixels);
        prop_assert!(pixels == full());
    }

    /// The region never loses a pixel: every pixel of every added rectangle stays in
    /// it, also after it collapses to its bounds.
    #[test]
    fn damage_keeps_every_added_pixel(
        rects in proptest::collection::vec((0i32..100, 0i32..100, 0i32..30, 0i32..30), 0..30)
    ) {
        let mut damage = Damage::default();
        for &(x, y, w, h) in &rects {
            damage.add(Rect::new(x, y, w, h));
        }
        prop_assert!(damage.rects().len() <= sash_ui::damage::MAX_RECTS);
        for (x, y, w, h) in rects {
            for py in y..y + h {
                for px in x..x + w {
                    prop_assert!(damage.contains(px, py));
                }
            }
        }
    }
}

/// A clip that cuts through a rounded fill or a ring (its anti-aliased corners) gives
/// exactly the pixels of a full draw (found in M18b: tiny-skia's coverage changed where
/// the path met the edge of a scratch pixmap cut to the clip).
#[test]
fn a_clip_through_a_rounded_shape_matches_a_full_draw() {
    let shapes = |canvas: &mut Canvas| {
        let tone = Color::rgb(0xFFFFFF).with_alpha(0.05);
        canvas.fill_rounded(Rect::new(20, 6, 90, 36), 9.0, tone);
        canvas.ring(Rect::new(20, 6, 90, 36), 9.0, 1, tone);
        canvas.ring(Rect::new(140, 3, 60, 42), 12.0, 2, Color::rgb(0x5EC4BE));
    };
    let mut full = vec![0; (W * H * 4) as usize];
    shapes(&mut Canvas::new(&mut full, W, H, Rect::sized(W as i32, H as i32)).unwrap());
    for x in (0..W as i32).step_by(3) {
        for w in [1, 2, 3, 7, 30] {
            let mut part = full.clone();
            let clip = Rect::new(x, 0, w, H as i32);
            let mut canvas = Canvas::new(&mut part, W, H, clip).unwrap();
            canvas.clear();
            shapes(&mut canvas);
            assert!(part == full, "clip {clip:?}");
        }
    }
}
