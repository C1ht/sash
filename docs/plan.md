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
- **M1 Nested window.** Done (tag `m1`).
- **M2 One client window.** Done (tag `m2`). compositor, xdg-shell, shm, seat, output. A client maps and gets keyboard and pointer focus.
  Deviations and findings:
  - `wl_data_device_manager` is advertised (no clipboard logic), because foot refuses to start without it.
  - The EGL swap interval is set to 0 in the nested backend. Smithay leaves it at 1, which gave 30 fps and blocked the event loop for about 17 ms per frame.
  - Negative xdg_toplevel min/max sizes are rejected before Smithay sees them. Smithay 0.7 panics on them in debug builds.
  - Smithay 0.7 does not expose a toplevel's xdg_surface. So a buffer before ack_configure gets `not_constructed` (not `unconfigured_buffer`), and an invalid window geometry is ignored and logged instead of answered with `xdg_surface.invalid_size`.
  - Popups are not handled yet; `new_popup` is a no-op.
  Proof:
  - An automated test with a small wayland-client test client that connects, maps an xdg_toplevel, and checks for keyboard and pointer focus events. Running foot by hand is extra.
  - An automated hidden-window test: hide the host window, check that a `wl_display.sync` roundtrip is still served, unhide, and check that exactly one frame follows.
  - Frame-callback policy (CLAUDE.md): no frame callbacks to clients while the output is not presenting; requests are still served.
- **M3 BSP tiling.** Done (tag `m3`). Design rules are in CLAUDE.md ("BSP tiling (M3)", "Keybinds"). Summary: one `Layout` trait. The BSP tree is a plain data structure that is independent of window mode, so M4 reuses it unchanged:
  - The tree holds every window on the monitor. Each window has a mode flag (tiled or floating).
  - Geometry is computed only over tiled leaves.
  - Focus moves and swaps use a minimal hardcoded keybind set.

  - New window splits the focused leaf along its longer side, ratio 0.5, new window second and focused; integer geometry (first child floor, second the remainder).
  - Close: sibling takes the parent's place; focus to the most recently focused window in that subtree.
  - Focus move and swap by direction (nearest overlapping neighbour, ties by most recently focused, no wrap). Swap keeps the tree structure.
  - Min/max size hints ignored while tiled (known limit). No gaps, borders, resize keybinds or workspaces.
  - Resize sync: one batch of configures; until a client commits at the new size, its last buffer is drawn at the new origin, clipped to the new rectangle.
  - Layout keeps updating while the output is hidden.
  - Minimal keybinds (mod = Alt nested, Super on DRM): mod+Return foot (debug builds only), mod+Q close, mod+H/J/K/L focus, mod+Shift+H/J/K/L swap, mod+Shift+E quit.
  - Owner decisions: square leaf splits side by side; focus follows a swapped window; focus ties without history go by position; tiled states in configures; unmap leaves the tree and re-inserts on remap; dialogs tile until M4.

  Proof:
  - Unit tests on the tree: open, close, swap, focus movement; rectangles cover the output exactly with no overlap; every window in exactly one leaf. Placeholder leaves (M4) take no space, and removing a placeholder lets its sibling take the parent's place.
  - Property tests (proptest) over random open/close/swap/focus sequences on random output sizes, same invariants.
  - Headless integration tests: three clients get the right configure sizes; closing the middle one refills the output; swap changes positions; focus follows the keybinds; the layout updates while the output is hidden.
  - Edge cases: one pixel of remainder, a very small output, closing the last window, closing a window with an unacked configure.
  - Implementation notes:
    - A toplevel enters the tree on its initial commit, so its initial configure already carries the tile size. It is drawn from its first buffer. It leaves the tree on unmap or destroy.
    - Tiles on outputs too small to give every window a pixel can be 0 wide or high. They are configured as 1 (0 would mean "client chooses") and are not reachable by focus moves.
  - Resize transactions (owner decision: not now; revisit after M4 with the floating checklist; the ~150 ms timeout would be a constant, never a setting): Keep showing the old arrangement until every affected client has committed a buffer for its new configure, or a timeout (about 150 ms) passes; then switch in one frame. Clients that miss the timeout fall back to the clip rule. This removes the frame where windows show stale or partly empty content at their new tiles, at the cost of up to one timeout of latency.
  - Screenshots of three tiled foot windows, before and after a swap. Startup, frame time with 3 clients, layout time after a keybind, idle RAM and CPU.
