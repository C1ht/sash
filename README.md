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

## Debian 13 (trixie) packages

- Build: `build-essential pkg-config libxkbcommon-dev`
- Runtime and tests (loaded with dlopen): `libwayland-client0`, plus `libegl1` for the nested window

## Updating the toolchain

The Rust version is pinned exactly in `rust-toolchain.toml`. To update:

1. `rustup check` to see the current stable version.
2. Change `channel` in `rust-toolchain.toml` to that exact version (for example `"1.100.0"`).
3. Run the full check list above, plus the Debian trixie container build.
4. Commit the toolchain change on its own, with the check output in the commit message.
