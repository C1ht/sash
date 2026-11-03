// SPDX-License-Identifier: Apache-2.0

//! Debug recording (feature `debug-record`, debug builds only; docs/DEBUG.md).
//!
//! When `SASH_DEBUG_RECORD=<dir>` is set, compositor-level events go into a 60-second
//! ring buffer in memory; mod+F11 writes a bundle (events.jsonl, state.json,
//! screenshots, log.txt) to a new directory there. Keys are recorded by name only when
//! they cannot be text: modifiers, non-printing keys and chords; every other key is
//! "printable". Screenshots are window outlines with ids drawn from the layout alone
//! (`outline`); only with SASH_DEBUG_SCREENSHOTS=full do they show window contents,
//! through the offscreen render path (`capture`). Never the screen.

#[cfg(not(debug_assertions))]
compile_error!("the debug-record feature is for debug builds only");

pub mod json;
pub mod outline;
mod snapshot;

use std::{
    cell::Cell,
    collections::VecDeque,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant, SystemTime},
};

use smithay::{
    backend::renderer::{Bind, ExportMem, ImportAll, ImportMem, Offscreen, Renderer, Texture},
    input::keyboard::{Keysym, ModifiersState},
    reexports::calloop::timer::{TimeoutAction, Timer},
    utils::{Physical, Size},
};

pub use self::{
    json::Json,
    snapshot::{layout_json, state_json},
};
use crate::{capture, state::Sash};

pub const DIR_VAR: &str = "SASH_DEBUG_RECORD";
pub const SESSION_VAR: &str = "SASH_DEBUG_RECORD_SESSION";
pub const KEYS_VAR: &str = "SASH_DEBUG_KEYS";
pub const SCREENSHOTS_VAR: &str = "SASH_DEBUG_SCREENSHOTS";

/// How much the ring buffers keep.
pub const WINDOW: Duration = Duration::from_secs(60);
/// A bound on events in the ring however fast they come.
const MAX_EVENTS: usize = 400_000;
/// Screenshots after layout changes: at most one per interval, and this many per session.
pub const SHOT_INTERVAL: Duration = Duration::from_secs(1);
pub const MAX_LAYOUT_SHOTS: u32 = 100;
pub const MAX_SHOT_WIDTH: usize = 1920;
/// Bundles and session directories older than this are deleted at startup.
pub const RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);
/// The hotkey's bundle waits this long for the next frame's screenshot.
const BUNDLE_WAIT: Duration = Duration::from_secs(1);

/// Items with a time, the oldest dropped once they are older than `window` or there
/// are more than `max`.
#[derive(Debug)]
pub struct Ring<T> {
    window: Duration,
    max: usize,
    items: VecDeque<(Duration, T)>,
}

impl<T> Ring<T> {
    pub fn new(window: Duration, max: usize) -> Self {
        Self {
            window,
            max,
            items: VecDeque::new(),
        }
    }

