// SPDX-License-Identifier: Apache-2.0

//! Replays a debug bundle (docs/DEBUG.md) in the headless harness and compares the
//! layout snapshots with the bundle's, so that a bug recorded on real hardware becomes
//! a regression test.
//!
//! Replayed: pointer positions and buttons (as sash-wm processed them, so acceleration
//! does not matter; the stand-ins read their events every READ_EVERY positions, so a
//! fast mouse cannot fill their sockets); modifier keys; keybind actions (a terminal spawn by its window
//! instead); windows opening, committing at their recorded sizes, unmapping and closing,
//! with stand-in test clients; output size changes. Not replayed: printable keys
//! (recorded as "printable" only), client requests (title-bar moves and resizes,
//! fullscreen and maximize from clients), popups, frames and pacing. A bundle that
//! starts with windows already open (the 60 s ring cut their opening) is refused.
//!
//! `tools/replay.sh <bundle>` replays any bundle.

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use smithay::backend::input::{ButtonState, KeyState};
use wayland_client::Proxy;

use super::{
    client::{TestClient, TestWindow},
    harness::Harness,
};
use crate::{
    keybinds::{self, Action, Modifier},
    record::{self, Config, Json, Recorder, json::parse},
};

/// A replay may run long (a real session has tens of thousands of pointer events).
const REPLAY_TIMEOUT: Duration = Duration::from_secs(300);
/// The stand-ins read their events after this many pointer positions.
const READ_EVERY: usize = 200;

/// What a replay found.
#[derive(Debug, Default)]
pub struct Report {
    /// Events replayed, by kind.
    pub replayed: BTreeMap<String, usize>,
    /// Events not replayed, by kind.
    pub skipped: BTreeMap<String, usize>,
    /// Layout snapshots compared (after merging repeats).
    pub compared: usize,
    /// Differences, first one first.
    pub mismatches: Vec<String>,
}

/// Where a stand-in window is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// Initial commit sent, no buffer yet.
    Initial,
    Mapped,
    /// Unmapped by a null buffer; the next commit without a buffer starts over.
    Unmapped,
}

/// Evdev codes of the modifier keys (the only keys replayed by name).
fn modifier_code(name: &str) -> Option<u32> {
    Some(match name {
        "Shift_L" => 42,
        "Shift_R" => 54,
        "Control_L" => 29,
        "Control_R" => 97,
        "Alt_L" => 56,
        "Alt_R" => 100,
        "Super_L" => 125,
        "Super_R" => 126,
        _ => return None,
    })
}

/// The keybind table's action with this recorded name.
fn action_named(name: &str) -> Option<Action> {
    keybinds::TABLE
        .iter()
        .map(|binding| binding.action)
        .find(|action| format!("{action:?}") == name)
}

fn events_of(bundle: &Path) -> Result<Vec<Json>, String> {
    let text = fs::read_to_string(bundle.join("events.jsonl"))
        .map_err(|e| format!("{}: {e}", bundle.display()))?;
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(parse)
        .collect()
}

fn kind(event: &Json) -> &str {
    event.get("kind").and_then(Json::as_str).unwrap_or("")
}

/// A layout snapshot for comparison: window ids renumbered in order of first
/// appearance, the "mapped" flag left out (it depends on client timing).
fn normalized(snapshot: &Json, ids: &HashMap<i64, i64>) -> Json {
    let id = |v: &Json| match v.as_i64() {
        Some(n) => Json::Int(*ids.get(&n).unwrap_or(&-n)),
        None => v.clone(),
    };
    match snapshot {
        Json::Obj(fields) => Json::Obj(
            fields
                .iter()
                .filter(|(k, _)| k != "mapped")
                .map(|(k, v)| {
                    let v = match k.as_str() {
                        "focus" | "id" | "window" => id(v),
                        _ => normalized(v, ids),
                    };
                    (k.clone(), v)
                })
                .collect(),
        ),
        Json::Arr(items) => Json::Arr(items.iter().map(|v| normalized(v, ids)).collect()),
        other => other.clone(),
    }
}