- **M4 Floating.** Done (tag `m4`). Manual checklist: docs/floating-checklist.md. Rules are in CLAUDE.md ("Floating (M4)", "Keybinds"). Summary:
  - Forced floating (dialogs, min == max), re-checked per commit; per-monitor default mode (tiled at start, debug toggle); float-all / tile-all; lossless toggling; placement (client size, center, cascade 32 px, wrap); floating above tiled, unclipped, 64 px visible per axis; z-order and click-to-focus; mod+drag move (floats tiled windows on press) and 3x3-grid resize; client move/resize requests; 40 px keyboard move/resize; hold-to-cycle mod+Tab.
  - Measurements: frame time and a frame-interval histogram during a drag and a resize, pointer-event-to-frame latency, time of toggle / float-all / tile-all, idle RAM and CPU with 3 windows. Real pointer drags need uinput/ydotool privileges, so they are measured only where no extra privilege is needed.
  - Deliverable: docs/floating-checklist.md, a manual checklist for how floating feels.
  - Implementation notes:
    - The `Layout` trait now has set_area / insert(hint) / remove / contains / rects; focus and history moved to `MonitorLayout`.
    - Every window keeps a leaf in the BSP tree; floating and forced-floating windows are placeholders.
  - Open item (no work): frame pacing in the nested backend. Render intervals spread from about 8 to 25 ms around a 16.9 ms mean, because sash-wm renders when both the host's frame callback and a client commit have arrived. The host's presentation timing is not visible from inside; measuring it needs presentation feedback from the host.
  Original M4 notes:
  - **Modes and placement:**
    - Per-window tiled/floating state, behind the same `Layout` trait.
    - The existing per-monitor default mode decides how new windows start.
    - Bulk actions per monitor: float all and tile all, each with a keybind in the minimal hardcoded set.
    - Toggling is lossless. When a window floats, it keeps its leaf in the tree as a placeholder and its sibling takes the freed space. Geometry is computed over tiled leaves only. Re-tiling returns the window to its leaf. Each window remembers its last floating rectangle. Float-all starts from the current tiled rectangles; tile-all restores the tree.
    - Placement rules: client size hints, cascade or center, stay on output.
    - Dialogs, transients (xdg parent) and fixed-size windows (min == max) float and center over their parent. They carry a separate "forced floating" flag; tile-all never touches them.
    - A new window opened while the default mode is floating gets a placeholder leaf next to the focused leaf. Placeholders take no space, so nothing else moves.
    - Closing a floating window removes its placeholder; its sibling takes the parent's place.
    - Splash windows only exist as an X11 window type, so that rule takes effect when XWayland arrives.
  - **Move and resize:** mouse and keyboard, from any edge or corner. Min/max sizes respected.
  - **Stacking:** raise on focus; floating windows always above tiled ones.

  Proof:
  - Unit tests for lossless toggling: tile, float-all, tile-all returns the identical tree and rectangles.
  - Unit tests for changes while floating: float a window, open a new window, close the old sibling, re-tile.
  - Unit test: closing a floating window removes its placeholder and its sibling takes the parent's place.
  - Unit test: tile-all leaves forced-floating windows floating.
  - Unit test: a new window while the default mode is floating gets a placeholder next to the focused leaf, and no tiled rectangle changes.
  - Unit test: the output size changes between float and re-tile. The tree structure stays identical and the rectangles are recomputed for the new size.
  - Definition of "identical tree" in these tests: if nothing changed while windows floated, the tree after re-tiling is identical to the tree before floating, compared as structure, split directions, split ratios, window per leaf, and resulting rectangles. If the tree changed while floating (windows opened or closed), the expected tree is the tree before floating with exactly those edits applied the way they would apply with every window tiled:
    - A new window splits the focused tiled leaf.
    - A closed window's leaf is removed and its sibling takes the parent's place.
    - The placeholder keeps its position relative to the surviving nodes. If its sibling closed, the placeholder takes the parent's place.
    - Re-tiling must produce exactly that tree and its rectangles.
    - "Identical rectangles" assumes the output size is unchanged. If the output size changed, only the structure must be identical, and the rectangles are recomputed.
  - Unit tests for placement geometry.
  - A written manual checklist for how floating feels.
  - Frame time measured during a window drag.
