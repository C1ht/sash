<!-- SPDX-License-Identifier: Apache-2.0 -->
# Sash: step 1–3 report and compositor plan

## Context
New project (empty dir). Step 1 (CLAUDE.md, LICENSE, NOTICE) is done. Steps 2–3 =
research + plan only. No compositor code until the owner approves (step 4).

## Step 1: done
- `CLAUDE.md`: all rules from the brief, plus toolchain, milestone-routine, frame-callback and settings sections.
- `LICENSE`: canonical Apache-2.0 text from apache.org (202 lines, sha256 cfc7749b…d30).
- `NOTICE`: "Sash / Copyright 2026 The Sash Authors". Holder confirmed by the owner.

## Step 2: research findings
**Smithay**
- Latest release: **0.7.0** (2025-06-24), MSRV 1.80.1. Nothing newer on crates.io (checked API + `cargo search`).
- Git master is active (commits through 2026-10-20) but now needs **rust 1.87 / edition 2024**, so master can't build on trixie. We pin the 0.7.0 release.
- License MIT, which works with an Apache-2.0 project.
- anvil (v0.7.0) layout: `main.rs` (picks `--winit` / `--x11` / `--tty-udev`), `state.rs` (AnvilState + all protocol handler impls),
  `winit.rs` / `x11.rs` / `udev.rs` (one file per backend: event loop + render loop), `input_handler.rs`, `focus.rs`,
  `render.rs`, `drawing.rs`, `cursor.rs`, `shell/{mod,xdg,x11,grabs,element,ssd}.rs`.
  Backends are gated by cargo features (`winit`, `x11`, `udev`, `xwayland`, `egl`).
- `smallvil` in the same repo is a minimal winit-only compositor. It's the better reference for M1–M2.

**Toolchain / Debian 13 (checked via ftp-master madison today)**
- Trixie now ships rustc **1.85.1** (13.7 point release), libc6 2.41, mesa 25.0.7, systemd 257.13,
  libwayland 1.23.1, xkbcommon 1.7.0, libinput 1.28.1, libdrm 2.4.124, libseat 0.9.1, pixman 0.44.0.
- This machine has rustc 1.99.1, wayland 1.26, xkbcommon 1.13, libinput 1.32, mesa 26.2.5, systemd 262. All are newer than trixie, so "builds here" does not mean "builds on trixie".
- Compatibility is proven by building in a `debian:trixie` container with the same pinned toolchain; library-version problems found there get reported.

**Pacman packages (all already installed)**
rustup, gcc, pkgconf, wayland, libxkbcommon, mesa + libglvnd (EGL/GBM), libinput, systemd-libs (libudev),
seatd (libseat), libdrm, pixman, xorg-xwayland. Test clients `foot` and `wayland-utils` (provides `wayland-info`) are installed, plus `grim` for screenshots.
kitty is installed and works as a client.

**Nested under Hyprland (0.56.2 here)**
- Smithay's winit backend opens as an ordinary Wayland client window on Hyprland (`WAYLAND_DISPLAY=wayland-1`), using EGL/GLES.
- sash-wm creates its own socket (e.g. `wayland-2`) and logs it. Clients are launched with that socket:
  `WAYLAND_DISPLAY=wayland-2 foot`.
- Optional: a Hyprland window rule to float the nested window. I'll check the exact syntax for 0.56 at that point.

## Step 3: plan

### Workspace layout
```
sash/
  Cargo.toml          # [workspace] resolver="3", edition 2024
  rust-toolchain.toml # pinned recent stable (1.99.1)
  CLAUDE.md LICENSE NOTICE README.md .gitignore
  sash-wm/            # bin; smithay = { version="0.7.0", default-features=false,
                      #   features=["backend_winit","desktop","wayland_frontend"] }
                      #   DRM features are added at M7
    src/main.rs state.rs backend/winit.rs layout/{mod.rs,bsp.rs,floating.rs} input.rs
  sash-ui/            # lib stub only (SPDX header + empty lib.rs) until "later"
```
- Each source file starts with `// SPDX-License-Identifier: Apache-2.0`.

