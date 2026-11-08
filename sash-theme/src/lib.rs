// SPDX-License-Identifier: Apache-2.0

//! The theme (owner, M18b): every colour sash-wm and sash-ui draw, exactly the tokens of
//! docs/design.md, dark and light, chosen by the appearance setting; the focus ring's
//! metrics; and the contrast maths the tests check them with.
//!
//! A test reads docs/design.md and compares its colour table with these tokens, item by
//! item: a token that differs from the spec fails it.

/// A colour of the spec: an sRGB value and an alpha in whole percent (the spec gives
/// alphas as percentages: "white 92%").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color {
    pub rgb: u32,
    /// 0 to 100.
    pub alpha: u8,
}

impl Color {
    /// An opaque colour.
    pub const fn hex(rgb: u32) -> Self {
        Self { rgb, alpha: 100 }
    }

    /// The same colour at `percent` alpha.
    pub const fn at(self, percent: u8) -> Self {
        Self {
            rgb: self.rgb,
            alpha: percent,
        }
    }

    pub const fn r(self) -> u8 {
        (self.rgb >> 16) as u8
    }

    pub const fn g(self) -> u8 {
        (self.rgb >> 8) as u8
    }

    pub const fn b(self) -> u8 {
        self.rgb as u8
    }

    /// The alpha in 0..=255, rounded.
    pub const fn a8(self) -> u8 {
        ((self.alpha as u32 * 255 + 50) / 100) as u8
    }

    /// Straight RGBA bytes.
    pub const fn rgba8(self) -> [u8; 4] {
        [self.r(), self.g(), self.b(), self.a8()]
    }

    /// Straight RGBA in 0..1 (the alpha exactly the percentage).
    pub fn rgba_f32(self) -> [f32; 4] {
        [
            f32::from(self.r()) / 255.0,
            f32::from(self.g()) / 255.0,
            f32::from(self.b()) / 255.0,
            f32::from(self.alpha) / 100.0,
        ]
    }

    pub fn hex_string(self) -> String {
        format!("#{:06X}", self.rgb)
    }
}

/// The spec's white and black (text, overlays, outlines, the inactive dim).
pub const WHITE: Color = Color::hex(0xFFFFFF);
pub const BLACK: Color = Color::hex(0x000000);

/// The appearance setting (closed settings list: dark / light / auto).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    #[default]
    Automatic,
    Light,
    Dark,
}

/// What an appearance resolves to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Dark,
    Light,
}

impl Appearance {
    /// Automatic has no source yet (a known limit until the settings protocol): dark.
    pub const fn mode(self) -> Mode {
        match self {
            Appearance::Light => Mode::Light,
            Appearance::Dark | Appearance::Automatic => Mode::Dark,
        }
    }
}

/// The accent presets. Teal is the default; blue stays a preset (owner, M18b).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Accent {
    #[default]
    Teal,
    Blue,
}

impl Accent {
    pub const fn color(self, mode: Mode) -> Color {
        match (self, mode) {
            (Accent::Teal, Mode::Dark) => Color::hex(0x5EC4BE),
            (Accent::Teal, Mode::Light) => Color::hex(0x0B6E6E),
            (Accent::Blue, Mode::Dark) => Color::hex(0x0A84FF),
            (Accent::Blue, Mode::Light) => Color::hex(0x007AFF),
        }
    }
}

/// The tokens of one appearance (docs/design.md, "Colours").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub mode: Mode,
    /// A focused window's bar, and chrome.
    pub chrome: Color,
    pub unfocused_bar: Color,
    pub page: Color,
    pub raised: Color,
    /// Dark only.
    pub card: Option<Color>,
    pub hairline: Color,
    pub text: Color,
    pub secondary: Color,
    pub accent: Color,
    pub accent_tile: Color,
    /// Text on an accent fill (the accent itself is the fill).
    pub on_accent: Color,
    pub close_hover: Color,
    /// The overlays at 5, 8 and 12%.
    pub overlays: [Color; 3],
    pub outline: Color,
    pub outline_focused: Color,
    /// The inactive dim over an unfocused window's client area.
    pub dim: Color,
}

