// SPDX-License-Identifier: Apache-2.0

//! Golden images, with their own runner (harness = false): Manrope-only strings run
//! everywhere; the fallback ones (CJK, emoji) need the Noto fonts and run in the
//! debian:trixie container (tools/text-goldens.sh). A case whose fonts are missing is
//! SKIPPED: the runner lists every skipped case with its reason and counts it apart
//! from the passed ones, never as passed (owner, after M14). A failing case fails the
//! run.

mod common;

use common::{golden, missing_fonts, render};
use sash_text::{Style, TextStack};

const LATIN: &str = "Settings - Displays & Monitors (5120x1440)";
const DANISH: &str = "Ærø Ø Å æblegrød på øen, Hvidovre Ålborg";
const ACCENTS: &str = "Café naïve façade Zürich São Paulo Łódź";

fn latin_danish_and_accents_at_three_scales() -> Outcome {
    let mut stack = TextStack::new();
    for (name, text) in [("latin", LATIN), ("danish", DANISH), ("accents", ACCENTS)] {
        for (scale, tag) in [(1.0, "1"), (1.5, "1.5"), (2.0, "2")] {
            let image = render(&mut stack, text, Style::default(), scale);
            assert!(image.pixels.iter().any(|b| *b != 0), "{name} drew nothing");
            golden(&format!("{name}-x{tag}"), &image);
        }
    }
    Outcome::Passed
}

/// Weights 400 and 500 are instances of the variable font's weight axis (one file):
/// both render, and differently.
fn weights_400_and_500_both_render() -> Outcome {
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
    Outcome::Passed
}

fn cjk_falls_back_to_noto_sans_cjk() -> Outcome {
    if let Some(missing) = missing_fonts(&["Noto Sans CJK"]) {
        return Outcome::Skipped(missing);
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
    Outcome::Passed
}

fn emoji_fall_back_to_noto_color_emoji_in_colour() -> Outcome {
    if let Some(missing) = missing_fonts(&["Noto Color Emoji"]) {
        return Outcome::Skipped(missing);
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
    Outcome::Passed
}

/// The two emoji that came out monochrome (DejaVu Sans) in the measurement come out
/// coloured: their pixels are not grey.
fn smile_and_heart_are_coloured() -> Outcome {
    if let Some(missing) = missing_fonts(&["Noto Color Emoji", "DejaVu Sans"]) {
        return Outcome::Skipped(missing);
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
    Outcome::Passed
}

/// A report, not pass or fail: what Arabic, Devanagari and Hebrew (alone, with
/// digits, mixed with Latin) look like. Run it in the
/// trixie container with --nocapture; the images go to SASH_TEXT_REPORT (a directory).
fn report_arabic_and_devanagari() -> Outcome {
    if let Some(missing) = missing_fonts(&["Noto Sans Arabic", "Noto Sans Devanagari"]) {
        return Outcome::Skipped(missing);
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
    Outcome::Passed
}

/// Found by the property test with the trixie fonts: Noto Color Emoji deletes a glyph
/// with a multiple substitution, where swash 0.2.10 computes `0 - 1` (correct when it
/// wraps, a panic with overflow checks; the workspace turns them off for swash).
fn a_regional_indicator_pair_and_a_joiner() -> Outcome {
    if let Some(missing) = missing_fonts(&["Noto Color Emoji"]) {
        return Outcome::Skipped(missing);
    }
    let mut stack = TextStack::new();
    for scale in [0.25, 1.0, 2.0] {
        for (text, max) in [("aa🇩🇩\u{200d}", None), ("🇩🇰\u{200d}🇩🇰\u{200d}", Some(10.0))]
        {
            let line = stack.line(&text.into(), Style::default(), max);
            stack.draw(&line, scale, common::INK);
        }
    }
    Outcome::Passed
}

/// How a case ended (a failure is a panic).
enum Outcome {
    Passed,
    Skipped(String),
}

fn main() {
    let cases: [(&str, fn() -> Outcome); 7] = [
        (
            "latin_danish_and_accents_at_three_scales",
            latin_danish_and_accents_at_three_scales,
        ),
        (
            "weights_400_and_500_both_render",
            weights_400_and_500_both_render,
        ),
        (
            "cjk_falls_back_to_noto_sans_cjk",
            cjk_falls_back_to_noto_sans_cjk,
        ),
        (
            "emoji_fall_back_to_noto_color_emoji_in_colour",
            emoji_fall_back_to_noto_color_emoji_in_colour,
        ),
        ("smile_and_heart_are_coloured", smile_and_heart_are_coloured),
        ("report_arabic_and_devanagari", report_arabic_and_devanagari),
        (
            "a_regional_indicator_pair_and_a_joiner",
            a_regional_indicator_pair_and_a_joiner,
        ),
    ];
    let filter: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    let (mut passed, mut failed) = (Vec::new(), Vec::new());
    let mut skipped = Vec::new();
    for (name, case) in cases {
        if !filter.is_empty() && !filter.iter().any(|f| name.contains(f.as_str())) {
            continue;
        }
        match std::panic::catch_unwind(case) {
            Ok(Outcome::Passed) => {
                println!("golden {name} ... ok");
                passed.push(name);
            }
            Ok(Outcome::Skipped(reason)) => {
                println!("golden {name} ... SKIPPED ({reason})");
                skipped.push((name, reason));
            }
            Err(_) => {
                println!("golden {name} ... FAILED");
                failed.push(name);
            }
        }
    }
    println!(
        "golden result: {} passed, {} skipped, {} failed",
        passed.len(),
        skipped.len(),
        failed.len()
    );
    for (name, reason) in &skipped {
        println!("  skipped: {name}: {reason}");
    }
    if !failed.is_empty() {
        std::process::exit(1);
    }
}