### Dev loop
- Build and check: `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `cargo test` (pinned toolchain).
- Run nested: `RUST_LOG=sash_wm=debug cargo run -p sash-wm`. If `WAYLAND_DISPLAY` is set, it runs nested (winit). The tty backend comes at M7.
- Test clients: `WAYLAND_DISPLAY=<socket> foot | kitty | wayland-info`.
- Before each milestone sign-off: a `debian:trixie` container build (podman, same toolchain via rustup) to prove it builds against trixie libraries.
- After each milestone: push `main` to the private remote.

### Milestones (compositor only, no visual effects)
- **M0 Skeleton.** Done.
- **M1 Nested window.** Done, pending approval.
- **M2 One client window.** compositor, xdg-shell, shm, seat, output. A client maps and gets keyboard and pointer focus.
  Proof:
  - An automated test with a small wayland-client test client that connects, maps an xdg_toplevel, and checks for keyboard and pointer focus events. Running foot by hand is extra.
  - An automated hidden-window test: hide the host window, check that a `wl_display.sync` roundtrip is still served, unhide, and check that exactly one frame follows.
  - Frame-callback policy (CLAUDE.md): no frame callbacks to clients while the output is not presenting; requests are still served.
- **M3 BSP tiling.** One `Layout` trait. The BSP tree is a plain data structure that is independent of window mode, so M4 reuses it unchanged:
  - The tree holds every window on the monitor. Each window has a mode flag (tiled or floating).
  - Geometry is computed only over tiled leaves.
  - Focus moves and swaps use a minimal hardcoded keybind set.

  Proof: unit tests on the tree and geometry, plus a 3-client run.
- **M4 Floating.**
  - **Modes and placement:**
    - Per-window tiled/floating state, behind the same `Layout` trait.
    - The existing per-monitor default mode decides how new windows start.
    - Bulk actions per monitor: float all and tile all, each with a keybind in the minimal hardcoded set.
    - Toggling is lossless. When a window floats, it keeps its leaf in the tree as a placeholder and its sibling takes the freed space. Geometry is computed over tiled leaves only. Re-tiling returns the window to its leaf. Each window remembers its last floating rectangle. Float-all starts from the current tiled rectangles; tile-all restores the tree.
    - Placement rules: client size hints, cascade or center, stay on output.
    - Dialogs, transients (xdg parent) and fixed-size windows (min == max) float and center over their parent.
    - Splash windows only exist as an X11 window type, so that rule takes effect when XWayland arrives.
  - **Move and resize:** mouse and keyboard, from any edge or corner. Min/max sizes respected.
  - **Stacking:** raise on focus; floating windows always above tiled ones.

  Proof:
  - Unit tests for lossless toggling: tile, float-all, tile-all returns the identical tree and rectangles.
  - Unit tests for changes while floating: float a window, open a new window, close the old sibling, re-tile.
  - Definition of "identical tree" in these tests: if nothing changed while windows floated, the tree after re-tiling is identical to the tree before floating, compared as structure, split directions, split ratios, window per leaf, and resulting rectangles. If the tree changed while floating (windows opened or closed), the expected tree is the tree before floating with exactly those edits applied the way they would apply with every window tiled:
    - A new window splits the focused tiled leaf.
    - A closed window's leaf is removed and its sibling takes the parent's place.
    - The placeholder keeps its position relative to the surviving nodes. If its sibling closed, the placeholder takes the parent's place.
    - Re-tiling must produce exactly that tree and its rectangles.
  - Unit tests for placement geometry.
  - A written manual checklist for how floating feels.
  - Frame time measured during a window drag.
- **M5 Snapping (separate milestone after M4).**
  - Snap zones are defined as data, with seven defaults: top edge = fullscreen, left half, right half, and the four corners.
  - Every snap zone has a keybind.
  - Windows also snap to neighbouring windows.
  - Proof: unit tests for snapping geometry, and frame time during a drag with snapping active.
- **M6 Input.** Full built-in default keybind table, merging the M3/M4 minimal set and the M5 snap-zone keybinds. Click and keyboard focus, xkb keymap, pointer and keyboard grabs. Every mouse action has a keybind. Proof: tests plus a written checklist run.
- **M7 DRM/KMS (later).** udev + libinput + libseat + GBM/EGL on a TTY. Starts with its own investigate-then-STOP pass.

Roadmap (no work now): layer-shell, XWayland.
Out of scope until asked: native settings protocol, decorations/effects, sash-ui.

### Verification
Every milestone report includes: real command output (build/clippy/test), the trixie container build result,
measured numbers where relevant (startup time, frame time), and a nested run with named clients. I stop after each one for approval.

### Owner decisions
- Copyright holder stays "The Sash Authors".
- Install foot and wayland-utils. Install podman only if it's missing.
- cgmath advisory (RUSTSEC-2026-0196): ignored in deny.toml until Smithay drops cgmath.
- Nested window: title "Sash", app id "sash-wm".
- Redraw miss accepted as hidden-window behaviour (no frame callbacks); no code change.
- Floating a window: its sibling takes the space, and the window keeps a placeholder leaf.
- Closed settings list recorded in CLAUDE.md.
- M5 snapping stays a separate milestone.
- Identity: The Sash Authors <115245472+C1ht@users.noreply.github.com>. Remote: https://github.com/C1ht/sash.git (private).
