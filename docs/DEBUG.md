<!-- SPDX-License-Identifier: Apache-2.0 -->
# Debug recording

A recorder for bugs seen on real hardware, where nothing else can watch. Rules are in
CLAUDE.md ("Debug recording").

## Building and enabling

- Cargo feature `debug-record`, off by default. Debug builds only: a release build with
  the feature fails to compile, and `tools/check-release.sh` fails if any recorder code
  is in the release binary.

  ```sh
  cargo build -p sash-wm --features debug-record
  ```

- Nothing is recorded unless `SASH_DEBUG_RECORD=<dir>` is set. The directory is created
  with mode 0700; a directory inside a git work tree (such as the repository) is refused.
- At startup a banner on stderr (and in the log) says that recording is on, where, and
  what is recorded.
- `SASH_DEBUG_RECORD_SESSION=1`: write everything as it happens (a session directory),
  not only when the hotkey is pressed.
- `SASH_DEBUG_KEYS=full`: record every key by name, printable ones included. A warning
  is printed at startup. Do not use it while typing passwords.
- `SASH_DEBUG_SCREENSHOTS=full`: screenshots with window contents instead of outlines.
  A warning is printed at startup. **Everything on screen is then in the bundle**,
  including any text a window shows (what you typed into a terminal, a document, a web
  page). Close or hide such windows before pressing the hotkey.

## What is recorded

Compositor-level data only, kept in a 60-second ring buffer in memory:

- libinput devices added and removed, with their capabilities;
- raw pointer events as received (relative deltas, absolute positions, buttons, axes)
  and the pointer position after sash-wm processed them;
- keys: modifiers, non-printing keys (Return, Escape, arrows, F-keys, ...) and chords
  (a key pressed with Ctrl, Alt or Super held) by name; every other key is recorded as
  `"printable"`, with its timestamp only. Nothing typed into a window can be read back;
- keybind matches and the actions they ran;
- keyboard focus changes with the reason;
- grabs (moves, resizes, popup grabs) starting and ending;
- windows: map, unmap, configure sent, configure acknowledged, commit;
- every layout operation, with the layout before and after;
- frames: start and end, page flip completed, whether the cursor went to the hardware
  cursor plane, was composited or was hidden (`frame_end`, field `cursor`); on DRM a
  `cursor_path` event each time that changes (`path`, and the `before` path);
- counters per second: pointer events in, position updates, redraws requested, frames
  rendered, flips completed, cursor updates, frames with the cursor on the plane
  (`cursor_plane_frames`), frames with it composited (`cursor_composited_frames`), and
  frame requests caused by pointer motion (`pointer_frame_requests`);
- screenshots after each layout change (at most one per second, 100 per session) and
  when the hotkey is pressed, at most 1920 px wide. By default they show **window
  outlines only**: each window as a flat box (tiled and floating in different shades),
  a 2 px border (blue for the focused window), its id in simple built-in digits, and
  the pointer as a red cross, drawn from the layout alone. No window contents. With
  `SASH_DEBUG_SCREENSHOTS=full` they show contents, rendered through the same offscreen
  path as `SIGUSR1` captures (never the screen);
- sash-wm's own log.

## The hotkey: mod+F11

mod is Super on the DRM backend and Alt when nested. The press marks a bug and writes a
bundle; neither the press nor the release reaches a client.

A bundle is a directory `bundle-<UTC time>/` inside `SASH_DEBUG_RECORD`:

- `events.jsonl`: one JSON object per line, `"t"` in microseconds of the monotonic clock
  since sash-wm started, `"kind"` and its fields;
- `state.json`: the layout tree (split directions and ratios), every window's rectangle,
  mode and app_id (no titles: they can carry what was typed), the z-order, focus and
  focus history, the window under the pointer, the active grab, the output mode;
- `screenshots/*.png`;
- `log.txt`: the last 60 seconds of sash-wm's log.

## Retention

At startup, bundles and session directories older than 7 days are deleted.

## Building and testing

```sh
cargo test -p sash-wm --features debug-record   # recorder tests included
tools/check-release.sh                          # proves none of it is in release builds
```

The VM scenario (tools/vm/scenario.py, check 17) runs it on the DRM backend.

## Replaying a bundle

```sh
tools/replay.sh ~/.cache/sash/debug/bundle-<time>
```

The bundle's events are replayed in the headless test harness (nothing on the
display) and its layout snapshots are compared, one by one, with the replay's. The
output lists what was replayed, what was not, how many snapshots were compared, and
every difference.

- Replayed: pointer positions and buttons as sash-wm processed them (pointer
  acceleration does not matter), modifier keys, keybind actions (Super+Return by the
  window it opened), windows opening, committing at their recorded sizes, unmapping
  and closing (stand-in test clients), output size changes.
- Not replayed: printable keys (only recorded as "printable") and other keys that are
  not modifiers (keybinds are replayed as actions), client requests (title-bar moves
  and resizes, fullscreen and maximize asked by a client, e.g. a double-click on
  foot's title bar), popups, frames, flips and pacing (DRM timing), session pauses
  and VT switches, input devices coming and going. Once a client request changed the
  layout, the snapshots after it differ.
- The stand-in windows read their events every 200 pointer positions (a fast mouse
  would otherwise fill their sockets), a replay may take up to 300 s, and the
  replay's own recorder keeps everything (a long session replays in over 60 s; the
  owner's 60-second session with an 8 kHz mouse takes about 70 s).
- A bundle that starts with windows already open is refused: the 60-second ring cut
  their opening. Record from an empty desktop, or with `SASH_DEBUG_RECORD_SESSION=1`.
- Window ids are matched by order of opening; the "mapped" flag is not compared
  (it depends on client timing); repeated and empty-desktop snapshots are merged.

A bundle that shows a bug becomes a regression test, never the whole bundle:

```sh
python3 -I tools/replay-fixture.py <bundle> sash-wm/src/tests/fixtures/replay/<name> <cut seconds>
```

keeps the events before the cut (end it before the first snapshot the replay cannot
reproduce), only the kinds the replay uses, downsampled (the last pointer position
before each button, key or window event, the first and every 25th during a drag), with
no typed keys and no device names. Aim for under 200 KB and a test under 10 s. Add a
README and a test like `the_owners_nvidia_session_replays` in
`sash-wm/src/tests/replay.rs`.
