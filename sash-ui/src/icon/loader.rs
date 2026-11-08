// SPDX-License-Identifier: Apache-2.0

//! Loading icons: first-party symbols, theme icons by name, checked absolute paths.
//! Every result is cached by source and size; every failure (no file, a file too
//! large, hostile or over its work budget) gives the fallback icon and is remembered,
//! so it is not tried again in the session.
//!
//! The system loader reads /usr/share/icons and /usr/share/pixmaps only (owner, M17
//! E3), starting from Adwaita; files are read only if, with links resolved, they lie
//! inside those roots.

use std::{
    cell::RefCell,
    collections::HashSet,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use resvg::tiny_skia::Pixmap;

use super::{
    IconRaster, IconSource,
    cache::IconCache,
    lookup::{Themes, read_capped},
    name::{IconName, IconPath},
    png::{self, MAX_PNG_BYTES},
    svg::{self, GUARD, MAX_SVG_BYTES},
    symbols::Symbol,
};

/// Rasterised icons kept (all sizes together).
pub const CACHE_BYTES: usize = 8 * 1024 * 1024;
pub const SYSTEM_ICONS: &str = "/usr/share/icons";
pub const SYSTEM_PIXMAPS: &str = "/usr/share/pixmaps";
pub const SYSTEM_THEME: &str = "Adwaita";

#[derive(Clone, PartialEq, Eq, Hash)]
enum Source {
    Symbol(Symbol),
    Name(IconName, u32),
    Path(IconPath),
}

/// Counters, for tests and measurements.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub rasterised: u64,
    pub failed: u64,
    pub fallbacks: u64,
}

pub struct IconLoader {
    themes: Themes,
    theme: String,
    require_root: bool,
    budget: Duration,
    cache: IconCache<(Source, u32)>,
    failed: HashSet<(Source, u32)>,
    stats: Stats,
}

impl IconLoader {
    /// /usr/share/icons and /usr/share/pixmaps, Adwaita first, root-owned files only.
    pub fn system() -> Self {
        Self::new(
            vec![PathBuf::from(SYSTEM_ICONS)],
            vec![PathBuf::from(SYSTEM_PIXMAPS)],
            SYSTEM_THEME,
            true,
        )
    }

    /// Other roots (tests); `require_root` refuses files not owned by root.
    pub fn new(
        icons: Vec<PathBuf>,
        pixmaps: Vec<PathBuf>,
        theme: &str,
        require_root: bool,
    ) -> Self {
        Self {
            themes: Themes::new(icons, pixmaps),
            theme: theme.to_string(),
            require_root,
            budget: GUARD,
            cache: IconCache::new(CACHE_BYTES),
            failed: HashSet::new(),
            stats: Stats::default(),
        }
    }

