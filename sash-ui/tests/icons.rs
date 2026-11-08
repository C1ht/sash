// SPDX-License-Identifier: Apache-2.0

//! Icons (M17 E3): index.theme parsing, the spec's lookup with inheritance, names and
//! paths from untrusted data, hostile SVG and PNG files, the cache, the Tabler set,
//! symbolic tint contrast, the contact sheet goldens at 1, 1.5 and 2, and the system's
//! Adwaita where it is installed.

mod common;

use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicU32, Ordering},
    time::{Duration, Instant},
};

use proptest::prelude::*;
use sash_ui::{
    Canvas, IconLoader, IconName, IconRaster, Node, Rect, Size, Surface, Symbol, Theme, Ui,
    canvas::to_device,
    icon::{
        IconPath, SymbolIcon, ThemedIcon,
        cache::IconCache,
        index::{self, DirType, Directory},
        lookup::{self, Themes},
        name::{NameError, PathError},
        png,
        svg::{self, SvgError},
    },
    theme::{NON_TEXT_CONTRAST, SURFACES, contrast},
    tree::{Align, Insets},
};

/// A fresh directory under Cargo's per-test temporary directory.
fn scratch() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "icons-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// A plain SVG: a filled square in `colour`.
fn square_svg(colour: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><rect x="2" y="2" width="12" height="12" fill="{colour}"/></svg>"#
    )
}

fn png_bytes(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = ::png::Encoder::new(&mut out, w, h);
        enc.set_color(::png::ColorType::Rgba);
        enc.set_depth(::png::BitDepth::Eight);
        let mut writer = enc.write_header().expect("header");
        let data: Vec<u8> = (0..w * h).flat_map(|_| rgba).collect();
        writer.write_image_data(&data).expect("data");
    }
    out
}

fn write(path: &Path, bytes: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
    fs::write(path, bytes).expect("write");
}

/// A theme `name` under `root` with the given directories (path, Size, Type, extra
/// keys) and parents.
fn theme(root: &Path, name: &str, inherits: &str, dirs: &[(&str, u32, &str, &str)]) {
    let list: Vec<&str> = dirs.iter().map(|d| d.0).collect();
    let mut text = format!(
        "[Icon Theme]\nName={name}\nInherits={inherits}\nDirectories={}\n",
        list.join(",")
    );
    for (path, size, kind, extra) in dirs {
        text += &format!("\n[{path}]\nSize={size}\nType={kind}\n{extra}\n");
    }
    write(&root.join(name).join("index.theme"), text);
}

fn loader(root: &Path, pixmaps: &Path, start: &str) -> IconLoader {
    IconLoader::new(
        vec![root.to_path_buf()],
        vec![pixmaps.to_path_buf()],
        start,
        false,
    )
}

fn name(s: &str) -> IconName {
    IconName::parse(s.as_bytes()).expect("a valid name")
}

/// Whether `raster` is the fallback icon at `px`.
fn is_fallback(loader: &mut IconLoader, raster: &IconRaster, px: u32) -> bool {
    *loader.symbol(Symbol::App, px) == *raster
}

// ---- index.theme ----

#[test]
fn index_theme_parses_the_spec_keys_and_defaults() {
    let text = "# a comment\n[Icon Theme]\nName=Test\nName[da]=Prøve\nInherits=Parent, hicolor\n\
                Directories=16x16/apps,scalable/apps,48x48/apps\nScaledDirectories=16x16@2/apps\n\n\
                [16x16/apps]\nSize=16\nType=Fixed\n[scalable/apps]\nSize=16\nType=Scalable\nMinSize=8\nMaxSize=512\n\
                [48x48/apps]\nSize=48\n[16x16@2/apps]\nSize=16\nScale=2\nType=Fixed\n";
    let idx = index::parse(text.as_bytes()).expect("parses");
    assert_eq!(idx.inherits, vec!["Parent", "hicolor"]);
    assert_eq!(idx.directories.len(), 4);
    let threshold = &idx.directories[2];
    assert_eq!(
        (
            threshold.kind,
            threshold.threshold,
            threshold.min,
            threshold.max
        ),
        (DirType::Threshold, 2, 48, 48),
        "Threshold by default, Threshold=2, Min and Max = Size"
    );
    assert_eq!(idx.directories[1].kind, DirType::Scalable);
    assert_eq!((idx.directories[1].min, idx.directories[1].max), (8, 512));
    assert_eq!(idx.directories[3].scale, 2);
}

