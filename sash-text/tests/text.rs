// SPDX-License-Identifier: Apache-2.0

//! The public API on untrusted input: truncation, ellipsis, caps, zero-width and
//! combining characters, invalid and overlong input, determinism, the fallback order.

mod common;

use proptest::prelude::*;
use sash_text::{ClientText, MAX_GLYPHS, MAX_TEXT_BYTES, Style, TextStack, fonts::FALLBACK};

/// `ClientText` implements neither Debug nor Display (it must never be logged). The
/// probe resolves to the trait-bound method only for types that implement it.
#[test]
fn client_text_has_no_debug_or_display() {
    struct Probe<T>(std::marker::PhantomData<T>);
    trait Fallback {
        fn debug(&self) -> bool {
            false
        }
        fn display(&self) -> bool {
            false
        }
    }
    impl<T> Fallback for &Probe<T> {}
    trait HasDebug {
        fn debug(&self) -> bool {
            true
        }
    }
    impl<T: std::fmt::Debug> HasDebug for Probe<T> {}
    trait HasDisplay {
        fn display(&self) -> bool {
            true
        }
    }
    impl<T: std::fmt::Display> HasDisplay for Probe<T> {}
    macro_rules! check {
        ($t:ty) => {{
            let probe = Probe::<$t>(std::marker::PhantomData);
            ((&probe).debug(), (&probe).display())
        }};
    }
    assert_eq!(check!(String), (true, true), "the probe works");
    assert_eq!(check!(ClientText), (false, false));
}

#[test]
fn the_fallback_order_is_fixed_with_colour_emoji_before_dejavu() {
    let names: Vec<&str> = FALLBACK.iter().map(|(n, _, _)| *n).collect();
    assert_eq!(
        names,
        [
            "Noto Sans",
            "Noto Sans Hebrew",
            "Noto Sans Arabic",
            "Noto Sans Devanagari",
            "Noto Sans CJK",
            "Noto Color Emoji",
            "DejaVu Sans",
        ]
    );
    assert!(FALLBACK.len() <= sash_text::fonts::MAX_FALLBACK);
    for (_, path, _) in FALLBACK {
        assert!(path.starts_with("/usr/share/fonts/"), "{path}");
    }
}

const SAMPLES: [&str; 5] = [
    "Settings - Displays & Monitors",
    "Ærø Ø Å æblegrød på øen",
    "e\u{301}a\u{30a}o\u{308} combining",
    "設定 日本語 🎉😀❤🇩🇰👍🏽",
    "zero\u{200b}width\u{200d}joiner\u{feff}bom",
];

/// Cutting the input at every byte position: valid text, never longer than the input,
/// a line and a drawing without panics.
#[test]
fn truncation_at_every_byte_position() {
    let mut stack = TextStack::new();
    for sample in SAMPLES {
        let bytes = sample.as_bytes();
        for cut in 0..=bytes.len() {
            let text = ClientText::new(&bytes[..cut]);
            assert!(
                text.len() <= cut + 3,
                "{cut}: U+FFFD for a split character at most"
            );
            let line = stack.line(&text, Style::default(), None);
            let image = stack.draw(&line, 1.5, common::INK);
            assert_eq!(image.pixels.len(), (image.stride * image.height) as usize);
        }
    }
}

/// The ellipsis at every width: the line always fits, is cut exactly when it has to be,
/// and gets no shorter as the room grows.
#[test]
fn the_ellipsis_fits_every_width() {
    let mut stack = TextStack::new();
    for sample in SAMPLES {
        let text = ClientText::from(sample);
        let full = stack.line(&text, Style::default(), None);
        assert!(!full.truncated());
        let mut previous = 0.0;
        for max in 0..=(full.width().ceil() as u32 + 2) {
            let max = max as f32;
            let line = stack.line(&text, Style::default(), Some(max));
            assert!(
                line.width() <= max + 0.01,
                "{sample:?}: {} > {max}",
                line.width()
            );
            assert_eq!(line.truncated(), full.width() > max, "{sample:?} at {max}");
            assert!(
                line.width() + 0.01 >= previous || !line.truncated() || line.glyph_count() == 0
            );
            previous = line.width();
        }
    }
}