impl Theme {
    pub const DARK: Theme = Theme {
        mode: Mode::Dark,
        chrome: Color::hex(0x15191C),
        unfocused_bar: Color::hex(0x1B2024),
        page: Color::hex(0x1D2226),
        raised: Color::hex(0x22282C),
        card: Some(Color::hex(0x23292E)),
        hairline: Color::hex(0x2B3238),
        text: WHITE.at(92),
        secondary: WHITE.at(62),
        accent: Accent::Teal.color(Mode::Dark),
        accent_tile: Color::hex(0x163331),
        on_accent: Color::hex(0x14181B),
        close_hover: Color::hex(0xFF453A),
        overlays: [WHITE.at(5), WHITE.at(8), WHITE.at(12)],
        outline: WHITE.at(8),
        outline_focused: WHITE.at(14),
        dim: BLACK.at(14),
    };

    pub const LIGHT: Theme = Theme {
        mode: Mode::Light,
        chrome: Color::hex(0xE5E9EC),
        unfocused_bar: Color::hex(0xEEF1F3),
        page: Color::hex(0xFFFFFF),
        raised: Color::hex(0xF3F5F7),
        card: None,
        hairline: Color::hex(0xDCE0E5),
        text: Color::hex(0x1B2126),
        secondary: Color::hex(0x5A636A),
        accent: Accent::Teal.color(Mode::Light),
        accent_tile: Color::hex(0xDCEFEE),
        on_accent: Color::hex(0xFFFFFF),
        // The spec lists the close hover once (in the dark table); the brief gives it
        // for both appearances.
        close_hover: Color::hex(0xFF453A),
        overlays: [
            Color::hex(0x1B2126).at(5),
            Color::hex(0x1B2126).at(8),
            Color::hex(0x1B2126).at(12),
        ],
        outline: BLACK.at(10),
        outline_focused: BLACK.at(18),
        dim: BLACK.at(4),
    };

    pub const fn of(mode: Mode) -> &'static Theme {
        match mode {
            Mode::Dark => &Theme::DARK,
            Mode::Light => &Theme::LIGHT,
        }
    }

    /// The bar's tone: focus shows by it (owner, M18b).
    pub const fn bar(&self, focused: bool) -> Color {
        if focused {
            self.chrome
        } else {
            self.unfocused_bar
        }
    }

    /// The window outline: brighter (darker in light) when focused, never coloured.
    pub const fn outline(&self, focused: bool) -> Color {
        if focused {
            self.outline_focused
        } else {
            self.outline
        }
    }

    /// Bar glyphs and the title: the text token when focused, else the secondary token
    /// (owner, M18b).
    pub const fn bar_ink(&self, focused: bool) -> Color {
        if focused { self.text } else { self.secondary }
    }

    /// A glyph on a hovered button's square: the text token whatever the focus (the
    /// secondary token on the close square is 2.07:1 dark and 1.80:1 light, under 3:1).
    pub const fn hovered_glyph(&self) -> Color {
        self.text
    }

    /// The overlay at 5, 8 or 12%.
    pub const fn overlay(&self, level: Overlay) -> Color {
        self.overlays[level as usize]
    }

    /// The tokens written the way docs/design.md writes them, in its order. The token
    /// test compares this with the spec's table.
    pub fn spec_items(&self) -> Vec<String> {
        let ink = |c: Color| match (c.alpha, c.rgb) {
            (100, _) => c.hex_string(),
            (a, 0xFFFFFF) => format!("white {a}%"),
            (a, 0x000000) => format!("black {a}%"),
            (a, _) => format!("{} at {a}%", c.hex_string()),
        };
        let [o5, o8, o12] = self.overlays;
        let overlay_ink = match o5.rgb {
            0xFFFFFF => "white".to_string(),
            _ => format!("{} at", o5.hex_string()),
        };
        let overlays = if o5.rgb == o8.rgb && o8.rgb == o12.rgb {
            format!(
                "overlays {overlay_ink} {}/{}/{}%",
                o5.alpha, o8.alpha, o12.alpha
            )
        } else {
            "overlays (differ)".to_string()
        };
        let chrome = match self.mode {
            Mode::Dark => "chrome/bar",
            Mode::Light => "chrome",
        };
        let mut out = vec![
            format!("{chrome} {}", ink(self.chrome)),
            format!("unfocused bar {}", ink(self.unfocused_bar)),
            format!("page {}", ink(self.page)),
            format!("raised {}", ink(self.raised)),
        ];
        out.extend(self.card.map(|c| format!("card {}", ink(c))));
        out.extend([
            format!("hairline {}", ink(self.hairline)),
            format!("text {}", ink(self.text)),
            format!("secondary {}", ink(self.secondary)),
            format!("accent {}", ink(self.accent)),
            format!("accent tile {}", ink(self.accent_tile)),
            format!("text on accent fill {}", ink(self.on_accent)),
        ]);
        if self.mode == Mode::Dark {
            out.push(format!("close hover {}", ink(self.close_hover)));
        }
        out.extend([
            overlays,
            format!(
                "window outline {} (focused {}%)",
                ink(self.outline),
                self.outline_focused.alpha
            ),
            format!("inactive dim {}", ink(self.dim)),
        ]);
        out
    }
}

