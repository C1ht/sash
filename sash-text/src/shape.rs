// SPDX-License-Identifier: Apache-2.0

//! One line: script runs, a font per cluster (fallback by charmap), shaping per run
//! of one font and script, then the glyph cap and the ellipsis.

use swash::{
    shape::ShapeContext,
    text::{
        Codepoint, Script, analyze,
        cluster::{CharCluster, CharInfo, Parser, Status, Token},
    },
};

use crate::{
    MAX_GLYPHS, Style,
    fonts::{EMOJI, Fonts},
};

/// A glyph placed on the line, in logical pixels from the line's origin (left edge,
/// baseline); y grows downwards.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placed {
    pub font: u8,
    pub id: u16,
    pub x: f32,
    pub y: f32,
}

/// A shaped line, ready to draw. Measurements are in logical pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub(crate) glyphs: Vec<Placed>,
    pub(crate) style: Style,
    width: f32,
    ascent: f32,
    descent: f32,
    truncated: bool,
    fonts_used: Vec<&'static str>,
}

impl Line {
    /// Advance width, including the ellipsis if any.
    pub fn width(&self) -> f32 {
        self.width
    }

    /// Ascent plus descent of the fonts used (at least Manrope's).
    pub fn height(&self) -> f32 {
        self.ascent + self.descent
    }

    /// Distance from the top of the line to the baseline.
    pub fn ascent(&self) -> f32 {
        self.ascent
    }

    /// Whether the line was cut (and ends with an ellipsis).
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }

    /// The fonts that drew this line, in order of first use.
    pub fn fonts_used(&self) -> &[&'static str] {
        &self.fonts_used
    }
}

/// A run of text set in one font and script.
struct Segment {
    font: usize,
    script: Script,
    start: usize,
    end: usize,
}

/// Where each cluster starts: its first glyph and the pen position.
#[derive(Clone, Copy)]
struct ClusterStart {
    glyph: usize,
    pen: f32,
}

pub(crate) fn line(
    fonts: &mut Fonts,
    shaper: &mut ShapeContext,
    text: &str,
    cut: bool,
    style: Style,
    max_width: Option<f32>,
) -> Line {
    let max_width = max_width.map(|w| if w.is_finite() { w.max(0.0) } else { 0.0 });
    // swash runs on untrusted text: should it ever panic, the line is drawn as an
    // ellipsis and the shaping state is replaced.
    let shaped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        shape_runs(fonts, shaper, text, style)
    }));
    let Shaped {
        mut glyphs,
        mut clusters,
        pen,
        capped,
        used,
    } = shaped.unwrap_or_else(|_| {
        *shaper = ShapeContext::new();
        Shaped {
            glyphs: Vec::new(),
            clusters: Vec::new(),
            pen: 0.0,
            capped: true,
            used: vec![0],
        }
    });
    let mut width = pen;
    let too_wide = max_width.is_some_and(|m| width > m);
    let truncated = cut || capped || too_wide;
    if truncated {
        let (ellipsis, ellipsis_width) = ellipsis(fonts, shaper, style);
        let limit = max_width.unwrap_or(f32::INFINITY) - ellipsis_width;
        if limit < 0.0 {
            glyphs.clear();
            width = 0.0;
        } else {
            // Keep whole clusters whose end is within the limit.
            clusters.push(ClusterStart {
                glyph: glyphs.len(),
                pen: width,
            });
            let keep = clusters
                .iter()
                .rev()
                .find(|c| c.pen <= limit)
                .copied()
                .unwrap_or(ClusterStart { glyph: 0, pen: 0.0 });
            glyphs.truncate(keep.glyph);
            glyphs.extend(ellipsis.into_iter().map(|g| Placed {
                x: g.x + keep.pen,
                ..g
            }));
            width = keep.pen + ellipsis_width;
        }
    }
    let (mut ascent, mut descent) = (0.0f32, 0.0f32);
    let mut fonts_used = Vec::new();
    for index in used {
        if let Some(font) = fonts.loaded(index) {
            let metrics = font.font().metrics(&[]).scale(style.size);
            ascent = ascent.max(metrics.ascent);
            descent = descent.max(metrics.descent.abs());
            if index == 0 || glyphs.iter().any(|g| usize::from(g.font) == index) {
                fonts_used.push(font.name);
            }
        }
    }
    Line {
        glyphs,
        style,
        width,
        ascent,
        descent,
        truncated,
        fonts_used,
    }
}

/// The shaped glyphs of a line before the ellipsis.
struct Shaped {
    glyphs: Vec<Placed>,
    clusters: Vec<ClusterStart>,
    pen: f32,
    capped: bool,
    used: Vec<usize>,
}

fn shape_runs(fonts: &mut Fonts, shaper: &mut ShapeContext, text: &str, style: Style) -> Shaped {
    let segments = segments(fonts, text);
    let mut glyphs: Vec<Placed> = Vec::new();
    let mut clusters: Vec<ClusterStart> = Vec::new();
    let mut pen = 0.0f32;
    let mut capped = false;
    let mut used: Vec<usize> = vec![0];
    for segment in &segments {
        let Some(font) = fonts.loaded(segment.font) else {
            continue;
        };
        if !used.contains(&segment.font) {
            used.push(segment.font);
        }
        let mut builder = shaper
            .builder(font.font())
            .script(segment.script)
            .size(style.size);
        if font.variable {
            builder = builder.variations(&[("wght", f32::from(style.weight))]);
        }
        let mut run = builder.build();
        run.add_str(&text[segment.start..segment.end]);
        run.shape_with(|cluster| {
            if capped || glyphs.len() + cluster.glyphs.len() > MAX_GLYPHS {
                capped = true;
                return;
            }
            clusters.push(ClusterStart {
                glyph: glyphs.len(),
                pen,
            });
            // A default-ignorable character the font lacks is invisible: no box, no
            // width.
            let source = &text[segment.start..segment.end];
            let range = cluster.source.to_range();
            let invisible = source
                .get(range)
                .is_some_and(|s| s.chars().all(default_ignorable));
            for g in cluster.glyphs {
                if g.id == 0 && invisible {
                    continue;
                }
                glyphs.push(Placed {
                    font: segment.font as u8,
                    id: g.id,
                    x: pen + g.x,
                    y: -g.y,
                });
                pen += g.advance;
            }
        });
    }
    Shaped {
        glyphs,
        clusters,
        pen,
        capped,
        used,
    }
}