    pub fn push(&mut self, at: Duration, item: T) {
        self.items.push_back((at, item));
        while self.items.len() > self.max
            || self
                .items
                .front()
                .is_some_and(|(t, _)| at.saturating_sub(*t) > self.window)
        {
            self.items.pop_front();
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &(Duration, T)> {
        self.items.iter()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.items.len()
    }
}

/// Screenshots after layout changes: one per SHOT_INTERVAL, MAX_LAYOUT_SHOTS per session.
#[derive(Debug, Default)]
pub struct ShotLimiter {
    last: Option<Duration>,
    taken: u32,
}

impl ShotLimiter {
    pub fn allow(&mut self, now: Duration) -> bool {
        if self.taken >= MAX_LAYOUT_SHOTS || self.last.is_some_and(|t| now < t + SHOT_INTERVAL) {
            return false;
        }
        self.last = Some(now);
        self.taken += 1;
        true
    }
}

/// Per-second counters; `Cell`s so they can count from `&Sash` (redraw requests).
#[derive(Debug, Default)]
pub struct Counters {
    pub pointer_in: Cell<u64>,
    pub position_updates: Cell<u64>,
    pub redraws_requested: Cell<u64>,
    pub frames_rendered: Cell<u64>,
    pub flips_completed: Cell<u64>,
    pub cursor_updates: Cell<u64>,
    pub cursor_plane_frames: Cell<u64>,
    pub cursor_composited_frames: Cell<u64>,
    pub pointer_frame_requests: Cell<u64>,
}

impl Counters {
    pub fn bump(counter: &Cell<u64>) {
        counter.set(counter.get() + 1);
    }

    fn take(&self) -> [(&'static str, u64); 9] {
        [
            ("pointer_events_in", self.pointer_in.take()),
            ("position_updates", self.position_updates.take()),
            ("redraws_requested", self.redraws_requested.take()),
            ("frames_rendered", self.frames_rendered.take()),
            ("flips_completed", self.flips_completed.take()),
            ("cursor_updates", self.cursor_updates.take()),
            ("cursor_plane_frames", self.cursor_plane_frames.take()),
            (
                "cursor_composited_frames",
                self.cursor_composited_frames.take(),
            ),
            ("pointer_frame_requests", self.pointer_frame_requests.take()),
        ]
    }
}

/// The recorder's settings, from the environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub dir: PathBuf,
    pub session: bool,
    pub keys_full: bool,
    /// Screenshots with window contents (SASH_DEBUG_SCREENSHOTS=full); outlines else.
    pub screenshots_full: bool,
}

impl Config {
    /// None if SASH_DEBUG_RECORD is unset or empty.
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Option<Config> {
        let dir = var(DIR_VAR).filter(|d| !d.is_empty())?;
        Some(Config {
            dir: PathBuf::from(dir),
            session: var(SESSION_VAR).as_deref() == Some("1"),
            keys_full: var(KEYS_VAR).as_deref() == Some("full"),
            screenshots_full: var(SCREENSHOTS_VAR).as_deref() == Some("full"),
        })
    }
}

/// Creates `dir` with mode 0700, or accepts it if it exists with mode 0700 and belongs
/// to this user. Refused inside a git work tree (never inside the repository).
pub fn prepare_dir(dir: &Path) -> Result<PathBuf, String> {
    let absolute = std::path::absolute(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    // The nearest existing ancestor decides whether it is inside a work tree.
    let mut probe = absolute.as_path();
    while !probe.exists() {
        probe = probe.parent().unwrap_or(Path::new("/"));
    }
    let real = probe
        .canonicalize()
        .map_err(|e| format!("{}: {e}", probe.display()))?;
    if let Some(tree) = real.ancestors().find(|a| a.join(".git").exists()) {
        return Err(format!(
            "{DIR_VAR}={} is inside the git work tree {}; choose a directory outside it \
             (e.g. ~/.cache/sash/debug)",
            dir.display(),
            tree.display()
        ));
    }
    if absolute.exists() {
        let meta = fs::metadata(&absolute).map_err(|e| e.to_string())?;
        let uid = nix_uid();
        if !meta.is_dir() || meta.mode() & 0o777 != 0o700 || meta.uid() != uid {
            return Err(format!(
                "{DIR_VAR}={}: an existing directory must be yours with mode 0700",
                dir.display()
            ));
        }
    } else {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&absolute)
            .map_err(|e| format!("{}: {e}", absolute.display()))?;
        // The mode is masked by the umask; set it exactly.
        fs::set_permissions(&absolute, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    Ok(absolute)
}

fn nix_uid() -> u32 {
    // Our own files tell us our uid without a new dependency.
    fs::metadata("/proc/self")
        .map(|m| m.uid())
        .unwrap_or(u32::MAX)
}

/// Deletes bundles and session directories in `dir` older than `max_age`. Returns
/// how many were deleted.
pub fn prune(dir: &Path, now: SystemTime, max_age: Duration) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("bundle-") || name.starts_with("session-")) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age > max_age);
        if old && fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// "20261102T163106Z" from wall-clock time (for directory names).
pub fn utc_stamp(time: SystemTime) -> String {
    let secs = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Days since 1970-01-01 to a civil date (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn create_private_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    DirBuilder::new().mode(0o700).create(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

/// How a key is recorded: by name only if it cannot be text (a modifier, a
/// non-printing key, or a chord with Ctrl, Alt or Super), else "printable".
pub fn key_label(sym: Keysym, mods: &ModifiersState, full: bool) -> String {
    let chord = mods.ctrl || mods.alt || mods.logo;
    let printable = sym.key_char().is_some_and(|c| !c.is_control());
    if full || chord || !printable {
        sym.name()
            .map(|n| n.trim_start_matches("XK_").to_string())
            .unwrap_or_else(|| format!("{:#x}", sym.raw()))
    } else {
        "printable".to_string()
    }
}

pub fn mods_json(mods: &ModifiersState) -> Json {
    let names = [
        ("Ctrl", mods.ctrl),
        ("Alt", mods.alt),
        ("Shift", mods.shift),
        ("Super", mods.logo),
    ];
    Json::Arr(
        names
            .into_iter()
            .filter(|(_, on)| *on)
            .map(|(n, _)| n.into())
            .collect(),
    )
}

/// The log as sash-wm wrote it, for log.txt: a ring shared with the tracing writer.
struct LogTee {
    start: Instant,
    ring: Ring<String>,
    session: Option<File>,
}

#[cfg(not(test))]
fn log_tee() -> &'static Mutex<Option<LogTee>> {
    static TEE: std::sync::OnceLock<Mutex<Option<LogTee>>> = std::sync::OnceLock::new();
    TEE.get_or_init(|| Mutex::new(None))
}

/// Tests: one tee per thread (each test's server runs on its own thread), so tests
/// running at the same time do not take over each other's log.
#[cfg(test)]
fn log_tee() -> &'static Mutex<Option<LogTee>> {
    thread_local! {
        static TEE: &'static Mutex<Option<LogTee>> = Box::leak(Box::new(Mutex::new(None)));
    }
    TEE.with(|tee| *tee)
}

/// The tracing writer: stderr, plus the recorder's log ring while recording is on.
#[derive(Default)]
pub struct LogWriter {
    line: Vec<u8>,
}

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Tests keep stderr quiet; the tee still gets every line.
        #[cfg(not(test))]
        io::stderr().write_all(buf)?;
        self.line.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stderr().flush()
    }
}

impl Drop for LogWriter {
    fn drop(&mut self) {
        if self.line.is_empty() {
            return;
        }
        let text = strip_ansi(&String::from_utf8_lossy(&self.line));
        if let Ok(mut tee) = log_tee().lock()
            && let Some(tee) = tee.as_mut()
        {
            if let Some(file) = tee.session.as_mut() {
                let _ = file.write_all(text.as_bytes());
            }
            let at = tee.start.elapsed();
            tee.ring.push(at, text);
        }
    }
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // ESC [ ... final byte in @..~
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) && c != '[' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

struct Shot {
    name: String,
    png: Vec<u8>,
}

struct Session {
    dir: PathBuf,
    events: File,
}

/// The recorder (one per compositor, in `Sash::recorder`).
pub struct Recorder {
    config: Config,
    dir: PathBuf,
    start: Instant,
    events: Ring<String>,
    shots: Ring<Shot>,
    limiter: ShotLimiter,
    shot_request: Option<String>,
    pub counters: Counters,
    session: Option<Session>,
    bundle_pending: bool,
    /// Why the current operation happens (keybind, click, map, ...): the reason given
    /// for layout and focus changes. Cleared after each arrange.
    context: Option<String>,
    last_layout: Option<Json>,
    in_drag: bool,
    last_cursor: Option<(i32, i32)>,
    last_cursor_path: Option<&'static str>,
    shot_seq: u32,
}

impl Recorder {
    /// Creates the directory, deletes old bundles, starts the log ring and prints the
    /// banner.
    pub fn new(config: Config, start: Instant) -> Result<Recorder, String> {
        let dir = prepare_dir(&config.dir)?;
        let pruned = prune(&dir, SystemTime::now(), RETENTION);
        let session = if config.session {
            let path = dir.join(format!("session-{}", utc_stamp(SystemTime::now())));
            create_private_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            create_private_dir(&path.join("screenshots")).map_err(|e| e.to_string())?;
            let events =
                create_private_file(&path.join("events.jsonl")).map_err(|e| e.to_string())?;
            let log = create_private_file(&path.join("log.txt")).map_err(|e| e.to_string())?;
            Some((Session { dir: path, events }, log))
        } else {
            None
        };
        let (session, session_log) = match session {
            Some((s, log)) => (Some(s), Some(log)),
            None => (None, None),
        };
        if let Ok(mut tee) = log_tee().lock() {
            *tee = Some(LogTee {
                start,
                ring: Ring::new(WINDOW, MAX_EVENTS),
                session: session_log,
            });
        }
        let recorder = Recorder {
            config,
            dir,
            start,
            events: Ring::new(WINDOW, MAX_EVENTS),
            shots: Ring::new(WINDOW, MAX_LAYOUT_SHOTS as usize + 10),
            limiter: ShotLimiter::default(),
            shot_request: None,
            counters: Counters::default(),
            session,
            bundle_pending: false,
            context: None,
            last_layout: None,
            in_drag: false,
            last_cursor: None,
            last_cursor_path: None,
            shot_seq: 0,
        };
        recorder.banner(pruned);
        Ok(recorder)
    }