/// The overlay levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    /// 5%: hover.
    Hover = 0,
    /// 8%: pressed and selected.
    Pressed = 1,
    /// 12%: a hovered bar button.
    Strong = 2,
}

/// The keyboard focus ring (docs/design.md, the launcher): 2 px of the accent, 3 px
/// off the widget.
pub const FOCUS_RING_WIDTH: i32 = 2;
pub const FOCUS_RING_OFFSET: i32 = 3;

/// Text contrast, WCAG 2.2 level AA.
pub const TEXT_CONTRAST: f64 = 4.5;
/// Non-text glyphs (icons) and the focus ring.
pub const NON_TEXT_CONTRAST: f64 = 3.0;

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
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// `fg` drawn over the opaque `bg` (8-bit alpha, source over, as the renderers do).
pub const fn over(fg: Color, bg: Color) -> Color {
    let a = fg.a8() as u32;
    let (r, g, b) = (
        mix(fg.r(), bg.r(), a),
        mix(fg.g(), bg.g(), a),
        mix(fg.b(), bg.b(), a),
    );
    Color::hex(r << 16 | g << 8 | b)
}

const fn mix(f: u8, b: u8, a: u32) -> u32 {
    (f as u32 * a + b as u32 * (255 - a) + 127) / 255
}

