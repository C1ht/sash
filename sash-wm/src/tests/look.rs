// SPDX-License-Identifier: Apache-2.0

//! How windows look (M18b, owner): the theme's bar tones, outline, dim, glyphs, hover
//! squares and the centred title. Goldens are contact sheets made by the CPU reference
//! (tests/reference.rs), one per appearance and scale, in fixtures/decorations/
//! (SASH_TEXT_BLESS=1 writes them; the owner looks at each).

use std::path::PathBuf;

use sash_text::TextStack;
use sash_theme::{Mode, Theme};

use super::reference::{Picture, Window, compose, title_box};
use crate::{
    decorations::Look,
    layout::{
        Rect,
        decor::{BUTTON, Button, Decoration, SLOTS, TITLE_MARGIN},
        gaps::{Frame, physical_edge},
    },
};

const TITLE: &str = "Text Editor — notes.txt";
const CELL: (i32, i32) = (420, 160);
const MARGIN: i32 = 21;

/// The cells of a sheet: a name, the decoration and the look.
fn cells() -> Vec<(&'static str, Decoration, Look)> {
    let focused = Look {
        focused: true,
        rounded: true,
        dim: true,
        ..Look::default()
    };
    let unfocused = Look {
        focused: false,
        ..focused
    };
    vec![
        ("focused", Decoration::FULL, focused),
        ("unfocused", Decoration::FULL, unfocused),
        (
            "hovered close",
            Decoration::FULL,
            Look {
                hover: Some(Button::Close),
                ..focused
            },
        ),
        (
            "hovered maximize",
            Decoration::FULL,
            Look {
                hover: Some(Button::Maximize),
                ..focused
            },
        ),
        (
            "maximized (restore glyph)",
            Decoration::FULL,
            Look {
                maximized: true,
                ..focused
            },
        ),
        ("tiled, focused", Decoration::BORDER, focused),
        ("tiled, unfocused, dim on", Decoration::BORDER, unfocused),
        (
            "tiled, unfocused, dim off",
            Decoration::BORDER,
            Look {
                dim: false,
                ..unfocused
            },
        ),
    ]
}

fn cell_rect(i: usize) -> Rect {
    let (col, row) = ((i % 2) as i32, (i / 2) as i32);
    Rect::new(
        MARGIN + col * (CELL.0 + MARGIN),
        MARGIN + row * (CELL.1 + MARGIN),
        CELL.0,
        CELL.1,
    )
}

/// The stand-in client: the left half the light page, the right half the dark page (an
/// app of either appearance), so the dim shows on both.
fn client(w: i32) -> impl Fn(i32, i32) -> [f32; 3] {
    let tone = |c: sash_theme::Color| [c.r(), c.g(), c.b()].map(|v| f32::from(v) / 255.0);
    let (light, dark) = (tone(Theme::LIGHT.page), tone(Theme::DARK.page));
    move |x, _| if x < w / 2 { light } else { dark }
}

/// A contact sheet of every cell for `theme` at `scale`.
pub fn sheet(stack: &mut TextStack, theme: &Theme, scale: f64) -> Picture {
    let size = (
        MARGIN + 2 * (CELL.0 + MARGIN),
        MARGIN + 4 * (CELL.1 + MARGIN),
    );
    let area = Rect::new(0, 0, size.0, size.1);
    let mut picture = Picture::new(Frame::plain(area, scale).physical, theme.page);
    for (i, (_, decoration, look)) in cells().into_iter().enumerate() {
        let outer = cell_rect(i);
        let frame = Frame::plain(outer, scale);
        let inner_w = (f64::from(decoration.inner(outer).w) * scale).round() as i32;
        let paint = client(inner_w);
        let window = Window {
            outer,
            decoration,
            look,
            title: Some(TITLE),
            client: &paint,
        };
        compose(&mut picture, stack, theme, &frame, &window);
    }
    picture
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/tests/fixtures/decorations")
}