    fn banner(&self, pruned: usize) {
        let mode = match &self.session {
            Some(s) => format!("writing everything to {}", s.dir.display()),
            None => "a 60 s ring in memory; mod+F11 writes a bundle".to_string(),
        };
        let text = format!(
            "\n*** DEBUG RECORDING IS ON ({DIR_VAR}={}) ***\n\
             {mode}. Records: input devices, pointer events and positions, keys \
             (modifiers, non-printing keys and chords by name; other keys as \
             \"printable\" only), keybinds, focus, grabs, window map/configure/commit, \
             layout changes, frames and flips, per-second counters, screenshots \
             ({shots}; after layout changes at most 1/s and 100 per session), the \
             log. Bundles older than 7 days deleted: {pruned}.\n",
            self.dir.display(),
            shots = if self.config.screenshots_full {
                "WITH WINDOW CONTENTS"
            } else {
                "window outlines and ids only, no window contents"
            },
        );
        eprintln!("{text}");
        tracing::warn!(dir = %self.dir.display(), session = self.session.is_some(), "debug recording is on");
        if self.config.screenshots_full {
            eprintln!(
                "*** WARNING: {SCREENSHOTS_VAR}=full: SCREENSHOTS SHOW WINDOW CONTENTS, \
                 including any text on screen. ***\n"
            );
            tracing::warn!("{SCREENSHOTS_VAR}=full: screenshots show window contents");
        }
        if self.config.keys_full {
            eprintln!(
                "*** WARNING: {KEYS_VAR}=full: EVERY KEY IS RECORDED BY NAME, including \
                 text and passwords. ***\n"
            );
            tracing::warn!("{KEYS_VAR}=full: every key is recorded by name");
        }
    }

