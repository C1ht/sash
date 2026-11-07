// SPDX-License-Identifier: Apache-2.0

//! Theme tokens (owner, M16 and after M16) and the contrast rules they must keep.
//!
//! The palette is fixed: three neutral surfaces per appearance, text as white (dark) or
//! black (light) at 92 / 55 / 35% alpha, borders at 10%, and one accent per appearance.
//! Two tokens are derived (owner, after M16), each the smallest change that passes:
//! - the accent fill under text: the accent made darker (HSL lightness lowered in
//!   steps of 0.001, hue and saturation kept) until primary text on it reaches 4.5:1;
//! - light-mode secondary text: the smallest whole-percent alpha of at least 55% that
//!   reaches 4.5:1 on all three light surfaces (56%).
//!
//! Thresholds (WCAG 2.2): text 4.5:1; non-text glyphs and the focus ring 3:1. Text at
//! the disabled alpha (35%) is only for disabled and decorative items, which WCAG
//! exempts. The accent is never text: labels have no accent style.

use crate::canvas::Color;

/// Text and glyph contrast, WCAG 2.2 level AA.
pub const TEXT_CONTRAST: f64 = 4.5;
/// Non-text glyphs (icons) and the focus ring.
pub const NON_TEXT_CONTRAST: f64 = 3.0;

/// How long an animation runs (owner, M16: about 180 ms).
pub const ANIMATION_MS: u64 = 180;

/// The surfaces a widget can sit on, darkest (dark) or palest (light) first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Background,
    Surface,
    Raised,
}

pub const SURFACES: [Surface; 3] = [Surface::Background, Surface::Surface, Surface::Raised];

/// The text levels; there is no accent level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextLevel {
    Primary,
    Secondary,
    /// Disabled and decorative items only.
    Disabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub appearance: Appearance,
    pub background: Color,
    pub surface: Color,
    pub raised: Color,
    /// The colour text and glyphs are drawn in, at the alphas below.
    pub ink: Color,
    pub primary_alpha: f32,
    pub secondary_alpha: f32,
    pub disabled_alpha: f32,
    pub border_alpha: f32,
    pub accent: Color,
    /// Derived: the accent as a fill under text.
    pub accent_fill: Color,
}

/// Derived token values (checked by the tests in this module).
pub const DARK_ACCENT_FILL: u32 = 0x006CDA;
pub const LIGHT_ACCENT_FILL: u32 = 0x006ADE;
pub const LIGHT_SECONDARY_ALPHA: f32 = 0.56;

impl Theme {
    pub const fn dark() -> Self {
        Self {
            appearance: Appearance::Dark,
            background: Color::rgb(0x1C1C1E),
            surface: Color::rgb(0x2C2C2E),
            raised: Color::rgb(0x3A3A3C),
            ink: Color::rgb(0xFFFFFF),
            primary_alpha: 0.92,
            secondary_alpha: 0.55,
            disabled_alpha: 0.35,
            border_alpha: 0.10,
            accent: Color::rgb(0x0A84FF),
            accent_fill: Color::rgb(DARK_ACCENT_FILL),
        }
    }

    pub const fn light() -> Self {
        Self {
            appearance: Appearance::Light,
            background: Color::rgb(0xF2F2F7),
            surface: Color::rgb(0xFFFFFF),
            raised: Color::rgb(0xE5E5EA),
            ink: Color::rgb(0x000000),
            primary_alpha: 0.92,
            secondary_alpha: LIGHT_SECONDARY_ALPHA,
            disabled_alpha: 0.35,
            border_alpha: 0.10,
            accent: Color::rgb(0x007AFF),
            accent_fill: Color::rgb(LIGHT_ACCENT_FILL),
        }
    }

    pub fn surface_color(&self, surface: Surface) -> Color {
        match surface {
            Surface::Background => self.background,
            Surface::Surface => self.surface,
            Surface::Raised => self.raised,
        }
    }

    /// Text (and glyph) colour at `level`, straight alpha.
    pub fn text(&self, level: TextLevel) -> Color {
        self.ink.with_alpha(match level {
            TextLevel::Primary => self.primary_alpha,
            TextLevel::Secondary => self.secondary_alpha,
            TextLevel::Disabled => self.disabled_alpha,
        })
    }

    /// Text on the accent fill: always white at the primary alpha.
    pub fn on_accent(&self) -> Color {
        Color::rgb(0xFFFFFF).with_alpha(self.primary_alpha)
    }

    /// The hover fill on `surface`: the next surface up (dark) or down (light).
    pub fn hover(&self, surface: Surface) -> Color {
        match surface {
            Surface::Background => self.surface,
            Surface::Surface | Surface::Raised => self.raised,
        }
    }

    pub fn border(&self) -> Color {
        self.ink.with_alpha(self.border_alpha)
    }
}