/// Compares with fixtures/decorations/NAME.png (SASH_TEXT_BLESS=1 writes it): the same
/// size, every channel within 2.
fn golden(name: &str, picture: &Picture) {
    let (w, h, rgb) = (picture.w as u32, picture.h as u32, picture.bytes());
    let path = golden_dir().join(format!("{name}.png"));
    if std::env::var_os("SASH_TEXT_BLESS").is_some() {
        std::fs::create_dir_all(golden_dir()).unwrap();
        let file = std::fs::File::create(&path).unwrap();
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&rgb)
            .unwrap();
        eprintln!("wrote {}", path.display());
        return;
    }
    let decoder = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(&path).unwrap_or_else(|e| panic!("{name}: {e}")),
    ));
    let mut reader = decoder.read_info().unwrap();
    let mut data = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut data).unwrap();
    assert_eq!((info.width, info.height), (w, h), "{name}: size");
    let worst = rgb
        .iter()
        .zip(&data)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    assert!(worst <= 2, "{name}: differs by {worst}");
}

/// The goldens the owner looks at: dark and light, at scales 1, 1.5 and 2; each sheet
/// has, left to right and top to bottom: focused, unfocused, hovered close, hovered
/// maximize, maximized (the restore glyph), and a tile focused, unfocused with the dim,
/// unfocused without it.
#[test]
fn contact_sheets_dark_and_light_at_three_scales() {
    let mut stack = TextStack::new();
    for (theme, mode) in [(&Theme::DARK, "dark"), (&Theme::LIGHT, "light")] {
        for (scale, tag) in [(1.0, "1"), (1.5, "1.5"), (2.0, "2")] {
            golden(
                &format!("windows-{mode}-x{tag}"),
                &sheet(&mut stack, theme, scale),
            );
        }
    }
}

/// A pixel of cell `i` at logical offset (dx, dy) from its outer corner, at scale 1.
fn cell_pixel(picture: &Picture, i: usize, dx: i32, dy: i32) -> [u8; 3] {
    let r = cell_rect(i);
    picture.at(r.x + dx, r.y + dy)
}

fn hex(c: sash_theme::Color) -> [u8; 3] {
    [c.r(), c.g(), c.b()]
}

/// Focus shows by the bar's tone: the focused bar is chrome, the unfocused one the
/// unfocused bar token (mutation check: swapping them fails here and in the goldens).
#[test]
fn the_bar_tone_shows_focus() {
    let mut stack = TextStack::new();
    for theme in [&Theme::DARK, &Theme::LIGHT] {
        let sheet = sheet(&mut stack, theme, 1.0);
        // In the bar, right of the reserved slots and left of the title.
        let at = (1 + SLOTS * BUTTON + 2, 20);
        assert_eq!(
            cell_pixel(&sheet, 0, at.0, at.1),
            hex(theme.chrome),
            "focused"
        );
        assert_eq!(
            cell_pixel(&sheet, 1, at.0, at.1),
            hex(theme.unfocused_bar),
            "unfocused"
        );
        assert_eq!(theme.bar(true), theme.chrome);
        assert_eq!(theme.bar(false), theme.unfocused_bar);
    }
}

/// The dim is over the client area only (mutation check: drawing it over the bar fails
/// here and in the goldens), never on a focused window; the tile without the dim shows
/// the client as it is.
#[test]
fn the_dim_covers_the_client_area_only() {
    let mut stack = TextStack::new();
    for theme in [&Theme::DARK, &Theme::LIGHT] {
        let sheet = sheet(&mut stack, theme, 1.0);
        let bar = (1 + SLOTS * BUTTON + 2, 20);
        // Floating focused (0) and unfocused (1): the bar has no dim on it.
        assert_eq!(
            cell_pixel(&sheet, 1, bar.0, bar.1),
            hex(theme.unfocused_bar)
        );
        // The client area: light left half, dark right half.
        let below = 34 + 1 + 40;
        let light = hex(Theme::LIGHT.page);
        assert_eq!(cell_pixel(&sheet, 0, 40, below), light, "focused: no dim");
        let dimmed = hex(sash_theme::over(theme.dim, Theme::LIGHT.page));
        assert_eq!(cell_pixel(&sheet, 1, 40, below), dimmed, "unfocused: dim");
        assert_ne!(dimmed, light);
        // Tiles: focused (5), unfocused with the dim (6), without (7).
        assert_eq!(cell_pixel(&sheet, 5, 40, 40), light);
        assert_eq!(cell_pixel(&sheet, 6, 40, 40), dimmed);
        assert_eq!(cell_pixel(&sheet, 7, 40, 40), light);
    }
}

