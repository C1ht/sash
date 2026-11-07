// SPDX-License-Identifier: Apache-2.0

//! index.theme, per the freedesktop Icon Theme Specification: the [Icon Theme] group's
//! Inherits, Directories and ScaledDirectories, and for each directory its Size, Scale,
//! Type (Fixed, Scalable, Threshold; Threshold by default), MinSize, MaxSize and
//! Threshold.
//!
//! The file is untrusted input: past [`MAX_INDEX_BYTES`] it is refused, invalid UTF-8
//! is replaced, lines that do not parse are skipped, at most [`MAX_DIRECTORIES`]
//! directories and [`MAX_INHERITS`] parents are kept, and a directory whose path is
//! absolute or has a ".." component is dropped. Nothing here panics.

use std::{
    collections::HashMap,
    path::{Component, Path},
};

pub const MAX_INDEX_BYTES: usize = 256 * 1024;
pub const MAX_DIRECTORIES: usize = 512;
pub const MAX_INHERITS: usize = 16;
/// Sizes and scales beyond these are refused (the directory is dropped).
pub const MAX_DIR_SIZE: u32 = 4096;
pub const MAX_DIR_SCALE: u32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirType {
    Fixed,
    Scalable,
    Threshold,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Directory {
    /// Relative to the theme directory, e.g. "48x48/apps".
    pub path: String,
    pub size: u32,
    pub scale: u32,
    pub kind: DirType,
    pub min: u32,
    pub max: u32,
    pub threshold: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Index {
    pub inherits: Vec<String>,
    pub directories: Vec<Directory>,
}

/// Parses index.theme; None if it is too large or has no [Icon Theme] group.
pub fn parse(bytes: &[u8]) -> Option<Index> {
    if bytes.len() > MAX_INDEX_BYTES {
        return None;
    }
    let text = String::from_utf8_lossy(bytes);
    let mut groups: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            // Only the groups a lookup can use are kept, so a file of a million groups
            // costs nothing.
            current = (groups.len() <= MAX_DIRECTORIES + 1).then(|| name.to_string());
            if let Some(name) = &current {
                groups.entry(name.clone()).or_default();
            }
            continue;
        }
        let (Some(group), Some((key, value))) = (&current, line.split_once('=')) else {
            continue;
        };
        // Localised keys (Name[da]) are not needed.
        let key = key.trim();
        if key.contains('[') {
            continue;
        }
        if let Some(g) = groups.get_mut(group) {
            g.entry(key.to_string())
                .or_insert_with(|| value.trim().to_string());
        }
    }
    let head = groups.get("Icon Theme")?;
    let list = |key: &str| -> Vec<String> {
        head.get(key)
            .map(|v| {
                v.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut inherits: Vec<String> = list("Inherits")
        .into_iter()
        .filter(|n| is_theme_name(n))
        .collect();
    inherits.dedup();
    inherits.truncate(MAX_INHERITS);
    let mut names = list("Directories");
    names.extend(list("ScaledDirectories"));
    let mut directories: Vec<Directory> = Vec::new();
    for name in names {
        if directories.len() >= MAX_DIRECTORIES {
            break;
        }
        if directories.iter().any(|d| d.path == name) || !is_relative_dir(&name) {
            continue;
        }
        if let Some(dir) = groups.get(&name).and_then(|g| directory(&name, g)) {
            directories.push(dir);
        }
    }
    Some(Index {
        inherits,
        directories,
    })
}

fn directory(path: &str, keys: &HashMap<String, String>) -> Option<Directory> {
    let num = |key: &str| keys.get(key).and_then(|v| v.parse::<u32>().ok());
    let size = num("Size").filter(|&s| (1..=MAX_DIR_SIZE).contains(&s))?;
    let scale = match keys.get("Scale") {
        None => 1,
        Some(_) => num("Scale").filter(|&s| (1..=MAX_DIR_SCALE).contains(&s))?,
    };
    let kind = match keys.get("Type").map(String::as_str) {
        Some("Fixed") => DirType::Fixed,
        Some("Scalable") => DirType::Scalable,
        Some("Threshold") | None => DirType::Threshold,
        Some(_) => return None,
    };
    let min = num("MinSize").unwrap_or(size).min(MAX_DIR_SIZE);
    let max = num("MaxSize").unwrap_or(size).min(MAX_DIR_SIZE);
    let threshold = num("Threshold").unwrap_or(2).min(MAX_DIR_SIZE);
    Some(Directory {
        path: path.to_string(),
        size,
        scale,
        kind,
        min: min.min(max),
        max: max.max(min),
        threshold,
    })
}

/// A theme name is one directory name under an icon root.
pub fn is_theme_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('.')
        && !name
            .chars()
            .any(|c| c == '/' || c == '\\' || c == '\0' || c.is_control())
}

fn is_relative_dir(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 255
        && !path.contains('\0')
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}