- **M5 Snapping and fullscreen.** Rules are in CLAUDE.md ("Snapping and fullscreen (M5)"). Step 0, a separate commit first: debug-only offscreen capture (below), used for all screenshots from now on.
  - Constants (owner tunes after trying): corner trigger 64 px from two edges; edge trigger 8 px; edge-snap threshold 12 px; preview fill #0A84FF at 30% alpha, 2 px border at 80% alpha.
  - Maximize vs fullscreen: until layer-shell exists both fill the whole output. Once an exclusive zone exists (panel), maximize will fill the output minus exclusive zones, fullscreen still the whole output.
  - Proposed keybind state table (mod+arrow), awaiting owner approval:

    | From \ key   | Left        | Right       | Up          | Down        |
    |--------------|-------------|-------------|-------------|-------------|
    | unsnapped    | left half   | right half  | fullscreen zone | nothing |
    | left half    | bottom-left | right half  | top-left    | restore     |
    | right half   | left half   | bottom-right| top-right   | restore     |
    | top-left     | nothing     | top-right   | fullscreen zone | left half |
    | top-right    | top-left    | nothing     | fullscreen zone | right half |
    | bottom-left  | nothing     | bottom-right| top-left    | left half   |
    | bottom-right | bottom-left | nothing     | top-right   | right half  |
    | fullscreen zone | left half | right half | nothing    | restore     |

    - Every zone is reachable: L (Left), R (Right), full (Up), TL (L, Up), TR (R, Up), BL (L, Left), BR (R, Right).
    - Repeated Down always ends at restore: quarter -> half -> restore; full -> restore.
    - Inside quarters, arrows move spatially; from a half, the arrow toward the edge it already touches goes to that side's bottom quarter (Up already goes to the top quarter).
    - "Restore" = the pre-snap floating rectangle. mod+arrows do not clash with mod+H/J/K/L (focus), mod+Shift+H/J/K/L (swap) or mod+Ctrl+H/J/K/L (move).
- **Previous M5 notes (snapping):**
  - Snap zones are defined as data, with seven defaults: top edge = fullscreen, left half, right half, and the four corners.
  - Every snap zone has a keybind.
  - Windows also snap to neighbouring windows.
  - Proof: unit tests for snapping geometry, and frame time during a drag with snapping active.
- **M6 Input.** Full built-in default keybind table, merging the M3/M4 minimal set and the M5 snap-zone keybinds. Click and keyboard focus, xkb keymap, pointer and keyboard grabs. Every mouse action has a keybind. Proof: tests plus a written checklist run.
- **M7 DRM/KMS (later).** udev + libinput + libseat + GBM/EGL on a TTY. Starts with its own investigate-then-STOP pass.

Proposals (no work until approved):
- **Tiled resize by split ratio.** mod+right-drag on a tiled window and mod+Ctrl+Shift+H/J/K/L on a tiled window change the ratio of the nearest split on that side (40 px steps, constant); each split keeps its own ratio, still integer geometry. Fits best as a small milestone right after M5: it reuses M4's grabs and keyboard resize, and M6 then puts its keybinds into the full default table.
- **Offscreen capture (debug builds only).** On SIGUSR1, sash-wm renders the current output once more into an offscreen GLES texture with the same render elements, reads it back and writes a PNG to `$XDG_RUNTIME_DIR/sash-wm-capture-<n>.png`. Screenshots then never touch the host screen and work while the nested window is hidden. Needs a PNG encoder dependency (the `png` crate, MIT OR Apache-2.0), which would be a new dependency to approve.

Roadmap (no work now), in the proposed order:
0. **presentation-time (wp_presentation).** On the DRM backend, so on-screen frame timing can be measured (the nested backend cannot see the host's presentation; owner decision after M4: no pacing work nested).
1. **xdg popups.** Needed before any sash-ui app (menus, dropdowns) and by most existing apps.
2. **linux-dmabuf.** Before the DRM backend: GPU clients need it (kitty falls back to software GL today), and it is the buffer path the DRM backend scans out. Once it exists, remeasure stale content during resizes with a heavier client (browser/Electron) before deciding on resize transactions.
3. **cursor-shape.** Before the DRM backend: on DRM, sash-wm draws the cursor itself (nested, the host draws it).
4. **Clipboard and drag-and-drop.** Turns the `wl_data_device_manager` stub into a working clipboard (selection follows keyboard focus), then drag-and-drop.
5. **xdg-decoration.** Lets clients drop their own title bars in the tiled layout; sash-wm draws none (no borders yet).
6. **layer-shell.** For the shell (panel, launcher, notifications), which is a sash-ui client.
7. **fractional-scale and viewporter.** HiDPI monitors; together, since fractional scaling relies on viewporter.
8. **XWayland.** Last: the largest piece, and only for legacy X11 apps. Splash windows (forced floating) come with it.
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
