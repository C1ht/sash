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
  - Popups were not handled until M7.
  Proof:
  - An automated test with a small wayland-client test client that connects, maps an xdg_toplevel, and checks for keyboard and pointer focus events. Running foot by hand is extra.
  - An automated hidden-window test: hide the host window, check that a `wl_display.sync` roundtrip is still served, unhide, and check that exactly one frame follows.
  - Frame-callback policy (CLAUDE.md): no frame callbacks to clients while the output is not presenting; requests are still served.
- **M3 BSP tiling.** Done (tag `m3`). Design rules are in CLAUDE.md ("BSP tiling (M3)", "Keybinds"). Summary: one `Layout` trait. The BSP tree is a plain data structure that is independent of window mode, so M4 reuses it unchanged:
  - The tree holds every window on the monitor. Each window has a mode flag (tiled or floating).
  - Geometry is computed only over tiled leaves.
  - Focus moves and swaps use a minimal hardcoded keybind set.

  - New window splits the focused leaf along its longer side, ratio 0.5, new window second and focused; integer geometry (first child floor, second the remainder). Changed after stage C run 1 (owner, rule A): the tiled window under the pointer is split, otherwise the focused one. After m9c: a pointer that has not moved since startup counts as no pointer; a parked pointer deciding keybind spawns and the repeated cutting of one area are documented behaviour; the alternative "pointer only if it moved in the last few seconds" is recorded, not built.
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
  - Definition of "identical tree" in these tests: if nothing changed while windows floated, the tree after re-tiling is identical to the tree before floating, compared as structure, split directions, split ratios (exact fractions since M6, so a tiled resize before floating survives the round trip), window per leaf, and resulting rectangles. If the tree changed while floating (windows opened or closed), the expected tree is the tree before floating with exactly those edits applied the way they would apply with every window tiled:
    - A new window splits the focused tiled leaf (with rule A: the tiled leaf under the pointer, if any).
    - A closed window's leaf is removed and its sibling takes the parent's place.
    - The placeholder keeps its position relative to the surviving nodes. If its sibling closed, the placeholder takes the parent's place.
    - Re-tiling must produce exactly that tree and its rectangles.
    - "Identical rectangles" assumes the output size is unchanged. If the output size changed, only the structure must be identical, and the rectangles are recomputed.
  - Unit tests for placement geometry.
  - A written manual checklist for how floating feels.
  - Frame time measured during a window drag.
- **M5 Snapping and fullscreen.** Done, pending approval. Manual checklist: docs/floating-checklist.md section 10. Rules are in CLAUDE.md ("Snapping and fullscreen (M5)"). Step 0, a separate commit first: debug-only offscreen capture (below), used for all screenshots from now on.
  - Constants (owner tunes after trying): corner trigger 64 px from two edges; edge trigger 8 px; edge-snap threshold 12 px; preview fill #0A84FF at 30% alpha, 2 px border at 80% alpha.
  - Maximize vs fullscreen: until layer-shell exists both fill the whole output. Once an exclusive zone exists (panel), maximize will fill the output minus exclusive zones, fullscreen still the whole output.
  - Keybind state table (mod+arrow), approved by the owner:

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
- **M6 Tiled resize and the full keybind table.** Done, pending approval. Keybinds: docs/keybinds.md (generated). Manual checklist: docs/floating-checklist.md section 11. Rules are in CLAUDE.md ("Tiled resize (M6)", "Debug-only tools", "Keybinds").
  - Step 0, a separate commit first: debug-only synthetic input (unix socket, mode 0600, same handlers as the backend), never in release builds; used for scripted drag, resize and snap runs so frame times during real drags are measured. Release builds ignore SIGUSR1.
  - Tiled resize: split ratios as exact fractions; keys and mod+right-drag move the split on the resized side; 64 px minimum tile; output-boundary edges do nothing; lossless toggling keeps ratios; live resize with configures batched per frame.
  - "Identical tree" now also compares split ratios (exactly, as fractions).
  - Full keybind table as data, docs/keybinds.md generated and checked by a test, duplicate-chord test.
  - Tests: ratio math, property tests with tiled resizes (coverage, 64 px minimum), integration (keyboard and scripted mouse resize, output-boundary drag, lossless toggle and output resize keep ratios), edge cases (close during resize, hidden output during resize).
  - Measure: frame time and interval histograms during a scripted floating drag, a snap drag with the overlay and a tiled resize; ratio update and rectangle recompute time; idle with 3 windows.
