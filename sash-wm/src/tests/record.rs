// SPDX-License-Identifier: Apache-2.0

//! Debug recording end to end (feature `debug-record`): what a bundle contains after
//! real input on the headless harness (modifier Alt).

use std::{fs, path::Path, time::Instant};

use smithay::backend::input::KeyState;

use super::{client::TestClient, harness::Harness};
use crate::record::{self, Config, Recorder};

/// Evdev codes.
const KEY_H: u32 = 35;
const KEY_U: u32 = 22;
const KEY_N: u32 = 49;
const KEY_T: u32 = 20;
const KEY_E: u32 = 18;
const KEY_R: u32 = 19;
const KEY_2: u32 = 3;
const KEY_ENTER: u32 = 28;
const KEY_F11: u32 = 87;
const KEY_ALT: u32 = 56;
const KEY_F: u32 = 33;

fn files_under(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn type_keys(h: &mut Harness, keys: &[u32]) {
    for &key in keys {
        h.key(key, KeyState::Pressed);
        h.key(key, KeyState::Released);
    }
}

/// "hunter2" typed into a focused window, then mod+F11: the bundle has the key
/// events, but no file contains the text, nor any of its letters as a key name.
#[test]
fn a_typed_password_never_reaches_a_bundle() {
    let dir = record::tests::scratch_dir("hunter2");
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    let bundle_dir = dir.clone();
    harness.run_with_remote(move |server| {
        server.on_server(move |h| {
            let config = Config {
                dir: bundle_dir,
                session: false,
                keys_full: false,
                screenshots_full: false,
            };
            h.server.state.recorder = Some(Recorder::new(config, Instant::now()).unwrap());
        });
        let mut c = TestClient::connect(&socket);
        let w = c.map_window();
        server.on_server(|h| {
            // A layout operation: mod+F floats the window (a chord: recorded by name).
            h.chord(&[KEY_ALT, KEY_F]);
        });
        // The client answers the configure with a commit.
        c.roundtrip();
        c.ack_and_commit(&w);
        c.roundtrip();
        server.on_server(|h| {
            type_keys(
                h,
                &[KEY_H, KEY_U, KEY_N, KEY_T, KEY_E, KEY_R, KEY_2, KEY_ENTER],
            );
            // mod+F11 marks the bug; the bundle is written right away here (no frame
            // comes in the headless harness, which has no renderer).
            h.chord(&[KEY_ALT, KEY_F11]);
            record::flush_bundle(&mut h.server.state);
        });
        c.roundtrip();
        // The client got the typed keys, but neither F11 nor its release.
        assert_eq!(
            c.events.keys_pressed,
            vec![
                KEY_ALT, KEY_H, KEY_U, KEY_N, KEY_T, KEY_E, KEY_R, KEY_2, KEY_ENTER, KEY_ALT
            ],
            "mod+F and mod+F11 are not delivered"
        );
    });

    let bundles: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("bundle-")
        })
        .collect();
    assert_eq!(bundles.len(), 1, "one bundle");
    let mut files = Vec::new();
    files_under(&bundles[0], &mut files);
    let mut all = String::new();
    for file in &files {
        all.push_str(&String::from_utf8_lossy(&fs::read(file).unwrap()));
    }
    assert!(!all.contains("hunter2"), "the password is in the bundle");
    for letter in ["h", "u", "n", "t", "e", "r", "2"] {
        assert!(
            !all.contains(&format!(r#""key":"{letter}""#)),
            "key {letter} recorded by name"
        );
    }
    // The pictures: window outlines only. Every pixel of every screenshot is one of
    // the outline palette's colours, so nothing a client drew (or showed as text) is
    // in them.
    let pictures: Vec<_> = files
        .iter()
        .filter(|f| f.extension().is_some_and(|e| e == "png"))
        .collect();
    assert!(!pictures.is_empty(), "the hotkey takes a screenshot");
    for picture in &pictures {
        let decoder = png::Decoder::new(std::io::BufReader::new(fs::File::open(picture).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        assert_eq!(info.color_type, png::ColorType::Rgba);
        for px in pixels[..info.buffer_size()].chunks(4) {
            assert!(
                record::outline::PALETTE.contains(&[px[0], px[1], px[2], px[3]]),
                "{} has a colour outside the outline palette: {px:?}",
                picture.display()
            );
        }
    }
    let events = fs::read_to_string(bundles[0].join("events.jsonl")).unwrap();
    let printable = events.matches(r#""key":"printable""#).count();
    assert_eq!(printable, 14, "7 presses and 7 releases, as \"printable\"");
    assert!(
        events.contains(r#""key":"Return""#),
        "non-printing keys by name"
    );
    assert!(events.contains(r#""key":"F11""#) && events.contains(r#""kind":"mark""#));
    assert!(
        events.contains(r#""kind":"layout","op":"keybind ToggleFloating""#),
        "the layout operation with its cause"
    );
    assert!(events.contains(r#""key":"f""#), "a chord by name");
    assert!(events.contains(r#""kind":"keybind","action":"ToggleFloating""#));
    assert!(events.contains(r#""kind":"configure""#) && events.contains(r#""kind":"commit""#));
    assert!(
        events.contains(r#""kind":"focus","from":null,"to":1"#),
        "{events}"
    );
    assert!(events.contains(r#""kind":"layout","op":"open window""#));
    assert!(events.contains(r#""kind":"map","window":1"#));
    let state = fs::read_to_string(bundles[0].join("state.json")).unwrap();
    assert!(
        state.contains(r#""tree":"#) && state.contains(r#""focus":1"#),
        "{state}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Without SASH_DEBUG_RECORD nothing is recorded and mod+F11 is an ordinary key.
#[test]
fn without_the_variable_nothing_records() {
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    harness.run_with_remote(move |server| {
        let mut c = TestClient::connect(&socket);
        let _w = c.map_window();
        server.on_server(|h| {
            assert!(h.server.state.recorder.is_none());
            h.chord(&[KEY_ALT, KEY_F11]);
        });
        c.roundtrip();
        assert_eq!(c.events.keys_pressed, vec![KEY_ALT, KEY_F11]);
    });
}

/// The frame requests caused by pointer motion are counted (merged ones once).
#[test]
fn pointer_frame_requests_are_counted() {
    let dir = record::tests::scratch_dir("pointer-frames");
    let mut harness = Harness::new();
    let config = Config {
        dir: dir.clone(),
        session: false,
        keys_full: false,
        screenshots_full: false,
    };
    harness.server.state.recorder = Some(Recorder::new(config, Instant::now()).unwrap());
    harness.server.state.draws_named_cursor = true;
    for _ in 0..5 {
        harness.step();
    }
    let requests = |h: &Harness| {
        let r = h.server.state.recorder.as_ref().unwrap();
        r.counters.pointer_frame_requests.get()
    };
    assert_eq!(requests(&harness), 0, "no motion, no request");
    for i in 0..50 {
        harness.pointer_to(100.0 + i as f64, 100.0);
    }
    assert_eq!(requests(&harness), 1, "merged until the frame starts");
    harness.step();
    harness.pointer_to(300.0, 300.0);
    assert_eq!(requests(&harness), 2);
    let _ = fs::remove_dir_all(&dir);
}
