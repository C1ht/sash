// SPDX-License-Identifier: Apache-2.0

//! Finding an icon file, per the freedesktop Icon Theme Specification's FindIcon:
//! the theme, then its parents (Inherits, depth first), then hicolor, then the
//! pixmaps directories. In each theme a directory whose size matches exactly wins;
//! otherwise the one with the smallest size distance (ties: the order in
//! index.theme).
//!
//! Inheritance is followed at most [`MAX_DEPTH`] levels deep and a theme already on
//! the way is never entered again (loops refused). Directory listings are read once
//! and capped; a name is only ever looked up as one file name in a listing.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use super::{
    index::{self, DirType, Directory, Index, MAX_INDEX_BYTES},
    name::IconName,
};

pub const MAX_DEPTH: usize = 8;
/// File names kept per directory listing.
pub const MAX_FILES_PER_DIR: usize = 20_000;
/// Formats looked for, in the spec's order (XPM is not supported).
pub const EXTENSIONS: [&str; 2] = ["png", "svg"];

/// Whether `dir` holds icons for `size` at `scale` (the spec's DirectoryMatchesSize).
pub fn matches_size(dir: &Directory, size: u32, scale: u32) -> bool {
    if dir.scale != scale {
        return false;
    }
    match dir.kind {
        DirType::Fixed => dir.size == size,
        DirType::Scalable => dir.min <= size && size <= dir.max,
        DirType::Threshold => {
            dir.size.saturating_sub(dir.threshold) <= size
                && size <= dir.size.saturating_add(dir.threshold)
        }
    }
}

/// How far `dir` is from `size` at `scale` (the spec's DirectorySizeDistance).
pub fn size_distance(dir: &Directory, size: u32, scale: u32) -> u64 {
    let want = u64::from(size) * u64::from(scale);
    let s = u64::from(dir.scale);
    match dir.kind {
        DirType::Fixed => (u64::from(dir.size) * s).abs_diff(want),
        DirType::Scalable => {
            if want < u64::from(dir.min) * s {
                u64::from(dir.min) * s - want
            } else if want > u64::from(dir.max) * s {
                want - u64::from(dir.max) * s
            } else {
                0
            }
        }
        DirType::Threshold => {
            let low = u64::from(dir.size.saturating_sub(dir.threshold)) * s;
            let high = u64::from(dir.size.saturating_add(dir.threshold)) * s;
            if want < low {
                (u64::from(dir.min) * s).abs_diff(want)
            } else if want > high {
                want.abs_diff(u64::from(dir.max) * s)
            } else {
                0
            }
        }
    }
}

/// Of `dirs`, the one to use for `size` at `scale`: the first exact match, else the
/// first with the smallest distance.
pub fn best<'a, I>(dirs: I, size: u32, scale: u32) -> Option<&'a Directory>
where
    I: IntoIterator<Item = &'a Directory> + Clone,
{
    dirs.clone()
        .into_iter()
        .find(|d| matches_size(d, size, scale))
        .or_else(|| {
            dirs.into_iter()
                .enumerate()
                .min_by_key(|(i, d)| (size_distance(d, size, scale), *i))
                .map(|(_, d)| d)
        })
}

struct Theme {
    index: Index,
    /// The theme's directory under every root that has it.
    bases: Vec<PathBuf>,
}

/// The icon roots and what was read from them (each index.theme and listing once).
pub struct Themes {
    icons: Vec<PathBuf>,
    pixmaps: Vec<PathBuf>,
    themes: HashMap<String, Option<Theme>>,
    listings: HashMap<PathBuf, HashSet<String>>,
}

impl Themes {
    pub fn new(icons: Vec<PathBuf>, pixmaps: Vec<PathBuf>) -> Self {
        Self {
            icons,
            pixmaps,
            themes: HashMap::new(),
            listings: HashMap::new(),
        }
    }

    pub fn roots(&self) -> impl Iterator<Item = &PathBuf> {
        self.icons.iter().chain(&self.pixmaps)
    }