/// The outline is never coloured: white (dark) or black (light) at the spec's alphas,
/// brighter or darker when focused.
#[test]
fn the_outline_is_never_coloured() {
    for theme in [&Theme::DARK, &Theme::LIGHT] {
        for focused in [true, false] {
            let c = theme.outline(focused);
            assert!(c.rgb == 0xFFFFFF || c.rgb == 0x000000, "{c:?}");
            assert_ne!(c.rgb, theme.accent.rgb);
        }
        assert!(theme.outline(true).alpha > theme.outline(false).alpha);
    }
}

/// Titles never reach the buttons or the reserved slot, and a short one is centred on
/// the bar (its line box within a pixel of the bar's centre), at every scale, for
/// long, empty, right-to-left and mixed-script titles.
#[test]
fn titles_are_centred_and_never_overlap_the_buttons() {
    let mut stack = TextStack::new();
    let long = "Text Editor — a very long document name that goes on and on ".repeat(4);
    let titles = [
        ("short", "Files", true),
        ("long", long.as_str(), false),
        ("empty", "", true),
        ("rtl", "مرحبا بالعالم - محرر النصوص", true),
        ("rtl long", &"שלום עולם, עורך טקסט ".repeat(8), false),
        ("mixed", "Notes — Ærø, 東京, مرحبا, 🎉 and more", true),
    ];
    for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
        for width in [300, 400, 703, 1200] {
            let outer = Rect::new(37, 11, width, 200);
            let frame = Frame::plain(outer, scale);
            let d = Decoration::FULL;
            let bar = frame.map(d.bar_rect(outer).unwrap());
            // Where the reserved slots end on each side.
            let reserve = physical_edge(SLOTS * BUTTON + TITLE_MARGIN, scale);
            for (name, text, short) in titles {
                for focused in [true, false] {
                    let Some(b) = title_box(&mut stack, &frame, outer, text, focused) else {
                        assert!(text.is_empty() || d.title_rect(outer).is_none(), "{name}");
                        continue;
                    };
                    let what = format!("{name}, width {width}, x{scale}");
                    assert!(
                        b.loc.x >= bar.loc.x + reserve - 1,
                        "{what}: starts in the buttons ({b:?}, bar {bar:?})"
                    );
                    assert!(
                        b.loc.x + b.size.w <= bar.loc.x + bar.size.w - reserve + 1,
                        "{what}: reaches the right end ({b:?}, bar {bar:?})"
                    );
                    let centre = |r: smithay::utils::Rectangle<i32, smithay::utils::Physical>| {
                        2 * r.loc.x + r.size.w
                    };
                    if short {
                        assert!(
                            (centre(b) - centre(bar)).abs() <= 2,
                            "{what}: not centred ({b:?}, bar {bar:?})"
                        );
                    }
                }
            }
        }
    }
}

/// The sheets are not empty: the light and dark themes differ, as do the scales.
#[test]
fn every_theme_draws_its_own_sheet() {
    let mut stack = TextStack::new();
    let dark = sheet(&mut stack, Theme::of(Mode::Dark), 1.0);
    let light = sheet(&mut stack, Theme::of(Mode::Light), 1.0);
    assert_ne!(dark.bytes(), light.bytes());
}
