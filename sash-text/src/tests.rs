// SPDX-License-Identifier: Apache-2.0

//! Unit tests that need the crate's insides: the glyph cache and the drawn bounds.

use std::rc::Rc;

use proptest::prelude::*;

use crate::{
    ClientText, Style, TextStack,
    cache::{GlyphCache, GlyphImage, Key},
    fonts::{Fonts, MANROPE},
    raster::{image_size, sanitize_scale},
};

fn key(id: u16) -> Key {
    Key {
        font: 0,
        id,
        size: 13 * 64,
        weight: 400,
    }
}

fn image(bytes: usize) -> GlyphImage {
    GlyphImage {
        width: bytes as u32,
        height: 1,
        data: vec![255; bytes],
        ..Default::default()
    }
}

#[test]
fn the_embedded_manrope_parses_and_has_a_weight_axis() {
    let font = swash::FontRef::from_index(MANROPE, 0).expect("Manrope parses");
    let wght = font
        .variations()
        .find(|v| v.tag() == swash::tag_from_bytes(b"wght"))
        .expect("a weight axis");
    assert_eq!((wght.min_value(), wght.max_value()), (200.0, 800.0));
}

#[test]
fn the_cache_evicts_least_recently_used_down_to_its_cap() {
    let mut cache = GlyphCache::new(10_000);
    for id in 0..30 {
        cache.begin_draw();
        cache.get(key(id), || image(1000));
        cache.end_draw();
        assert!(cache.bytes() <= 10_000, "{} bytes", cache.bytes());
    }
    // The newest are kept, the oldest gone.
    assert!(cache.contains(&key(29)) && !cache.contains(&key(0)));
    // Using an old one makes it recent.
    cache.begin_draw();
    cache.get(key(25), || image(1000));
    cache.end_draw();
    for id in 30..35 {
        cache.begin_draw();
        cache.get(key(id), || image(1000));
        cache.end_draw();
    }
    assert!(cache.contains(&key(25)), "recently used is kept");
}

/// A glyph a caller holds survives its eviction; glyphs of the draw in progress are
/// never evicted, even when that draw alone is larger than the cap.
#[test]
fn eviction_never_frees_a_glyph_in_use() {
    let mut cache = GlyphCache::new(5_000);
    cache.begin_draw();
    let held: Rc<GlyphImage> = cache.get(key(1), || image(1000));
    cache.end_draw();
    // One draw using 20 glyphs of 1000 bytes: all pinned until it ends.
    cache.begin_draw();
    let mut in_draw = Vec::new();
    for id in 100..120 {
        in_draw.push(cache.get(key(id), || image(1000)));
        for earlier in 100..=id {
            assert!(
                cache.contains(&key(earlier)),
                "glyph {earlier} evicted mid-draw"
            );
        }
    }
    cache.end_draw();
    assert!(
        cache.bytes() <= 5_000,
        "back within the cap once the draw ended"
    );
    assert!(!cache.contains(&key(1)), "evicted from the cache");
    assert_eq!(held.data.len(), 1000, "but still alive for its holder");
    assert!(held.data.iter().all(|b| *b == 255));
}

/// Drawing through a cache far smaller than the line gives the same pixels as through
/// a large one.
#[test]
fn a_line_larger_than_the_cache_draws_the_same() {
    let mut big = TextStack::new();
    let mut tiny = TextStack::new();
    tiny.cache = GlyphCache::new(1);
    let text = ClientText::from("The quick brown fox jumps over the lazy dog, æøå ÆØÅ");
    let line = big.line(&text, Style::default(), None);
    let a = big.draw(&line, 2.0, [0, 0, 0, 255]);
    let line = tiny.line(&text, Style::default(), None);
    let b = tiny.draw(&line, 2.0, [0, 0, 0, 255]);
    assert_eq!(a, b);
}

/// Every glyph's ink lies within the drawn image, and the image covers the measured
/// advance box: measurement against drawn bounds.
fn check_bounds(text: &str, size: f32, weight: u16, scale: f32) {
    let mut fonts = Fonts::with_fallback(&[]);
    let mut stack = TextStack::with_fonts(Fonts::with_fallback(&[]));
    let line = stack.line(&ClientText::from(text), Style { size, weight }, None);
    let image = stack.draw(&line, scale, [0, 0, 0, 255]);
    let scale = sanitize_scale(scale);
    let (w, h) = image_size(&line, scale);
    let (ox, oy) = (image.origin.0 as i32, image.origin.1 as i32);
    assert!(
        ox as u32 + w <= image.width && oy as u32 + h <= image.height,
        "the box is inside"
    );
    let px = line.style.size * scale;
    let baseline = (line.ascent() * scale).round() as i32;
    let mut ctx = swash::scale::ScaleContext::new();
    for g in &line.glyphs {
        let font = fonts.get(usize::from(g.font)).expect("Manrope");
        let mut scaler = ctx
            .builder(font.font())
            .size(px)
            .hint(true)
            .variations(&[("wght", f32::from(line.style.weight))])
            .build();
        let Some(img) = swash::scale::Render::new(&[swash::scale::Source::Outline])
            .format(swash::zeno::Format::Alpha)
            .render(&mut scaler, g.id)
        else {
            continue;
        };
        let p = img.placement;
        if p.width == 0 || p.height == 0 {
            continue;
        }
        let x = ox + (g.x * scale).round() as i32 + p.left;
        let y = oy + baseline + (g.y * scale).round() as i32 - p.top;
        assert!(
            x >= 0
                && y >= 0
                && x + p.width as i32 <= image.width as i32
                && y + p.height as i32 <= image.height as i32,
            "{text:?} at {size} px x{scale}: glyph ink {x},{y} {}x{} outside {}x{}",
            p.width,
            p.height,
            image.width,
            image.height
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// Measurement against drawn bounds: Latin, Danish and accented text at any size,
    /// weight and scale: no ink is clipped, and the image covers the advance box.
    #[test]
    fn ink_stays_inside_the_drawn_image(
        text in "[a-zA-Z0-9æøåÆØÅéèüñçł .,;:!?()&@#/_-]{0,40}",
        size in 6.0f32..48.0,
        weight in 200u16..=800,
        scale in prop_oneof![Just(1.0f32), Just(1.25), Just(1.5), Just(2.0), 0.5f32..4.0],
    ) {
        check_bounds(&text, size, weight, scale);
    }
}
