// SPDX-License-Identifier: Apache-2.0

//! The cursor theme (CLAUDE.md, "Cursor themes (M11a, owner)"): Adwaita from
//! /usr/share/icons at size 24, shapes loaded on first use and kept for the session. A
//! shape is looked up by its CSS name, then its old X11 names, then the theme's default
//! shape; when nothing is found the caller draws the built-in arrow. Theme files are
//! read with limits, so a broken or hostile file only costs its shape.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    input::pointer::CursorIcon,
    utils::Transform,
};

/// Where themes are looked for (the only directory).
pub const THEME_ROOT: &str = "/usr/share/icons";
pub const THEME: &str = "Adwaita";
/// Nominal cursor size in pixels (scale 1).
pub const SIZE: u32 = 24;
/// A theme file larger than this is not read (Adwaita's largest, an animation: 4.7 MB).
const MAX_FILE_BYTES: u64 = 8 << 20;
/// Images wider or taller than this are skipped (the largest hardware cursor plane).
pub const MAX_IMAGE: u32 = 256;
/// How deep Inherits= chains are followed.
const MAX_INHERITS: usize = 8;

/// One image of a cursor shape, ready to draw.
pub struct Frame {
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the plane-size tests check them")
    )]
    pub width: u32,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the plane-size tests check them")
    )]
    pub height: u32,
    pub hotspot: (i32, i32),
    /// How long it is shown (animations; zero for a static shape).
    pub delay: Duration,
    /// ARGB8888, the hardware cursor plane's format.
    pub buffer: MemoryRenderBuffer,
}

/// A loaded shape: one frame, or several for an animation.
pub struct CursorImage {
    pub frames: Vec<Frame>,
}

impl CursorImage {
    /// The frame to show `elapsed` after the shape appeared, and how long until the next
    /// one (None: a static shape, nothing will change).
    pub fn frame_at(&self, elapsed: Duration) -> (usize, Option<Duration>) {
        let total: Duration = self.frames.iter().map(|f| f.delay).sum();
        if self.frames.len() < 2 || total.is_zero() {
            return (0, None);
        }
        let mut t = Duration::from_nanos((elapsed.as_nanos() % total.as_nanos()) as u64);
        for (i, frame) in self.frames.iter().enumerate() {
            if t < frame.delay {
                return (i, Some(frame.delay - t));
            }
            t -= frame.delay;
        }
        (0, Some(self.frames[0].delay))
    }

    pub fn animated(&self) -> bool {
        self.frame_at(Duration::ZERO).1.is_some()
    }
}

/// The theme: its cursor directories (the theme's, then inherited ones) and the shapes
/// loaded so far (None: looked for, not found).
pub struct CursorTheme {
    dirs: Vec<PathBuf>,
    size: u32,
    cache: RefCell<HashMap<CursorIcon, Option<Rc<CursorImage>>>>,
}

impl CursorTheme {
    /// The theme sash-wm uses: Adwaita at size 24; in debug builds XCURSOR_THEME and
    /// XCURSOR_SIZE may choose another one (still from /usr/share/icons).
    pub fn system() -> Self {
        #[allow(unused_mut)]
        let (mut name, mut size) = (THEME.to_string(), SIZE);
        #[cfg(debug_assertions)]
        {
            if let Ok(theme) = std::env::var("XCURSOR_THEME")
                && !theme.is_empty()
            {
                name = theme;
            }
            if let Some(s) = std::env::var("XCURSOR_SIZE")
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .filter(|s| (8..=MAX_IMAGE).contains(s))
            {
                size = s;
            }
            if name != THEME || size != SIZE {
                tracing::info!(name, size, "cursor theme override (debug build)");
            }
        }
        let theme = Self::load(Path::new(THEME_ROOT), &name, size);
        if theme.dirs.is_empty() {
            tracing::warn!(name, "no cursor theme found: the built-in arrow is used");
        }
        theme
    }

    /// The theme `name` under `root`, following its Inherits= within `root`.
    pub fn load(root: &Path, name: &str, size: u32) -> Self {
        Self {
            dirs: theme_dirs(root, name),
            size,
            cache: RefCell::default(),
        }
    }

    /// The image for `icon`, or None: draw the built-in arrow.
    pub fn image(&self, icon: CursorIcon) -> Option<Rc<CursorImage>> {
        if let Some(cached) = self.cache.borrow().get(&icon) {
            return cached.clone();
        }
        let found = self
            .find(icon)
            .or_else(|| (icon != CursorIcon::Default).then(|| self.image(CursorIcon::Default))?);
        self.cache.borrow_mut().insert(icon, found.clone());
        found
    }