    pub fn keys_full(&self) -> bool {
        self.config.keys_full
    }

    pub fn screenshots_full(&self) -> bool {
        self.config.screenshots_full
    }

    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// Records one event.
    pub fn push(&mut self, kind: &str, fields: Vec<(&str, Json)>) {
        let at = self.elapsed();
        let mut all = vec![
            ("t_us", Json::from(at.as_micros() as u64)),
            ("kind", Json::from(kind)),
        ];
        all.extend(fields);
        let line = Json::obj(all).to_string();
        if let Some(session) = self.session.as_mut() {
            let _ = writeln!(session.events, "{line}");
        }
        self.events.push(at, line);
    }

    pub fn set_context(&mut self, context: impl Into<String>) {
        self.context = Some(context.into());
    }

    pub fn context(&self) -> String {
        self.context.clone().unwrap_or_else(|| "other".into())
    }

    pub fn set_in_drag(&mut self, in_drag: bool) {
        self.in_drag = in_drag;
    }

    /// The layout after an arrange: records an operation if it changed (a drag is one
    /// operation from its start to its end) and asks for a screenshot.
    pub fn layout_after_arrange(&mut self, now: Json) {
        if self.in_drag {
            self.context = None;
            return;
        }
        let context = self.context.take().unwrap_or_else(|| "other".into());
        if self.last_layout.as_ref() == Some(&now) {
            return;
        }
        let before = self.last_layout.replace(now.clone()).unwrap_or(Json::Null);
        self.push(
            "layout",
            vec![("op", context.into()), ("before", before), ("after", now)],
        );
        if self.limiter.allow(self.elapsed()) {
            self.shot_request.get_or_insert_with(|| "layout".into());
        }
    }

    /// The cursor was drawn at `at` in a frame (plane or composited): counts an update
    /// when it moved.
    pub fn cursor_drawn(&mut self, at: (i32, i32)) {
        if self.last_cursor != Some(at) {
            self.last_cursor = Some(at);
            Counters::bump(&self.counters.cursor_updates);
        }
    }

    /// A frame went on screen with the cursor drawn by `path` (DRM: "plane",
    /// "composited" or "hidden"): counts plane and composited frames, and logs a
    /// `cursor_path` event when the path changes.
    pub fn cursor_path(&mut self, path: &'static str) {
        match path {
            "plane" => Counters::bump(&self.counters.cursor_plane_frames),
            "composited" => Counters::bump(&self.counters.cursor_composited_frames),
            _ => {}
        }
        if self.last_cursor_path != Some(path) {
            let before = self.last_cursor_path.replace(path);
            self.push(
                "cursor_path",
                vec![
                    ("path", path.into()),
                    ("before", before.map_or(Json::Null, Json::from)),
                ],
            );
        }
    }