    /// The wall-clock guard for parsing and for drawing one SVG (tests lower it); the
    /// work budget is in `svg`.
    pub fn set_budget(&mut self, budget: Duration) {
        self.budget = budget;
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    pub fn cache_bytes(&self) -> usize {
        self.cache.bytes()
    }

    pub fn cached(&self) -> usize {
        self.cache.len()
    }

    /// The roots files may be read from.
    pub fn roots(&self) -> Vec<PathBuf> {
        self.themes.roots().cloned().collect()
    }

    /// A first-party chrome icon (a mask).
    pub fn symbol(&mut self, symbol: Symbol, px: u32) -> Rc<IconRaster> {
        self.get(Source::Symbol(symbol), px, |loader, px| {
            loader.svg(symbol.svg(), px).map(mask)
        })
    }

    /// A theme icon by name, at `px` device pixels on an output at `scale`.
    pub fn named(&mut self, name: &IconName, px: u32, scale: f32) -> Rc<IconRaster> {
        let scale = if scale.is_finite() && scale >= 1.0 {
            (scale.ceil() as u32).min(16)
        } else {
            1
        };
        let symbolic = name.is_symbolic();
        let name_for = name.clone();
        self.get(Source::Name(name.clone(), scale), px, move |loader, px| {
            let size = (px / scale).max(1);
            let theme = loader.theme.clone();
            let file = loader.themes.find(&name_for, size, scale, &theme)?;
            loader.file(&file, px, symbolic)
        })
    }

    /// An icon from a checked absolute path (Desktop Entry `Icon=/...`).
    pub fn path(&mut self, path: &IconPath, px: u32) -> Rc<IconRaster> {
        let file = path.path().to_path_buf();
        self.get(Source::Path(path.clone()), px, move |loader, px| {
            loader.file(&file, px, false)
        })
    }

    /// The icon shown when an icon cannot be found or drawn.
    pub fn fallback(&mut self, px: u32) -> Rc<IconRaster> {
        self.stats.fallbacks += 1;
        self.symbol(Symbol::App, px)
    }

    fn get(
        &mut self,
        source: Source,
        px: u32,
        load: impl FnOnce(&mut Self, u32) -> Option<IconRaster>,
    ) -> Rc<IconRaster> {
        let px = px.clamp(1, super::MAX_ICON_PX);
        let key = (source, px);
        if let Some(hit) = self.cache.get(&key) {
            return hit;
        }
        let is_fallback = key.0 == Source::Symbol(Symbol::App);
        if self.failed.contains(&key) {
            return if is_fallback {
                Rc::new(IconRaster::Mask(vec![0; (px * px) as usize]))
            } else {
                self.fallback(px)
            };
        }
        match load(self, px) {
            Some(raster) => {
                self.stats.rasterised += 1;
                let raster = Rc::new(raster);
                self.cache.insert(key, raster.clone());
                raster
            }
            None => {
                self.stats.failed += 1;
                self.failed.insert(key);
                if is_fallback {
                    Rc::new(IconRaster::Mask(vec![0; (px * px) as usize]))
                } else {
                    self.fallback(px)
                }
            }
        }
    }

    fn file(&self, file: &Path, px: u32, symbolic: bool) -> Option<IconRaster> {
        // With links resolved, the file must still be inside a root.
        let real = file.canonicalize().ok()?;
        let inside = self
            .themes
            .roots()
            .filter_map(|r| r.canonicalize().ok())
            .any(|r| real.starts_with(r));
        if !inside {
            return None;
        }
        if self.require_root {
            use std::os::unix::fs::MetadataExt;
            if std::fs::metadata(&real).ok()?.uid() != 0 {
                return None;
            }
        }
        let pixmap = match real.extension().and_then(|e| e.to_str()) {
            Some("svg") => self.svg(&read_capped(&real, MAX_SVG_BYTES)?, px)?,
            Some("png") => png::render(&read_capped(&real, MAX_PNG_BYTES)?, px).ok()?,
            _ => return None,
        };
        Some(if symbolic {
            mask(pixmap)
        } else {
            IconRaster::Color(pixmap.take())
        })
    }

    fn svg(&self, bytes: &[u8], px: u32) -> Option<Pixmap> {
        svg::render(bytes, px, self.budget).ok()
    }
}

/// The alpha channel only: a symbolic icon, tinted when drawn.
fn mask(pixmap: Pixmap) -> IconRaster {
    IconRaster::Mask(
        pixmap
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[3])
            .collect(),
    )
}

/// A first-party icon for a widget.
pub struct SymbolIcon {
    loader: Rc<RefCell<IconLoader>>,
    symbol: Symbol,
}

impl SymbolIcon {
    pub fn new(loader: Rc<RefCell<IconLoader>>, symbol: Symbol) -> Rc<Self> {
        Rc::new(Self { loader, symbol })
    }
}

impl IconSource for SymbolIcon {
    fn raster(&self, px: u32, _scale: f32) -> Option<Rc<IconRaster>> {
        Some(self.loader.borrow_mut().symbol(self.symbol, px))
    }
}

/// A theme icon by (untrusted) name for a widget.
pub struct ThemedIcon {
    loader: Rc<RefCell<IconLoader>>,
    name: IconName,
}

impl ThemedIcon {
    pub fn new(loader: Rc<RefCell<IconLoader>>, name: IconName) -> Rc<Self> {
        Rc::new(Self { loader, name })
    }
}

impl IconSource for ThemedIcon {
    fn raster(&self, px: u32, scale: f32) -> Option<Rc<IconRaster>> {
        Some(self.loader.borrow_mut().named(&self.name, px, scale))
    }
}
