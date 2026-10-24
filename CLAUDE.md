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
  and center over their parent regardless of the default mode. They
  carry a separate "forced floating" flag; tile-all never touches them.
- When a window floats, its sibling takes the freed space and the
  window keeps its leaf as a placeholder. Placeholders take no space.
  Geometry is computed over tiled leaves only. Re-tiling returns the
  window to its leaf.
- A new window opened while the default mode is floating gets a
  placeholder leaf next to the focused leaf, so nothing else moves.
- Closing a floating window removes its placeholder; its sibling takes
  the parent's place.

## BSP tiling (M3)

- The BSP tree is plain data in its own module, with no Smithay types,
  and independent of window mode. Each leaf holds a window id and a
  tiled/floating flag; only "tiled" is used until M4.
- The `Layout` trait is the interface M4's floating layout also
  implements.
- New window: splits the focused leaf along the longer side of its
  rectangle, ratio 0.5; the new window becomes the second child and
  gets focus. With no windows, it fills the output.
- Integer geometry only: the first child gets the floor, the second the
  remainder, so rectangles cover the output exactly.
- Close: the sibling takes the parent's place. Focus goes to the most
  recently focused window in that sibling subtree.
- Focus move (left/right/up/down): the neighbouring leaf whose edge
  overlaps the focused leaf's span, nearest first, ties broken by most
  recently focused. No wrap-around.
- Swap exchanges the windows in two leaves; the tree structure stays.
- Client min/max size hints are ignored while tiled (known limit);
  M4's forced-floating rule covers fixed-size windows.
- No gaps, borders, resize-by-keybind or workspaces. Fixed constants
  only, no new settings.
- Resize synchronisation: configure every affected window in one batch.
  Until a client commits a buffer at the new size, draw its last buffer
  at the new rectangle's origin, clipped to the rectangle.
- Layout updates keep running while the output is hidden (requests are
  served; only frame callbacks stop).
- Owner decisions (M3):
  - A square leaf splits side by side (width >= height -> left | right).
  - Swap: focus follows the moved window.
  - Focus-move ties with no focus history: position decides (topmost
    for left/right, leftmost for up/down).
  - Tiled windows get the xdg_toplevel tiled states in configures.
  - A client that unmaps (null buffer) leaves the tree like a close and
    is inserted as a new window when it maps again.
  - Dialogs and transients tile like any window until M4.

## Floating (M4)

- Per window: tiled or floating, plus a separate forced-floating flag.
  Still one `Layout` trait and exactly two modes.
- Forced floating: xdg_toplevels with a parent (dialogs), and windows
  whose min and max size are equal and non-zero. Re-checked on every
  commit; when the reason goes away, the window follows its own
  tiled/floating state again. Centered over the parent (or on the
  output if none), whatever the default mode. mod+F does nothing on a
  forced-floating window; tile-all never touches it.
- The per-monitor default mode (tiled at startup) decides how new
  windows start. No other setting: every step size and timeout is a
  constant. Until the settings protocol exists, mod+Ctrl+D toggles the
  default in debug builds only.
- Bulk actions per monitor: float-all starts each window at its current
  tiled rectangle; tile-all restores the tree.
- Lossless toggling: the BSP tree and each window's last floating
  rectangle are kept ("identical tree" definition in docs/plan.md). The
  first time a tiled window floats it keeps its current tile rectangle.
- New floating window: the client chooses its size (configure 0x0),
  clamped to min/max and the output. The first is centered on the
  output; the next ones cascade by 32 px and wrap back to the center
  when they would leave the output.
- Floating windows draw above tiled ones, are not clipped to tiles, and
  keep at least min(64, size) px visible on the output on each axis
  when moved. An output resize clamps them back.
- Z-order: one list per monitor of floating windows. Focusing or
  clicking a window raises it. Pointer hit-testing uses the topmost.
- Click-to-focus for all windows: a plain click focuses, raises and is
  delivered to the client; mod+click starts a move/resize and is never
  delivered.
- Close: closing a tiled window keeps the M3 focus rule; closing a
  floating window focuses the most recently focused window.
- Directional focus (mod+H/J/K/L) considers every window by geometry: a
  candidate's center lies past the focused window's center in that
  direction and the perpendicular spans overlap; nearest by edge gap
  (overlap counts as 0), ties by most recent focus, then position.