    fn take_shot_request(&mut self) -> Option<String> {
        self.shot_request.take()
    }

    fn add_shot(&mut self, reason: &str, png: Vec<u8>) {
        let at = self.elapsed();
        self.shot_seq += 1;
        let name = format!("{:03}-{}ms-{reason}.png", self.shot_seq, at.as_millis());
        if let Some(session) = &self.session {
            let path = session.dir.join("screenshots").join(&name);
            if let Ok(mut file) = create_private_file(&path) {
                let _ = file.write_all(&png);
            }
        }
        self.push(
            "screenshot",
            vec![("file", name.clone().into()), ("reason", reason.into())],
        );
        self.shots.push(at, Shot { name, png });
    }

    /// The events in the ring, as written to events.jsonl (replay tests).
    /// Keeps every event, not only the last 60 s (replays run longer than that).
    #[cfg(test)]
    pub fn keep_everything(&mut self) {
        self.events = Ring::new(Duration::MAX, usize::MAX);
    }

    #[cfg(test)]
    pub fn event_lines(&self) -> Vec<String> {
        self.events.iter().map(|(_, line)| line.clone()).collect()
    }

    /// Writes a bundle: the ring's events, `state`, the screenshots and the log.
    pub fn write_bundle(&mut self, state: Json) -> io::Result<PathBuf> {
        let mut path = self
            .dir
            .join(format!("bundle-{}", utc_stamp(SystemTime::now())));
        let mut n = 1;
        while path.exists() {
            n += 1;
            path = self
                .dir
                .join(format!("bundle-{}-{n}", utc_stamp(SystemTime::now())));
        }
        create_private_dir(&path)?;
        let mut events = io::BufWriter::new(create_private_file(&path.join("events.jsonl"))?);
        for (_, line) in self.events.iter() {
            writeln!(events, "{line}")?;
        }
        events.flush()?;
        let mut file = create_private_file(&path.join("state.json"))?;
        writeln!(file, "{state}")?;
        create_private_dir(&path.join("screenshots"))?;
        for (_, shot) in self.shots.iter() {
            create_private_file(&path.join("screenshots").join(&shot.name))?
                .write_all(&shot.png)?;
        }
        let mut log = io::BufWriter::new(create_private_file(&path.join("log.txt"))?);
        if let Ok(tee) = log_tee().lock()
            && let Some(tee) = tee.as_ref()
        {
            for (_, line) in tee.ring.iter() {
                log.write_all(line.as_bytes())?;
            }
        }
        log.flush()?;
        self.bundle_pending = false;
        Ok(path)
    }
}

/// Starts recording if SASH_DEBUG_RECORD is set: the recorder in `state`, and the
/// one-second counter timer.
pub fn install(state: &mut Sash) -> Result<(), String> {
    let Some(config) = Config::from_env(|name| std::env::var(name).ok()) else {
        return Ok(());
    };
    let recorder = Recorder::new(config, state.started)?;
    state.recorder = Some(recorder);
    let timer = Timer::from_duration(Duration::from_secs(1));
    state
        .loop_handle
        .insert_source(timer, |_, _, server| {
            if let Some(recorder) = server.state.recorder.as_mut() {
                let counts = recorder.counters.take();
                if counts.iter().any(|(_, n)| *n > 0) {
                    let fields = counts
                        .into_iter()
                        .map(|(k, n)| (k, Json::from(n)))
                        .collect();
                    recorder.push("counters_1s", fields);
                }
            }
            TimeoutAction::ToDuration(Duration::from_secs(1))
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// mod+F11: marks a bug, asks for a screenshot with the next frame and writes the
/// bundle then (or after BUNDLE_WAIT without one, e.g. while the output is hidden).
pub fn mark(state: &mut Sash) {
    let Some(recorder) = state.recorder.as_mut() else {
        return;
    };
    tracing::warn!(dir = %recorder.dir.display(), "debug hotkey (mod+F11): writing a bundle");
    recorder.push("mark", vec![]);
    recorder.shot_request = Some("hotkey".into());
    recorder.bundle_pending = true;
    if !recorder.screenshots_full() {
        // Outlines need no frame: the picture and the bundle are written now.
        take_outline_shot(state);
        flush_bundle(state);
        return;
    }
    state.request_redraw();
    let timer = Timer::from_duration(BUNDLE_WAIT);
    let _ = state.loop_handle.insert_source(timer, |_, _, server| {
        flush_bundle(&mut server.state);
        TimeoutAction::Drop
    });
}

/// Writes the pending bundle, if there is one.
pub fn flush_bundle(state: &mut Sash) {
    if !state.recorder.as_ref().is_some_and(|r| r.bundle_pending) {
        return;
    }
    let snapshot = state_json(state);
    let Some(recorder) = state.recorder.as_mut() else {
        return;
    };
    match recorder.write_bundle(snapshot) {
        Ok(path) => tracing::warn!(path = %path.display(), "debug bundle written"),
        Err(err) => {
            recorder.bundle_pending = false;
            tracing::error!(%err, "the debug bundle could not be written");
        }
    }
}

/// After a frame was drawn: takes a requested screenshot through the offscreen render
/// path (never the screen), then writes a pending bundle.
pub fn after_frame<R, T>(state: &mut Sash, renderer: &mut R, size: Size<i32, Physical>)
where
    R: Renderer + ImportAll + ImportMem + Offscreen<T> + Bind<T> + ExportMem,
    R::TextureId: Texture + Send + Clone + 'static,
    R::Error: 'static,
{
    if !state
        .recorder
        .as_ref()
        .is_some_and(Recorder::screenshots_full)
    {
        take_outline_shot(state);
        flush_bundle(state);
        return;
    }
    let Some(reason) = state
        .recorder
        .as_mut()
        .and_then(Recorder::take_shot_request)
    else {
        flush_bundle(state);
        return;
    };
    match capture::render_rgba::<R, T>(renderer, state, size) {
        Ok(pixels) => {
            let (w, h) = (size.w as usize, size.h as usize);
            let (scaled, sw, sh) = downscale(&pixels, w, h, MAX_SHOT_WIDTH);
            match capture::encode_png(&scaled, sw as u32, sh as u32) {
                Ok(png) => {
                    if let Some(recorder) = state.recorder.as_mut() {
                        recorder.add_shot(&reason, png);
                    }
                }
                Err(err) => tracing::warn!(%err, "debug screenshot: PNG encoding failed"),
            }
        }
        Err(err) => tracing::warn!(%err, "debug screenshot: offscreen render failed"),
    }
    flush_bundle(state);
}

/// Takes a requested screenshot as window outlines (no window contents).
pub fn take_outline_shot(state: &mut Sash) {
    let Some(reason) = state
        .recorder
        .as_mut()
        .and_then(Recorder::take_shot_request)
    else {
        return;
    };
    let area = state.windows.layout.area();
    let focused = state.windows.layout.focused();
    let windows: Vec<outline::Outline> = state
        .windows
        .layout
        .stacking()
        .into_iter()
        .map(|(id, rect, layer)| outline::Outline {
            id,
            rect,
            layer,
            focused: Some(id) == focused,
        })
        .collect();
    let pointer = state.seat.get_pointer().map(|p| {
        let at = p.current_location();
        (at.x.floor() as i32, at.y.floor() as i32)
    });
    let canvas = outline::draw(
        (area.w.max(1) as usize, area.h.max(1) as usize),
        &windows,
        pointer,
        MAX_SHOT_WIDTH,
    );
    match capture::encode_png(&canvas.pixels, canvas.width as u32, canvas.height as u32) {
        Ok(png) => {
            if let Some(recorder) = state.recorder.as_mut() {
                recorder.add_shot(&reason, png);
            }
        }
        Err(err) => tracing::warn!(%err, "debug screenshot: PNG encoding failed"),
    }
}

/// RGBA pixels scaled down by a whole factor (box filter) to at most `max_width`.
pub fn downscale(pixels: &[u8], w: usize, h: usize, max_width: usize) -> (Vec<u8>, usize, usize) {
    let k = w.div_ceil(max_width).max(1);
    if k == 1 {
        return (pixels.to_vec(), w, h);
    }
    let (sw, sh) = (w / k, h / k);
    let mut out = vec![0u8; sw * sh * 4];
    for y in 0..sh {
        for x in 0..sw {
            let mut sum = [0u32; 4];
            for dy in 0..k {
                let row = (y * k + dy) * w;
                for dx in 0..k {
                    let i = (row + x * k + dx) * 4;
                    for c in 0..4 {
                        sum[c] += pixels[i + c] as u32;
                    }
                }
            }
            let o = (y * sw + x) * 4;
            for c in 0..4 {
                out[o + c] = (sum[c] / (k * k) as u32) as u8;
            }
        }
    }
    (out, sw, sh)
}

#[cfg(test)]
pub mod tests;
