// SPDX-License-Identifier: Apache-2.0

//! SVG icons through resvg, for files that are untrusted (themes, apps).
//!
//! Before usvg sees a document, sash-ui parses it itself with roxmltree (no DTD, so no
//! entity expansion; at most [`MAX_SVG_NODES`] nodes) and refuses:
//! - more than [`MAX_SVG_BYTES`], more than [`MAX_SVG_DEPTH`] levels of nesting;
//! - scripts and event attributes, `foreignObject`, `image`, `feImage`, `pattern`
//!   (a tiny tile repeats without bound), fonts (`font`, `font-face`), CSS `@import`;
//! - every reference that is not to the document itself (`href` not starting with
//!   "#", `url(...)` not starting with "#");
//! - `use` beyond [`MAX_USES`], a `use` of something that holds a `use`, or `use`s
//!   that would copy more than [`MAX_SVG_NODES`] nodes in all.
//!
//! usvg then gets resolvers that load nothing and no resource directory; sash-ui
//! builds it without text, so no font is ever read. A canvas with a side beyond
//! [`MAX_SVG_SIDE`] (or none) is refused. The work is budgeted deterministically (owner,
//! M18c): the converted tree may hold at most [`MAX_TREE_NODES`] nodes,
//! [`MAX_PATH_SEGMENTS`] path segments, [`MAX_LAYERS`] isolated layers and
//! [`MAX_FILTER_PRIMITIVES`] filter primitives (clip paths and masks counted too), so
//! the same file gives the same answer on any machine under any load. Parsing and
//! drawing are also timed, against [`GUARD`] only: a generous backstop that a loaded
//! machine never reaches for anything the work budget lets through.
//! Drawn geometry beyond [`MAX_GEOMETRY`] is refused (tiny-skia indexes past its
//! buffer on a circle of radius 1e30, found by the fuzz-style tests), and a panic
//! inside usvg or resvg is still caught and becomes an error.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    time::{Duration, Instant},
};

use resvg::{
    tiny_skia::{Pixmap, Transform},
    usvg::{self, ImageHrefResolver, roxmltree},
};

pub const MAX_SVG_BYTES: usize = 256 * 1024;
pub const MAX_SVG_NODES: u32 = 20_000;
pub const MAX_SVG_DEPTH: usize = 64;
pub const MAX_USES: usize = 16;
pub const MAX_SVG_SIDE: f32 = 16_384.0;
/// Drawn geometry stays within this many units of the origin.
pub const MAX_GEOMETRY: f32 = 65_536.0;
/// The work budget of the converted tree (owner, M18c).
pub const MAX_TREE_NODES: usize = 20_000;
pub const MAX_PATH_SEGMENTS: usize = 50_000;
pub const MAX_LAYERS: usize = 64;
pub const MAX_FILTER_PRIMITIVES: usize = 16;
/// The wall-clock guard, for parsing and again for drawing: generous, a backstop only.
pub const GUARD: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvgError {
    TooLarge,
    NotUtf8,
    Xml,
    TooManyNodes,
    TooDeep,
    /// Scripts, event handlers, foreign content, images, patterns, fonts, @import.
    Forbidden,
    /// A reference outside the document.
    External,
    UseExpansion,
    BadSize,
    Convert,
    OverBudget,
    /// usvg or resvg panicked (tiny-skia asserts on some extreme geometry).
    Panicked,
}

const FORBIDDEN: [&str; 8] = [
    "script",
    "foreignObject",
    "image",
    "feImage",
    "pattern",
    "font",
    "font-face",
    "iframe",
];

/// What drawing a tree costs, counted (the work budget).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Work {
    pub nodes: usize,
    pub segments: usize,
    pub layers: usize,
    pub filter_primitives: usize,
}

impl Work {
    /// Counts `root` and everything under it, clip paths and masks included.
    pub fn of(root: &usvg::Group) -> Self {
        let mut work = Work::default();
        let mut groups = vec![root];
        while let Some(group) = groups.pop() {
            work.nodes += 1;
            if group.should_isolate() {
                work.layers += 1;
            }
            work.filter_primitives += group
                .filters()
                .iter()
                .map(|f| f.primitives().len())
                .sum::<usize>();
            if let Some(clip) = group.clip_path() {
                groups.push(clip.root());
            }
            let mut mask = group.mask();
            while let Some(m) = mask {
                groups.push(m.root());
                mask = m.mask();
            }
            for child in group.children() {
                match child {
                    usvg::Node::Group(g) => groups.push(g),
                    usvg::Node::Path(path) => {
                        work.nodes += 1;
                        work.segments += path.data().verbs().len();
                    }
                    _ => work.nodes += 1,
                }
            }
            // Never count past what is refused anyway.
            if !work.within_budget() {
                break;
            }
        }
        work
    }

    pub fn within_budget(&self) -> bool {
        self.nodes <= MAX_TREE_NODES
            && self.segments <= MAX_PATH_SEGMENTS
            && self.layers <= MAX_LAYERS
            && self.filter_primitives <= MAX_FILTER_PRIMITIVES
    }
}

