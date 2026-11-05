// SPDX-License-Identifier: Apache-2.0

//! Golden images. Manrope-only strings run everywhere; the fallback ones (CJK, emoji)
//! need the Noto fonts and run in the debian:trixie container (tools/text-goldens.sh),
//! saying "skipped: font not installed" elsewhere.

mod common;

use common::{fonts_installed, golden, render};
use sash_text::{Style, TextStack};

const LATIN: &str = "Settings - Displays & Monitors (5120x1440)";
const DANISH: &str = "Ærø Ø Å æblegrød på øen, Hvidovre Ålborg";
const ACCENTS: &str = "Café naïve façade Zürich São Paulo Łódź";

#[test]
fn latin_danish_and_accents_at_three_scales() {
    let mut stack = TextStack::new();
    for (name, text) in [("latin", LATIN), ("danish", DANISH), ("accents", ACCENTS)] {
        for (scale, tag) in [(1.0, "1"), (1.5, "1.5"), (2.0, "2")] {
            let image = render(&mut stack, text, Style::default(), scale);
            assert!(image.pixels.iter().any(|b| *b != 0), "{name} drew nothing");
            golden(&format!("{name}-x{tag}"), &image);
        }
    }
}

/// Weights 400 and 500 are instances of the variable font's weight axis (one file):
/// both render, and differently.
#[test]
fn weights_400_and_500_both_render() {
    let mut stack = TextStack::new();
    let regular = render(
        &mut stack,
        LATIN,
        Style {
            size: 13.0,
            weight: 400,
        },
        1.0,
    );
    let medium = render(
        &mut stack,
        LATIN,
        Style {
            size: 13.0,
            weight: 500,
        },
        1.0,
    );
    let ink = |i: &sash_text::Image| i.pixels.chunks(4).map(|p| u64::from(p[3])).sum::<u64>();
    assert!(
        ink(&regular) > 0 && ink(&medium) > ink(&regular),
        "500 is darker than 400"
    );
    golden("latin-w400", &regular);
    golden("latin-w500", &medium);
}

#[test]
fn cjk_falls_back_to_noto_sans_cjk() {
    if !fonts_installed("cjk_falls_back_to_noto_sans_cjk", &["Noto Sans CJK"]) {
        return;
    }
    let mut stack = TextStack::new();
    let text = "設定 日本語のテキスト 中文标题 한국어 제목";
    let line = stack.line(&text.into(), Style::default(), None);
    assert!(
        line.fonts_used().contains(&"Noto Sans CJK"),
        "{:?}",
        line.fonts_used()
    );
    golden("cjk", &render(&mut stack, text, Style::default(), 1.0));
}

#[test]
fn emoji_fall_back_to_noto_color_emoji_in_colour() {
    if !fonts_installed(
        "emoji_fall_back_to_noto_color_emoji_in_colour",
        &["Noto Color Emoji"],
    ) {
        return;
    }
    let mut stack = TextStack::new();
    let text = "Party 🎉 smile 😀 heart ❤ flag 🇩🇰";
    let line = stack.line(&text.into(), Style::default(), None);
    assert!(line.fonts_used().contains(&"Noto Color Emoji"));
    assert!(
        !line.fonts_used().contains(&"DejaVu Sans"),
        "{:?}",
        line.fonts_used()
    );
    golden("emoji", &render(&mut stack, text, Style::default(), 1.0));
}

/// The two emoji that came out monochrome (DejaVu Sans) in the measurement come out
/// coloured: their pixels are not grey.
#[test]
fn smile_and_heart_are_coloured() {
    if !fonts_installed(
        "smile_and_heart_are_coloured",
        &["Noto Color Emoji", "DejaVu Sans"],
    ) {
        return;
    }
    let mut stack = TextStack::new();
    for emoji in ["😀", "❤"] {
        let image = render(&mut stack, emoji, Style::default(), 2.0);
        let coloured = (0..image.height)
            .flat_map(|y| (0..image.width).map(move |x| (x, y)))
            .map(|(x, y)| image.argb(x, y))
            .filter(|(a, r, g, b)| *a > 128 && (r.abs_diff(*g) > 40 || g.abs_diff(*b) > 40))
            .count();
        assert!(coloured > 10, "{emoji}: {coloured} coloured pixels");
    }
}

/// A report, not pass or fail: what Arabic, Devanagari and Hebrew (alone, with
/// digits, mixed with Latin) look like. Run it in the
/// trixie container with --nocapture; the images go to SASH_TEXT_REPORT (a directory).
#[test]
fn report_arabic_and_devanagari() {
    if !fonts_installed(
        "report_arabic_and_devanagari",
        &["Noto Sans Arabic", "Noto Sans Devanagari"],
    ) {
        return;
    }
    let mut stack = TextStack::new();
    for (name, text) in [
        ("arabic", "مرحبا بالعالم"),
        ("devanagari", "नमस्ते दुनिया, हिन्दी"),
        ("hebrew", "שלום עולם"),
        ("hebrew-digits", "שלום 123 עולם"),
        ("mixed", "Files - קובץ.txt"),
    ] {
        let line = stack.line(&text.into(), Style::default(), None);
        eprintln!(
            "report {name}: {} glyphs, width {:.1} px, fonts {:?}",
            line.glyph_count(),
            line.width(),
            line.fonts_used()
        );
        if let Some(dir) = std::env::var_os("SASH_TEXT_REPORT") {
            let image = stack.draw(&line, 3.0, common::INK);
            common::write_png(
                &std::path::Path::new(&dir).join(format!("{name}.png")),
                &image,
            );
        }
    }
}