    fn load(&mut self, name: &str) -> bool {
        if !self.themes.contains_key(name) {
            let theme = self.read_theme(name);
            self.themes.insert(name.to_string(), theme);
        }
        self.themes.get(name).is_some_and(Option::is_some)
    }

    fn read_theme(&self, name: &str) -> Option<Theme> {
        if !index::is_theme_name(name) {
            return None;
        }
        let bases: Vec<PathBuf> = self
            .icons
            .iter()
            .map(|root| root.join(name))
            .filter(|dir| dir.is_dir())
            .collect();
        // The first index.theme found is the theme's (the spec's rule).
        let index = bases
            .iter()
            .find_map(|dir| read_capped(&dir.join("index.theme"), MAX_INDEX_BYTES))
            .and_then(|bytes| index::parse(&bytes))?;
        Some(Theme { index, bases })
    }

    /// The themes to search for `start`, in order: depth first through Inherits,
    /// then hicolor. Loops and anything deeper than MAX_DEPTH are left out.
    pub fn chain(&mut self, start: &str) -> Vec<String> {
        let mut out = Vec::new();
        self.walk(start, 0, &mut out);
        if !out.iter().any(|t| t == "hicolor") && self.load("hicolor") {
            out.push("hicolor".to_string());
        }
        out
    }

    fn walk(&mut self, name: &str, depth: usize, out: &mut Vec<String>) {
        if depth >= MAX_DEPTH || out.iter().any(|t| t == name) || !self.load(name) {
            return;
        }
        out.push(name.to_string());
        let parents = self
            .themes
            .get(name)
            .and_then(Option::as_ref)
            .map(|t| t.index.inherits.clone())
            .unwrap_or_default();
        for parent in parents {
            self.walk(&parent, depth + 1, out);
        }
    }

    fn listing(&mut self, dir: PathBuf) -> &HashSet<String> {
        self.listings.entry(dir).or_insert_with_key(|dir| {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return HashSet::new();
            };
            entries
                .filter_map(Result::ok)
                .take(MAX_FILES_PER_DIR)
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
    }

    /// The file for `icon` at `size` and `scale` (whole numbers, the spec's), looked up
    /// from `theme`.
    pub fn find(&mut self, icon: &IconName, size: u32, scale: u32, theme: &str) -> Option<PathBuf> {
        for name in self.chain(theme) {
            if let Some(path) = self.find_in(&name, icon, size, scale) {
                return Some(path);
            }
        }
        // The fallback: the pixmaps directories, any size.
        for dir in self.pixmaps.clone() {
            for ext in EXTENSIONS {
                let file = format!("{}.{ext}", icon.as_str());
                if self.listing(dir.clone()).contains(&file) {
                    return Some(dir.join(file));
                }
            }
        }
        None
    }

    fn find_in(&mut self, theme: &str, icon: &IconName, size: u32, scale: u32) -> Option<PathBuf> {
        let (dirs, bases) = {
            let t = self.themes.get(theme)?.as_ref()?;
            (t.index.directories.clone(), t.bases.clone())
        };
        // The directories (with a base and an extension) that hold the icon.
        let mut found: Vec<(Directory, PathBuf)> = Vec::new();
        for dir in &dirs {
            'dir: for base in &bases {
                for ext in EXTENSIONS {
                    let file = format!("{}.{ext}", icon.as_str());
                    let path = base.join(&dir.path);
                    if self.listing(path.clone()).contains(&file) {
                        found.push((dir.clone(), path.join(file)));
                        break 'dir;
                    }
                }
            }
        }
        let chosen = best(found.iter().map(|(d, _)| d), size, scale)?;
        found
            .iter()
            .find(|(d, _)| d == chosen)
            .map(|(_, p)| p.clone())
    }
}

/// Reads at most `cap` bytes; None if the file is missing, unreadable or longer.
pub fn read_capped(path: &std::path::Path, cap: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(cap as u64 + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() <= cap).then_some(bytes)
}
