# Sash — project rules

Permanent record for every session. These rules override defaults. Do not
change this file without the owner's approval.

## Project

- Sash is a Wayland desktop environment written in Rust.
- Repository: `sash`. Crates:
  - `sash-wm` — the compositor, built on Smithay.
  - `sash-ui` — the first-party UI toolkit (later).
- Do not create any other crate without asking first.
- Development platform: Arch Linux. Target distro: Debian 13 (trixie).
  Everything must build and run with what trixie ships (see "Target
  constraints").

## Names

- Use only: sash, sash-wm, sash-ui.
- First-party apps get plain, unbranded names: Settings, Files,
  Text Editor, etc. No "Sash Files", no invented product names.

## Window management

- Exactly two window modes: **BSP tiling** and **floating**.
- Both sit behind one layout trait.
- Never add a third mode (no stacking/tabbed/monocle/scrolling, etc.).
- Window state is tiled or floating, per window. Still exactly two
  modes, one Layout trait.
- The per-monitor default mode (tiled or floating) decides how new
  windows start. It is the existing "per-monitor default mode" setting,
  not a new one. Do not add other settings.
- Bulk actions per monitor: float all, tile all. Each has a keybind.
- Toggling is lossless: the BSP tree is kept while windows float, and
  each window remembers its last floating rectangle. Float-all starts
  from the current tiled rectangles; tile-all restores the tree.
- Dialogs, transients (xdg parent), splash and fixed-size windows float
  and center over their parent regardless of the default mode.

## Settings and configuration

- The compositor owns all settings.
- No Lua, no scripting, no plugins, no user config file.
- Clients (Settings app, shell, CLI) talk to the compositor through a
  native Sash protocol.
- The settings list is closed. Do not add a setting without asking.

## UI

- First-party apps use sash-ui. Never GTK, never Qt.
- UX rules:
  - Enumerable values are pickers, never free text.
  - Every mouse action has a keybind.
  - No hidden controls.
  - Confirmations only for destructive actions.
  - Undo instead of dialogs.

## Working rules

- New area: investigate, report, then STOP and wait for approval.
- Confirm with the owner before moving to the next step or milestone.
- No features beyond what was asked.
- Report measurements and real test/command output, never "looks done".

## Licensing

- All first-party code is Apache-2.0.
- Every source file starts with an SPDX header, e.g. for Rust:
  `// SPDX-License-Identifier: Apache-2.0`
- The repo root has `LICENSE` (full Apache-2.0 text) and `NOTICE`.

## Toolchain and target compatibility

- Rust: a recent stable toolchain, pinned to an exact version in
  `rust-toolchain.toml`. Edition 2024. Update it deliberately (see README).
- Smithay: stay on the 0.7.0 release from crates.io (not git master).
- Debian 13 trixie ships glibc 2.41, Mesa 25.0.7, systemd 257,
  libwayland 1.23, xkbcommon 1.7, libinput 1.28. Compatibility is proven by
  building in a `debian:trixie` container with the same pinned toolchain;
  report any library-version problem found there.
- Dependency licenses are checked with `cargo deny check` (`deny.toml`).
  A dependency outside the allowed list is reported to the owner, never
  allowed on my own.

## Milestone routine

- After each milestone: build/clippy/fmt/test output, trixie container
  build, push `main` to the private remote, then STOP for approval.