#[test]
fn index_theme_refuses_bad_input_without_panicking() {
    assert_eq!(index::parse(b"Name=x\n"), None, "no [Icon Theme] group");
    assert_eq!(index::parse(b""), None);
    let huge = vec![b'a'; index::MAX_INDEX_BYTES + 1];
    assert_eq!(index::parse(&huge), None, "too large");
    // Truncated in the middle of a group: what is complete is kept.
    let full = "[Icon Theme]\nDirectories=a,b\n[a]\nSize=16\n[b]\nSi";
    let idx = index::parse(full.as_bytes()).expect("parses");
    assert_eq!(idx.directories.len(), 1);
    // Directories that would leave the theme, bad sizes and unknown types are dropped.
    let bad = "[Icon Theme]\nInherits=../up,ok,/abs\nDirectories=../x,/etc,ok,zero,big,kind,sc\n\
               [../x]\nSize=16\n[/etc]\nSize=16\n[ok]\nSize=16\n[zero]\nSize=0\n[big]\nSize=99999\n\
               [kind]\nSize=16\nType=Weird\n[sc]\nSize=16\nScale=0\n";
    let idx = index::parse(bad.as_bytes()).expect("parses");
    assert_eq!(idx.inherits, vec!["ok"]);
    assert_eq!(
        idx.directories
            .iter()
            .map(|d| d.path.as_str())
            .collect::<Vec<_>>(),
        vec!["ok"]
    );
    // Binary garbage.
    let garbage: Vec<u8> = (0..20_000u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
        .collect();
    let _ = index::parse(&garbage);
    // A million directories: capped.
    let mut many = String::from("[Icon Theme]\nDirectories=");
    many += &(0..5000)
        .map(|i| format!("d{i}"))
        .collect::<Vec<_>>()
        .join(",");
    many += "\n";
    for i in 0..600 {
        many += &format!("[d{i}]\nSize=16\n");
    }
    let idx = index::parse(many.as_bytes()).expect("parses");
    assert!(idx.directories.len() <= index::MAX_DIRECTORIES);
}

// ---- lookup ----

fn dir(kind: DirType, size: u32, scale: u32, min: u32, max: u32, threshold: u32) -> Directory {
    Directory {
        path: format!("{size}"),
        size,
        scale,
        kind,
        min,
        max,
        threshold,
    }
}

#[test]
fn size_matching_follows_the_spec() {
    let fixed = dir(DirType::Fixed, 24, 1, 24, 24, 2);
    let scalable = dir(DirType::Scalable, 16, 1, 8, 512, 2);
    let threshold = dir(DirType::Threshold, 32, 1, 32, 32, 2);
    let doubled = dir(DirType::Fixed, 24, 2, 24, 24, 2);
    assert!(lookup::matches_size(&fixed, 24, 1));
    assert!(!lookup::matches_size(&fixed, 23, 1));
    assert!(
        !lookup::matches_size(&doubled, 24, 1),
        "the scale must match"
    );
    assert!(lookup::matches_size(&doubled, 24, 2));
    assert!(lookup::matches_size(&scalable, 300, 1));
    assert!(lookup::matches_size(&threshold, 30, 1) && lookup::matches_size(&threshold, 34, 1));
    assert!(!lookup::matches_size(&threshold, 35, 1));
    assert_eq!(lookup::size_distance(&fixed, 48, 1), 24);
    assert_eq!(lookup::size_distance(&doubled, 24, 1), 24);
    assert_eq!(lookup::size_distance(&scalable, 600, 1), 88);
    let dirs = [fixed.clone(), scalable.clone(), doubled.clone()];
    assert_eq!(
        lookup::best(dirs.iter(), 24, 1),
        Some(&fixed),
        "exact first"
    );
    assert_eq!(lookup::best(dirs.iter(), 24, 2), Some(&doubled));
    assert_eq!(
        lookup::best([&fixed].into_iter(), 30, 1),
        Some(&fixed),
        "closest"
    );
}

fn arb_dir() -> impl Strategy<Value = Directory> {
    (0u8..3, 1u32..300, 1u32..4, 1u32..300, 1u32..300, 0u32..10).prop_map(
        |(k, size, scale, a, b, threshold)| Directory {
            path: String::new(),
            size,
            scale,
            kind: [DirType::Fixed, DirType::Scalable, DirType::Threshold][k as usize],
            min: a.min(b),
            max: a.max(b),
            threshold,
        },
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn size_matching_picks_an_exact_match_else_the_nearest(
        dirs in proptest::collection::vec(arb_dir(), 1..12),
        size in 1u32..600,
        scale in 1u32..4,
    ) {
        let chosen = lookup::best(dirs.iter(), size, scale).expect("one of them");
        prop_assert!(dirs.iter().any(|d| std::ptr::eq(d, chosen)));
        if let Some(first) = dirs.iter().find(|d| lookup::matches_size(d, size, scale)) {
            prop_assert!(std::ptr::eq(first, chosen), "the first exact match");
        } else {
            let min = dirs.iter().map(|d| lookup::size_distance(d, size, scale)).min().unwrap();
            prop_assert_eq!(lookup::size_distance(chosen, size, scale), min);
        }
        // Deterministic.
        let again = lookup::best(dirs.iter(), size, scale).unwrap();
        prop_assert!(std::ptr::eq(again, chosen));
    }
}

#[test]
fn lookup_uses_the_theme_then_its_parents_then_hicolor_then_pixmaps() {
    let root = scratch();
    let pix = root.join("pixmaps");
    theme(
        &root,
        "Top",
        "Mid",
        &[("16", 16, "Fixed", ""), ("48", 48, "Fixed", "")],
    );
    theme(&root, "Mid", "", &[("32", 32, "Threshold", "")]);
    theme(&root, "hicolor", "", &[("64", 64, "Fixed", "")]);
    write(&root.join("Top/16/both.svg"), square_svg("red"));
    write(&root.join("Top/48/both.svg"), square_svg("red"));
    write(
        &root.join("Mid/32/mid.png"),
        png_bytes(32, 32, [0, 255, 0, 255]),
    );
    write(&root.join("hicolor/64/hi.svg"), square_svg("blue"));
    write(&pix.join("pix.png"), png_bytes(8, 8, [9, 9, 9, 255]));
    let mut themes = Themes::new(vec![root.clone()], vec![pix.clone()]);
    assert_eq!(themes.chain("Top"), vec!["Top", "Mid", "hicolor"]);
    assert_eq!(
        themes.find(&name("both"), 48, 1, "Top"),
        Some(root.join("Top/48/both.svg"))
    );
    assert_eq!(
        themes.find(&name("both"), 20, 1, "Top"),
        Some(root.join("Top/16/both.svg")),
        "nearest size"
    );
    assert_eq!(
        themes.find(&name("mid"), 24, 1, "Top"),
        Some(root.join("Mid/32/mid.png"))
    );
    assert_eq!(
        themes.find(&name("hi"), 24, 1, "Top"),
        Some(root.join("hicolor/64/hi.svg"))
    );
    assert_eq!(
        themes.find(&name("pix"), 24, 1, "Top"),
        Some(pix.join("pix.png"))
    );
    assert_eq!(themes.find(&name("nothing"), 24, 1, "Top"), None);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn inheritance_loops_are_refused_and_depth_is_capped() {
    let root = scratch();
    theme(&root, "A", "B", &[("16", 16, "Fixed", "")]);
    theme(&root, "B", "A, B", &[("16", 16, "Fixed", "")]);
    let mut themes = Themes::new(vec![root.clone()], vec![]);
    assert_eq!(themes.chain("A"), vec!["A", "B"], "a loop ends");
    // A chain twenty deep: only MAX_DEPTH themes are searched.
    for i in 0..20 {
        theme(
            &root,
            &format!("T{i}"),
            &format!("T{}", i + 1),
            &[("16", 16, "Fixed", "")],
        );
    }
    write(&root.join("T15/16/deep.svg"), square_svg("red"));
    write(&root.join("T3/16/shallow.svg"), square_svg("red"));
    let mut themes = Themes::new(vec![root.clone()], vec![]);
    assert_eq!(themes.chain("T0").len(), lookup::MAX_DEPTH);
    assert!(themes.find(&name("shallow"), 16, 1, "T0").is_some());
    assert_eq!(themes.find(&name("deep"), 16, 1, "T0"), None);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn icon_names_are_names_never_paths() {
    for bad in [
        &b""[..],
        b"../../etc/passwd",
        b"a/b",
        b"..",
        b".",
        b".hidden",
        b"x\0y",
        b"tab\there",
        b"back\\slash",
        &[0xff, 0xfe],
    ] {
        assert!(IconName::parse(bad).is_err(), "{bad:?} accepted");
    }
    assert_eq!(IconName::parse(&[b'a'; 256]), Err(NameError::TooLong));
    assert!(IconName::parse(b"org.gnome.Nautilus").is_ok());
    assert_eq!(
        IconName::parse(b"firefox.png").expect("name"),
        name("firefox"),
        "a stray extension is dropped"
    );
    assert!(name("audio-volume-high-symbolic").is_symbolic());
}

#[test]
fn absolute_icon_paths_are_checked_separately() {
    let root = scratch();
    let outside = scratch();
    write(&root.join("app.svg"), square_svg("red"));
    write(&root.join("note.txt"), "x");
    write(&outside.join("evil.svg"), square_svg("red"));
    std::os::unix::fs::symlink(outside.join("evil.svg"), root.join("link.svg")).expect("link");
    let roots = vec![root.clone()];
    let check = |p: &Path| IconPath::check(p.as_os_str().as_encoded_bytes(), &roots, false);
    assert!(check(&root.join("app.svg")).is_ok());
    assert_eq!(check(Path::new("app.svg")), Err(PathError::NotAbsolute));
    assert_eq!(
        check(
            &root
                .join("../")
                .join(root.file_name().unwrap())
                .join("app.svg")
        ),
        Err(PathError::OutsideRoots),
        "no .."
    );
    assert_eq!(
        check(&outside.join("evil.svg")),
        Err(PathError::OutsideRoots)
    );
    assert_eq!(
        check(&root.join("link.svg")),
        Err(PathError::OutsideRoots),
        "a link out"
    );
    assert_eq!(check(&root.join("note.txt")), Err(PathError::NotPngOrSvg));
    assert_eq!(check(&root.join("missing.svg")), Err(PathError::NotAFile));
    assert_eq!(
        IconPath::check(
            root.join("app.svg").as_os_str().as_encoded_bytes(),
            &roots,
            true
        ),
        Err(PathError::NotRootOwned),
        "test files are not root's"
    );
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(outside);
}

#[test]
fn a_theme_file_linking_out_of_the_roots_is_not_read() {
    let root = scratch();
    let outside = scratch();
    theme(&root, "T", "", &[("16", 16, "Fixed", "")]);
    write(&outside.join("secret.svg"), square_svg("red"));
    fs::create_dir_all(root.join("T/16")).expect("dir");
    std::os::unix::fs::symlink(outside.join("secret.svg"), root.join("T/16/out.svg"))
        .expect("link");
    let mut l = loader(&root, &root.join("none"), "T");
    let raster = l.named(&name("out"), 16, 1.0);
    assert!(is_fallback(&mut l, &raster, 16));
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(outside);
}

#[test]
fn names_and_paths_never_print() {
    const MARKER: &str = "app-icon-name-marker-4711";
    let n = name(MARKER);
    let printed = format!("{n:?} {:?}", IconName::parse(b"a/b"));
    assert!(!printed.contains(MARKER), "{printed}");
}

// ---- SVG ----

fn svg_err(text: &str) -> SvgError {
    svg::render(text.as_bytes(), 24, svg::GUARD).expect_err("refused")
}

const HEAD: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="24" height="24" viewBox="0 0 24 24">"#;

#[test]
fn hostile_svg_files_are_refused() {
    let laughs = r#"<?xml version="1.0"?><!DOCTYPE lolz [<!ENTITY lol "lol"><!ENTITY lol2 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;"><!ENTITY lol3 "&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;"><!ENTITY lol4 "&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;">]><svg xmlns="http://www.w3.org/2000/svg"><text>&lol4;</text></svg>"#;
    assert_eq!(svg_err(laughs), SvgError::Xml, "no DTD at all");
    let huge = r#"<svg xmlns="http://www.w3.org/2000/svg" width="1000000000" height="1000000000"><rect width="10" height="10"/></svg>"#;
    assert_eq!(svg_err(huge), SvgError::BadSize);
    for (body, want) in [
        (
            r#"<image href="file:///etc/passwd" width="24" height="24"/>"#,
            SvgError::Forbidden,
        ),
        (r#"<use href="other.svg#a"/>"#, SvgError::External),
        (
            r#"<use xlink:href="http://example.invalid/a.svg#b"/>"#,
            SvgError::External,
        ),
        (
            r#"<rect width="9" height="9" fill="url(http://example.invalid/p)"/>"#,
            SvgError::External,
        ),
        (
            r#"<rect width="9" height="9" style="fill: url( 'file:///x' )"/>"#,
            SvgError::External,
        ),
        (
            r#"<style>@import url(http://example.invalid/a.css);</style>"#,
            SvgError::Forbidden,
        ),
        (r#"<script>alert(1)</script>"#, SvgError::Forbidden),
        (
            r#"<rect width="9" height="9" onload="alert(1)"/>"#,
            SvgError::Forbidden,
        ),
        (
            r#"<foreignObject><div/></foreignObject>"#,
            SvgError::Forbidden,
        ),
        (
            r#"<pattern id="p" width="0.001" height="0.001"/><rect width="24" height="24" fill="url(#p)"/>"#,
            SvgError::Forbidden,
        ),
        (r#"<font-face font-family="x"/>"#, SvgError::Forbidden),
        (
            r#"<filter id="f"><feImage href="file:///etc/hosts"/></filter>"#,
            SvgError::Forbidden,
        ),
    ] {
        assert_eq!(svg_err(&format!("{HEAD}{body}</svg>")), want, "{body}");
    }
    let deep = format!("{HEAD}{}{}</svg>", "<g>".repeat(100), "</g>".repeat(100));
    assert_eq!(svg_err(&deep), SvgError::TooDeep);
    let many = format!(
        "{HEAD}{}</svg>",
        "<rect width=\"1\" height=\"1\"/>".repeat(25_000)
    );
    assert!(matches!(
        svg_err(&many),
        SvgError::TooManyNodes | SvgError::TooLarge
    ));
    let big = format!("{HEAD}<!--{}--></svg>", "x".repeat(svg::MAX_SVG_BYTES));
    assert_eq!(svg_err(&big), SvgError::TooLarge);
    // A use bomb: few elements, many copies.
    let group: String = "<rect width=\"1\" height=\"1\"/>".repeat(1500);
    let uses: String = (0..16).map(|_| "<use href=\"#g\"/>").collect();
    let bomb = format!("{HEAD}<defs><g id=\"g\">{group}</g></defs>{uses}</svg>");
    assert_eq!(svg_err(&bomb), SvgError::UseExpansion);
    let nested = format!(
        "{HEAD}<defs><g id=\"a\"><rect width=\"1\" height=\"1\"/></g><g id=\"b\"><use href=\"#a\"/></g></defs><use href=\"#b\"/></svg>"
    );
    assert_eq!(svg_err(&nested), SvgError::UseExpansion);
    let too_many_uses = format!(
        "{HEAD}<defs><g id=\"a\"/></defs>{}</svg>",
        "<use href=\"#a\"/>".repeat(svg::MAX_USES + 1)
    );
    assert_eq!(svg_err(&too_many_uses), SvgError::UseExpansion);
    assert_eq!(svg_err("not svg at all"), SvgError::Xml);
    assert_eq!(
        svg::render(&[0xff, 0xfe, 0x00], 24, svg::GUARD).unwrap_err(),
        SvgError::NotUtf8
    );
    // Over the wall-clock guard (a zero guard; the real one is a generous backstop).
    let ok = format!("{HEAD}<circle cx=\"12\" cy=\"12\" r=\"9\"/></svg>");
    assert_eq!(
        svg::render(ok.as_bytes(), 24, Duration::ZERO).unwrap_err(),
        SvgError::OverBudget
    );
    assert!(svg::render(ok.as_bytes(), 24, svg::GUARD).is_ok());
}

/// The work budget (owner, M18c) is deterministic: past a cap a file is refused as over
/// budget whatever the machine's load, even with no wall-clock guard at all, and a file
/// just inside the caps draws.
#[test]
fn the_work_budget_is_deterministic() {
    let unlimited = Duration::from_secs(3600);
    let over = [
        (
            "path segments",
            format!(
                "{HEAD}<path d=\"M0 0{}\"/></svg>",
                "L1 1".repeat(svg::MAX_PATH_SEGMENTS)
            ),
        ),
        (
            "layers",
            format!(
                "{HEAD}{}</svg>",
                "<g opacity=\"0.5\"><rect width=\"1\" height=\"1\"/></g>"
                    .repeat(svg::MAX_LAYERS + 1)
            ),
        ),
        (
            "filter primitives",
            format!(
                "{HEAD}<filter id=\"f\">{}</filter><rect width=\"9\" height=\"9\" filter=\"url(#f)\"/></svg>",
                "<feFlood flood-color=\"red\"/>".repeat(svg::MAX_FILTER_PRIMITIVES + 1)
            ),
        ),
    ];
    for (what, text) in &over {
        for guard in [svg::GUARD, unlimited] {
            assert_eq!(
                svg::render(text.as_bytes(), 24, guard).err(),
                Some(SvgError::OverBudget),
                "{what}"
            );
        }
    }
    let inside = format!(
        "{HEAD}<path d=\"M0 0{}\"/></svg>",
        "L1 1".repeat(svg::MAX_PATH_SEGMENTS - 1)
    );
    assert!(svg::render(inside.as_bytes(), 24, svg::GUARD).is_ok());
}

#[test]
fn extreme_geometry_never_crashes() {
    // Refused before drawing: tiny-skia indexes past its buffer on these (release
    // builds too; found by the long fuzz run).
    for body in [
        r#"<circle r="1e30"/>"#,
        r#"<path d="M0 0L1e38 1e38L-1e38 1e38Z"/>"#,
        r#"<rect width="1e30" height="1e30" transform="rotate(45)"/>"#,
        r#"<g transform="translate(1e9 0)"><rect width="1" height="1"/></g>"#,
    ] {
        let result = svg::render(format!("{HEAD}{body}</svg>").as_bytes(), 24, svg::GUARD);
        assert_eq!(result.err(), Some(SvgError::BadSize), "{body}");
    }
}

#[test]
fn text_in_an_svg_reads_no_font() {
    // sash-ui builds usvg without text: no font is ever looked up, nothing is drawn.
    let text = format!(
        "{HEAD}<text x=\"2\" y=\"20\" font-family=\"url(#x)\" font-size=\"20\">Hi</text></svg>"
    );
    let pixmap = svg::render(text.as_bytes(), 24, svg::GUARD).expect("renders");
    assert!(pixmap.data().iter().all(|&b| b == 0));
}

#[test]
fn a_hostile_theme_file_gives_the_fallback_once() {
    let root = scratch();
    theme(&root, "T", "", &[("16", 16, "Fixed", "")]);
    write(&root.join("T/16/bad.svg"), format!("{HEAD}<script/></svg>"));
    write(
        &root.join("T/16/broken.png"),
        b"\x89PNG\r\n\x1a\nnot really",
    );
    let mut l = loader(&root, &root.join("none"), "T");
    for icon in ["bad", "broken", "missing"] {
        let first = l.named(&name(icon), 16, 1.0);
        assert!(is_fallback(&mut l, &first, 16), "{icon}");
    }
    let failed = l.stats().failed;
    for icon in ["bad", "broken", "missing"] {
        l.named(&name(icon), 16, 1.0);
    }
    assert_eq!(
        l.stats().failed,
        failed,
        "failures are remembered, not retried"
    );
    let _ = fs::remove_dir_all(root);
}

// ---- PNG ----

#[test]
fn png_icons_decode_scale_and_refuse_bad_files() {
    let ok = png_bytes(48, 48, [255, 0, 0, 128]);
    let pixmap = png::render(&ok, 24).expect("decodes");
    let p = pixmap.pixel(12, 12).expect("pixel");
    assert_eq!((p.red(), p.alpha()), (128, 128), "premultiplied");
    assert!(png::render(&ok[..ok.len() / 2], 24).is_err(), "truncated");
    assert!(png::render(b"garbage", 24).is_err());
    assert!(
        png::render(&vec![0; png::MAX_PNG_BYTES + 1], 24).is_err(),
        "too large"
    );
    // A header claiming 100000 x 100000 pixels: refused before decoding.
    let mut claim = png_bytes(1, 1, [0; 4]);
    claim[16..20].copy_from_slice(&100_000u32.to_be_bytes());
    claim[20..24].copy_from_slice(&100_000u32.to_be_bytes());
    assert!(png::render(&claim, 24).is_err());
    // 1024 x 1024 drawn at 24 px.
    let large = png_bytes(1024, 1024, [0, 0, 255, 255]);
    let px = png::render(&large, 24)
        .expect("decodes")
        .pixel(12, 12)
        .expect("pixel");
    assert_eq!((px.blue(), px.alpha()), (255, 255));
}

// ---- cache ----

#[test]
fn cache_eviction_never_frees_an_icon_in_use() {
    let mut cache: IconCache<u32> = IconCache::new(1000);
    let held = Rc::new(IconRaster::Mask(vec![1; 400]));
    cache.insert(0, held.clone());
    for k in 1..10 {
        cache.insert(k, Rc::new(IconRaster::Mask(vec![k as u8; 400])));
        assert!(cache.contains(&0), "the held icon stays");
        assert!(cache.bytes() <= 1000 || cache.len() == 1);
    }
    assert!(Rc::ptr_eq(&cache.get(&0).expect("held"), &held));
    // Least recently used goes first.
    let mut cache: IconCache<u32> = IconCache::new(1000);
    for k in 0..2 {
        cache.insert(k, Rc::new(IconRaster::Mask(vec![0; 400])));
    }
    cache.get(&0);
    cache.insert(2, Rc::new(IconRaster::Mask(vec![0; 400])));
    assert!(cache.contains(&0) && !cache.contains(&1) && cache.contains(&2));
    // Only held icons left: the cache stays above its cap until they are let go.
    let mut cache: IconCache<u32> = IconCache::new(100);
    let a = Rc::new(IconRaster::Mask(vec![0; 400]));
    cache.insert(0, a.clone());
    assert_eq!(cache.bytes(), 400);
    drop(a);
    cache.insert(1, Rc::new(IconRaster::Mask(vec![0; 10])));
    assert!(!cache.contains(&0));
}

#[test]
fn the_loader_cache_is_byte_capped() {
    let mut l = IconLoader::new(vec![], vec![], "none", false);
    for px in 1..200u32 {
        l.symbol(Symbol::Power, px);
    }
    assert!(l.cache_bytes() <= sash_ui::icon::loader::CACHE_BYTES);
}

// ---- the Tabler set ----

#[test]
fn every_tabler_glyph_renders_on_its_grid() {
    let mut l = IconLoader::new(vec![], vec![], "none", false);
    for symbol in Symbol::ALL {
        let svg = String::from_utf8(symbol.svg().to_vec()).expect("utf-8");
        assert!(
            svg.contains(r#"viewBox="0 0 24 24""#),
            "{symbol:?}: the 24 px grid"
        );
        assert!(
            svg.contains(r#"stroke-width="2""#),
            "{symbol:?}: the 2 px stroke"
        );
        for px in [24, 48] {
            let raster = l.symbol(symbol, px);
            let IconRaster::Mask(alpha) = &*raster else {
                panic!("{symbol:?} is not a mask")
            };
            assert!(alpha.iter().any(|&a| a > 200), "{symbol:?} at {px}");
        }
    }
    assert_eq!(l.stats().failed, 0);
}

/// The contact sheet: every glyph as a 32 px icon button, two rows.
fn sheet(theme: Theme, loader: &Rc<RefCell<IconLoader>>) -> (Ui, i32, i32) {
    let per_row = 10;
    let (w, h) = (per_row * 40 + 8, 2 * 40 + 8);
    let mut ui = Ui::new(
        Node::column()
            .padding(Insets::all(4))
            .width(Size::Flex(1))
            .height(Size::Flex(1)),
        theme,
        Surface::Raised,
    );
    let root = ui.root();
    for row in Symbol::ALL.chunks(per_row as usize) {
        let r = ui.add(
            root,
            Node::row()
                .spacing(8)
                .padding(Insets::all(4))
                .align(Align::Center)
                .group(),
        );
        for &symbol in row {
            ui.add(
                r,
                Node::icon_button(SymbolIcon::new(loader.clone(), symbol), symbol.label())
                    .icon_size(24),
            );
        }
    }
    ui.resize(w, h);
    assert!(ui.issues().is_empty());
    (ui, w, h)
}

fn draw(ui: &mut Ui, w: i32, h: i32, scale: f32) -> (Vec<u8>, u32, u32) {
    ui.set_scale(scale);
    let full = to_device(Rect::sized(w, h), scale);
    let (dw, dh) = (full.w as u32, full.h as u32);
    let mut pixels = vec![0; (dw * dh * 4) as usize];
    let mut canvas = Canvas::new(&mut pixels, dw, dh, full)
        .expect("canvas")
        .with_scale(scale);
    ui.draw(&mut canvas);
    (pixels, dw, dh)
}

#[test]
fn contact_sheet_goldens_at_scales_1_1_5_and_2() {
    let loader = Rc::new(RefCell::new(IconLoader::new(vec![], vec![], "none", false)));
    let (mut ui, w, h) = sheet(Theme::dark(), &loader);
    for scale in [1.0f32, 1.5, 2.0] {
        let (pixels, dw, dh) = draw(&mut ui, w, h, scale);
        let tag = format!("{scale}").replace('.', "_");
        common::golden(&format!("icons-contact-x{tag}"), &pixels, dw, dh);
    }
}

#[test]
fn symbolic_tint_passes_3_to_1_on_every_surface() {
    for theme in [Theme::dark(), Theme::light()] {
        for surface in SURFACES {
            let loader = Rc::new(RefCell::new(IconLoader::new(vec![], vec![], "none", false)));
            let mut ui = Ui::new(Node::row(), theme, surface);
            let root = ui.root();
            ui.add(
                root,
                Node::icon(SymbolIcon::new(loader.clone(), Symbol::Settings)).icon_size(24),
            );
            ui.resize(24, 24);
            let (pixels, _, _) = draw(&mut ui, 24, 24, 1.0);
            let bg = theme.surface_color(surface);
            // The most-covered pixel is the tint over the surface.
            let best = pixels
                .chunks(4)
                .map(|p| sash_ui::Color {
                    r: p[0],
                    g: p[1],
                    b: p[2],
                    a: 255,
                })
                .map(|c| contrast(c, bg))
                .fold(0.0f64, f64::max);
            assert!(
                best >= NON_TEXT_CONTRAST,
                "{:?} on {surface:?}: {best:.2}",
                theme.tokens.mode
            );
        }
    }
}

#[test]
fn the_icon_widget_loads_theme_icons_by_name() {
    let root = scratch();
    theme(&root, "T", "", &[("24", 24, "Fixed", "")]);
    write(
        &root.join("T/24/app.png"),
        png_bytes(24, 24, [0, 200, 0, 255]),
    );
    let loader = Rc::new(RefCell::new(loader(&root, &root.join("none"), "T")));
    let mut ui = Ui::new(Node::row(), Theme::dark(), Surface::Raised);
    let r = ui.root();
    ui.add(
        r,
        Node::icon(ThemedIcon::new(loader.clone(), name("app"))).icon_size(24),
    );
    ui.add(
        r,
        Node::icon(ThemedIcon::new(loader.clone(), name("no-such-icon"))).icon_size(24),
    );
    ui.resize(48, 24);
    let (pixels, w, _) = draw(&mut ui, 48, 24, 1.0);
    let at = |x: u32, y: u32| &pixels[((y * w + x) * 4) as usize..][..4];
    assert_eq!(at(12, 12), &[0, 200, 0, 255], "the theme's PNG, in colour");
    assert_eq!(
        loader.borrow().stats().fallbacks,
        1,
        "the missing one: fallback"
    );
    let _ = fs::remove_dir_all(root);
}

// ---- the system's Adwaita ----

#[test]
fn system_adwaita_icons_load_where_installed() {
    if !Path::new("/usr/share/icons/Adwaita/index.theme").exists() {
        println!("  skipped: Adwaita not installed in /usr/share/icons");
        return;
    }
    let mut l = IconLoader::system();
    for icon in [
        "folder",
        "audio-volume-high-symbolic",
        "system-shutdown-symbolic",
    ] {
        let raster = l.named(&name(icon), 24, 1.0);
        assert!(!is_fallback(&mut l, &raster, 24), "{icon} not found");
        assert_eq!(
            matches!(&*raster, IconRaster::Mask(_)),
            icon.ends_with("-symbolic"),
            "{icon}: symbolic icons are masks"
        );
    }
}

// ---- fuzz-style ----

fn fuzz_one(bytes: &[u8]) {
    let _ = index::parse(bytes);
    let _ = svg::render(bytes, 16, svg::GUARD);
    let _ = png::render(bytes, 16);
    let _ = IconName::parse(bytes);
    let _ = IconPath::check(bytes, &[PathBuf::from("/nonexistent")], true);
}

/// SVG-ish input: random tags, attributes and references.
fn svgish() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        Just("<g>".to_string()),
        Just("</g>".to_string()),
        Just("<use href=\"#a\"/>".to_string()),
        Just("<rect id=\"a\" width=\"5\" height=\"5\"/>".to_string()),
        Just("<path d=\"M0 0L24 24Z\"/>".to_string()),
        Just("<circle r=\"1e30\"/>".to_string()),
        Just("<filter id=\"f\"><feGaussianBlur stdDeviation=\"1e9\"/></filter>".to_string()),
        Just("<rect width=\"24\" height=\"24\" filter=\"url(#f)\"/>".to_string()),
        Just("<!DOCTYPE x>".to_string()),
        Just("&amp;".to_string()),
        "[a-z<>/=\" #]{0,12}".prop_map(|s| s),
    ];
    proptest::collection::vec(piece, 0..40).prop_map(|v| format!("{HEAD}{}</svg>", v.concat()))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2_000))]

    #[test]
    fn no_input_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        fuzz_one(&bytes);
    }

    #[test]
    fn no_svgish_input_panics(text in svgish()) {
        let _ = svg::render(text.as_bytes(), 24, svg::GUARD);
    }
}

/// The long run (owner, M17 E3: a fuzz-style long proptest on stable). Ignored by
/// default; `cargo test --release -p sash-ui --test icons -- --ignored long_fuzz`.
#[test]
#[ignore]
fn long_fuzz() {
    let cases = 200_000;
    let started = Instant::now();
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases,
        ..ProptestConfig::default()
    });
    runner
        .run(&proptest::collection::vec(any::<u8>(), 0..2048), |b| {
            fuzz_one(&b);
            Ok(())
        })
        .expect("bytes");
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases,
        ..ProptestConfig::default()
    });
    runner
        .run(&svgish(), |t| {
            let _ = svg::render(t.as_bytes(), 24, svg::GUARD);
            Ok(())
        })
        .expect("svg");
    println!(
        "long fuzz: {cases} byte cases and {cases} SVG-ish cases in {:?}",
        started.elapsed()
    );
}