- **M7 xdg popups.** Done, pending approval. Manual checklist: docs/floating-checklist.md section 12. Rules and owner decisions are in CLAUDE.md ("Popups (M7)").
  - xdg_popup and xdg_positioner (anchor, gravity, offset, size, slide/flip/resize per axis), reposition, nested popups, tooltips and grabbed popups, on tiled, floating, snapped and fullscreen windows. Smithay's documented popup support is used.
  - Placement constrained to the output; popups draw directly above their parent (popups of tiled windows above every tile), unclipped, follow it, and receive pointer input outside the parent's tile. Reactive popups are re-constrained when the parent or output changes; on an output resize non-reactive popups are dismissed.
  - Grabs: keyboard focus to the popup; a click outside dismisses the chain innermost first and is not delivered; focus returns to the parent. Every keybind and mod+drag start dismisses the target window's chain first, except keyboard resize and mod+right-drag resize (popups follow) and the debug default-mode toggle.
  - Lifetime and errors per protocol: parent unmap or destroy dismisses; not_the_topmost_popup on out-of-order destroy; an invalid grab dismisses.
  - Subsurfaces: investigate and report only (drawing and pointer input with a real client); build a fix only if it is a few lines. Result: drawn and routed already (Firefox renders through a subsurface; a headless test covers pointer input on subsurfaces of windows and popups); no fix needed.
  - Tests: positioner math (every anchor/gravity, offsets, each adjustment alone and together, protocol order, popup larger than the output, reposition); property tests (inside the output when the adjustments allow it, deterministic, earlier invariants); headless integration (configure position, click-outside dismissal, nested dismissal order, following a moved/snapped/resized parent, parent unmap, out-of-order destroy, invalid grab serial, keybind dismissal, tooltip without focus, pointer input outside the tile, reposition); edge cases (parent unmapped, client disconnect with an open chain, hidden output, output resize, fixed-size parent at the output edge). A real client with menus: pavucontrol (installed).
  - Manual checklist: a popups section in docs/floating-checklist.md.
  - Measure with synthetic input: frame times and intervals with a popup chain open while the parent moves; popup placement and render cost; idle RAM and CPU with 3 windows and an open menu.
