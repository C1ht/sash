<!-- SPDX-License-Identifier: Apache-2.0 -->

# M14 step 1: text stack measurement and proposal

Rules: CLAUDE.md, "Text stack". Measured 2026-11-05 in throwaway crates outside the
repository (`~/.cache/sash/m14/`: `textbench` for cosmic-text, `alt-swash` for swash
alone, `sash-copy` for the size added to sash-wm). No code in the repository.

Setup: toolchain 1.99.1 (the repository's pin), release profile as the repository
(cargo defaults). Timings on the development machine (host, and the debian:trixie
container for every fallback measurement); medians of 50 (cold) and 200 (warm) runs.

## Fonts used

| Font | Source | Size | sha256 |
|---|---|---|---|
| Manrope (variable, wght 200-800) | google/fonts, `ofl/manrope` at commit b31870aff700ab7a1d74fa0c6887d95beb9e0037 | 164 700 B | 3ae11c49db0455a3cc33e37d380f20fdb8c7f8b41dc07625c177e3d87a9d6ae6 |
| its licence, OFL.txt | same commit | 4 387 B | 58172e0c0fac2cda8a37b348164bb55e44b0e69051e557e92b1d3f6910141f7b |

Manrope: "Copyright 2018 The Manrope Project Authors" (OFL.txt; METADATA.pb says 2019),
SIL OFL 1.1, **no Reserved Font Name declared**, designer Mikhail Sharanda. Duties: ship
the copyright notice and the OFL text with the font (kept next to the font file), list
it in NOTICE, never sell the font on its own; a modified version may keep the name
(no RFN).

Fallback set (trixie packages, read from /usr/share/fonts in the container only; the
host has none of them): fonts-dejavu-core (DejaVuSans.ttf 0.76 MB), fonts-noto-core
(NotoSans-Regular 0.51 MB, NotoSansHebrew 27 KB, NotoSansArabic 0.24 MB),
fonts-noto-cjk (NotoSansCJK-Regular.ttc 19.5 MB), fonts-noto-color-emoji
(NotoColorEmoji.ttf 11.3 MB). apt: 80.7 MB downloaded, 153 MB installed, container only.

## cosmic-text 0.19.0 and its crates

cosmic-text 0.19.0 (MIT OR Apache-2.0, MSRV 1.89; our toolchain is 1.99.1). Features:
default = std, swash, fontconfig. Measured with `default-features = false, features =
["std", "swash"]`; `fontconfig` adds 2 crates and 6 KB and is not needed (sash-text
loads a fixed list of files).

27 crates new to sash-wm (`cargo tree` with and without it):

| Crate | Licence | MSRV | What it does |
|---|---|---|---|
| cosmic-text 0.19.0 | MIT OR Apache-2.0 | 1.89 | buffers, layout, editing, fallback, glue |
| harfrust 0.5.2 | MIT | 1.85 | HarfBuzz shaping port |
| skrifa 0.40.0 and 0.44.0 | MIT OR Apache-2.0 | 1.85 | font tables, outlines (two versions: swash and cosmic-text pin different ones) |
| read-fonts 0.37.0 and 0.41.0 | MIT OR Apache-2.0 | 1.85 | OpenType parsing (two versions) |
| font-types 0.11.3 and 0.12.7 | MIT OR Apache-2.0 | 1.85 | scalar font types (two versions) |
| swash 0.2.10 | Apache-2.0 OR MIT | - | glyph scaling, hinting, rasterisation, colour glyphs |
| zeno 0.3.3 | Apache-2.0 OR MIT | - | path rasterisation (used by swash) |
| yazi 0.2.2 | Apache-2.0 OR MIT | - | zlib (compressed font tables) |
| fontdb 0.23.0 | MIT | 1.60 | font database, CSS-like queries |
| **ttf-parser 0.25.1** | MIT OR Apache-2.0 | 1.63 | font parsing for fontdb: **unmaintained, RUSTSEC-2026-0192** |
| unicode-bidi 0.3.18 | MIT OR Apache-2.0 | 1.47 | bidi algorithm |
| unicode-linebreak 0.1.5 | Apache-2.0 | 1.56 | line breaking |
| unicode-script 0.5.9 | MIT OR Apache-2.0 | - | script property (fallback, itemisation) |
| unicode-segmentation 1.13.3 | MIT OR Apache-2.0 | 1.85 | graphemes |
| sys-locale 0.3.2 | MIT OR Apache-2.0 | 1.56 | system locale (fallback choice); part of `std`, not avoidable |
| smol_str 0.3.7 | MIT OR Apache-2.0 | 1.89 | small strings |
| rangemap 1.8.0 | MIT/Apache-2.0 | 1.81 | ranges |
| slotmap 1.1.1 | Zlib | 1.58 | slot maps |
| self_cell 1.3.0 | Apache-2.0 OR GPL-2.0-only | - | self-referential structs |
| rustc-hash 2.1.3 | Apache-2.0 OR MIT | 1.77 | hashing |
| tinyvec 1.13.3 | Zlib OR Apache-2.0 OR MIT | - | small vectors |
| linebender_resource_handle 0.1.1 | Apache-2.0 OR MIT | 1.70 | blob handles |
| core_maths 0.1.1, libm 0.2.16 | MIT | - / 1.63 | float maths |

`cargo deny` with the repository's deny.toml: **licences ok, bans ok, sources ok;
advisories FAILED** on ttf-parser (unmaintained, no safe upgrade; reached through
fontdb, which cosmic-text requires). Avoidable by features: only fontconfig. fontdb,
ttf-parser, harfrust, sys-locale and the unicode crates come with `std`.

## Measurements

### Size added

| Build | Before | After | Added |
|---|---|---|---|
| sash-wm release, cosmic-text linked and used | 12 204 336 B | 15 196 000 B | **+2.99 MB** |
| same, stripped | 8 063 360 B | 10 514 232 B | **+2.45 MB** |
| standalone (baseline binary vs title bench), stripped | 341 344 B | 2 919 000 B | +2.58 MB |
| standalone, swash alone, stripped | 341 344 B | 1 984 584 B | +1.64 MB |

Bundling Manrope adds 165 KB.

### Time (cosmic-text, trixie container, Manrope + 6 fallback files)

| What | Result |
|---|---|
| load Manrope only (fontdb, memory-mapped) | 11.6 µs median, 0 KiB heap |
| load Manrope + fallback set (7 files) | 82.8 µs median, 7 KiB heap (the files are memory-mapped; tables parsed on first use) |
| first use: process start to the first rasterised title | 0.49 ms (0.37 ms with Manrope alone, host) |
| 40-char title, 13 px, scale 1 | 40 glyphs; cold (empty cache) 64 µs, warm 13.4 µs; 2 116 B of masks |
| scale 1.5 | cold 67 µs, warm 13.3 µs; 4 240 B |
| scale 2 | cold 75 µs, warm 13.3 µs; 7 249 B |
| swash alone (host, Manrope, same title): scale 1 / 1.5 / 2 | cold 28 / 32 / 37 µs, warm 4.2 µs; first use 0.21 ms |

Warm time is shaping (cosmic-text re-shapes each call); the glyph images come from the
cache. One title costs well under 0.1 ms per frame even cold; titles change rarely,
so a shaped-line cache per window makes the per-frame cost a texture upload only.

### Glyph cache memory

| Load | Heap added |
|---|---|
| 1000 distinct Latin titles (with æøå é), scale 1 / 1.5 / 2 | 33 / 43 / 54 KiB (about 100 distinct glyphs: the cache is per glyph, not per title) |
| 1000 titles of 20 distinct CJK characters (20 000 glyphs), scale 1 | 5.8 MiB |
| same, scale 2 | 14.7 MiB |

cosmic-text's SwashCache never evicts: a client cycling its title through CJK
characters grows it without bound. sash-text needs its own cache with a byte cap and
eviction (rule: a constant).

### Scripts and fallback (scale 1, cold)

| Text | Glyphs | Fonts that drew it | Time |
|---|---|---|---|
| Danish "Ærø Ø Å æblegrød på øen, Hvidovre Ålborg" | 40 | Manrope | 0.11 ms |
| accents "Café naïve façade Zürich São Paulo Łódź" | 39 | Manrope | 0.10 ms |
| CJK "設定 日本語のテキスト 中文标题 한국어 제목" | 23 | Noto Sans CJK SC, JP, KR (+ Manrope for spaces) | 0.51 ms (first use of the 19.5 MB face) |
| emoji "Party 🎉 smile 😀 heart ❤ flag 🇩🇰" | 29 | Manrope, Noto Color Emoji, **DejaVu Sans** | 0.64 ms |
| Hebrew and Arabic | 23 | Noto Sans Hebrew, Noto Sans Arabic | 0.54 ms |
| combining marks and a zero-width space | 14 | Manrope | 0.06 ms |

Without the fallback set (host), every character falls back to Manrope's .notdef box.

### Colour emoji

Noto Color Emoji is a bitmap font (CBDT, one large bitmap strike). swash returns a **colour
image** (RGBA, scaled from the strike: 16x15 at 13 px) for 🎉, the flag 🇩🇰 (one
glyph) and 👍🏽 (skin tone, one glyph). With cosmic-text's own fallback order, 😀 and ❤
were drawn by **DejaVu Sans** as monochrome masks, because its unix fallback list puts
DejaVu Sans before Noto Color Emoji. Our output is ARGB8888, so colour glyphs fit;
masks are tinted with the text colour, colour glyphs are drawn as they are. The order
is ours to fix either way (cosmic-text takes a custom `Fallback`; swash alone means
sash-text picks the font per cluster).

## Known limits (plainly)

- **Bidi:** titles are one line; with cosmic-text, right-to-left runs would be reordered
  by unicode-bidi; with swash alone they are shaped in logical order and **not
  reordered** (Hebrew reads backwards) unless sash-text adds unicode-bidi. Proposal:
  not reordered in M14, documented; mixed-direction titles are rare in a bar.
- **IME:** none. sash-text draws text; it never edits text (no cursor, no preedit).
- **Vertical text:** not supported.
- **Hinting on the 5120x1440 panel:** 1190 mm wide, about 109 px per inch. swash hints
  outlines (Manrope is a variable font with little hinting) and renders grayscale
  anti-aliasing; no subpixel (LCD) rendering, so no colour fringes and no dependence on
  the panel's subpixel order. At 13 px (scale 1) the stems are about 1 px: legible but
  soft; scale 1.5 or 2 (fractional scale is a later milestone) is where text is crisp.
- **Colour fonts:** bitmap (CBDT) and COLRv0 are drawn; COLRv1 gradients are not.
- **Font coverage:** what the fallback files cover; anything else is a .notdef box.

## Alternatives (brief)

| Option | Crates new to sash-wm | Shaping | Fallback, layout, cache | Advisory | Notes |
|---|---|---|---|---|---|
| **cosmic-text** | 27 (three duplicated pairs) | HarfBuzz quality (harfrust), bidi | built in (fallback order, no cache cap) | ttf-parser (fontdb) | +2.45 MB; most of it (editing, buffers, multi-line) is unused by single-line titles |
| **swash alone** | 6 (swash, skrifa, read-fonts, font-types, zeno, yazi) | swash's own shaper: kerning, ligatures, marks, Arabic joining, most scripts; less complete than HarfBuzz for Indic | ours: a fixed fallback list by charmap, one line, ellipsis, LRU cache with a byte cap | none | +1.64 MB; 2-3x faster here; colour bitmaps and COLRv0 included |
| harfrust + skrifa + zeno | about 15 (duplicate font-types and read-fonts) | HarfBuzz quality | ours, plus rasterisation glue (outlines to zeno) and colour bitmaps by hand | none | the best shaping without cosmic-text, the most code of ours |
| fontdue 0.9.5 | 8 | none (kerning only) | partial | ttf-parser | no complex scripts, no colour |
| ab_glyph 0.2.32 | 4 | none | none | ttf-parser (owned_ttf_parser) | rasteriser only |

## Proposal

**Engine: swash alone** (recommended), behind sash-text's own API so the engine can be
swapped later. Reasons: no unmaintained crate (cargo deny passes), 6 crates instead of
27, smaller and faster, and sash-text needs only one line with ellipsis, which is
little code; the cost is swash's shaper instead of HarfBuzz's (fine for Latin, Danish,
CJK, Hebrew/Arabic joining; weaker for some Indic scripts) and no bidi reordering.
Alternative if HarfBuzz-quality shaping matters more: cosmic-text with a custom
Fallback, and an ignore for RUSTSEC-2026-0192 in deny.toml (owner decision, like the
cgmath advisory).

**Crate layout** (`sash-text/`, Apache-2.0, depends only on the engine crates):

```
sash-text/
  Cargo.toml
  fonts/Manrope[wght].ttf   bundled, embedded with include_bytes!
  fonts/OFL.txt             the font's licence, next to it (also installed with the .deb)
  src/lib.rs                public API, constants
  src/untrusted.rs          client text: UTF-8 repair, control characters, byte and glyph caps
  src/fonts.rs              Manrope + the fixed fallback list from /usr/share/fonts
  src/shape.rs              one line: clusters, per-cluster fallback, ellipsis
  src/cache.rs              glyph cache: byte cap, LRU eviction
  src/raster.rs             masks and colour glyphs to ARGB8888 at a scale
  tests/, golden/           golden images, property tests, fuzz target
```

**API** (all infallible; bad input gives fewer glyphs, never a panic):

```rust
pub struct TextStack;               // fonts + glyph cache; one per process
impl TextStack {
    pub fn new() -> TextStack;      // Manrope always; missing fallback files are skipped
    pub fn line(&mut self, text: &ClientText, style: Style, max_width: Option<u32>) -> Line;
    pub fn draw(&mut self, line: &Line, scale: f32, color: [u8; 4]) -> Image;
}
pub struct ClientText;              // untrusted bytes, validated once
impl ClientText { pub fn new(bytes: &[u8]) -> ClientText; }   // lossy UTF-8, caps, no controls
pub struct Style { pub size: f32, pub weight: u16 }            // logical px; Manrope's wght axis
pub struct Line { /* glyphs */ }
impl Line {
    pub fn width(&self) -> f32;     // logical px, after the ellipsis
    pub fn height(&self) -> f32;    // ascent + descent
    pub fn truncated(&self) -> bool;
}
pub struct Image { pub width: u32, pub height: u32, pub stride: u32, pub pixels: Vec<u8> } // ARGB8888, premultiplied
```

Constants (proposed values): glyph cache 4 MiB (about 13 000 Latin glyphs at scale 2),
client text 1024 bytes and 256 glyphs before truncation, fallback fonts at most 8.
Fallback order (fixed): Manrope, Noto Sans, Noto Sans Hebrew, Noto Sans Arabic, Noto
Sans CJK (JP for Han), Noto Color Emoji for emoji presentation, DejaVu Sans last.
`ClientText` has no `Debug` that prints its content, so it cannot reach a log by
accident (enforced by a test in step 2).