- Mouse: mod+left-drag moves. On a tiled window the press floats it at
  its tile rectangle (owner decision: on press, not on first motion).
  mod+right-drag resizes a floating window; the edge or corner comes
  from the pointer's cell in a 3x3 grid over the window, the center cell
  uses the bottom-right corner. Min/max sizes are respected.
- Client xdg_toplevel.move is handled; on a tiled window it floats the
  window first. xdg_toplevel.resize is handled for floating windows and
  ignored for tiled ones. show_window_menu is ignored.
- Keyboard move/resize on a tiled window floats it first.
- Swap works on tiled windows only: with a floating window focused it
  does nothing, and floating windows are never swap candidates.
- An output resize clamps floating windows with the same 64 px rule
  (moved just enough, size kept).
- mod+Tab / mod+Shift+Tab: hold-to-cycle. While mod is held, each press
  steps through a frozen most-recently-used snapshot, focusing and
  raising as it goes; releasing mod commits the choice.
- Known limits: client fullscreen and maximize requests are not handled
  (M5 snapping covers them); border hit zones for resizing come with
  decorations.

## Snapping and fullscreen (M5)

- Snapped and fullscreen are states of a floating window, not layout
  modes: there are still exactly two modes.
- Snap zones are data: a table the settings protocol will edit later
  ("snap zones" is on the closed settings list). No UI, no config file,
  no new settings.
- Seven default zones, as fractions of the output, integer geometry
  only (halves and quadrants use the floor/remainder rule and cover the
  output exactly): top edge = fullscreen (whole output), left edge = left
  half, right edge = right half, four corners = four quadrants. No
  bottom-edge zone.
- Triggers during a window drag (constants): pointer within 64 px of
  two output edges = corner; otherwise within 8 px of the top, left or
  right edge = that edge's zone. Corners win over edges.
- While the pointer is in a zone, a preview rectangle is drawn: fill
  #0A84FF at 30% alpha plus a 2 px border at 80% alpha. Plain rectangle,
  no blur, rounding or animation. It disappears when the pointer leaves
  the zone or the drag ends. Releasing inside a zone snaps the window;
  dragging a tiled window to a zone floats and snaps it.
- A snapped window remembers its pre-snap floating rectangle and its
  zone. Dragging it away restores that size, with the pointer at the
  same relative position in the window. Tile-all then float again
  restores the zone, recomputed from the current output size; an output
  resize recomputes every snapped window from its zone.
- Edge snapping: during a mouse move or resize of a floating window,
  its edges snap to the output edges and to edges of other visible
  windows within 12 px (constant), nearest first. Shift held during the
  drag disables it. Not applied to keyboard moves.
- Client fullscreen and maximize requests are handled (set/unset
  fullscreen, set/unset maximized) with the right configure states.
  Until layer-shell exists both fill the whole output.
- A fullscreen window is above all other windows. One per monitor: a
  new one replaces the old, which returns to its previous state. A tiled
  window going fullscreen keeps its leaf and tile; leaving fullscreen
  restores it. Focusing another window (key or click) ends the old
  window's fullscreen. Closing a fullscreen window restores the layout.
- Snap keybinds: mod+Left/Right/Up/Down, by the state table in
  docs/plan.md (owner-approved).
- Owner decisions (M5):
  - The top zone is a maximized snap: a whole-output rectangle with the
    xdg maximized state, normal stacking, not ended by focus changes.
    Client set_maximized is the same. Only set_fullscreen gives the
    fullscreen state.
  - Maximize (client or mod+Up) on a tiled window keeps its leaf and
    tile; unmaximize returns it there. It never floats by maximizing.
  - mod+Left/Right on a tiled window floats it, then snaps. From a
    maximized tiled window, mod+Left/Right floats it (pre-snap rect =
    its tile) and snaps to the half.
  - Keyboard move/resize or mod+right-drag resize of a snapped window
    unsnaps it and continues from its snapped rectangle; the pre-snap
    rectangle and zone are forgotten.
  - Snapped halves and quarters get the xdg tiled states for each edge
    on an output edge; the maximized snap gets the maximized state.
  - A fixed-size window that asks for fullscreen gets it: above all,
    centered at its own size, with an opaque black backdrop over the
    rest of the output.
  - mod+Tab previews do not end fullscreen; only the committed choice of
    a different window does.

## Keybinds

- Until M6 there is one minimal hardcoded keybind table; M6 replaces it
  with the full default table.
- The modifier is one constant: Alt when nested (the host captures
  Super), Super on DRM.
