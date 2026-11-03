// SPDX-License-Identifier: Apache-2.0

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
    time::{Duration, Instant, SystemTime},
};

use smithay::input::keyboard::{Keysym, ModifiersState};

use super::*;
use crate::keybinds::{self, Modifier};

/// A fresh directory under the system temp directory (never the repository).
pub fn scratch_dir(name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "sash-record-test-{}-{}-{name}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn mode_of(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn the_ring_keeps_sixty_seconds_and_at_most_max_items() {
    let mut ring = Ring::new(WINDOW, 1000);
    for s in 0..=120 {
        ring.push(Duration::from_secs(s), s);
    }
    let kept: Vec<u64> = ring.iter().map(|(_, s)| *s).collect();
    assert_eq!(kept.first(), Some(&60), "older than 60 s dropped");
    assert_eq!(kept.last(), Some(&120));
    assert_eq!(ring.len(), 61);

    let mut small = Ring::new(WINDOW, 10);
    for i in 0..100 {
        small.push(Duration::from_millis(i), i);
    }
    assert_eq!(small.len(), 10, "bounded by count within the window");
    assert_eq!(small.iter().next().map(|(_, i)| *i), Some(90));
}

#[test]
fn layout_screenshots_are_limited_to_one_per_second_and_a_hundred() {
    let mut limiter = ShotLimiter::default();
    let ms = Duration::from_millis;
    assert!(limiter.allow(ms(0)));
    assert!(!limiter.allow(ms(500)), "within a second");
    assert!(!limiter.allow(ms(999)));
    assert!(limiter.allow(ms(1000)));
    let mut taken = 2;
    let mut t = 2000;
    while limiter.allow(ms(t)) {
        taken += 1;
        t += 1000;
    }
    assert_eq!(taken, MAX_LAYOUT_SHOTS, "a hundred per session");
    assert!(!limiter.allow(ms(t + 60_000)), "none after the hundredth");
}

fn mods(ctrl: bool, alt: bool, shift: bool, logo: bool) -> ModifiersState {
    ModifiersState {
        ctrl,
        alt,
        shift,
        logo,
        ..Default::default()
    }
}

#[test]
fn printable_keys_are_never_recorded_by_identity() {
    let none = mods(false, false, false, false);
    let shift = mods(false, false, true, false);
    for sym in [
        Keysym::h,
        Keysym::_2,
        Keysym::space,
        Keysym::exclam,
        Keysym::adiaeresis,
    ] {
        assert_eq!(key_label(sym, &none, false), "printable", "{sym:?}");
        assert_eq!(
            key_label(sym, &shift, false),
            "printable",
            "Shift alone: {sym:?}"
        );
    }
    // Non-printing keys and modifiers by name.
    assert_eq!(key_label(Keysym::Return, &none, false), "Return");
    assert_eq!(key_label(Keysym::F11, &none, false), "F11");
    assert_eq!(key_label(Keysym::Super_L, &none, false), "Super_L");
    assert_eq!(key_label(Keysym::Left, &shift, false), "Left");
    // Chords (Ctrl, Alt or Super held) by name.
    assert_eq!(
        key_label(Keysym::c, &mods(true, false, false, false), false),
        "c"
    );
    assert_eq!(
        key_label(Keysym::h, &mods(false, false, false, true), false),
        "h"
    );
    // SASH_DEBUG_KEYS=full: everything by name.
    assert_eq!(key_label(Keysym::h, &none, true), "h");
    assert_eq!(
        mods_json(&mods(true, false, true, true)).to_string(),
        r#"["Ctrl","Shift","Super"]"#
    );
}

#[test]
fn the_hotkey_clashes_with_no_keybind() {
    // mod+F11, alone or with Shift/Ctrl, under either modifier.
    for modifier in [Modifier::Alt, Modifier::Super] {
        for (ctrl, shift) in [(false, false), (false, true), (true, false), (true, true)] {
            let held = match modifier {
                Modifier::Alt => mods(ctrl, true, shift, false),
                Modifier::Super => mods(ctrl, false, shift, true),
            };
            assert_eq!(keybinds::lookup(modifier, &held, Some(Keysym::F11)), None);
        }
    }
}