#[test]
fn zero_width_and_combining_characters() {
    let mut stack = TextStack::new();
    let style = Style::default();
    let width = |stack: &mut TextStack, s: &str| stack.line(&s.into(), style, None).width();
    // A zero-width space, joiner, non-joiner and BOM add no width and draw nothing
    // (at the end, so no kerning pair is broken: between "o" and "w" a zero-width
    // space rightly stops their kerning).
    let plain = width(&mut stack, "zero width");
    for invisible in ["\u{200b}", "\u{200c}", "\u{200d}", "\u{feff}", "\u{2060}"] {
        let joined = format!("zero width{invisible}");
        assert!(
            (width(&mut stack, &joined) - plain).abs() < 0.01,
            "{joined:?}"
        );
        let line = stack.line(&joined.as_str().into(), style, None);
        let alone = stack.line(&"zero width".into(), style, None);
        assert_eq!(
            stack.draw(&line, 1.0, common::INK),
            stack.draw(&alone, 1.0, common::INK),
            "{joined:?} draws something"
        );
    }
    // A combining accent composes: "e" + U+0301 is as wide as "é".
    assert!((width(&mut stack, "e\u{301}") - width(&mut stack, "é")).abs() < 0.01);
    // A pile of 300 combining marks: no panic, a bounded line.
    let pile = format!("a{}", "\u{301}".repeat(300));
    let line = stack.line(&pile.as_str().into(), style, None);
    assert!(line.glyph_count() <= MAX_GLYPHS + 3);
    stack.draw(&line, 2.0, common::INK);
}

#[test]
fn caps_on_bytes_and_glyphs_end_with_an_ellipsis() {
    let mut stack = TextStack::new();
    let long = "x".repeat(10 * 1024 * 1024);
    let text = ClientText::new(long.as_bytes());
    assert_eq!(text.len(), MAX_TEXT_BYTES);
    assert!(text.was_cut());
    let line = stack.line(&text, Style::default(), None);
    assert!(line.truncated());
    assert!(
        line.glyph_count() <= MAX_GLYPHS + 3,
        "{}",
        line.glyph_count()
    );
    let fits = ClientText::from("x".repeat(200).as_str());
    assert!(!stack.line(&fits, Style::default(), None).truncated());
}

#[test]
fn controls_and_bidi_overrides_are_removed() {
    let text = ClientText::new(b"a\tb\nc\x1b[31m\x7f\xc2\x85d");
    let mut stack = TextStack::new();
    let spaced = stack.line(&text, Style::default(), None).glyph_count();
    assert_eq!(
        spaced,
        stack
            .line(&"a b c [31m  d".into(), Style::default(), None)
            .glyph_count()
    );
    let overridden = ClientText::from("\u{202e}abc\u{202c}\u{2067}d\u{2069}");
    assert_eq!(overridden.len(), "abcd".len());
}

#[test]
fn drawing_is_deterministic() {
    let mut a = TextStack::new();
    let mut b = TextStack::new();
    for sample in SAMPLES {
        for scale in [1.0, 1.5, 2.0] {
            let one = common::render(&mut a, sample, Style::default(), scale);
            let again = common::render(&mut a, sample, Style::default(), scale);
            let other = common::render(&mut b, sample, Style::default(), scale);
            assert_eq!(one, again, "{sample:?} twice");
            assert_eq!(one, other, "{sample:?} in two stacks");
        }
    }
}

#[test]
fn odd_sizes_and_scales_are_clamped() {
    let mut stack = TextStack::new();
    for size in [f32::NAN, f32::INFINITY, -5.0, 0.0, 1e9] {
        for scale in [f32::NAN, f32::NEG_INFINITY, 0.0, -1.0, 1e9] {
            let line = stack.line(&"Ab".into(), Style { size, weight: 9999 }, Some(f32::NAN));
            let image = stack.draw(&line, scale, common::INK);
            assert!(image.width <= 4096 && image.height <= 4096);
        }
    }
}

