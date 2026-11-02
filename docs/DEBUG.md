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
  cursor plane or was composited;
- counters per second: pointer events in, position updates, redraws requested, frames
  rendered, flips completed, cursor updates;
- screenshots, through the same offscreen render path as `SIGUSR1` captures (never the
  screen): after each layout change (at most one per second, 100 per session) and when
  the hotkey is pressed; scaled to at most 1920 px wide. **They show window contents,
  so any text a window displays (including what you typed into a terminal) is in
  them.** Close or hide windows with private content before pressing the hotkey;
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

## Not yet

Replaying a bundle's pointer and keybind events (`--replay`) is proposed, not built.