    fn find(&self, icon: CursorIcon) -> Option<Rc<CursorImage>> {
        let names = std::iter::once(icon.name()).chain(icon.alt_names().iter().copied());
        for name in names {
            for dir in &self.dirs {
                let path = dir.join(name);
                if path.exists() {
                    match load_file(&path, self.size) {
                        Some(image) => return Some(Rc::new(image)),
                        None => tracing::debug!(name, "cursor file unusable"),
                    }
                }
            }
        }
        None
    }
}

/// The cursors directories of `name` and the themes it inherits, in lookup order. A
/// theme name may not leave `root`; loops and chains deeper than MAX_INHERITS stop.
fn theme_dirs(root: &Path, name: &str) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = vec![(name.to_string(), 0)];
    while let Some((theme, depth)) = queue.pop() {
        let valid = !theme.is_empty()
            && !theme.contains('/')
            && theme != "."
            && theme != ".."
            && depth <= MAX_INHERITS;
        if !valid || !seen.insert(theme.clone()) {
            continue;
        }
        let dir = root.join(&theme);
        if dir.join("cursors").is_dir() {
            dirs.push(dir.join("cursors"));
        }
        let inherits = fs::read_to_string(dir.join("index.theme"))
            .ok()
            .and_then(|text| {
                text.lines()
                    .find_map(|l| l.trim().strip_prefix("Inherits=").map(str::to_owned))
            })
            .unwrap_or_default();
        // Depth first, in the listed order.
        let parents: Vec<String> = inherits
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        for parent in parents.into_iter().rev() {
            queue.push((parent, depth + 1));
        }
    }
    dirs
}

