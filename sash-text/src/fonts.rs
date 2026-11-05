// SPDX-License-Identifier: Apache-2.0

//! The fonts: Manrope, embedded, and a fixed list of fallback fonts read from
//! /usr/share/fonts (Debian 13 package paths) when first needed. Fallback fonts are
//! never bundled or downloaded.

use swash::{CacheKey, FontRef};

/// Manrope (SIL OFL 1.1, fonts/OFL.txt), a variable font with a weight axis.
pub static MANROPE: &[u8] = include_bytes!("../fonts/Manrope[wght].ttf");

/// The fallback fonts, in the order they are tried after Manrope (owner, after M14
/// step 1): script fonts, CJK, colour emoji, then DejaVu Sans last. For an emoji
/// cluster Noto Color Emoji is tried first. (name, Debian package path, face index).
pub const FALLBACK: [(&str, &str, u32); 7] = [
    (
        "Noto Sans",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        0,
    ),
    (
        "Noto Sans Hebrew",
        "/usr/share/fonts/truetype/noto/NotoSansHebrew-Regular.ttf",
        0,
    ),
    (
        "Noto Sans Arabic",
        "/usr/share/fonts/truetype/noto/NotoSansArabic-Regular.ttf",
        0,
    ),
    (
        "Noto Sans Devanagari",
        "/usr/share/fonts/truetype/noto/NotoSansDevanagari-Regular.ttf",
        0,
    ),
    (
        // Face 0 of the collection: Noto Sans CJK JP.
        "Noto Sans CJK",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        0,
    ),
    (
        "Noto Color Emoji",
        "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
        0,
    ),
    (
        "DejaVu Sans",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        0,
    ),
];

/// At most this many fallback fonts (owner, after M14 step 1).
pub const MAX_FALLBACK: usize = 8;
const _: () = assert!(FALLBACK.len() <= MAX_FALLBACK);

/// Index of Noto Color Emoji in the font list (Manrope is 0).
pub(crate) const EMOJI: usize = 6;
/// A font file larger than this is not read (the largest, Noto Sans CJK, is 19.5 MB).
const MAX_FONT_FILE: u64 = 64 * 1024 * 1024;

/// A font's file and where its table directory is.
pub(crate) struct FontData {
    pub name: &'static str,
    data: Data,
    offset: u32,
    key: CacheKey,
    /// The font has a weight axis (Manrope).
    pub variable: bool,
}

enum Data {
    Static(&'static [u8]),
    Owned(Vec<u8>),
}

impl FontData {
    fn new(name: &'static str, data: Data, index: u32) -> Option<Self> {
        let bytes = match &data {
            Data::Static(b) => *b,
            Data::Owned(b) => b.as_slice(),
        };
        let font = FontRef::from_index(bytes, index as usize)?;
        let (offset, key) = (font.offset, font.key);
        let variable = font
            .variations()
            .any(|v| v.tag() == swash::tag_from_bytes(b"wght"));
        Some(Self {
            name,
            data,
            offset,
            key,
            variable,
        })
    }

    pub fn font(&self) -> FontRef<'_> {
        let data = match &self.data {
            Data::Static(b) => *b,
            Data::Owned(b) => b.as_slice(),
        };
        FontRef {
            data,
            offset: self.offset,
            key: self.key,
        }
    }
}

enum Slot {
    NotLoaded(&'static str, &'static str, u32),
    Loaded(FontData),
    Missing(&'static str),
}

/// Manrope and the fallback fonts.
pub(crate) struct Fonts {
    manrope: FontData,
    fallback: Vec<Slot>,
}

impl Fonts {
    /// Manrope and the system fallback list.
    pub fn system() -> Self {
        Self::with_fallback(&FALLBACK)
    }

    pub fn with_fallback(list: &[(&'static str, &'static str, u32)]) -> Self {
        let manrope = FontData::new("Manrope", Data::Static(MANROPE), 0)
            .expect("the embedded Manrope parses (checked by a test)");
        Self {
            manrope,
            fallback: list
                .iter()
                .take(MAX_FALLBACK)
                .map(|(name, path, index)| Slot::NotLoaded(name, path, *index))
                .collect(),
        }
    }

    /// Number of fonts: Manrope plus the fallback slots.
    pub fn count(&self) -> usize {
        1 + self.fallback.len()
    }

    /// Font `index` (0 is Manrope), reading its file the first time. None if it is not
    /// installed or does not parse.
    pub fn get(&mut self, index: usize) -> Option<&FontData> {
        if index == 0 {
            return Some(&self.manrope);
        }
        let slot = self.fallback.get_mut(index - 1)?;
        if let Slot::NotLoaded(name, path, face) = *slot {
            *slot = match load(name, path, face) {
                Some(font) => Slot::Loaded(font),
                None => Slot::Missing(name),
            };
        }
        match slot {
            Slot::Loaded(font) => Some(font),
            _ => None,
        }
    }

    /// Font `index` if it is already loaded (drawing never reads files).
    pub fn loaded(&self, index: usize) -> Option<&FontData> {
        if index == 0 {
            return Some(&self.manrope);
        }
        match self.fallback.get(index - 1)? {
            Slot::Loaded(font) => Some(font),
            _ => None,
        }
    }

    pub fn status(&self) -> Vec<(&'static str, bool)> {
        std::iter::once(("Manrope", true))
            .chain(self.fallback.iter().map(|slot| match slot {
                Slot::NotLoaded(name, ..) | Slot::Missing(name) => (*name, false),
                Slot::Loaded(font) => (font.name, true),
            }))
            .collect()
    }
}

fn load(name: &'static str, path: &str, index: u32) -> Option<FontData> {
    let size = std::fs::metadata(path).ok()?.len();
    if size > MAX_FONT_FILE {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    FontData::new(name, Data::Owned(bytes), index)
}