- Keys that trigger a keybind are not forwarded to clients.
- mod+Return (spawn foot) exists in debug builds only.
- Every mouse action has a keybind. Until M6 the table is:
  mod+Return foot (debug), mod+Q close, mod+H/J/K/L focus,
  mod+Shift+H/J/K/L swap, mod+Shift+E quit, mod+F toggle floating,
  mod+Shift+F float all, mod+Shift+T tile all, mod+Ctrl+H/J/K/L move
  40 px, mod+Ctrl+Shift+H/J/K/L resize 40 px (L wider, H narrower,
  J taller, K shorter, top-left fixed), mod+Tab / mod+Shift+Tab cycle,
  mod+Ctrl+D toggle default mode (debug).

## Protocol support and known limits

- `wl_data_device_manager` is a stub: advertised because some clients
  (foot) refuse to start without it. No clipboard or drag-and-drop
  logic yet.
- xdg-shell protocol errors are limited by Smithay 0.7, which does not
  expose a toplevel's `xdg_surface`:
  - a buffer before the first `ack_configure` gets
    `xdg_surface.not_constructed`, not `unconfigured_buffer`;
  - an invalid window geometry (width or height <= 0) is ignored and
    logged, not answered with `xdg_surface.invalid_size`.
  Do not claim full xdg-shell spec conformance.
- Negative `xdg_toplevel` min/max sizes are rejected by sash-wm before
  Smithay sees them (Smithay 0.7 panics on them in debug builds).

## Backends and frame pacing

- Nested (winit) backend only: the EGL swap interval is set to 0 and
  frames are paced by the host's frame callbacks via winit. Smithay
  leaves it at 1, which blocks every swap for about one frame.
- The DRM backend paces by vblank / page flip, not by swap interval.

## Settings and configuration

- The compositor owns all settings.
- No Lua, no scripting, no plugins, no user config file.
- Clients (Settings app, shell, CLI) talk to the compositor through a
  native Sash protocol.
- The settings list is closed. Do not add a setting without asking.
  The complete list:
  - appearance (dark / light / auto)
  - accent
  - transparency (default 60)
  - effect toggles
  - per-monitor default mode
  - snap zones
  - wallpaper
  - keybinds
  - monitors

## Frame callbacks

- No frame callbacks to clients while the output is not presenting
  (for example a hidden nested window). Client requests are still served.

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

## Originality (Smithay examples)

- Smithay's example compositors (smallvil, anvil) and the Smithay
  documentation may be read and cloned as learning material, to
  understand how a feature works.
- Do not copy code from them: no pasting, no line-by-line translation,
  no renaming variables of copied code, and no reproducing their module
  structure and function breakdown.
- Method: read the relevant example, close it, then write the
  implementation from your own understanding of what is needed. Design
  sash-wm's module structure yourself. Do not keep the example open
  while writing.
- If something ends up closely following an example, say so, rewrite
  it, or (if it is truly adapted) keep the MIT notice on that file and
  list it in NOTICE.
- Each milestone report includes a code-similarity check of
  sash-wm/src against smallvil and anvil.

## Toolchain and target compatibility

- Rust: a recent stable toolchain, pinned to an exact version in
  `rust-toolchain.toml`. Edition 2024. Update it deliberately (see README).
- Smithay: stay on the 0.7.0 release from crates.io (not git master).
- Debian 13 trixie ships glibc 2.41, Mesa 25.0.7, systemd 257,
  libwayland 1.23, xkbcommon 1.7, libinput 1.28. Compatibility is proven by
  building in a `debian:trixie` container with the same pinned toolchain;
  report any library-version problem found there.
- Release binary glibc floor: currently GLIBC_2.39 (`pidfd_spawnp`,
  `pidfd_getpid` from Rust std process spawning); trixie ships 2.41. Do
  not raise it silently: check `objdump -T` on the release binary every
  milestone and report any change.
- Dependency licenses are checked with `cargo deny check` (`deny.toml`).
  A dependency outside the allowed list is reported to the owner, never
  allowed on my own.

## Screenshots and measurements

- Screenshots capture only the nested sash-wm window, through the guard
  (nested window on the active workspace, no fullscreen window there),
  and every image is looked at before it is used. Never capture the
  whole host screen.
- Busy-client runs report a frame-interval histogram, not only render
  time.

## Milestone routine

- After each milestone: build/clippy/fmt/test output, trixie container
  build, push `main` to the private remote, then STOP for approval.