#[test]
fn the_directory_is_private_and_never_inside_a_git_work_tree() {
    let dir = scratch_dir("private").join("nested").join("record");
    let made = prepare_dir(&dir).unwrap();
    assert_eq!(mode_of(&made), 0o700);
    assert!(
        prepare_dir(&dir).is_ok(),
        "an existing 0700 directory is accepted"
    );

    let loose = scratch_dir("loose");
    fs::create_dir_all(&loose).unwrap();
    fs::set_permissions(&loose, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        prepare_dir(&loose).is_err(),
        "an existing 0755 directory is refused"
    );

    let repo = scratch_dir("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let err = prepare_dir(&repo.join("debug")).unwrap_err();
    assert!(err.contains("git work tree"), "{err}");
    // The repository this test runs in, too.
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("debug-record-test");
    assert!(prepare_dir(&here).is_err());
    assert!(!here.exists());
    for d in [dir.parent().unwrap().parent().unwrap(), &loose, &repo] {
        let _ = fs::remove_dir_all(d);
    }
}

#[test]
fn bundles_are_private() {
    let dir = scratch_dir("bundle");
    let config = Config {
        dir: dir.clone(),
        session: false,
        keys_full: false,
        screenshots_full: false,
    };
    let mut recorder = Recorder::new(config, Instant::now()).unwrap();
    recorder.push("test", vec![("n", 1.into())]);
    recorder.add_shot("test", vec![1, 2, 3]);
    let bundle = recorder
        .write_bundle(Json::obj([("ok", true.into())]))
        .unwrap();
    assert_eq!(mode_of(&dir), 0o700);
    assert_eq!(mode_of(&bundle), 0o700);
    assert_eq!(mode_of(&bundle.join("screenshots")), 0o700);
    for file in ["events.jsonl", "state.json", "log.txt"] {
        assert_eq!(mode_of(&bundle.join(file)), 0o600, "{file}");
    }
    let shots: Vec<_> = fs::read_dir(bundle.join("screenshots")).unwrap().collect();
    assert_eq!(shots.len(), 1);
    let events = fs::read_to_string(bundle.join("events.jsonl")).unwrap();
    assert!(events.contains(r#""kind":"test","n":1"#), "{events}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn old_bundles_are_deleted() {
    let dir = scratch_dir("prune");
    for name in ["bundle-old", "session-old", "bundle-new", "keep-me"] {
        fs::create_dir_all(dir.join(name)).unwrap();
    }
    let eight_days = SystemTime::now() - Duration::from_secs(8 * 24 * 3600);
    for name in ["bundle-old", "session-old", "keep-me"] {
        fs::File::open(dir.join(name))
            .unwrap()
            .set_modified(eight_days)
            .unwrap();
    }
    assert_eq!(prune(&dir, SystemTime::now(), RETENTION), 2);
    let mut left: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(
        left,
        ["bundle-new", "keep-me"],
        "only old bundles and sessions"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn utc_stamps() {
    let at = |s| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
    assert_eq!(utc_stamp(at(0)), "19700101T000000Z");
    assert_eq!(utc_stamp(at(951_868_799)), "20000229T235959Z");
    assert_eq!(utc_stamp(at(1_793_637_066)), "20261102T163106Z");
}

#[test]
fn screenshots_are_scaled_to_at_most_1920_wide() {
    let (w, h) = (5120, 12);
    let pixels = vec![200u8; w * h * 4];
    let (out, sw, sh) = downscale(&pixels, w, h, MAX_SHOT_WIDTH);
    assert_eq!((sw, sh), (1706, 4), "a factor of 3");
    assert_eq!(out.len(), sw * sh * 4);
    assert!(out.iter().all(|&b| b == 200));
    let (_, sw, sh) = downscale(&vec![0u8; 1920 * 2 * 4], 1920, 2, MAX_SHOT_WIDTH);
    assert_eq!((sw, sh), (1920, 2), "1920 wide stays");
    let (_, sw, _) = downscale(&vec![0u8; 3840 * 2 * 4], 3840, 2, MAX_SHOT_WIDTH);
    assert_eq!(sw, 1920);
}

/// Screenshots come only from the offscreen render path: the recorder calls
/// `capture::render_rgba` (an offscreen buffer) and nothing that reads a screen, a
/// host window or a scanout buffer.
#[test]
fn screenshots_use_the_offscreen_path_only() {
    let source = include_str!("mod.rs");
    assert!(source.contains("capture::render_rgba::<R, T>(renderer, state, size)"));
    for forbidden in [
        "copy_framebuffer",
        "screencopy",
        "grim",
        "WinitGraphicsBackend",
        "DrmCompositor",
        "/dev/fb",
        "/dev/dri",
    ] {
        assert!(
            !source.contains(forbidden),
            "record/mod.rs mentions {forbidden}"
        );
    }
    let capture = include_str!("../capture.rs");
    assert!(capture.contains("renderer.create_buffer(Fourcc::Abgr8888, buffer_size)"));
}

#[test]
fn ansi_codes_are_stripped_from_the_log() {
    assert_eq!(
        strip_ansi("\u{1b}[2m2026\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m x\n"),
        "2026  INFO x\n"
    );
}

#[test]
fn config_from_the_environment() {
    let env = |vars: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            vars.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.to_string())
        }
    };
    assert_eq!(Config::from_env(env(&[])), None);
    assert_eq!(Config::from_env(env(&[(DIR_VAR, "")])), None);
    let config = Config::from_env(env(&[
        (DIR_VAR, "/x"),
        (SESSION_VAR, "1"),
        (KEYS_VAR, "full"),
        (SCREENSHOTS_VAR, "full"),
    ]))
    .unwrap();
    assert!(config.session && config.keys_full && config.screenshots_full);
    let plain = Config::from_env(env(&[(DIR_VAR, "/x"), (KEYS_VAR, "yes")])).unwrap();
    assert!(
        !plain.session && !plain.keys_full && !plain.screenshots_full,
        "only \"1\" and \"full\" switch them on; screenshots are outlines by default"
    );
}

#[test]
fn the_cursor_path_is_counted_and_logged_when_it_changes() {
    let dir = scratch_dir("cursor-path");
    let config = Config {
        dir: dir.clone(),
        session: false,
        keys_full: false,
        screenshots_full: false,
    };
    let mut recorder = Recorder::new(config, Instant::now()).unwrap();
    for path in [
        "composited",
        "composited",
        "plane",
        "plane",
        "hidden",
        "plane",
    ] {
        recorder.cursor_path(path);
    }
    let counts = recorder.counters.take();
    let count = |name| counts.iter().find(|(n, _)| *n == name).unwrap().1;
    assert_eq!(count("cursor_plane_frames"), 3);
    assert_eq!(count("cursor_composited_frames"), 2);
    let events: Vec<String> = recorder
        .event_lines()
        .into_iter()
        .filter(|l| l.contains(r#""kind":"cursor_path""#))
        .collect();
    assert_eq!(events.len(), 4, "{events:#?}");
    assert!(events[0].contains(r#""path":"composited","before":null"#));
    assert!(events[1].contains(r#""path":"plane","before":"composited""#));
    assert!(events[2].contains(r#""path":"hidden","before":"plane""#));
    assert!(events[3].contains(r#""path":"plane","before":"hidden""#));
    let _ = fs::remove_dir_all(&dir);
}