/// Draws `bytes` into a `px` x `px` pixmap (premultiplied RGBA), fitted and centred.
/// `guard` is the wall-clock backstop ([`GUARD`]); the work budget decides.
pub fn render(bytes: &[u8], px: u32, guard: Duration) -> Result<Pixmap, SvgError> {
    if bytes.len() > MAX_SVG_BYTES {
        return Err(SvgError::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| SvgError::NotUtf8)?;
    let started = Instant::now();
    let options = roxmltree::ParsingOptions {
        allow_dtd: false,
        nodes_limit: MAX_SVG_NODES,
        ..Default::default()
    };
    let doc = roxmltree::Document::parse_with_options(text, options).map_err(|e| match e {
        roxmltree::Error::NodesLimitReached => SvgError::TooManyNodes,
        _ => SvgError::Xml,
    })?;
    check(&doc)?;
    let opt = usvg::Options {
        resources_dir: None,
        image_href_resolver: ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..usvg::Options::default()
    };
    // A panic inside usvg or resvg on a hostile file becomes an error, never the
    // caller's crash (the release profile unwinds; owner, after M14).
    let tree = catch_unwind(AssertUnwindSafe(|| usvg::Tree::from_xmltree(&doc, &opt)))
        .map_err(|_| SvgError::Panicked)?
        .map_err(|_| SvgError::Convert)?;
    let size = tree.size();
    let (w, h) = (size.width(), size.height());
    let ok = |v: f32| v.is_finite() && v > 0.0 && v <= MAX_SVG_SIDE;
    if !ok(w) || !ok(h) {
        return Err(SvgError::BadSize);
    }
    // Geometry far outside the canvas (a circle of radius 1e30) makes tiny-skia index
    // past its buffer (found by the long fuzz run, release builds too): refused before
    // drawing; the catch_unwind below stays as the backstop.
    let b = tree.root().abs_stroke_bounding_box();
    let near = |v: f32| v.is_finite() && v.abs() <= MAX_GEOMETRY;
    if !(near(b.left()) && near(b.top()) && near(b.right()) && near(b.bottom())) {
        return Err(SvgError::BadSize);
    }
    // The deterministic checks first, then the guard.
    if !Work::of(tree.root()).within_budget() {
        return Err(SvgError::OverBudget);
    }
    if started.elapsed() > guard {
        return Err(SvgError::OverBudget);
    }
    let px = px.clamp(1, super::MAX_ICON_PX);
    let mut pixmap = Pixmap::new(px, px).ok_or(SvgError::BadSize)?;
    let scale = px as f32 / w.max(h);
    let transform = Transform::from_row(
        scale,
        0.0,
        0.0,
        scale,
        (px as f32 - w * scale) / 2.0,
        (px as f32 - h * scale) / 2.0,
    );
    let drawing = Instant::now();
    catch_unwind(AssertUnwindSafe(|| {
        resvg::render(&tree, transform, &mut pixmap.as_mut());
    }))
    .map_err(|_| SvgError::Panicked)?;
    if drawing.elapsed() > guard {
        return Err(SvgError::OverBudget);
    }
    Ok(pixmap)
}

/// The checks before usvg sees the document.
fn check(doc: &roxmltree::Document) -> Result<(), SvgError> {
    let mut uses = Vec::new();
    for node in doc.descendants().filter(roxmltree::Node::is_element) {
        if node.ancestors().count() > MAX_SVG_DEPTH {
            return Err(SvgError::TooDeep);
        }
        let tag = node.tag_name().name();
        if FORBIDDEN.contains(&tag) {
            return Err(SvgError::Forbidden);
        }
        if tag == "style" {
            let css: String = node.descendants().filter_map(|n| n.text()).collect();
            if css.to_ascii_lowercase().contains("@import") || !local_urls(&css) {
                return Err(SvgError::Forbidden);
            }
        }
        for attr in node.attributes() {
            let name = attr.name();
            if name.len() > 2 && name[..2].eq_ignore_ascii_case("on") {
                return Err(SvgError::Forbidden);
            }
            if name == "href" && !attr.value().trim_start().starts_with('#') {
                return Err(SvgError::External);
            }
            if !local_urls(attr.value()) {
                return Err(SvgError::External);
            }
        }
        if tag == "use" {
            uses.push(node);
        }
    }
    if uses.len() > MAX_USES {
        return Err(SvgError::UseExpansion);
    }
    let mut copied: u64 = 0;
    for u in uses {
        let target = u
            .attributes()
            .find(|a| a.name() == "href")
            .map(|a| a.value().trim_start().trim_start_matches('#'))
            .and_then(|id| doc.descendants().find(|n| n.attribute("id") == Some(id)));
        let Some(target) = target else { continue };
        if target
            .descendants()
            .any(|n| n.is_element() && n.tag_name().name() == "use")
        {
            return Err(SvgError::UseExpansion);
        }
        copied += target.descendants().count() as u64;
        if copied > u64::from(MAX_SVG_NODES) {
            return Err(SvgError::UseExpansion);
        }
    }
    Ok(())
}

/// Whether every `url(...)` in `text` refers inside the document ("#id").
fn local_urls(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let mut rest = lower.as_str();
    while let Some(i) = rest.find("url(") {
        let after = rest[i + 4..].trim_start_matches([' ', '\t', '\n', '\r', '"', '\'']);
        if !after.starts_with('#') {
            return false;
        }
        rest = &rest[i + 4..];
    }
    true
}