/// WCAG relative luminance of an opaque colour.
pub fn luminance(c: Color) -> f64 {
    let lin = |v: u8| {
        let v = f64::from(v) / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

/// `fg` (straight alpha) drawn over the opaque `bg`, as the canvas draws it (8-bit
/// alpha, source over).
pub fn over(fg: Color, bg: Color) -> Color {
    let a = u32::from(fg.a);
    let mix = |f: u8, b: u8| ((u32::from(f) * a + u32::from(b) * (255 - a) + 127) / 255) as u8;
    Color {
        r: mix(fg.r, bg.r),
        g: mix(fg.g, bg.g),
        b: mix(fg.b, bg.b),
        a: 255,
    }
}

/// The contrast ratio of `fg` (drawn over `bg`) against `bg`.
pub fn contrast(fg: Color, bg: Color) -> f64 {
    let (a, b) = (luminance(over(fg, bg)), luminance(bg));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// The derivation of the accent fill: the lightest colour of the accent's hue and
/// saturation, at most its own lightness, under which `text` reaches 4.5:1.
pub fn derive_accent_fill(accent: Color, text: Color) -> Color {
    let (h, s, l) = hsl(accent);
    let mut step = (l * 1000.0).floor() as i32;
    while step > 0 {
        let fill = from_hsl(h, s, f64::from(step) / 1000.0);
        if contrast(text, fill) >= TEXT_CONTRAST {
            return fill;
        }
        step -= 1;
    }
    Color::rgb(0)
}

fn hsl(c: Color) -> (f64, f64, f64) {
    let [r, g, b] = [c.r, c.g, c.b].map(|v| f64::from(v) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

fn from_hsl(h: f64, s: f64, l: f64) -> Color {
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let channel = |mut t: f64| {
        t = t.rem_euclid(1.0);
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round() as u8
    };
    Color {
        r: channel(h + 1.0 / 3.0),
        g: channel(h),
        b: channel(h - 1.0 / 3.0),
        a: 255,
    }
}

/// A token pair with its threshold, for the contrast test and the report.
#[derive(Clone, Debug)]
pub struct Pair {
    pub what: String,
    pub ratio: f64,
    /// None: exempt (disabled or decorative).
    pub threshold: Option<f64>,
}

/// Every pair the widgets draw, in both appearances.
pub fn pairs() -> Vec<Pair> {
    let mut out = Vec::new();
    for theme in [Theme::dark(), Theme::light()] {
        let mode = match theme.appearance {
            Appearance::Dark => "dark",
            Appearance::Light => "light",
        };
        for surface in SURFACES {
            let bg = theme.surface_color(surface);
            let hex = hex_of(bg);
            for (level, name, threshold) in [
                (TextLevel::Primary, "primary text", Some(TEXT_CONTRAST)),
                (TextLevel::Secondary, "secondary text", Some(TEXT_CONTRAST)),
                (TextLevel::Disabled, "disabled text (exempt)", None),
            ] {
                out.push(Pair {
                    what: format!("{mode}: {name} on {hex}"),
                    ratio: contrast(theme.text(level), bg),
                    threshold,
                });
            }
            out.push(Pair {
                what: format!("{mode}: icon glyph (primary) on {hex}"),
                ratio: contrast(theme.text(TextLevel::Primary), bg),
                threshold: Some(NON_TEXT_CONTRAST),
            });
            out.push(Pair {
                what: format!("{mode}: focus ring {} on {hex}", hex_of(theme.accent)),
                ratio: contrast(theme.accent, bg),
                threshold: Some(NON_TEXT_CONTRAST),
            });
            // A hovered button's content sits on the hover fill.
            let hover = theme.hover(surface);
            out.push(Pair {
                what: format!(
                    "{mode}: primary text on the hover fill {} (over {hex})",
                    hex_of(hover)
                ),
                ratio: contrast(theme.text(TextLevel::Primary), hover),
                threshold: Some(TEXT_CONTRAST),
            });
        }
        out.push(Pair {
            what: format!(
                "{mode}: text on the accent fill {}",
                hex_of(theme.accent_fill)
            ),
            ratio: contrast(theme.on_accent(), theme.accent_fill),
            threshold: Some(TEXT_CONTRAST),
        });
    }
    out
}

pub fn hex_of(c: Color) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_token_pair_meets_its_threshold() {
        let mut failed = Vec::new();
        for pair in pairs() {
            println!(
                "{:<70} {:>6.2}  {}",
                pair.what,
                pair.ratio,
                pair.threshold
                    .map_or("exempt".to_string(), |t| format!(">= {t}"))
            );
            if pair.threshold.is_some_and(|t| pair.ratio < t) {
                failed.push(pair.what);
            }
        }
        assert!(failed.is_empty(), "under the threshold: {failed:?}");
    }

    #[test]
    fn the_derived_accent_fills_are_the_lightest_that_pass() {
        for theme in [Theme::dark(), Theme::light()] {
            let derived = derive_accent_fill(theme.accent, theme.on_accent());
            assert_eq!(derived, theme.accent_fill, "{}", hex_of(derived));
            // Same hue and saturation, darker.
            let (h0, s0, l0) = hsl(theme.accent);
            let (h1, s1, l1) = hsl(derived);
            assert!((h0 - h1).abs() < 0.01 && (s0 - s1).abs() < 0.01 && l1 < l0);
            // One step lighter fails.
            let lighter = from_hsl(h0, s0, l1 + 0.002);
            assert!(contrast(theme.on_accent(), lighter) < TEXT_CONTRAST);
        }
    }

    #[test]
    fn light_secondary_alpha_is_the_smallest_passing_percent() {
        let theme = Theme::light();
        let passes = |alpha: f32| {
            SURFACES.iter().all(|&s| {
                contrast(theme.ink.with_alpha(alpha), theme.surface_color(s)) >= TEXT_CONTRAST
            })
        };
        assert!(!passes(0.55), "55% would pass; no change needed");
        let smallest = (55..=60)
            .map(|p| p as f32 / 100.0)
            .find(|&a| passes(a))
            .expect("a change of up to 5 points passes");
        assert_eq!(smallest, LIGHT_SECONDARY_ALPHA);
    }

    #[test]
    fn the_accent_alone_is_never_text_on_dark_surfaces() {
        // The rule holds by construction (TextLevel has no accent); this records why:
        // the accent as 13 px text fails 4.5:1 on dark surfaces.
        let theme = Theme::dark();
        assert!(
            SURFACES
                .iter()
                .any(|&s| contrast(theme.accent, theme.surface_color(s)) < TEXT_CONTRAST)
        );
    }
}
