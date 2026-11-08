// SPDX-License-Identifier: Apache-2.0

//! The theme as sash-ui draws it (owner, M18b): every colour comes from sash-theme, the
//! tokens of docs/design.md. This module only says which token a widget uses.
//!
//! - Surfaces: page, raised and chrome (the spec's card is dark-only and unused).
//! - Text: the text and secondary tokens. There is no 35% level: a disabled control is
//!   drawn with the secondary token and a state shape.
//! - Hover is the overlay at 5%, pressed and selected the overlay at 8%.
//! - The accent fill is the accent; text on it is the spec's "text on accent fill".
//! - The focus ring: 2 px of the accent, 3 px off the widget.
//!
//! Thresholds (WCAG 2.2): text 4.5:1; non-text glyphs and the focus ring 3:1. A pair
//! under its threshold goes to the owner; a token is never changed to pass.

pub use sash_theme::{
    FOCUS_RING_OFFSET, FOCUS_RING_WIDTH, NON_TEXT_CONTRAST, Overlay, TEXT_CONTRAST,
};

use crate::canvas::Color;

/// How long an animation runs (owner, M16: about 180 ms).
pub const ANIMATION_MS: u64 = 180;

/// The surfaces a widget can sit on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Page,
    Raised,
    Chrome,
}

pub const SURFACES: [Surface; 3] = [Surface::Page, Surface::Raised, Surface::Chrome];

/// The text levels; there is no accent level and no disabled level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextLevel {
    Primary,
    Secondary,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub tokens: &'static sash_theme::Theme,
}

/// A token as the canvas's colour (straight alpha, 8 bits).
pub fn color(c: sash_theme::Color) -> Color {
    let [r, g, b, a] = c.rgba8();
    Color { r, g, b, a }
}

impl Theme {
    pub const fn dark() -> Self {
        Self {
            tokens: &sash_theme::Theme::DARK,
        }
    }

    pub const fn light() -> Self {
        Self {
            tokens: &sash_theme::Theme::LIGHT,
        }
    }

    pub fn of(mode: sash_theme::Mode) -> Self {
        Self {
            tokens: sash_theme::Theme::of(mode),
        }
    }

    fn surface_token(&self, surface: Surface) -> sash_theme::Color {
        match surface {
            Surface::Page => self.tokens.page,
            Surface::Raised => self.tokens.raised,
            Surface::Chrome => self.tokens.chrome,
        }
    }

    pub fn surface_color(&self, surface: Surface) -> Color {
        color(self.surface_token(surface))
    }

    /// Text (and glyph) colour at `level`, straight alpha.
    pub fn text(&self, level: TextLevel) -> Color {
        color(self.text_token(level))
    }

    pub fn text_token(&self, level: TextLevel) -> sash_theme::Color {
        match level {
            TextLevel::Primary => self.tokens.text,
            TextLevel::Secondary => self.tokens.secondary,
        }
    }

    pub fn accent(&self) -> Color {
        color(self.tokens.accent)
    }

    /// The accent fill under text: the accent itself.
    pub fn accent_fill(&self) -> Color {
        color(self.tokens.accent)
    }

    /// Text on the accent fill.
    pub fn on_accent(&self) -> Color {
        color(self.tokens.on_accent)
    }

    /// The fill of a hovered (5%) or pressed and selected (8%) control.
    pub fn overlay(&self, level: Overlay) -> Color {
        color(self.tokens.overlay(level))
    }

    /// Lines: a hovered flat button's edge.
    pub fn hairline(&self) -> Color {
        color(self.tokens.hairline)
    }
}

/// The contrast of the canvas colour `fg` (drawn over the opaque `bg`, 8-bit alpha)
/// against `bg`.
pub fn contrast(fg: Color, bg: Color) -> f64 {
    let a = u32::from(fg.a);
    let mix = |f: u8, b: u8| (u32::from(f) * a + u32::from(b) * (255 - a) + 127) / 255;
    let hex = |r: u32, g: u32, b: u32| sash_theme::Color::hex(r << 16 | g << 8 | b);
    let top = hex(mix(fg.r, bg.r), mix(fg.g, bg.g), mix(fg.b, bg.b));
    sash_theme::contrast(top, hex(bg.r.into(), bg.g.into(), bg.b.into()))
}

/// A token pair with its threshold, for the contrast test and the report.
#[derive(Clone, Debug)]
pub struct Pair {
    pub what: String,
    pub ratio: f64,
    pub threshold: f64,
}

/// Every pair sash-ui's widgets draw that sash-theme's own list does not hold: text and
/// glyphs on the hover and pressed fills over each surface (a disabled control never
/// shows them, so secondary text is never on them); the focus ring and an accent
/// button's outline on each surface.
pub fn widget_pairs() -> Vec<Pair> {
    use sash_theme::{contrast, over};
    let mut out = Vec::new();
    for theme in [Theme::dark(), Theme::light()] {
        let t = theme.tokens;
        let mode = match t.mode {
            sash_theme::Mode::Dark => "dark",
            sash_theme::Mode::Light => "light",
        };
        for surface in SURFACES {
            let bg = theme.surface_token(surface);
            let name = format!("{surface:?}").to_lowercase();
            for (fill, state) in [(Overlay::Hover, "hover"), (Overlay::Pressed, "pressed")] {
                let under = over(t.overlay(fill), bg);
                for (level, what, threshold) in [
                    (TextLevel::Primary, "text", TEXT_CONTRAST),
                    (TextLevel::Primary, "glyph", NON_TEXT_CONTRAST),
                ] {
                    let fg = theme.text_token(level);
                    out.push(Pair {
                        what: format!(
                            "{mode}: {what} on the {state} fill over {name} ({})",
                            under.hex_string()
                        ),
                        ratio: contrast(fg, under),
                        threshold,
                    });
                }
            }
            out.push(Pair {
                what: format!(
                    "{mode}: focus ring and accent outline {} on {name} {}",
                    t.accent.hex_string(),
                    bg.hex_string()
                ),
                ratio: contrast(t.accent, bg),
                threshold: NON_TEXT_CONTRAST,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every pair the spec uses (sash-theme's list) and every pair the widgets add:
    /// none under its threshold. Printed: the report.
    #[test]
    fn every_token_pair_meets_its_threshold() {
        let mut failed = Vec::new();
        let spec = sash_theme::pairs().into_iter().map(|p| Pair {
            what: p.what,
            ratio: p.ratio,
            threshold: p.threshold,
        });
        for pair in spec.chain(widget_pairs()) {
            println!(
                "{:<92} {:>6.2}  >= {}",
                pair.what, pair.ratio, pair.threshold
            );
            if pair.ratio < pair.threshold {
                failed.push(pair.what);
            }
        }
        assert!(failed.is_empty(), "under the threshold: {failed:?}");
    }

    /// The old derived tokens are gone (owner, M18b): the accent fill is the accent and
    /// the secondary token is the spec's.
    #[test]
    fn no_derived_tokens() {
        for theme in [Theme::dark(), Theme::light()] {
            assert_eq!(theme.accent_fill(), theme.accent());
            assert_eq!(theme.on_accent(), color(theme.tokens.on_accent));
        }
        assert_eq!(
            Theme::light().text(TextLevel::Secondary),
            color(sash_theme::Color::hex(0x5A636A))
        );
        assert_eq!(
            Theme::dark().on_accent(),
            color(sash_theme::Color::hex(0x14181B))
        );
        assert_eq!(
            Theme::light().on_accent(),
            color(sash_theme::Color::hex(0xFFFFFF))
        );
    }
}