- **M8 linux-dmabuf and cursor-shape.** Done, pending approval. Manual checklist: docs/floating-checklist.md section 13. Rules are in CLAUDE.md ("linux-dmabuf and cursor-shape (M8)").
  - Investigation first, in a docs-only commit: Smithay 0.7.0's dmabuf support (feedback v4: main device, tranches, formats, modifiers), import into GLES, what the nested winit backend can offer, what the NVIDIA driver imports; explicit sync (Smithay, NVIDIA, Hyprland), report only; cursor handling today (nested and DRM) and what wp_cursor_shape_v1 needs.
  - zwp_linux_dmabuf_v1 at the highest version Smithay supports, feedback from the renderer's importable formats; only verified formats and modifiers advertised; import on attach; protocol errors or the failed event on failure, never a panic; limits as constants (listed here after the investigation).
  - dmabuf buffers on the same render paths as shm (clipping, floating, popups, snap preview); frame callbacks and release per protocol.
  - wp_cursor_shape_manager_v1; nested: the shape goes to the host window's cursor through winit; client cursor surfaces keep working. No settings. No explicit sync without approval.
  - Tests: headless params errors with a fake format table (duplicate plane, missing plane, bad dimensions, unsupported format, modifier mismatch, create before all planes, destroy after create_immed; the server survives, a new client connects); ignored GPU tests importing a real dmabuf and sampling pixels through the capture path; property tests mixing dmabuf and shm windows and popups; edge cases (disconnect with a dmabuf attached, destroyed before commit, resize with different buffer sizes, a dmabuf popup, cursor shape without pointer focus).
  - By hand (throwaway profiles): kitty and foot on the GPU, Firefox with a local animated page (no network), pavucontrol. Compare with M7: drag frame times and intervals, stale content (configure to first buffer at the new size) for foot, pavucontrol and Firefox, idle RAM and CPU. Manual checklist: a dmabuf section in docs/floating-checklist.md.
  - Remeasure stale content during resizes with a heavier client (browser) once dmabuf exists, before deciding on resize transactions.
  - Docs only: the M9 plan (docs/m9-plan.md), with safe testing on the owner's daily NVIDIA machine.
  - Investigation results: docs/m8-investigation.md. Limits (constants): 4 planes per dmabuf (Smithay's MAX_PLANES); at most 16384 x 16384.
- **M8.1 Quarter snapping.** Done, pending approval. Manual checklist: docs/floating-checklist.md section 14. Rules are in CLAUDE.md ("Quarter snapping (M8.1)").
  - First a report of what exists: the mod+arrow state table and how each quarter is reached by mouse and keyboard, with a test or screenshot for each.
  - mod+Y / mod+U / mod+B / mod+N snap to the top-left / top-right / bottom-left / bottom-right quarter from any state except fullscreen (ignored); a tiled window floats first; the current quarter's chord does nothing; arrows unchanged; four new keybind table rows.
  - Tests: unit (every chord from every state, exact rectangles on odd and tiny outputs, pre-snap restore), property (chords in the random sequences, M5 invariants), integration (chord on a tiled window with the configure size, from a half, restore with mod+Down, keys never reach clients). Checklist: a quarters section. Offscreen screenshots of all four quarters.
- **M9 DRM/KMS with presentation-time.** Plan: docs/m9-plan.md. Stages A and B done, pending approval (tag m9a). Rules: CLAUDE.md ("DRM/KMS backend, M9 stages A and B").
  - Results (stages A and B): the probe (tools/drm-probe.py) found one GPU (card1, nvidia-drm), HDMI-A-1 at 3440x1440@60 preferred, atomic, a cursor plane up to 256x256, SYNCOBJ_TIMELINE. The VM run (tools/vm/scenario.py, Debian 13 guest, virtio-gpu, Mesa 25.0.7 software rendering) passes 13 of 13 scripted checks. Decisions made in stage B: winit stays the default backend and DRM is chosen with `--backend drm`; named cursor shapes use the built-in arrow on DRM; sash-wm waits up to 10 s (constant) for its session to become active.
  - Stage A (host, read-only): a probe in tools/ listing DRM devices, connectors, modes, planes and plane formats with queries only; no DRM master, no mode set.
  - Stage B (VM only: QEMU/KVM, virtio-gpu, minimal Debian 13 guest, the release binary from the trixie container): Smithay's DRM/KMS path (libseat, udev, libinput, GBM/EGL, atomic modesetting on one connector at its preferred mode, page-flip pacing, presentation-time, hardware cursor with software fallback, VT switching via session pause/resume); modifier Super; mod+Shift+E; VT switches before anything else; debug-only `--exit-after <seconds>`; startup errors restore the display. Input per the owner's decision (fixed defaults, XKB_DEFAULT_* honoured). Hotplug out of scope; unplugging must not crash. Tests without GPU or session: mode selection, the pacing state machine, pause/resume sequences, input device add/remove with fakes; in the VM by script where possible. Measure in the VM (frame intervals under a drag, idle CPU and RAM); VM numbers do not predict NVIDIA hardware.
  - Docs: docs/m9-stage-c.md, the exact checklist for the first run on the real machine.
  - After stage B (owner decisions, rules in CLAUDE.md): the top-edge rule for floating windows; automatic backend selection; a render cap at the nominal refresh when flips are not vblank-paced; same-connector disconnect/reconnect (stop, then clear and modeset at the same mode); tools/stage-c.sh and monitor power-off/on and cable-pull runs in docs/m9-stage-c.md; a VM check that a udev event arrives.
  - After stage C run 1 (owner, tag m9c): debug recording screenshots are outlines by default (`SASH_DEBUG_SCREENSHOTS=full` for contents); rule A for new windows; a frame per pointer motion when sash-wm draws the cursor (merged to one per refresh); pacing classification ignores the first flip after a modeset or resume and needs 3 consecutive short intervals; replay of bundles in the test harness (tools/replay.sh).
  - After m9c (owner): rule A ignores a pointer that has not moved since startup; the recorder logs the cursor path on DRM (plane or composited) and counts plane frames, composited frames and pointer frame requests per second. docs/v1.md keeps its one gate; the owner writes the v1 checklist.
  - M9d (owner, after the first real-hardware bundle, run 4): no frames for pointer motions that leave the clamped position unchanged (drags at a screen edge); udev change events merged into one connector re-check per 200 ms window, after libinput is back on resume; refused page flips around a pause logged quietly and counted; the debug hotkey logs each press and where the bundle went; the replay harness reads stand-in events, has a longer timeout and an unlimited recorder, and the run-4 bundle (cut at 20.0 s, downsampled) is a regression fixture. Open for the owner: focus on open (keys typed between a new window's first commit and its first buffer are lost); skipping re-checks for events about other connectors (needs the uevent's properties, which Smithay 0.7 does not pass on).
  - M9e (owner): focus moves to a new window when it maps, not when it opens (keys before the map go to the old window); timing points around the resume (recorder and debug log); a test that checks the real-hardware replay fixture's contents. Part 2(c) of M9d is not built: revisit with a direct udev monitor (the `udev` crate, already approved through Smithay's udev feature) if the one remaining connector re-check stall after a resume (about 190 ms on NVIDIA) is noticeable. No dependency change.
  - Mode selection rule: approved by the owner after stage B, with TEST_ONLY validation and a fallback chain, placeholder physical sizes, and a debug-only `--mode WxH@R` (rules in CLAUDE.md). Reconnect keeps the previous mode if offered, else the rule (confirmed). Backend selection needs XDG_SESSION_ID, XDG_SEAT and XDG_VTNR. The VM lives in ~/.cache/sash/vm. Tag m9b.
  - Done after stage B, pending approval: the decisions above are built and tested (unit, property and headless tests; the VM scenario passes 16 of 16 checks, including a udev change event with unplug and replug, and tools/stage-c.sh on a guest console). Reconnect when the display no longer offers the mode in use: the mode-selection rule picks one (Claude's decision, to be confirmed).
  - Stage C (first bare-metal run) only with the owner. Explicit sync (wp_linux_drm_syncobj_v1) is decided at that first bare-metal run (owner decision after M8).

Proposals (no work until approved):
- **Offscreen capture (debug builds only).** On SIGUSR1, sash-wm renders the current output once more into an offscreen GLES texture with the same render elements, reads it back and writes a PNG to `$XDG_RUNTIME_DIR/sash-wm-capture-<n>.png`. Screenshots then never touch the host screen and work while the nested window is hidden. Needs a PNG encoder dependency (the `png` crate, MIT OR Apache-2.0), which would be a new dependency to approve.

- **M10 Clipboard and drag and drop.** Done, pending approval (tag m10). Rules: CLAUDE.md ("Clipboard and drag and drop (M10)"). Manual checklist: docs/floating-checklist.md section 15.
  - Results: a 10 MB paste takes 8 ms through the pipe (headless and nested wl-paste) and the event loop is never blocked (its iterations stay at the 2 ms dispatch timeout during the transfer); sash-wm's own pointer-to-frame time during a drag is 6 us p50, 9 us p99 (release, headless); idle with 3 foot windows 0.000% CPU, RSS 121.7 MB (17.7 MB anonymous), PSS 48.2 MB. Found in the by-hand run: Firefox sets the same source as the selection again; sending used_source for it crashed Firefox, so a reused source is accepted (CLAUDE.md). Nested on the owner's NVIDIA desktop, Firefox makes the winit backend lose its EGL context (m9e does the same; not an M10 change), which stopped the nested drag measurement.
  - sash-wm's own wl_data_device_manager (v3), wl_data_source, wl_data_device and wl_data_offer: Smithay 0.7's implementation cannot cap offers per client or rate-limit receive requests.
  - Selection offered only to the keyboard-focused client; receive refused for a client without keyboard focus; the selection ends with its source.
  - Limits (owner): 32 MIME types, 256 bytes per MIME string and 4096 bytes of MIME names per source; 16 live offers per client; 16 receive requests per offer per second (sliding window; beyond, the fd is closed and counted). Ignored and logged, no disconnect.
  - Drag and drop with icon (above everything, hotspot, no pointer focus), copy/move/ask, drop/finish/cancel, Escape and focus keybinds cancel, serial from the implicit grab's press only.
  - Privacy decisions (owner): no wlr-data-control, ext-data-control or other clipboard-manager protocol (any holder reads everything copied); decided later. No primary selection for now (below).
  - Nested: no host clipboard bridge (known limit).
  - Recorder: events without data or MIME names; marker test over every bundle file.
  - Tests: headless copy/paste (small and 10 MB, event-loop latency measured during the transfer), selection replace/clear/source and destination disconnect/focus rules; drags (between windows, to a popup, to nothing, at an edge, Escape, focus keybind, disconnects, icon hotspot, invalid serial); protocol errors; the receive rate (40 pastes over 10 s succeed; 100 in one second get end of file beyond the 16th); property tests with selections and drags keeping every layout invariant; VM copy and paste with wl-clipboard (apt in the guest).
  - By hand (throwaway profiles): foot, pavucontrol, a local Firefox test page (text, an image, a dragged file name). Checklist: docs/floating-checklist.md.
  - Primary selection (zwp_primary_selection_device_manager_v1), pros and cons for a privacy-first desktop. Owner decision (M10): left out for now.
    - For: middle-click paste is a strong habit for terminal and X11 users; foot, Firefox and GTK/Qt apps support it; it needs no keyboard shortcut.
    - Against: every highlighted text becomes readable by the next focused client without any copy action, so text selected only to read it (a password in a terminal, a private message) leaks with one stray middle-click; it doubles the selection state to explain and to protect; it is easy to paste by accident.
    - If built later: the same focus rule as the clipboard (offered only to the keyboard-focused client, no data-control).

- **After M10 (owner):** decisions confirmed (MIME name limits, close/swap/quit cancel a drag, reused sources accepted). New process rule for networked and crash-reporting clients and for clients that show device names (CLAUDE.md, enforced in tools/). Investigate only: the nested window stops drawing once Firefox runs (lost EGL context).
- **Known limit: the clipboard empties when its source closes.** sash-wm passes the reader's fd to the source client and keeps no copy, so once the copying app closes, there is nothing to paste. Options (none built; owner decides):
  1. Keep as is. For: sash-wm never sees a payload (the M10 rule holds), no memory or privacy cost, the standard Wayland behaviour. Against: copy, close the app, paste gives nothing (surprises users coming from desktops with a clipboard keeper).
  2. A small compositor-side keeper for text only: when the source goes away, sash-wm has already read the text types (bounded size) and serves them itself. For: the common case (copy text, close the app) works with nothing to install. Against: breaks the current rule that sash-wm never reads a payload; the compositor holds copied passwords in its memory (they outlive the app, and a debug build's recorder must be proven never to see them); a read can block or be slow (needs a bounded async reader and limits); MIME choice and size limits become policy.
  3. A separate opt-in clipboard app (a sash-ui client) that copies the selection when its source goes away. For: sash-wm stays payload-free; users who want history opt in; it can show and clear what it keeps (no hidden state). Against: it needs a way to read the selection without keyboard focus, which is exactly the data-control access M10 refused, unless a narrower native Sash protocol grants it to that one first-party app; one more app to build.
- **M11 Cursor themes and xdg-decoration: investigate and propose first.** Rules: CLAUDE.md ("Cursor themes, xdg-decoration (M11)"). Report: docs/m11-investigation.md. No implementation; waiting for the owner.
  - Nested window going blank: the swap fails (Smithay recreates the EGL window surface, eglCreateWindowSurface gives EGL_BAD_ALLOC, reported as "context lost") in about half the runs with GPU Firefox, also before M10; the nested backend then never draws again. Neither backend recreates its renderer; a recovery rule is proposed.
  - Cursor themes: proposal: the system's Adwaita (Debian adwaita-icon-theme 48.1, CC-BY-SA-3.0 or LGPL-3, all 36 shapes, sizes 24 to 96), not bundled, size 24, built-in arrow as fallback, xcursor 0.3.11 as a direct dependency (to approve) or an own parser.
  - xdg-decoration: recommendation (4) built as 3b: server-side mode for every client that supports it, sash-wm draws a bar (maximize, close) only on floating windows; sash-ui apps client-side later.
  - M12 effects: themes before or with M12, usable area and z-order with bars, one text stack.
- **M11a Cursor themes and renderer recovery.** In progress. Rules: CLAUDE.md ("Cursor themes (M11a, owner)", "Renderer recovery (M11a, owner)").
  - Cursor themes: Adwaita from /usr/share/icons, size 24, xcursor 0.3.11 direct, debug-only XCURSOR_THEME/XCURSOR_SIZE, fallback to the built-in arrow, animated shapes only while shown. Debian .deb Depends: adwaita-icon-theme (trixie 48.1-1 ships /usr/share/icons/Adwaita/cursors).
  - Recovery: retry on a timer (0.5, 1, 2 s), rebuild the renderer once after 3 failures or a context loss, then a clean exit; a 2 s flip watchdog; debug-only fault injection (SASH_DEBUG_FAIL_SWAPS, SASH_DEBUG_CONTEXT_LOST, SASH_DEBUG_STOP_FLIPS).
  - The nested Firefox failure is reproduced once more with the m11a build under the owner's limits (CLAUDE.md).
  - Proposal only (owner): compare cursor sizes 24 and 32 by eye on the real hardware; cursor size may join the closed settings list later as an accessibility option. Nothing is built for it.
- **M11b Decorations.** Rules only (CLAUDE.md, "Decorations (M11b, owner; rules only, not built yet)"); bar height and zone widths to be fixed by the owner first. Not started.

Roadmap after M9 (no work now), in order:
1. **Clipboard and drag-and-drop.** M10 (above).
2. **xdg-decoration.** Lets clients drop their own title bars in the tiled layout; sash-wm draws none (no borders yet).
3. **layer-shell.** For the shell (panel, launcher, notifications), which is a sash-ui client.
4. **fractional-scale and viewporter.** HiDPI monitors; together, since fractional scaling relies on viewporter.
5. **XWayland.** Last: the largest piece, and only for legacy X11 apps. Splash windows (forced floating) come with it.

Later (owner, after M9 stage B): **variable refresh rate (VRR)**, off in M9.

Before daily use (owner, after M9 stage B): **Xcursor theme loading.** On DRM, sash-wm draws only its built-in arrow for every named cursor shape; client-chosen shapes (text beam, resize arrows, hand) need an Xcursor theme loaded from the system's icon theme directories.

### Proposal: input settings on real hardware (owner decides; nothing implemented)

On DRM (M9), sash-wm reads keyboards and pointers through libinput and xkb itself, so it needs values for: keyboard layout (and variant/options), key repeat delay and rate, pointer speed (acceleration), natural scrolling, and tap-to-click on touchpads. The closed settings list has no input entries today.

- **Option A: add an "input" group to the closed settings list.** Five entries: keyboard layout (a picker of xkb layouts and variants), key repeat (delay and rate as pickers or sliders), pointer speed (slider), natural scrolling (on/off), tap-to-click (on/off). Edited by the Settings app through the settings protocol.
  - For: users with non-US layouts can type at all; touchpad users get the behaviour they expect; it matches every mainstream desktop.
  - Against: five new settings. It needs per-device rules later (a mouse and a touchpad often want different speeds), and it needs a layout list that maps xkb data to plain names.
- **Option B: fixed defaults, no settings.** The xkb layout comes from the system (Debian's /etc/default/keyboard, via the environment or localed), repeat 600 ms / 25 Hz (today's constants), libinput's default speed, natural scrolling off, tap-to-click on.
  - For: no new settings, nothing to build in Settings; the system-wide keyboard config already exists on Debian.
  - Against: no way to change anything from the desktop. Anyone whose preferences differ from the defaults (very common for scrolling direction and speed) has no recourse without editing system files, which contradicts "the compositor owns all settings" in spirit.
- **Middle ground to consider:** B now, with a fixed list of exactly which values would become settings under A, so the decision can be revisited at M9 when real devices exist.
- **Owner decision (after M8): the middle ground.** Fixed defaults now; the keymap honours the XKB_DEFAULT_* environment variables and reads nothing from system configuration (not /etc/default/keyboard, not localed); the "input" group joins the closed settings list when the settings protocol is built. Recorded in CLAUDE.md ("Settings and configuration").

### Mode selection (proposal after stage B; approved by the owner with additions, see CLAUDE.md)

The owner's monitor (probe after M9 stage B, `tools/drm-probe.py --all-modes`) is a 1190x340 mm (32:9) panel on HDMI-A-1. Its EDID preferred mode is 3840x1080@120, which is not its native resolution; Hyprland runs it at 5120x1440@240. The largest mode by pixel count is 3840x2160 (a 16:9 scaled mode), so "largest" is wrong too.

Proposed rule (no setting; "monitors" on the settings list overrides it later):
1. Ignore interlaced modes.
2. Native resolution: if the connector reports a physical size, the largest mode (by pixel count) whose aspect ratio is within 5% of the panel's physical aspect ratio. If none matches or the size is unknown (0x0 mm, as in VMs and some projectors): the preferred mode's resolution, else the largest mode.
3. At that resolution, the highest refresh rate, compared by the exact rate (pixel clock / (htotal x vtotal)), not the kernel's rounded value.
4. Ties: the first in the kernel's list.

On the owner's monitor this picks 5120x1440 at 240.000 Hz.

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
