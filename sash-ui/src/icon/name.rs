// SPDX-License-Identifier: Apache-2.0

//! Icon names and absolute icon paths from untrusted app data.
//!
//! A name is a name: it never contains a path separator, is never "." or "..", and is
//! only ever joined to a theme directory as one file name. The Desktop Entry
//! Specification also allows an absolute path; that case is a different type, checked
//! on its own: it must name a regular PNG or SVG file inside one of the icon roots.

use std::{
    fmt,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};

/// The longest icon name kept (bytes); longer names are refused.
pub const MAX_NAME_BYTES: usize = 255;

/// A validated icon name. Its `Debug` never shows the name (it comes from an app).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct IconName(String);

impl fmt::Debug for IconName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("IconName(..)")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameError {
    Empty,
    TooLong,
    NotUtf8,
    /// A separator, a NUL, a control character, a leading dot, or "." / "..".
    NotAName,
}

impl IconName {
    /// Validates `bytes` as an icon name. A trailing ".png", ".svg" or ".xpm" (which
    /// some desktop files carry against the spec) is dropped.
    pub fn parse(bytes: &[u8]) -> Result<Self, NameError> {
        if bytes.is_empty() {
            return Err(NameError::Empty);
        }
        if bytes.len() > MAX_NAME_BYTES {
            return Err(NameError::TooLong);
        }
        let text = std::str::from_utf8(bytes).map_err(|_| NameError::NotUtf8)?;
        let text = [".png", ".svg", ".xpm"]
            .iter()
            .find_map(|ext| text.strip_suffix(ext))
            .unwrap_or(text);
        if text.is_empty()
            || text.starts_with('.')
            || text
                .chars()
                .any(|c| c == '/' || c == '\\' || c == '\0' || c.is_control())
        {
            return Err(NameError::NotAName);
        }
        Ok(Self(text.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether it is a symbolic icon ("-symbolic", drawn as a tinted mask).
    pub fn is_symbolic(&self) -> bool {
        self.0.ends_with("-symbolic")
    }
}

/// A checked absolute icon path. Its `Debug` never shows the path.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct IconPath(PathBuf);

impl fmt::Debug for IconPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("IconPath(..)")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathError {
    NotAbsolute,
    /// A ".." component, or a file outside every icon root once resolved.
    OutsideRoots,
    NotPngOrSvg,
    NotAFile,
    NotRootOwned,
}

impl IconPath {
    /// Checks `bytes` as an absolute icon path: no "..", a .png or .svg file that,
    /// with every symbolic link resolved, lies inside one of `roots`, and (with
    /// `require_root`) belongs to root.
    pub fn check(bytes: &[u8], roots: &[PathBuf], require_root: bool) -> Result<Self, PathError> {
        let text = std::str::from_utf8(bytes).map_err(|_| PathError::NotAbsolute)?;
        let path = Path::new(text);
        if !path.is_absolute() || text.contains('\0') {
            return Err(PathError::NotAbsolute);
        }
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(PathError::OutsideRoots);
        }
        let ext = path.extension().and_then(|e| e.to_str());
        if !matches!(ext, Some("png" | "svg")) {
            return Err(PathError::NotPngOrSvg);
        }
        let real = path.canonicalize().map_err(|_| PathError::NotAFile)?;
        // A link to some other kind of file is refused too.
        if !matches!(
            real.extension().and_then(|e| e.to_str()),
            Some("png" | "svg")
        ) {
            return Err(PathError::NotPngOrSvg);
        }
        let inside = roots
            .iter()
            .filter_map(|r| r.canonicalize().ok())
            .any(|r| real.starts_with(r));
        if !inside {
            return Err(PathError::OutsideRoots);
        }
        let meta = std::fs::metadata(&real).map_err(|_| PathError::NotAFile)?;
        if !meta.is_file() {
            return Err(PathError::NotAFile);
        }
        if require_root && meta.uid() != 0 {
            return Err(PathError::NotRootOwned);
        }
        Ok(Self(real))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}