/// Window ids in order of their opening (their first commit) in an event stream.
fn opening_order(events: &[Json]) -> HashMap<i64, i64> {
    let mut ids = HashMap::new();
    for event in events {
        if kind(event) == "commit"
            && let Some(w) = event.get("window").and_then(Json::as_i64)
        {
            let next = ids.len() as i64 + 1;
            ids.entry(w).or_insert(next);
        }
    }
    ids
}

/// A snapshot without its "focus" field.
fn without_focus(snapshot: &Json) -> Json {
    match snapshot {
        Json::Obj(fields) => Json::Obj(
            fields
                .iter()
                .filter(|(k, _)| k != "focus")
                .cloned()
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The layout snapshots of an event stream, normalized, repeats merged, empty
/// desktops left out (startup order differs between a backend and the harness).
/// Focus on open: before M9e a new window got focus when it opened, since M9e when it
/// maps. So the focus of an "open window" snapshot is not compared, and a "map"
/// snapshot that only moves focus is left out; bundles from before and after compare
/// alike, and every other snapshot still compares focus.
fn snapshots(events: &[Json]) -> Vec<(String, Json)> {
    let ids = opening_order(events);
    let mut out: Vec<(String, Json)> = Vec::new();
    for event in events.iter().filter(|e| kind(e) == "layout") {
        let Some(after) = event.get("after") else {
            continue;
        };
        let op = event
            .get("op")
            .and_then(Json::as_str)
            .unwrap_or("?")
            .to_string();
        let empty = matches!(after.get("windows"), Some(Json::Arr(w)) if w.is_empty());
        let mut snapshot = normalized(after, &ids);
        if op == "open window" {
            snapshot = without_focus(&snapshot);
        }
        let repeat = out.last().is_some_and(|(_, last)| {
            *last == snapshot || (op == "map" && without_focus(last) == without_focus(&snapshot))
        });
        if empty || repeat {
            continue;
        }
        out.push((op, snapshot));
    }
    out
}

/// Replays `bundle` (a directory with events.jsonl and state.json).
pub fn replay(bundle: &Path) -> Result<Report, String> {
    let events = events_of(bundle)?;
    let state = fs::read_to_string(bundle.join("state.json"))
        .ok()
        .and_then(|t| parse(&t).ok());
    let modifier = match state
        .as_ref()
        .and_then(|s| s.get("modifier"))
        .and_then(Json::as_str)
    {
        Some("Alt") => Modifier::Alt,
        _ => Modifier::Super,
    };
    // The bundle must show every window opening: a commit without a buffer first.
    let mut opened = HashMap::new();
    for event in events.iter().filter(|e| kind(e) == "commit") {
        let w = event.get("window").and_then(Json::as_i64).unwrap_or(0);
        let buffer = event.get("buffer").and_then(Json::as_bool).unwrap_or(false);
        if !opened.contains_key(&w) && buffer {
            return Err(format!(
                "window {w} was open before the bundle starts (the 60 s ring cut its \
                 opening); record from an empty desktop, or with \
                 SASH_DEBUG_RECORD_SESSION=1"
            ));
        }
        opened.insert(w, ());
    }
    let output = events
        .iter()
        .filter(|e| kind(e) == "layout")
        .find_map(|e| e.get("after").and_then(|a| a.get("output")).cloned());
    let (width, height) = match &output {
        Some(Json::Arr(wh)) if wh.len() == 2 => (
            wh[0].as_i64().unwrap_or(800) as i32,
            wh[1].as_i64().unwrap_or(600) as i32,
        ),
        _ => (800, 600),
    };

    // Bundles from before M11b have no server_side field: their windows had no
    // decorations, so a client's committed size was the whole window. The stand-ins
    // (which get the 1 px border) commit that size minus the border, so the windows
    // come out as recorded.
    let before_decorations = !events
        .iter()
        .any(|e| kind(e) == "commit" && e.get("server_side").is_some());

    let mut harness = Harness::with_modifier(modifier);
    harness.timeout = REPLAY_TIMEOUT;
    let socket = harness.socket_path();
    let replay_dir = record::tests::scratch_dir("replay");
    let remove = replay_dir.clone();
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let out = lines.clone();
    let recorded = events.clone();
    let report = harness.run_with_remote(move |server| {
        let mut report = Report::default();
        let dir = replay_dir.clone();
        server.on_server(move |h| {
            h.server.state.set_layout_area((width, height).into());
            let config = Config {
                dir,
                session: false,
                keys_full: false,
                screenshots_full: false,
            };
            let mut recorder = Recorder::new(config, Instant::now()).unwrap();
            recorder.keep_everything();
            h.server.state.recorder = Some(recorder);
        });
        // Recorded window id -> stand-in client, its window, and where it is.
        let mut windows: HashMap<i64, (TestClient, Option<TestWindow>, Stage)> = HashMap::new();
        // The last configure serial each stand-in acked.
        let mut acked: HashMap<i64, u32> = HashMap::new();
        let mut positions = 0;
        for event in &events {
            let kind = kind(event).to_string();
            let mut done = true;
            match kind.as_str() {
                "pointer_position" => {
                    let x = event.get("x").and_then(Json::as_f64).unwrap_or(0.0);
                    let y = event.get("y").and_then(Json::as_f64).unwrap_or(0.0);
                    server.on_server(move |h| h.pointer_to(x, y));
                    positions += 1;
                    if positions % READ_EVERY == 0 {
                        for (client, _, _) in windows.values_mut() {
                            client.roundtrip();
                        }
                    }
                }
                "pointer_button" => {
                    let button = event.get("button").and_then(Json::as_i64).unwrap_or(0) as u32;
                    let pressed = event.get("pressed").and_then(Json::as_bool) == Some(true);
                    let state = if pressed {
                        ButtonState::Pressed
                    } else {
                        ButtonState::Released
                    };
                    server.on_server(move |h| h.button(button, state));
                }
                "key" => {
                    let name = event.get("key").and_then(Json::as_str).unwrap_or("");
                    let pressed = event.get("pressed").and_then(Json::as_bool) == Some(true);
                    match modifier_code(name) {
                        Some(code) => {
                            let state = if pressed {
                                KeyState::Pressed
                            } else {
                                KeyState::Released
                            };
                            server.on_server(move |h| h.key(code, state));
                        }
                        None => done = false,
                    }
                }
                "keybind" => {
                    let name = event.get("action").and_then(Json::as_str).unwrap_or("");
                    match action_named(name) {
                        // The terminal's window arrives through its own events.
                        Some(Action::SpawnTerminal) => done = false,
                        Some(action) => server.on_server(move |h| {
                            h.server.state.run_action(action);
                            h.server.flush();
                        }),
                        None => done = false,
                    }
                }
                "commit" => {
                    let w = event.get("window").and_then(Json::as_i64).unwrap_or(0);
                    let buffer = event.get("buffer").and_then(Json::as_bool) == Some(true);
                    let size = match event.get("geometry") {
                        Some(Json::Arr(g)) if g.len() == 4 => (
                            g[2].as_i64().unwrap_or(0) as i32,
                            g[3].as_i64().unwrap_or(0) as i32,
                        ),
                        _ => (0, 0),
                    };
                    let size = if before_decorations && size.0 > 0 && size.1 > 0 {
                        crate::layout::decor::Decoration::BORDER.client_size(size)
                    } else {
                        size
                    };
                    let server_side =
                        event.get("server_side").and_then(Json::as_bool) == Some(true);
                    let entry = windows
                        .entry(w)
                        .or_insert_with(|| (TestClient::connect(&socket), None, Stage::Initial));
                    let (client, window, stage) = entry;
                    match (window.as_ref(), buffer, *stage) {
                        // A new window: its initial commit; the configure comes back.
                        (None, _, _) => {
                            let seen = client.events.configures;
                            let win = client.create_window_without_commit();
                            // A client that negotiated decorations does so before its
                            // initial commit.
                            if server_side {
                                let _ = client.decorate(&win, None);
                            }
                            win.surface.commit();
                            *window = Some(win);
                            client.dispatch_until("initial configure", |e| e.configures > seen);
                        }
                        // Its first buffer, or a new one: at the recorded size.
                        // A new configure is acked first; content commits are not.
                        (Some(win), true, _) => {
                            client.roundtrip();
                            let serial = client.events.serials.get(&win.xdg_surface.id()).copied();
                            let fresh = serial.is_some() && acked.get(&w) != serial.as_ref();
                            match (fresh, size.0 > 0 && size.1 > 0) {
                                (true, true) => client.ack_and_commit_size(win, size),
                                (true, false) => client.ack_and_commit(win),
                                (false, true) => client.commit_size(win, size),
                                (false, false) => {
                                    win.surface.commit();
                                    client.roundtrip();
                                }
                            }
                            if fresh && let Some(serial) = serial {
                                acked.insert(w, serial);
                            }
                            *stage = Stage::Mapped;
                        }
                        // A null buffer on a mapped window: unmapped.
                        (Some(win), false, Stage::Mapped) => {
                            client.unmap(win);
                            *stage = Stage::Unmapped;
                        }
                        // After an unmap: a new initial commit, the configure comes back.
                        (Some(win), false, Stage::Unmapped) => {
                            let seen = client.events.configures;
                            win.surface.commit();
                            client.dispatch_until("configure", |e| e.configures > seen);
                            *stage = Stage::Initial;
                        }
                        // Another commit before its first buffer (clients do this).
                        (Some(win), false, Stage::Initial) => {
                            win.surface.commit();
                            client.roundtrip();
                        }
                    }
                }
                "destroy" => {
                    let w = event.get("window").and_then(Json::as_i64).unwrap_or(0);
                    if let Some((mut client, Some(win), _)) = windows.remove(&w) {
                        client.destroy(win);
                    }
                }
                "layout" if event.get("op").and_then(Json::as_str) == Some("output size") => {
                    if let Some(Json::Arr(wh)) = event.get("after").and_then(|a| a.get("output"))
                        && wh.len() == 2
                    {
                        let (w, h) = (
                            wh[0].as_i64().unwrap_or(800) as i32,
                            wh[1].as_i64().unwrap_or(600) as i32,
                        );
                        server.on_server(move |hs| hs.server.state.set_layout_area((w, h).into()));
                    }
                }
                _ => done = false,
            }
            let counts = if done {
                &mut report.replayed
            } else {
                &mut report.skipped
            };
            *counts.entry(kind).or_insert(0) += 1;
        }
        for (client, _, _) in windows.values_mut() {
            client.roundtrip();
        }
        server.on_server(move |h| {
            if let Some(recorder) = h.server.state.recorder.as_ref() {
                *out.lock().unwrap() = recorder.event_lines();
            }
        });
        report
    });
    let _ = fs::remove_dir_all(remove);

    let mut report = report;
    let replayed: Vec<Json> = lines
        .lock()
        .unwrap()
        .iter()
        .map(|l| parse(l))
        .collect::<Result<_, _>>()?;
    let expected = snapshots(&recorded);
    let actual = snapshots(&replayed);
    report.compared = expected.len();
    for (i, ((op, want), got)) in expected.iter().zip(actual.iter()).enumerate() {
        if want != &got.1 {
            report.mismatches.push(format!(
                "layout {i} (recorded op \"{op}\", replayed op \"{}\"):\n  recorded {want}\n  replayed {}",
                got.0, got.1
            ));
        }
    }
    if expected.len() != actual.len() {
        report.mismatches.push(format!(
            "{} layout snapshots recorded, {} replayed",
            expected.len(),
            actual.len()
        ));
    }
    Ok(report)
}

/// Replays the bundle in SASH_REPLAY_BUNDLE (tools/replay.sh).
#[test]
#[ignore = "run by tools/replay.sh with SASH_REPLAY_BUNDLE set"]
fn replay_a_bundle() {
    let bundle = std::env::var("SASH_REPLAY_BUNDLE").expect("set SASH_REPLAY_BUNDLE");
    let report = replay(Path::new(&bundle)).unwrap();
    println!("replayed: {:?}", report.replayed);
    println!("not replayed: {:?}", report.skipped);
    println!("layout snapshots compared: {}", report.compared);
    for mismatch in &report.mismatches {
        println!("MISMATCH {mismatch}");
    }
    assert!(report.mismatches.is_empty(), "the replay differs");
}

/// A session recorded in the harness (windows opened with the pointer in different
/// places, a float, a click, a close, a drag) replays to the same layouts.
#[test]
fn a_recorded_session_replays_to_the_same_layouts() {
    use crate::seat::BTN_LEFT;
    const ALT: u32 = 56;
    const KEY_F: u32 = 33;
    let dir = record::tests::scratch_dir("session");
    let mut harness = Harness::new();
    let socket = harness.socket_path();
    let recording = dir.clone();
    harness.run_with_remote(move |server| {
        server.on_server(move |h| {
            let config = Config {
                dir: recording,
                session: false,
                keys_full: false,
                screenshots_full: false,
            };
            h.server.state.recorder = Some(Recorder::new(config, Instant::now()).unwrap());
        });
        let mut clients = Vec::new();
        // Window 1; window 2 with the pointer over 1; window 3 with the pointer over 1's
        // lower part; window 4 with the pointer over window 2.
        for at in [(10.0, 10.0), (100.0, 100.0), (100.0, 500.0), (700.0, 300.0)] {
            server.on_server(move |h| h.pointer_to(at.0, at.1));
            let mut c = TestClient::connect(&socket);
            let w = c.map_window();
            clients.push((c, w));
        }
        // mod+F floats the newest window; a click focuses window 1; window 3 closes.
        server.on_server(|h| {
            h.chord(&[ALT, KEY_F]);
            h.pointer_to(50.0, 50.0);
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.button(BTN_LEFT, ButtonState::Released);
        });
        let (mut c3, w3) = clients.remove(2);
        c3.destroy(w3);
        // A mod+drag moves window 2 (it floats on the press).
        server.on_server(|h| {
            h.pointer_to(700.0, 100.0);
            h.key(ALT, KeyState::Pressed);
            h.button(BTN_LEFT, ButtonState::Pressed);
            h.pointer_to(600.0, 150.0);
            h.button(BTN_LEFT, ButtonState::Released);
            h.key(ALT, KeyState::Released);
            h.chord(&[ALT, 87]); // mod+F11: the bundle
        });
        for (c, _) in clients.iter_mut() {
            c.roundtrip();
        }
    });
    let bundle = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("bundle-")
        })
        .expect("a bundle");
    let report = replay(&bundle).unwrap();
    assert!(report.mismatches.is_empty(), "{:#?}", report.mismatches);
    assert!(report.compared >= 6, "{report:?}");
    assert!(
        report.replayed.get("keybind").is_some_and(|n| *n >= 1),
        "{report:?}"
    );
    assert!(
        report
            .replayed
            .get("pointer_button")
            .is_some_and(|n| *n >= 4),
        "{report:?}"
    );

    // The comparison is not vacuous: a recorded rectangle changed is a mismatch.
    let events = fs::read_to_string(bundle.join("events.jsonl")).unwrap();
    let tampered = events.replacen(r#""rect":[400,0,400,600]"#, r#""rect":[400,0,401,600]"#, 1);
    assert_ne!(tampered, events);
    fs::write(bundle.join("events.jsonl"), &tampered).unwrap();
    let report = replay(&bundle).unwrap();
    assert!(
        !report.mismatches.is_empty(),
        "a changed rectangle must not match"
    );

    // A bundle that starts after a window opened is refused.
    let cut: String = events
        .lines()
        .filter(|l| !l.contains(r#""kind":"commit","window":1,"buffer":false"#))
        .map(|l| format!("{l}\n"))
        .collect();
    fs::write(bundle.join("events.jsonl"), cut).unwrap();
    let err = replay(&bundle).unwrap_err();
    assert!(err.contains("open before the bundle starts"), "{err}");
    let _ = fs::remove_dir_all(&dir);
}

/// The owner's first real-hardware session (NVIDIA, 5120x1440, stage C run 4), cut
/// before foot's first maximize request and downsampled (tools/replay-fixture.py):
/// four windows under rule A and nine drags, three of them snapping, replay to the
/// same layouts.
#[test]
fn the_owners_nvidia_session_replays() {
    let bundle = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/tests/fixtures/replay/nvidia-run4-20261103");
    let started = Instant::now();
    let report = replay(&bundle).unwrap();
    assert!(report.mismatches.is_empty(), "{:#?}", report.mismatches);
    assert_eq!(report.compared, 13, "snapshots 0 to 12: {report:?}");
    assert_eq!(report.replayed.get("pointer_button"), Some(&16));
    assert!(started.elapsed() < Duration::from_secs(10), "under 10 s");
}

/// The real-hardware fixtures hold only what the replay needs (owner, M9e): the listed
/// event kinds, modifier keys only, and no app ids, titles, device names or typed keys,
/// checked down to every field name.
#[test]
fn real_hardware_fixtures_hold_only_replay_data() {
    const KINDS: [&str; 6] = [
        "layout",
        "commit",
        "destroy",
        "pointer_button",
        "pointer_position",
        "key",
    ];
    const FIELDS: [&str; 33] = [
        "t_us",
        "kind",
        "op",
        "before",
        "after",
        "output",
        "focus",
        "windows",
        "tree",
        "id",
        "mapped",
        "mode",
        "forced_floating",
        "rect",
        "shown",
        "snapped",
        "maximized",
        "fullscreen",
        "depth",
        "split",
        "ratio",
        "window",
        "leaf",
        "buffer",
        "server_side",
        "geometry",
        "button",
        "pressed",
        "x",
        "y",
        "grabbed",
        "key",
        "mods",
    ];
    fn fields(value: &Json, out: &mut Vec<String>) {
        match value {
            Json::Obj(items) => {
                for (k, v) in items {
                    out.push(k.clone());
                    fields(v, out);
                }
            }
            Json::Arr(items) => items.iter().for_each(|v| fields(v, out)),
            _ => {}
        }
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/tests/fixtures/replay/nvidia-run4-20261103");
    let text = fs::read_to_string(dir.join("events.jsonl")).unwrap();
    for forbidden in ["app_id", "title", "\"name\"", "printable", "input_device"] {
        assert!(
            !text.contains(forbidden),
            "the fixture contains {forbidden}"
        );
    }
    for line in text.lines() {
        let event = parse(line).unwrap();
        let kind = kind(&event);
        assert!(KINDS.contains(&kind), "event kind {kind}");
        if kind == "key" {
            let name = event.get("key").and_then(Json::as_str).unwrap_or("");
            assert!(
                modifier_code(name).is_some(),
                "a key that is not a modifier: {name}"
            );
        }
        let mut names = Vec::new();
        fields(&event, &mut names);
        for name in names {
            assert!(FIELDS.contains(&name.as_str()), "field {name} in: {line}");
        }
    }
}

/// No fixture holds client text (owner, after M14): no app ids or titles in any file of
/// any replay fixture. (The VM session of 2026-11-02 listed app ids in its state.json,
/// recorded before they were taken out of bundles; it was removed.)
#[test]
fn no_fixture_holds_client_text() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tests/fixtures/replay");
    let mut checked = 0;
    for fixture in fs::read_dir(&root).unwrap().flatten() {
        for file in fs::read_dir(fixture.path()).unwrap().flatten() {
            let path = file.path();
            if path.extension().is_some_and(|e| e == "md") {
                continue;
            }
            let text = fs::read_to_string(&path).unwrap();
            for forbidden in ["app_id", "title"] {
                assert!(
                    !text.contains(forbidden),
                    "{} contains {forbidden}",
                    path.display()
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 0, "no fixture files found");
}