/// The contrast ratio of `fg` (drawn over `bg`) against `bg`.
pub fn contrast(fg: Color, bg: Color) -> f64 {
    let (a, b) = (luminance(over(fg, bg)), luminance(bg));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// A token pair with its threshold, for the contrast test and the report.
#[derive(Clone, Debug)]
pub struct Pair {
    pub what: String,
    pub ratio: f64,
    pub threshold: f64,
}

/// Every pair the spec uses, in both appearances: text and secondary text on every
/// surface and bar; text on the accent fill; bar glyphs and titles (focused on the
/// focused bar, unfocused on the unfocused bar, the hovered glyph on a hovered button's
/// square); the focus ring against every surface.
pub fn pairs() -> Vec<Pair> {
    let mut out = Vec::new();
    for theme in [&Theme::DARK, &Theme::LIGHT] {
        let mode = match theme.mode {
            Mode::Dark => "dark",
            Mode::Light => "light",
        };
        let name = |c: Color| match (c.alpha, c.rgb) {
            (100, _) => c.hex_string(),
            (a, 0xFFFFFF) => format!("white {a}%"),
            (a, _) => format!("{} {a}%", c.hex_string()),
        };
        let mut surfaces = vec![
            ("page", theme.page),
            ("raised", theme.raised),
            ("chrome (focused bar)", theme.chrome),
            ("unfocused bar", theme.unfocused_bar),
        ];
        surfaces.extend(theme.card.map(|c| ("card", c)));
        let mut push = |what: String, fg: Color, bg: Color, threshold: f64| {
            out.push(Pair {
                what: format!("{mode}: {what} ({} on {})", name(fg), name(bg)),
                ratio: contrast(fg, bg),
                threshold,
            });
        };
        for (surface, bg) in &surfaces {
            push(format!("text on {surface}"), theme.text, *bg, TEXT_CONTRAST);
            push(
                format!("secondary on {surface}"),
                theme.secondary,
                *bg,
                TEXT_CONTRAST,
            );
        }
        push(
            "text on the accent fill".into(),
            theme.on_accent,
            theme.accent,
            TEXT_CONTRAST,
        );
        for focused in [true, false] {
            let state = if focused { "focused" } else { "unfocused" };
            let bar = theme.bar(focused);
            let ink = theme.bar_ink(focused);
            push(format!("{state} title on its bar"), ink, bar, TEXT_CONTRAST);
            push(format!("{state} bar glyph"), ink, bar, NON_TEXT_CONTRAST);
            let hover = over(theme.overlay(Overlay::Strong), bar);
            push(
                format!("{state} window: hovered glyph on the maximize square"),
                theme.hovered_glyph(),
                hover,
                NON_TEXT_CONTRAST,
            );
            push(
                format!("{state} window: hovered glyph on the close square"),
                theme.hovered_glyph(),
                theme.close_hover,
                NON_TEXT_CONTRAST,
            );
        }
        for (surface, bg) in &surfaces {
            push(
                format!("focus ring on {surface}"),
                theme.accent,
                *bg,
                NON_TEXT_CONTRAST,
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = include_str!("../../docs/design.md");

    /// The spec's text between `start` and `end`, on one line, without the last full
    /// stop, split at ", ".
    fn spec_list(start: &str, end: &str) -> Vec<String> {
        let from = SPEC.find(start).expect(start) + start.len();
        let to = from + SPEC[from..].find(end).expect(end);
        let text = SPEC[from..to]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        text.trim_end_matches('.')
            .split(", ")
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn every_token_is_the_spec_table() {
        assert_eq!(
            Theme::DARK.spec_items(),
            spec_list("Colours, dark:", "Colours, light:"),
            "dark"
        );
        assert_eq!(
            Theme::LIGHT.spec_items(),
            spec_list("Colours, light:", "Default accent"),
            "light"
        );
        // The close hover is listed once; the light theme uses the same.
        assert_eq!(Theme::LIGHT.close_hover, Theme::DARK.close_hover);
    }

    #[test]
    fn the_accent_presets_are_the_spec() {
        let spec = SPEC.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(spec.contains("Default accent is teal"));
        assert_eq!(Accent::default(), Accent::Teal);
        assert_eq!(Theme::DARK.accent, Accent::Teal.color(Mode::Dark));
        assert_eq!(Theme::LIGHT.accent, Accent::Teal.color(Mode::Light));
        let blue = format!(
            "blue {}/{} stays a preset",
            Accent::Blue.color(Mode::Dark).hex_string(),
            Accent::Blue.color(Mode::Light).hex_string()
        );
        assert!(spec.contains(&blue), "{blue}");
    }

    #[test]
    fn the_focus_ring_is_the_spec() {
        let spec = SPEC.split_whitespace().collect::<Vec<_>>().join(" ");
        let ring =
            format!("Keyboard focus: {FOCUS_RING_WIDTH} px accent ring {FOCUS_RING_OFFSET} px off");
        assert!(spec.contains(&ring), "{ring}");
    }

    #[test]
    fn automatic_means_dark_until_it_has_a_source() {
        assert_eq!(Appearance::default(), Appearance::Automatic);
        assert_eq!(Appearance::Automatic.mode(), Mode::Dark);
        assert_eq!(Appearance::Light.mode(), Mode::Light);
        assert_eq!(Appearance::Dark.mode(), Mode::Dark);
    }

    #[test]
    fn alphas_round_to_bytes() {
        assert_eq!(WHITE.at(92).a8(), 235);
        assert_eq!(WHITE.at(14).a8(), 36);
        assert_eq!(BLACK.at(4).a8(), 10);
        assert_eq!(Color::hex(0x15191C).rgba8(), [0x15, 0x19, 0x1C, 255]);
        assert_eq!(over(WHITE.at(50), BLACK), Color::hex(0x808080));
    }

    /// Every pair with its ratio and threshold (printed: the report). A pair under its
    /// threshold fails; a token is never changed to make it pass (owner, M18b): such a
    /// pair goes to the owner.
    #[test]
    fn contrast_of_every_pair_the_spec_uses() {
        let mut failing = Vec::new();
        for pair in pairs() {
            let ok = pair.ratio >= pair.threshold;
            println!(
                "{:<88} {:>6.2}  >= {}  {}",
                pair.what,
                pair.ratio,
                pair.threshold,
                if ok { "ok" } else { "UNDER" }
            );
            if !ok {
                failing.push(pair.what);
            }
        }
        assert!(failing.is_empty(), "under the threshold: {failing:?}");
    }

    /// Why a hovered glyph is the text token: the secondary token fails on the close
    /// square (owner, M18b report).
    #[test]
    fn secondary_on_the_close_square_would_fail() {
        for theme in [&Theme::DARK, &Theme::LIGHT] {
            assert!(contrast(theme.secondary, theme.close_hover) < NON_TEXT_CONTRAST);
            assert!(contrast(theme.hovered_glyph(), theme.close_hover) >= NON_TEXT_CONTRAST);
        }
    }
}