// ---- measurements ----

/// Load and rasterisation time per icon at 24 and 48 px, cache memory. Ignored by
/// default; run in release: `cargo test --release -p sash-ui --test icons -- --ignored
/// --nocapture measure`.
#[test]
#[ignore]
fn measure_icon_costs() {
    let pct = |v: &mut Vec<Duration>, p: f64| {
        v.sort();
        v[((v.len() - 1) as f64 * p) as usize]
    };
    for px in [24u32, 48] {
        let mut times = Vec::new();
        for _ in 0..20 {
            for symbol in Symbol::ALL {
                let t = Instant::now();
                svg::render(symbol.svg(), px, svg::GUARD).expect("renders");
                times.push(t.elapsed());
            }
        }
        println!(
            "Tabler SVG at {px} px: p50 {:?} p99 {:?} ({} renders)",
            pct(&mut times, 0.5),
            pct(&mut times, 0.99),
            times.len()
        );
    }
    let mut l = IconLoader::new(vec![], vec![], "none", false);
    for px in [24, 48] {
        for symbol in Symbol::ALL {
            l.symbol(symbol, px);
        }
    }
    println!(
        "cache: {} icons, {} bytes (20 symbols at 24 and 48 px)",
        l.cached(),
        l.cache_bytes()
    );
    if !Path::new("/usr/share/icons/Adwaita/index.theme").exists() {
        println!("  skipped: Adwaita not installed");
        return;
    }
    for (icon, px) in [
        ("folder", 24u32),
        ("folder", 48),
        ("audio-volume-high-symbolic", 24),
        ("audio-volume-high-symbolic", 48),
        ("user-trash", 48),
    ] {
        let mut times = Vec::new();
        for _ in 0..20 {
            // A fresh loader each time: the lookup, the read and the rasterisation.
            let mut l = IconLoader::system();
            let t = Instant::now();
            l.named(&name(icon), px, 1.0);
            times.push(t.elapsed());
        }
        // The theme already read (index.theme, listings), this icon not cached.
        let mut theme_read = Vec::new();
        for _ in 0..20 {
            let mut l = IconLoader::system();
            l.named(&name("edit-copy"), 16, 1.0);
            let t = Instant::now();
            l.named(&name(icon), px, 1.0);
            theme_read.push(t.elapsed());
        }
        let mut warm = Vec::new();
        let mut l = IconLoader::system();
        l.named(&name(icon), px, 1.0);
        for _ in 0..20 {
            let t = Instant::now();
            l.named(&name(icon), px, 1.0);
            warm.push(t.elapsed());
        }
        println!(
            "Adwaita {icon} at {px} px: cold p50 {:?} p99 {:?}; theme read p50 {:?} p99 {:?}; cached p50 {:?}",
            pct(&mut times, 0.5),
            pct(&mut times, 0.99),
            pct(&mut theme_read, 0.5),
            pct(&mut theme_read, 0.99),
            pct(&mut warm, 0.5)
        );
    }
}

/// How the system's own SVG icons fare under the limits: every .svg under
/// /usr/share/icons, counted by result. Ignored by default; `cargo test --release -p
/// sash-ui --test icons -- --ignored --nocapture survey`.
#[test]
#[ignore]
fn survey_system_svgs() {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "svg") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(Path::new("/usr/share/icons"), &mut files);
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    let mut slowest = Duration::ZERO;
    for f in &files {
        let Ok(bytes) = fs::read(f) else { continue };
        let t = Instant::now();
        let result = svg::render(&bytes, 48, svg::GUARD);
        slowest = slowest.max(t.elapsed());
        let theme = f
            .strip_prefix("/usr/share/icons")
            .ok()
            .and_then(|p| p.components().next())
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_default();
        let key = match result {
            Ok(_) => format!("{theme}: drawn"),
            Err(e) => format!("{theme}: {e:?}"),
        };
        *counts.entry(key).or_default() += 1;
    }
    println!(
        "{} SVG files under /usr/share/icons: {counts:?}; slowest {slowest:?}",
        files.len()
    );
}