fn arbitrary_text() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        prop::collection::vec(any::<u8>(), 0..3000),
        "\\PC{0,400}".prop_map(String::into_bytes),
        prop::collection::vec(
            prop::sample::select(vec![
                "a", "é", "\u{301}", "\u{200d}", "😀", "🇩", "🇰", "設", "ש", "م", "न", "\u{94d}",
                " ", "\u{fe0f}"
            ]),
            0..400
        )
        .prop_map(|parts| parts.concat().into_bytes()),
    ]
}

fn exercise(stack: &mut TextStack, bytes: &[u8], size: f32, scale: f32, max: Option<f32>) {
    let text = ClientText::new(bytes);
    assert!(text.len() <= MAX_TEXT_BYTES);
    let line = stack.line(&text, Style { size, weight: 500 }, max);
    assert!(line.glyph_count() <= MAX_GLYPHS + 3);
    if let Some(max) = max {
        assert!(line.width() <= max + 0.01);
    }
    let image = stack.draw(&line, scale, common::INK);
    assert_eq!(image.pixels.len(), (image.stride * image.height) as usize);
    assert!(stack.cache_bytes() <= sash_text::CACHE_BYTES);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Invalid, random and overlong input never panics.
    #[test]
    fn any_input_is_safe(
        bytes in arbitrary_text(),
        size in 1.0f32..64.0,
        scale in 0.25f32..4.0,
        max in prop::option::of(0.0f32..2000.0),
    ) {
        let mut stack = TextStack::new();
        exercise(&mut stack, &bytes, size, scale, max);
    }
}

/// The long run (owner, after M14 step 1): the same as `any_input_is_safe` with one
/// stack for the whole run (so the cache fills and evicts), for
/// SASH_TEXT_LONG_SECONDS (default 600) seconds. Run with --ignored, in release.
#[test]
#[ignore = "long-running: tools/text-goldens.sh --long"]
fn long_run_any_input_is_safe() {
    use proptest::{
        strategy::ValueTree,
        test_runner::{Config, TestRunner},
    };
    let seconds: u64 = std::env::var("SASH_TEXT_LONG_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let mut stack = TextStack::new();
    let mut runner = TestRunner::new(Config {
        cases: u32::MAX,
        failure_persistence: None,
        ..Config::default()
    });
    let strategy = (
        arbitrary_text(),
        1.0f32..64.0,
        0.25f32..4.0,
        prop::option::of(0.0f32..2000.0),
    );
    let mut cases = 0u64;
    while std::time::Instant::now() < deadline {
        let value = strategy.new_tree(&mut runner).expect("value").current();
        exercise(&mut stack, &value.0, value.1, value.2, value.3);
        cases += 1;
    }
    eprintln!(
        "long run: {cases} cases in {seconds} s; cache {} bytes",
        stack.cache_bytes()
    );
}

/// Found by `any_input_is_safe` with the trixie fonts: Noto Color Emoji deletes a glyph
/// with a multiple substitution, where swash 0.2.10 computes `0 - 1` (correct when it
/// wraps, a panic with overflow checks; the workspace turns them off for swash).
#[test]
fn a_regional_indicator_pair_and_a_joiner() {
    if !common::fonts_installed(
        "a_regional_indicator_pair_and_a_joiner",
        &["Noto Color Emoji"],
    ) {
        return;
    }
    let mut stack = TextStack::new();
    for scale in [0.25, 1.0, 2.0] {
        exercise(&mut stack, "aa🇩🇩\u{200d}".as_bytes(), 1.0, scale, None);
        exercise(
            &mut stack,
            "🇩🇰\u{200d}🇩🇰\u{200d}".as_bytes(),
            13.0,
            scale,
            Some(10.0),
        );
    }
}