/// One cursor file at the nominal size nearest to `size`: its frames, or None if the
/// file is too large, unreadable, truncated or has no usable image.
fn load_file(path: &Path, size: u32) -> Option<CursorImage> {
    if fs::metadata(path).ok()?.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    let images = xcursor::parser::parse_xcursor(&bytes)?;
    let usable = |i: &&xcursor::parser::Image| {
        i.width > 0
            && i.height > 0
            && i.width <= MAX_IMAGE
            && i.height <= MAX_IMAGE
            && i.pixels_rgba.len() == (i.width * i.height * 4) as usize
    };
    let nominal = images
        .iter()
        .filter(usable)
        .map(|i| i.size)
        .min_by_key(|s| (s.abs_diff(size), u32::MAX - s))?;
    let frames: Vec<Frame> = images
        .iter()
        .filter(usable)
        .filter(|i| i.size == nominal)
        .map(|i| {
            // RGBA bytes to ARGB8888 in memory (B, G, R, A), the cursor plane's format.
            let mut argb = i.pixels_rgba.clone();
            for px in argb.as_chunks_mut::<4>().0 {
                px.swap(0, 2);
            }
            Frame {
                width: i.width,
                height: i.height,
                hotspot: (
                    i.xhot.min(i.width - 1) as i32,
                    i.yhot.min(i.height - 1) as i32,
                ),
                delay: Duration::from_millis(i.delay as u64),
                buffer: MemoryRenderBuffer::from_slice(
                    &argb,
                    Fourcc::Argb8888,
                    (i.width as i32, i.height as i32),
                    1,
                    Transform::Normal,
                    None,
                ),
            }
        })
        .collect();
    (!frames.is_empty()).then_some(CursorImage { frames })
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// An xcursor file with `images`: (nominal size, width, height, xhot, yhot, delay).
    pub fn xcursor_file(images: &[(u32, u32, u32, u32, u32, u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        let header = 16u32;
        let toc = 12 * images.len() as u32;
        out.extend_from_slice(b"Xcur");
        for v in [header, 0x1_0000, images.len() as u32] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        let mut pos = header + toc;
        let mut chunks = Vec::new();
        for &(size, w, h, xhot, yhot, delay) in images {
            for v in [0xfffd_0002u32, size, pos] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            let mut chunk = Vec::new();
            for v in [36u32, 0xfffd_0002, size, 1, w, h, xhot, yhot, delay] {
                chunk.extend_from_slice(&v.to_le_bytes());
            }
            for i in 0..w * h {
                chunk.extend_from_slice(&(0xff00_0000u32 | i).to_le_bytes());
            }
            pos += chunk.len() as u32;
            chunks.push(chunk);
        }
        for chunk in chunks {
            out.extend_from_slice(&chunk);
        }
        out
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sash-cursor-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn theme(root: &Path, name: &str, inherits: Option<&str>, files: &[(&str, Vec<u8>)]) {
        let dir = root.join(name);
        fs::create_dir_all(dir.join("cursors")).unwrap();
        if let Some(parents) = inherits {
            fs::write(
                dir.join("index.theme"),
                format!("[Icon Theme]\nName={name}\nInherits={parents}\n"),
            )
            .unwrap();
        }
        for (file, bytes) in files {
            fs::write(dir.join("cursors").join(file), bytes).unwrap();
        }
    }

    #[test]
    fn the_nearest_nominal_size_and_its_frames_are_loaded() {
        let root = scratch("sizes");
        let file = xcursor_file(&[
            (24, 24, 24, 3, 4, 0),
            (32, 32, 32, 4, 5, 0),
            (48, 48, 48, 6, 7, 0),
        ]);
        theme(&root, "T", None, &[("default", file)]);
        for (want, got) in [(24, 24), (30, 32), (28, 32), (100, 48), (8, 24)] {
            let t = CursorTheme::load(&root, "T", want);
            let image = t.image(CursorIcon::Default).expect("loaded");
            assert_eq!(image.frames[0].width, got, "size {want}");
        }
        let t = CursorTheme::load(&root, "T", 24);
        let image = t.image(CursorIcon::Default).unwrap();
        assert_eq!(image.frames[0].hotspot, (3, 4));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn css_name_then_old_names_then_the_default_then_nothing() {
        let root = scratch("names");
        let one = || xcursor_file(&[(24, 24, 24, 0, 0, 0)]);
        // "pointer" by its CSS name, "text" only under its old name "xterm".
        theme(
            &root,
            "T",
            None,
            &[("default", one()), ("pointer", one()), ("xterm", one())],
        );
        let t = CursorTheme::load(&root, "T", 24);
        assert!(t.image(CursorIcon::Pointer).is_some());
        assert!(t.image(CursorIcon::Text).is_some(), "old X11 name");
        // A missing shape falls back to the theme's default (the same image).
        let wait = t.image(CursorIcon::Wait).unwrap();
        assert!(Rc::ptr_eq(&wait, &t.image(CursorIcon::Default).unwrap()));
        // No theme: nothing (the caller draws the built-in arrow).
        let none = CursorTheme::load(&root, "Missing", 24);
        assert!(none.image(CursorIcon::Default).is_none());
        // A theme without a default: a missing shape gives nothing.
        theme(&root, "U", None, &[("pointer", one())]);
        let u = CursorTheme::load(&root, "U", 24);
        assert!(u.image(CursorIcon::Wait).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn broken_files_and_themes_are_handled_safely() {
        let root = scratch("broken");
        let good = xcursor_file(&[(24, 24, 24, 0, 0, 0)]);
        let mut truncated = good.clone();
        truncated.truncate(good.len() / 2);
        let zero = xcursor_file(&[(24, 0, 0, 0, 0, 0)]);
        let oversized = xcursor_file(&[(24, 300, 300, 0, 0, 0)]);
        let mut garbage = b"Xcur".to_vec();
        garbage.extend_from_slice(&[0xff; 64]);
        // An oversized image next to a usable one: the usable one is taken (its hotspot
        // at the edge, which the parser allows, is kept inside the image).
        let mixed = xcursor_file(&[(24, 400, 10, 0, 0, 0), (32, 32, 32, 32, 32, 0)]);
        theme(
            &root,
            "T",
            None,
            &[
                ("default", good),
                ("pointer", truncated),
                ("text", zero),
                ("wait", oversized),
                ("help", garbage),
                ("crosshair", mixed),
            ],
        );
        let t = CursorTheme::load(&root, "T", 24);
        let default = t.image(CursorIcon::Default).unwrap();
        for icon in [
            CursorIcon::Pointer,
            CursorIcon::Text,
            CursorIcon::Wait,
            CursorIcon::Help,
        ] {
            let image = t.image(icon).expect("falls back to the default");
            assert!(Rc::ptr_eq(&image, &default), "{icon:?}");
        }
        let crosshair = t.image(CursorIcon::Crosshair).unwrap();
        assert_eq!(crosshair.frames[0].width, 32);
        assert_eq!(
            crosshair.frames[0].hotspot,
            (31, 31),
            "hotspot kept inside the image"
        );
        // A file larger than the limit is not read at all.
        fs::write(
            root.join("T/cursors/move"),
            vec![0u8; (MAX_FILE_BYTES + 1) as usize],
        )
        .unwrap();
        assert!(Rc::ptr_eq(&t.image(CursorIcon::Move).unwrap(), &default));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn inherits_are_followed_safely() {
        let root = scratch("inherits");
        let one = || xcursor_file(&[(24, 24, 24, 0, 0, 0)]);
        // A -> B -> C -> A (a loop), C has the shape; also a parent outside the root.
        theme(&root, "A", Some("B,../outside,/etc"), &[]);
        theme(&root, "B", Some("C"), &[]);
        theme(&root, "C", Some("A"), &[("default", one())]);
        let t = CursorTheme::load(&root, "A", 24);
        assert_eq!(t.dirs.len(), 3, "each theme once, nothing outside the root");
        assert!(t.image(CursorIcon::Default).is_some());
        // A chain deeper than the limit stops there.
        for i in 0..20 {
            theme(&root, &format!("D{i}"), Some(&format!("D{}", i + 1)), &[]);
        }
        let deep = CursorTheme::load(&root, "D0", 24);
        assert_eq!(deep.dirs.len(), MAX_INHERITS + 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn animation_frames_follow_the_delays_and_static_shapes_never_change() {
        let root = scratch("animation");
        let anim = xcursor_file(&[
            (24, 24, 24, 0, 0, 30),
            (24, 24, 24, 0, 0, 50),
            (24, 24, 24, 0, 0, 20),
        ]);
        theme(
            &root,
            "T",
            None,
            &[
                ("default", xcursor_file(&[(24, 24, 24, 0, 0, 0)])),
                ("wait", anim),
            ],
        );
        let t = CursorTheme::load(&root, "T", 24);
        let wait = t.image(CursorIcon::Wait).unwrap();
        assert!(wait.animated());
        let ms = Duration::from_millis;
        assert_eq!(wait.frame_at(ms(0)), (0, Some(ms(30))));
        assert_eq!(wait.frame_at(ms(29)), (0, Some(ms(1))));
        assert_eq!(wait.frame_at(ms(30)), (1, Some(ms(50))));
        assert_eq!(wait.frame_at(ms(95)), (2, Some(ms(5))));
        assert_eq!(wait.frame_at(ms(100)), (0, Some(ms(30))), "loops");
        assert_eq!(wait.frame_at(ms(1_000_045)), (1, Some(ms(35))));
        // A static shape: no next change, so no frames.
        let default = t.image(CursorIcon::Default).unwrap();
        assert!(!default.animated());
        assert_eq!(default.frame_at(ms(12345)), (0, None));
        let _ = fs::remove_dir_all(&root);
    }

    /// The real Adwaita (skipped where it is not installed): every one of the 36 shapes
    /// loads by its own file, at size 24 every image fits the smallest cursor plane in use
    /// (the VM's 64x64; NVIDIA's is 256x256), and at the largest debug size it still fits
    /// NVIDIA's.
    #[test]
    fn adwaita_has_all_shapes_and_fits_the_cursor_planes() {
        let root = Path::new(THEME_ROOT);
        if !root.join(THEME).join("cursors").is_dir() {
            eprintln!("no {THEME} cursors in {THEME_ROOT}: skipped");
            return;
        }
        let t = CursorTheme::load(root, THEME, SIZE);
        let default = t.image(CursorIcon::Default).expect("default");
        let mut own = 0;
        for icon in all_icons() {
            let image = t.image(icon).expect("every shape loads");
            if icon == CursorIcon::Default || !Rc::ptr_eq(&image, &default) {
                own += 1;
            }
            for frame in &image.frames {
                assert!(frame.width <= 64 && frame.height <= 64, "{icon:?}");
                assert!(
                    frame.hotspot.0 < frame.width as i32 && frame.hotspot.1 < frame.height as i32
                );
            }
        }
        assert_eq!(own, all_icons().len(), "every shape has its own image");
        let large = CursorTheme::load(root, THEME, 96);
        for icon in all_icons() {
            for frame in &large.image(icon).unwrap().frames {
                assert!(frame.width <= MAX_IMAGE && frame.height <= MAX_IMAGE);
            }
        }
    }

    pub fn all_icons() -> Vec<CursorIcon> {
        use CursorIcon::*;
        vec![
            Default,
            ContextMenu,
            Help,
            Pointer,
            Progress,
            Wait,
            Cell,
            Crosshair,
            Text,
            VerticalText,
            Alias,
            Copy,
            Move,
            NoDrop,
            NotAllowed,
            Grab,
            Grabbing,
            EResize,
            NResize,
            NeResize,
            NwResize,
            SResize,
            SeResize,
            SwResize,
            WResize,
            EwResize,
            NsResize,
            NeswResize,
            NwseResize,
            ColResize,
            RowResize,
            AllScroll,
            ZoomIn,
            ZoomOut,
            DndAsk,
            AllResize,
        ]
    }
}
