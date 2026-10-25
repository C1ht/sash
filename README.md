<!-- SPDX-License-Identifier: Apache-2.0 -->
# Sash

A Wayland desktop environment in Rust.

- `sash-wm`: the compositor, built on Smithay.
- `sash-ui`: the first-party UI toolkit (not started yet).

Licensed under Apache-2.0. See `LICENSE` and `NOTICE`.

## Build and run

```sh
cargo build
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo test
cargo deny check
```

Nested inside an existing Wayland session (for example Hyprland):

```sh
RUST_LOG=sash_wm=debug cargo run -p sash-wm
```

sash-wm logs the name of its own Wayland socket. Start clients against it
with `WAYLAND_DISPLAY=<socket> <client>`.

## Synthetic input (debug builds)

Debug builds open `$XDG_RUNTIME_DIR/sash-wm-input-<wayland socket>.sock` (mode 0600).
Each line is one command, answered with `ok` or `error: ...`:

```text
motion <x> <y>                                  pointer to (x, y), output coordinates
button <left|right|middle|code> <press|release>
key <evdev code> <press|release>                e.g. 56 = left Alt
```

Release builds contain neither this nor the capture code: `tools/check-release.sh`
checks the release binary (CI runs it).

## Screenshots (debug builds)

```sh
SASH_CAPTURE_DIR=/tmp/shots cargo run -p sash-wm &
kill -USR1 <sash-wm pid>     # writes /tmp/shots/sash-wm-capture-1.png
```

sash-wm renders its current output once more into an offscreen buffer and
writes it as a PNG; the host screen is never read. Release builds ignore
SIGUSR1.

## Measurement and similarity tools

- `tools/measure/`: scripted measurements through the synthetic input. Build the
  measurement binary (optimized, debug assertions on) with
  `source tools/measure/common.sh && build_measure`, then run
  `tools/measure/run-drags.sh <log>` (drags on three foot windows),
  `tools/measure/run-popup.sh <log> <capture dir>` (pavucontrol menu while dragging),
  `tools/measure/run-resize.sh <log> <steps> <client...>` (configure to first buffer at
  the new size, keyboard resizes) or
  `tools/measure/idle.sh <log> <windows> <seconds>` (idle CPU and memory, release
  build). Each prints frame-time and frame-interval statistics from the log.
- `tools/similarity/`: the per-milestone similarity check against Smithay's examples.
  In a venv with `pip install copydetect==0.5.0` and a clone of Smithay at tag
  `v0.7.0`: `python -I tools/similarity/summary.py sash-wm/src <smithay clone> 25 30 4`;
  `tools/similarity/pair.py <sash file> <reference file> 25 30` shows what matched.

## Debian 13 (trixie) packages

- Build: `build-essential pkg-config libxkbcommon-dev`
- Runtime and tests (loaded with dlopen): `libwayland-client0`, plus `libegl1` for the nested window

## Updating the toolchain

The Rust version is pinned exactly in `rust-toolchain.toml`. To update:

1. `rustup check` to see the current stable version.
2. Change `channel` in `rust-toolchain.toml` to that exact version (for example `"1.100.0"`).
3. Run the full check list above, plus the Debian trixie container build.
4. Commit the toolchain change on its own, with the check output in the commit message.