/// The ellipsis in Manrope ("…", or "..." if it had none), placed from x = 0.
fn ellipsis(fonts: &mut Fonts, shaper: &mut ShapeContext, style: Style) -> (Vec<Placed>, f32) {
    let Some(manrope) = fonts.loaded(0) else {
        return (Vec::new(), 0.0);
    };
    let font = manrope.font();
    let text = if font.charmap().map('…') != 0 {
        "…"
    } else {
        "..."
    };
    let mut run = shaper
        .builder(font)
        .size(style.size)
        .variations(&[("wght", f32::from(style.weight))])
        .build();
    run.add_str(text);
    let mut out = Vec::new();
    let mut pen = 0.0;
    run.shape_with(|cluster| {
        for g in cluster.glyphs {
            out.push(Placed {
                font: 0,
                id: g.id,
                x: pen + g.x,
                y: -g.y,
            });
            pen += g.advance;
        }
    });
    (out, pen)
}

/// Splits `text` into runs of one script and one font. Characters of no particular
/// script (spaces, digits, punctuation, combining marks) join the run they are in.
fn segments(fonts: &mut Fonts, text: &str) -> Vec<Segment> {
    let tokens: Vec<Token> = text
        .char_indices()
        .zip(analyze(text.chars()))
        .map(|((offset, ch), (properties, boundary))| Token {
            ch,
            offset: offset as u32,
            len: ch.len_utf8() as u8,
            info: CharInfo::new(properties, boundary),
            data: 0,
        })
        .collect();
    let mut out: Vec<Segment> = Vec::new();
    let mut start = 0;
    while start < tokens.len() {
        let script = run_script(&tokens[start..]);
        let mut end = start + 1;
        while end < tokens.len() {
            let s = tokens[end].ch.script();
            if !neutral(s) && s != script {
                break;
            }
            end += 1;
        }
        let mut parser = Parser::new(script, tokens[start..end].iter().copied());
        let mut cluster = CharCluster::new();
        while parser.next(&mut cluster) {
            let ignorable = cluster.chars().iter().all(|c| default_ignorable(c.ch));
            let font = match out.last() {
                // Zero-width characters stay in the font before them: they never pull
                // in a fallback font (which could give them width).
                Some(last) if ignorable => last.font,
                _ => font_for(fonts, &mut cluster),
            };
            let range = cluster.range().to_range();
            match out.last_mut() {
                Some(last)
                    if last.font == font && last.script == script && last.end == range.start =>
                {
                    last.end = range.end;
                }
                _ => out.push(Segment {
                    font,
                    script,
                    start: range.start,
                    end: range.end,
                }),
            }
        }
        start = end;
    }
    out
}

fn neutral(script: Script) -> bool {
    matches!(script, Script::Common | Script::Inherited | Script::Unknown)
}

/// The script of a run: its first character with a script of its own.
fn run_script(tokens: &[Token]) -> Script {
    tokens
        .iter()
        .map(|t| t.ch.script())
        .find(|s| !neutral(*s))
        .unwrap_or(Script::Latin)
}

/// The first font that has every character of the cluster; else the one that has the
/// most; else Manrope (its .notdef box). Emoji (not plain ASCII like digits) try Noto
/// Color Emoji first.
fn font_for(fonts: &mut Fonts, cluster: &mut CharCluster) -> usize {
    let emoji =
        cluster.info().is_emoji() && cluster.chars().first().is_some_and(|c| !c.ch.is_ascii());
    let order = std::iter::once(EMOJI)
        .filter(|_| emoji)
        .chain((0..fonts.count()).filter(|i| !emoji || *i != EMOJI));
    let mut best = 0;
    for index in order {
        let Some(font) = fonts.get(index) else {
            continue;
        };
        let charmap = font.font().charmap();
        // Zero-width characters count as present in every font (they draw nothing),
        // so "h" + ZWNJ stays in the font of the "h".
        match cluster.map(|ch| {
            if default_ignorable(ch) {
                1
            } else {
                charmap.map(ch)
            }
        }) {
            Status::Complete => return index,
            Status::Keep => best = index,
            Status::Discard => {}
        }
    }
    best
}

/// Unicode's Default_Ignorable_Code_Point (DerivedCoreProperties): format and
/// variation characters that are never shown on their own.
fn default_ignorable(ch: char) -> bool {
    matches!(
        u32::from(ch),
        0x00AD
            | 0x034F
            | 0x061C
            | 0x115F..=0x1160
            | 0x17B4..=0x17B5
            | 0x180B..=0x180F
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x206F
            | 0x3164
            | 0xFE00..=0xFE0F
            | 0xFEFF
            | 0xFFA0
            | 0xFFF0..=0xFFF8
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0000..=0xE0FFF
    )
}
