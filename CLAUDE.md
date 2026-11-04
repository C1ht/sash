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
- New window: splits the tiled window under the pointer, otherwise the
  focused leaf (owner, after stage C run 1: rule A), along the longer
  side of its rectangle, ratio 0.5; the new window becomes the second
  child and gets focus when it maps (see "Focus on open"). With no
  windows, it fills the output. "Under the
  pointer" means the tile that contains the pointer position, also when a
  floating, maximized or fullscreen window is drawn above it; the
  pointer outside every tile (or no pointer) means the focused leaf. A
  pointer that has not moved since sash-wm started counts as no pointer
  (owner, after m9c). This holds for every new tiled window, also one
  spawned by a keybind. No preselect keys. A new window while the
  default mode is floating still gets its placeholder next to the
  focused leaf.
- Focus on open (owner, M9e): focus (keyboard and keybind target) stays
  on the previous window until the new window maps (its first buffer
  commit), then moves to it. Keys typed before the map go to the old
  window; nothing is queued. A window closed, or whose client crashes,
  before it maps leaves focus where it was. Of several windows opening
  at once, focus ends on the last one that mapped. Until the map, the
  "focused leaf" of rule A is the previous window.
- Rule A, documented behaviour (owner, after m9c; not bugs):
  - A pointer resting over another window decides keybind spawns too:
    mod+Return splits the tile under the pointer, not the window being
    typed in, and focus moves to the new window there.
  - Repeated spawns with a parked pointer keep cutting the same area:
    the new window is the second (right or lower) child, a point on a
    split line belongs to it, so the pointer stays in the newest tile
    while other tiles stay large.
  - Recorded alternative, not implemented: use the pointer only if it
    moved in the last few seconds, else the focused leaf.
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
- Top edge (owner, after M9 stage B): a floating window's top edge never
  goes above the top of the output's usable area (the whole output
  until layer-shell exclusive zones exist), in any operation: mouse
  drag, keyboard move, resize, restore from a snap or maximize, client
  move and resize requests, output resize. The sides and the bottom keep
  the 64 px rule. A resize by the top edge stops there (the window gets
  shorter; it is not pushed down).
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
- Keyboard move on a tiled window floats it first. Keyboard resize of a
  tiled window moves splits instead (see "Tiled resize (M6)").
- Swap works on tiled windows only: with a floating window focused it
  does nothing, and floating windows are never swap candidates.
- An output resize clamps floating windows with the same 64 px rule
  and the top-edge rule (moved just enough, size kept).
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
  zone. Dragging it away restores that size; the grab point keeps its
  relative horizontal position under the pointer, and vertically the
  window is clamped by the top-edge rule (owner, after M9 stage B).
  Tile-all then float again restores the zone, recomputed from the
  current output size; an output resize recomputes every snapped window
  from its zone.
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
    fullscreen state. (Confirmed by the owner after M5.)
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
  - Edge snapping to another window's edge only when the two windows
    overlap on the other axis; output edges always count.
  - Dialogs snap like any floating window; fixed-size windows never
    snap (no preview, snap keys and maximize do nothing), but are
    granted fullscreen.
  - The snap preview is drawn above all windows except the dragged one.
  - Dragging a snapped window restores its pre-snap size on the first
    pointer motion, not on press.

## Tiled resize (M6)

- Resizing a tiled window changes the ratio of the split on the side
  being resized. Keys: the floating resize keys (mod+Ctrl+Shift+H/J/K/L,
  40 px per step) also resize tiled windows by moving the nearest split
  in that direction. Mouse: mod+right-drag on a tiled window uses the 3x3
  grid: an edge cell moves that edge's split, a corner moves both, the
  center moves the bottom-right.
- Ratios are exact fractions or fixed-point, never drifting floats.
  Rectangles keep the exact-coverage rule. A tile is never smaller than
  64 px on an axis (constant); if the output is too small for that, the
  tiny-output rule applies.
- Edges on the output boundary do nothing. Lossless toggling keeps
  ratios.
- Resize applies live while dragging; configures are batched per frame
  (at most one per window per frame).

## Debug-only tools

- Offscreen capture (SIGUSR1, see Screenshots) and synthetic input (a
  unix socket in $XDG_RUNTIME_DIR, mode 0600, feeding the same input
  handlers as the backend) exist in debug builds only and are never
  compiled into release builds; a check enforces this.
- In release builds SIGUSR1 is ignored (SIG_IGN, via nix as a direct
  dependency), not the default action.
- The release check is tools/check-release.sh; GitHub Actions runs it
  with build, clippy, fmt, test and cargo deny.
- Owner decisions (M6):
  - Tiled keyboard resize has the floating meaning: L wider, H narrower,
    J taller, K shorter, by moving the window's right (L/H) or bottom
    (J/K) edge's split; nothing if that edge is on the output boundary.
  - Configures from pointer drags (floating and tiled) are batched once
    per presented frame, with a 16 ms timer fallback while the output
    is not presenting. Keybind actions stay immediate.
  - Mouse bindings are rows of the same keybind table and doc.
- Owner decisions (M6 review):
  - Client xdg_toplevel.resize on tiled windows stays ignored.
  - Mouse bindings ignore Shift: confirmed.
  - GitHub Actions is blocked by account billing; ignore it. The local
    debian:trixie container build is the compatibility check.

## Popups (M7)

- xdg_popup with xdg_positioner: anchor, gravity, offset, size and
  constraint adjustments (slide, flip, resize in x and y), reposition
  requests, nested popups, popups on tiled and floating windows,
  tooltips (no grab) and grabbed popups (menus, dropdowns).
- Smithay's documented popup support is used (positioner math,
  PopupManager, popup grabs); sash-wm's own code is written from its own
  understanding (originality rule).
- Placement: relative to the parent surface, constrained to the output
  area; the adjustments follow the protocol's order (flip, then slide,
  then resize, per axis; a flip that does not remove the constraint is
  reverted).
- Rendering: a popup draws directly above its parent window in stacking
  order (tiled windows: see the owner decisions below) and is not
  clipped to the parent's tile. A fullscreen window's
  popups draw above it. Popups follow their parent when it moves,
  resizes, snaps or changes layout.
- Input: pointer hit-testing finds the topmost surface including popups,
  also outside the parent's tile.
- Grabs: a grabbed popup takes keyboard focus. A click outside the popup
  chain dismisses the chain with popup_done, innermost (topmost) first
  as the protocol requires; that click is not delivered to the window
  underneath. Focus returns to the parent. Tooltips never take focus.
- Keybinds and popups (owner decision): every keybind action and every
  mod+drag start first dismisses the popup chain of the window it acts
  on (the focused window; for mod+drag, the dragged window). Exceptions:
  keyboard resize and mod+right-drag resize keep the popups open and
  they follow the parent; the debug default-mode toggle changes nothing
  on screen and dismisses nothing.
- Lifetime: the parent unmapping or being destroyed dismisses its
  popups. Destroying a popup that is not the topmost gives the protocol
  error not_the_topmost_popup. An invalid grab serial, or a grab whose
  parent is not the topmost popup, dismisses the popup per protocol.
- Output resize or hidden output: popups are re-constrained; a hidden
  output still serves requests and sends no frame callbacks.
- No new settings.
- Owner decisions (M7):
  - Parent changes (move, resize, snap, layout): reactive popups
    (set_reactive) are re-constrained to the output with a new
    configure; non-reactive popups keep their position relative to the
    parent and follow it.
  - Output resize: reactive popups are re-constrained; non-reactive
    popups are dismissed (popup_done). Smithay 0.7, following the
    protocol, refuses a new configure for a non-reactive popup.
  - Tiled windows have no z-order among themselves, so popups of tiled
    windows are drawn and hit-tested above every tiled window, below
    maximized, floating and fullscreen windows. Popups of other windows
    are directly above their parent.
  - A popup whose parent toplevel is not mapped is dismissed at once
    (popup_done); it is never shown.
  - Popups are dismissed innermost first (the brief contradicted
    itself; the protocol rule wins). Confirmed by the owner after M7.
  - Known limit: on toplevels, the part of a subsurface outside the
    window's own rectangle does not receive pointer input.
  - Smithay 0.7 does not expose a client's xdg_wm_base, so sash-wm
    dispatches xdg_wm_base itself (forwarding to Smithay) to record it
    and post not_the_topmost_popup.

## Debug recording (owner, after stage C run 1)

- Behind the cargo feature `debug-record`, off by default and never in a
  release build (a release build with the feature fails to compile).
  Enabled at runtime only by `SASH_DEBUG_RECORD=<dir>`; the directory is
  created with mode 0700 and refused if it lies inside a git work tree
  (never inside the repo). A banner at startup says it is on and what it
  records.
- Compositor-level data only. Keyboard: chords (a key pressed with Ctrl,
  Alt or Super held), modifiers and non-printing keys by name; plain
  printable keys (Shift alone counts as plain) are logged as "printable"
  with a timestamp, never by identity. Full key names only with
  `SASH_DEBUG_KEYS=full`, with a warning at startup.
- Screenshots only through the offscreen render path, never the host
  screen: on the hotkey, and after each layout change at most one per
  second and 100 per session, scaled to at most 1920 px wide.
- Screenshot contents (owner, after stage C run 1): by default plain
  window outlines with their ids on the clear colour, no window
  contents; the ids are drawn with simple built-in digits (no font
  dependency). Full contents only with `SASH_DEBUG_SCREENSHOTS=full`,
  with a startup banner warning and a docs/DEBUG.md warning. The tests
  prove the default bundle has no window contents and that typed text
  is in no file of it, pictures included.
- Hotkey: mod+F11 (Super on DRM, Alt nested) marks a bug and writes a
  bundle. Each press is logged, with where the bundle went, or why
  writing it failed (owner, M9d). It clashes with no keybind (a test checks the table) and is
  never forwarded to clients. Not a keybind-table row: it exists only in
  builds with the feature.
- A 60-second in-memory ring buffer; nothing is written until the
  hotkey, unless `SASH_DEBUG_RECORD_SESSION=1` (then everything is
  written as it happens).
- Retention: bundles older than 7 days are deleted at startup.
- Cursor path (owner, after m9c; recording only, no behaviour change):
  on DRM, a `cursor_path` event each time the way the cursor is drawn
  changes (hardware cursor plane, or composited into the frame), and per
  second the frames with the cursor on the plane, the frames with it
  composited, and the frame requests caused by pointer motion.
- docs/DEBUG.md describes it; docs/v1.md has the gate "debug recording
  removed or proven compiled out"; tools/check-release.sh fails if any
  of it is in the release binary.
- Replay (owner, after stage C run 1): a bundle's pointer, keybind and
  layout events are replayed in the headless test harness with stand-in
  windows, and the layout snapshots are compared with the bundle's.
  Runs through the test harness (`tools/replay.sh <bundle>`, a debug
  test build), not as a sash-wm flag: the stand-in clients use the
  test-only Wayland client library. Bundles from real hardware become
  regression tests (fixtures with no typed text).
- Replay harness (owner, M9d; test code only): the stand-ins read their
  events while the replay runs; a replay has a longer timeout than
  other harness tests; the replay's own recorder is not limited to
  60 s. A fixture from real hardware keeps only the event kinds the
  replay uses, up to the last matching snapshot, downsampled (the last
  pointer position before each button, key or window event, plus every
  25th motion during a drag), with no typed keys and no device names;
  under 200 KB and a test time under 10 s. The full bundle is never
  copied into the repository. A test checks every real-hardware
  fixture: only those event kinds, only modifier keys, no app ids,
  titles or device names (owner, M9e).

## Keybinds

- Until M6 there is one minimal hardcoded keybind table; M6 replaces it
  with the full default table.
- The modifier is one constant: Alt when nested (the host captures
  Super), Super on DRM.
- Keys that trigger a keybind are not forwarded to clients.
- mod+Return (spawn foot) exists in debug builds only.
- From M6 there is one data table (chord, action, description, modifier
  set) in one module; docs/keybinds.md is generated from it and a test
  fails if it is out of date or if two chords collide. No action is added
  without asking.
- Every mouse action has a keybind. The table lives in
  sash-wm/src/keybinds.rs; docs/keybinds.md lists it. Pointer rows:
  mod+left-drag move, mod+right-drag resize, plain click focus and raise
  (delivered). Shift is ignored when matching pointer rows, because
  Shift during a drag turns edge snapping off.

## linux-dmabuf and cursor-shape (M8)

- Investigate first (docs-only commit, report), then build.
- zwp_linux_dmabuf_v1 at the highest version Smithay 0.7.0 supports,
  with feedback derived from what the renderer can import. Only formats
  and modifiers verified to import are advertised.
- A client buffer is imported when it is attached. Failures give the
  zwp_linux_buffer_params_v1 protocol errors or the failed event, as the
  protocol says; never a panic. Limits (planes, size) are constants,
  listed in docs/plan.md.
- dmabuf buffers render through the same paths as shm buffers: tile
  clipping, floating, popups and snap previews all still work.
- Frame callbacks and buffer release follow the protocol: a buffer is
  released when it is no longer used for presentation.
- wp_cursor_shape_manager_v1 is implemented. In the nested backend the
  shape is applied to the host window's cursor through winit. Client
  cursor surfaces keep working. Cursor position and visibility rules
  stay as they are. No settings.
- No explicit sync (wp_linux_drm_syncobj_v1) without the owner's
  approval after the investigation.
- M9 (DRM/KMS) is planned in docs only during M8; no M9 code.
- Investigation: docs/m8-investigation.md. M9 plan: docs/m9-plan.md.
- Owner decisions (M8):
  - Client cursor surfaces (wl_pointer.set_cursor) are drawn by sash-wm
    at the pointer, hotspot respected, with the host cursor hidden while
    one is set; named shapes (cursor-shape) go to the host cursor through
    winit; the hidden state hides it. The DRM backend reuses this.
  - "Verified to import": the formats and modifiers EGL reports as
    importable, after a startup self-test (a GL texture exported as a
    dmabuf and imported back). If the self-test fails, no dmabuf global.
  - Limits: 4 planes (Smithay's MAX_PLANES), at most 16384 x 16384.
- Owner decisions (M8 review):
  - The cursor resets to the arrow when the pointer moves to another
    surface (or off every surface), until that client sets its own.
  - Explicit sync is decided at the first bare-metal run (M9 stage C);
    not built before.

## Quarter snapping (M8.1)

- Direct chords, from any state (tiled, floating, snapped): mod+Y
  top-left quarter, mod+U top-right, mod+B bottom-left, mod+N
  bottom-right. A fullscreen window ignores them. Checked against every
  existing chord first; a clash stops the work and goes to the owner.
- A chord on a tiled window floats it first, then snaps it, and
  remembers the pre-snap rectangle as the other snaps do. The chord of
  the quarter the window is already in does nothing. The mod+arrow
  transitions stay exactly as they are.
- Snap zones stay data; the closed settings list does not change. The
  four chords are rows of the keybind table, so docs/keybinds.md and its
  stale-doc and duplicate-chord tests cover them.

## DRM/KMS backend, M9 stages A and B

- Stages: A (host, read-only probe), B (VM only), C (first run on the
  real machine, by the owner's checklist docs/m9-stage-c.md). Stage C is
  not started without the owner.
- Stage A: a probe in tools/ lists DRM devices, connectors, modes (and
  the preferred one), planes and plane formats with queries only. It
  never takes DRM master, sets a mode or touches Hyprland's session. If
  it needs sudo or a group change, stop and ask.
- Stage B runs only in a VM (QEMU/KVM, virtio-gpu, minimal Debian 13
  guest); nothing runs on the real display. Ask before downloading an
  image (state its size) or installing anything with sudo. The guest
  runs the release binary built in the debian:trixie container.
- Backend: Smithay's DRM/KMS path with its documented features; every
  new crate and system package (host and trixie) is listed and approved
  by the owner first. Pieces: libseat session (logind or seatd), udev,
  libinput, GBM/EGL, atomic modesetting on one connector at its
  preferred mode (no settings), page-flip pacing (render after the
  previous flip completes, frame callbacks at presentation),
  presentation-time, the cursor on the hardware plane with a software
  fallback, VT switching through the session's pause and resume (stop
  rendering and release the device on pause; restore everything on
  resume).
- The modifier constant is Super on DRM.
- Safety: mod+Shift+E quits; Ctrl+Alt+F1..F12 are handled by the session
  layer before anything else, so the keyboard can never be trapped. A
  debug-only `--exit-after <seconds>` flag is an extra watchdog. Errors
  at startup restore the display and exit with a message; no panic may
  leave the device held.
- Hotplug of other monitors and GPUs is out of scope. The connector in
  use (owner, after M9 stage B): on every udev change event of the DRM
  device it is re-probed. Disconnected (cable pulled, or a monitor that
  drops hotplug when powered off): rendering and frame callbacks stop;
  windows, layout and the output stay; client requests are served.
  Connected again: the CRTC is cleared and a full modeset is committed
  at the previous mode if the display still offers it; otherwise the
  mode-selection rule picks one on that connector (confirmed by the
  owner after M9 stage B). A resolution change goes through the normal
  output-resize path (floating windows re-clamped). Then the whole
  output is redrawn. Never a panic. Stage C tests a monitor power
  off/on and a cable pull; the VM test confirms that a udev event
  arrives.
- Backend selection (owner, after M9 stage B): winit when WAYLAND_DISPLAY
  or DISPLAY is set; DRM only when XDG_SESSION_ID, XDG_SEAT and
  XDG_VTNR are all set (a local console session); otherwise exit with
  "DRM needs a local console session; this looks like a remote login".
  Empty values count as unset. `--backend winit|drm` overrides.
- Unpaced flips (owner, after M9 stage B): when page flips complete
  without vblank pacing (two flips completing less than half a refresh
  period apart, as on virtio-gpu), rendering is capped at the output's
  nominal refresh rate by a fixed schedule (owner, after M9b): each
  frame has a slot one refresh period after the previous slot, counted
  from the start of rendering (not its end); a frame that starts late
  but within the next period takes that slot, so lateness does not add
  up; after an idle gap the schedule restarts. With vblank pacing
  nothing changes.
- Pacing classification (owner, after stage C run 1): the first flip
  after a modeset or a resume is not used; the output becomes unpaced
  after 3 consecutive flip intervals under half a period (longer gaps
  are idle and reset nothing). Unpaced stays until the next modeset or
  resume, when the classification starts over as paced: while the cap
  is on, the cap itself spaces the flips, so they cannot show pacing
  (a "paced again after normal intervals" rule oscillated in the tests).
  Every change is logged. Tested with the owner's real NVIDIA flip
  times.
- Pointer frames (owner, after stage C run 1): when the cursor sash-wm
  draws (the arrow on DRM, a client cursor surface anywhere) has to
  move, a frame is requested, at most one per refresh period (merged
  until the next frame starts); no motion, no frame. The hardware
  cursor plane is moved through that frame (only the cursor plane
  changes, nothing else is rendered again) when the driver gives a
  cursor plane; otherwise the arrow is composited.
- Edge frames (owner, M9d): a pointer motion requests a frame only
  when the final, clamped pointer position changed or the cursor image
  changed. This holds during drags too: a motion that leaves the
  clamped position where it was (the pointer pinned at a screen edge)
  changes nothing and requests no frame.
- Connector re-check after udev events (owner, M9d): udev change events
  of the DRM device are merged: the first one arms a 200 ms window
  (constant), and one re-check runs when it ends, whatever more events
  arrive inside it. On resume, libinput is restored before any
  re-check runs (a re-check pending at resume runs only after the input
  devices are back). The re-check itself, a forced probe of the
  connector, and the hotplug behaviour it drives are unchanged.
  Skipping the re-check when an event does not concern our connector
  is not possible with Smithay 0.7's udev events (they carry only the
  device number); open, for the owner.
- Resume timing points (owner, M9e; recording and debug log only, no
  behaviour change): `resume_step` events and debug lines for the
  resume start, each resume step (activate, state reset, libinput
  restored, redraw) with its duration, the first frame start, its draw
  and commit times (the first modeset), the first flip, the first
  input event, and every connector re-check with its duration; each
  with the time since the resume began, so resume-to-first-frame splits
  into its parts.
- Page flips refused around a pause (owner, M9d): a frame whose commit
  fails with "Permission denied" (DRM master already gone while the
  session is pausing) is an expected race: logged at debug level and
  counted; the count is logged with the next session pause or resume.
- Mode selection (owner, after M9 stage B), no setting:
  1. Interlaced modes are ignored.
  2. Native resolution: if the physical size is known, the largest
     mode (by pixel count) whose aspect ratio is within 5% of the
     panel's physical aspect ratio. Physical size unknown: 0x0 mm, or
     the placeholder sizes 160x90 mm and 16x9 (cm values); then, and
     when no mode matches, the preferred mode's resolution, else the
     largest.
  3. At that resolution the highest exact refresh rate (pixel clock /
     (htotal x vtotal)); ties: the kernel's order.
  4. Validation before anything is committed: each candidate is tested
     with an atomic TEST_ONLY commit (allow-modeset). Candidates in
     order: the selected mode, the same resolution at the next lower
     refresh rates, then the next lower resolutions of the same aspect
     (within 5%), each from its highest refresh down. Every candidate
     and the reason it was rejected is logged. If none passes, sash-wm
     exits with a message and the display is left untouched (the device
     is opened without disabling outputs; they are turned off only
     after a mode passed).
  5. Debug builds only: `--mode WxH@R` picks that mode (still
     validated; refused if not offered). Not compiled into release
     builds; tools/check-release.sh proves it.
  6. Debug builds only (owner, after M9b), fault injection for the
     validation: SASH_DEBUG_REJECT_MODES="WxH@R,..." makes the test
     commit of those modes fail; SASH_DEBUG_REJECT_ALL_MODES=1 makes
     every one fail. Not compiled into release builds;
     tools/check-release.sh proves it. The VM scenario covers the
     fallback chain and the all-rejected exit (no commit, console
     untouched).
- Variable refresh rate stays off in M9 (roadmap).
- Stage C is run by the owner, never by Claude: tools/stage-c.sh (the
  timeout wrapper, a fsynced log in ~/.cache/sash/stage-c/, the escape
  instructions printed first) and docs/m9-stage-c.md. Nothing runs on
  the real display. Run 1 uses `--mode 5120x1440@60`, the next runs the
  selected mode.
- The VM (owner, after M9 stage B): its image, guest disk, packages and
  test downloads live in ~/.cache/sash/vm, each download checked
  against a checksum. Nothing in tools/ depends on the scratchpad.
- Owner approvals (M9 stage B): Smithay features backend_drm,
  backend_gbm, backend_libinput, backend_udev, backend_session_libseat and
  their crates (drm, drm-ffi, drm-sys, gbm, gbm-sys, input, input-sys,
  libseat, libseat-sys, udev, libudev-sys, io-lifetimes, bytemuck_derive,
  a second linux-raw-sys); trixie build packages libseat-dev,
  libinput-dev, libudev-dev, libgbm-dev. The VM: the Debian 13 nocloud
  cloud image (407 MB) run with QEMU/KVM as the owner's user (no sudo, no
  libvirt, no window on the real display), and about 150 MB of runtime
  packages installed by apt inside the guest.

## Clipboard and drag and drop (M10)

- The `wl_data_device_manager` stub is replaced by a real implementation
  (version 3): clipboard selection between clients, and drag and drop
  with a drag icon, MIME lists, copy/move/ask actions, finish and cancel.
  sash-wm implements wl_data_device_manager, wl_data_source,
  wl_data_device and wl_data_offer itself: Smithay 0.7's data device has
  no hook for the limits below. Smithay's seat, pointer grabs and
  surface roles are used. The originality rule applies.
- Selection: only the client with keyboard focus can set or clear it (a
  request from any other client is ignored). It is offered only to the
  client with keyboard focus: when keyboard focus moves to a client, that
  client gets the current selection (a new offer, or none); a client
  without keyboard focus gets no selection offer, and a receive request
  on any offer from a client without keyboard focus is refused (the fd
  is closed at once). No client can read the clipboard otherwise.
- The selection survives its source losing focus. It ends when replaced,
  cleared, or when its source is destroyed or its client disconnects;
  then the focused client is told there is no selection.
- Transfers go through client pipes: a receive request's fd is passed to
  the source client's send event and sash-wm closes its copy. sash-wm
  never reads, writes or waits on a payload and never blocks on one.
- Limits (constants, owner, M10). Beyond a limit the excess is ignored
  and logged at debug level; no client is disconnected for it:
  - 32 MIME types per source; a MIME string of at most 256 bytes; at
    most 4096 bytes of MIME names per source;
  - 16 live offers per client: a client holding 16 offers gets no new
    ones (selection or drag) until it destroys some;
  - 16 receive requests per offer per second (sliding window); beyond
    that the fd is closed at once (the reader sees end of file) and the
    refusal is counted.
- No data-control (owner, M10, a deliberate privacy decision):
  wlr-data-control, ext-data-control and every other clipboard-manager
  protocol are not implemented, because any client holding one can read
  everything that is copied. The owner decides later.
- No primary selection (zwp_primary_selection_device_manager_v1, middle-
  click paste) for now (owner, M10); pros and cons in docs/plan.md.
- Drag and drop starts only with a start_drag whose serial is the one
  of the pointer button press that began the current implicit grab, and
  only from the client that got that press. Any other start_drag is
  ignored, as the protocol allows. (A touch down will count the same
  once sash-wm handles touch input; it has none yet.)
- A source used again (set_selection or start_drag) is accepted: the
  protocol only says used_source "may" be sent, and Firefox sets the
  same source again (found in the M10 by-hand run; sending it crashed
  Firefox). set_actions after a source was used stays an error.
- Drag icon: a surface with the dnd_icon role (another role gives the
  protocol's role error). Drawn above every window, popup and overlay
  (the snap preview included), at the pointer plus its offset (the
  hotspot, kept up to date from its attach and offset requests), for
  shm and dmabuf buffers. It never takes pointer focus and is never hit
  by hit-testing.
- During a drag: the target is found by the normal hit-testing (windows
  and popups); the target gets enter, motion and leave with an offer
  listing the source's MIME types (the limits apply). Actions follow the
  protocol: the action is chosen from the source's actions and the
  target's accepted actions and preference (copy, move, ask). A drop on
  a target that accepted a MIME type and an action is a drop (the source
  is told dropped, then finished when the target finishes); a drop on
  nothing or on a target that accepted nothing is a cancel. At an output
  edge (the pointer pinned there) the surface under the pointer decides,
  as anywhere else. Escape cancels (the key is not delivered). A keybind that
  changes focus (focus moves, mod+Tab, close, swap, quit) cancels the
  drag first. A mod+drag cannot start during a drag and a drag cannot
  start during a mod+drag (no press reached a client). A source or
  target that disconnects mid-drag ends it cleanly on the other side.
- Nested winit backend: no bridge to the host's clipboard (known
  limit). Copy and paste work between sash-wm's own clients only.
- Debug recorder (owner, M10): `selection_set`, `selection_cleared`,
  `dnd_start`, `dnd_drop` and `dnd_cancel` events with window ids and
  the action only; never MIME names, sizes or payloads. A test puts a
  marker in a copied payload and in a MIME name and proves it is in no
  bundle file, screenshots and log included.
- VM scenario: copy and paste between two clients on DRM with
  wl-clipboard installed in the guest by apt (owner, M10).
- Confirmed by the owner after M10: 256 bytes per MIME name and 4096
  bytes of names per source; close, swap and quit cancel a drag like the
  focus keybinds; a reused source is accepted. The used_source change
  was made together with the code (commit 78558ed), from the by-hand
  run, not in a rules commit first.
- Known limit (owner, after M10): the clipboard is empty once its source
  client closes (sash-wm keeps no copy; it never reads payloads). The
  options are in docs/plan.md and docs/DEBUG.md; none is built.

## Cursor themes, xdg-decoration (M11): investigate and propose first

- M11 is an investigation: report tables and short notes in
  docs/m11-investigation.md, a summary in docs/plan.md, then STOP and
  ask. No implementation. M12 is not started.
- A. Cursor themes: what Smithay 0.7 and the xcursor crate give; which
  default theme can ship (licence, size, hi-dpi variants, what Debian 13
  packages); how cursor-shape names map to theme files; loading cost and
  caching; the fallback without a theme; scaling at 5120x1440. The theme
  is fixed: no new setting. XCURSOR_THEME and XCURSOR_SIZE in debug
  builds only, and only if the owner agrees.
- B. xdg-decoration: options (always client-side; server-side title
  bars everywhere; server-side only for floating windows that ask or
  draw none; a different split for sash-ui apps), with pros, cons and a
  recommendation, against the UI rules, hit-testing and resize edges,
  snapping, fullscreen, tiled windows, the 64 px visibility rule, drawing
  cost, the title bar's content, and the window-menu and maximize
  requests clients send today.
- C. How A and B change the M12 layer-shell plan.
- Also after M10 (investigate only, no fix): the nested window stops
  drawing once Firefox runs and does not recover; what is lost, how,
  whether other GL clients trigger it, what the nested and DRM backends
  do with a lost context, and a proposed recovery rule.
- Investigation accepted by the owner; M11 is split into M11a (cursor
  themes, renderer recovery: built) and M11b (decorations: rules only
  until the owner starts it). M12 is not started.

## Cursor themes (M11a, owner)

- The theme is Adwaita, loaded from /usr/share/icons only (no other
  directory: not ~/.icons, not XCURSOR_PATH), at the constant size 24.
  No setting. Debian: the trixie package adwaita-icon-theme ships
  /usr/share/icons/Adwaita/cursors; it goes into the .deb's Depends.
- xcursor 0.3.11 is a direct dependency (MIT, already in the tree
  through Smithay), checked by cargo deny.
- A cursor-shape name is looked up as its CSS name, then its old X11
  names (cursor-icon's alternatives), then the theme's default shape,
  then the built-in arrow. No theme found: the built-in arrow for every
  shape. Inherits are followed within /usr/share/icons only, depth
  limited, loops refused.
- Theme files are read with limits (constants): a maximum file size; an
  image larger than the cursor limit, of zero size or truncated is
  skipped. A broken file never stops sash-wm: the shape falls back.
- Loading: the default shape at startup, every other shape on first use,
  cached for the session per shape and size.
- Debug builds only: XCURSOR_THEME and XCURSOR_SIZE choose another theme
  and size (still from /usr/share/icons). Not compiled into release
  builds; tools/check-release.sh proves it.
- Animated shapes (wait, progress): frames are requested only while an
  animated shape is shown, at the theme's frame delays. A static cursor
  and a still pointer still give 0 frames.
- On DRM the themed image goes to the hardware cursor plane when it fits
  the plane (NVIDIA 256x256, the VM 64x64), else it is composited.
  Nested, named shapes still go to the host's cursor through winit (M8);
  client cursor surfaces are unchanged.

## Renderer recovery (M11a, owner)

- A frame that fails to render, swap or flip is retried on a timer with
  backoff: 0.5 s, then 1 s, then 2 s (constants), never by waiting for a
  frame callback. After 3 consecutive failures, or one "context lost",
  the renderer is rebuilt once: a new EGL context and renderer on the
  same display (nested: the same window; DRM: the compositor's buffers
  are reset), then a full redraw; every window's current buffer (shm and
  dmabuf) and the cursor are imported again without any help from the
  clients. If the rebuild fails, or frames fail again after it, sash-wm
  gives up and exits cleanly with a message (DRM: the console restored,
  as for every exit). A frame that succeeds ends the episode; the next
  failure starts the schedule again with one rebuild allowed.
- Rebuild cap (owner, after M11a): more than 3 renderer rebuilds within
  60 s (a rolling count, kept across episodes) is a clean exit too
  (DRM: the console restored).
- Flip watchdog: frames requested but no page flip (DRM) or
  presentation (nested) for 2 s (constant) counts as one failure. Never
  while the session is paused or switched away, the output is hidden or
  not presenting, or the connector is off.
- Every step (failure, retry, rebuild, give-up, watchdog) is logged and
  recorded (recorder events, no data).
- Debug builds only, fault injection (the SASH_DEBUG_ family):
  SASH_DEBUG_FAIL_SWAPS=N fails the next N swaps;
  SASH_DEBUG_CONTEXT_LOST=K reports "context lost" at frame K;
  SASH_DEBUG_STOP_FLIPS=K stops page flips after frame K. Not compiled
  into release builds; tools/check-release.sh proves it.
- Item 3, the nested Firefox reproduction (owner, M11a only): a nested
  sash-wm window on Hyprland is allowed for this one investigation:
  Firefox only through start_firefox on about:blank (no network, crash
  reporter off, throwaway profile), at most 6 attempts, timeouts and
  kills by recorded PIDs only, keys sent only to the nested window, none
  of the owner's other windows touched, focused, resized or moved,
  Hyprland debug logging off, no screenshots except outline captures
  from sash-wm's own offscreen path (looked at before keeping). The
  owner is told before the runs start. "Nothing on the real display"
  holds for everything else, the DRM backend included.
- The VM guest gets adwaita-icon-theme (trixie, apt in the guest; owner
  after M11a: about 0.5 MB, at most 5.5 MB with its dependencies) so the
  VM scenario checks the theme path, not only the fallback.
- tools/stage-c.sh --firefox (owner, after M11a): starts Firefox only
  through start_firefox (throwaway profile, about:blank, no network,
  crash reporter and telemetry off, only recorded PIDs killed), under
  the same timeout and logging. The owner runs it on the real display.

## Decorations (M11b, owner)

- xdg-decoration: every client that negotiates it is answered
  server_side, also when it asks for client_side (recorded in the log and
  by the recorder).
- Floating windows (also snapped and top-zone maximized): a 32 px bar
  plus a 1 px border. Tiled windows: the 1 px border only, no bar.
  Fullscreen: neither. Clients that do not negotiate keep their own
  decorations and get no bar, but get the 1 px border.
- Bar and border belong to the window's OUTER rectangle. Snap zones,
  tiling coverage and every earlier invariant apply to outer
  rectangles; the client gets outer minus bar minus borders. The
  top-edge rule applies to the outer rectangle.
- The bar has buttons only, no title text (no font dependency; glyphs
  are drawn from simple shapes). Buttons on the LEFT (a constant), in
  the order close, maximize. Close does what mod+Q does; maximize
  toggles the top-zone snap. No minimize, no menu, no double-click
  action. A left-drag on the bar moves the window like mod+left-drag
  (snapping and zones included).
- Colours (dark values until the appearance setting exists): bar fill
  #2C2C2E; border focused #0A84FF, unfocused #3A3A3C; glyphs focused
  #FFFFFF at 92% alpha, unfocused #FFFFFF at 35% alpha; hover background
  close #FF453A, maximize #3A3A3C. Hover also changes the button's shape
  or fill, never colour only; focus is shown by border and glyph
  brightness together.
- Constants: bar height 32 px, border 1 px, button hit area 32x32,
  glyph about 12 px, resize zones 6 px wide inside the border and 16x16
  px at the corners, clamped to the output.
- Where a button's 32x32 area overlaps a resize zone or corner, the
  button wins (owner, M11b): the zones apply only outside the buttons.
- A button acts on release, only if the pointer is still over it; any
  press on the bar focuses and raises its window like a plain click.
  While a window is maximized its maximize glyph shows "restore" (two
  overlapping squares).
- Resize zones show the matching resize cursor (Adwaita names, through
  the cursor machinery) and start the normal resize. On floating windows
  (also snapped and maximized) that is the floating resize; on tiled
  windows (owner, M11b) a zone moves the split on that side like
  mod+right-drag on that edge or corner, and edges on the output
  boundary have no zone. Every mouse action has an existing keybind;
  docs/keybinds.md says so (pointer rows of the keybind table).
- Pointer events over the bar, border, buttons and zones never reach
  the client (it gets a pointer leave when the pointer moves onto them).
  Popups, subsurfaces, drag icons and the clipboard focus rules are
  unchanged.
- A window changing between tiled, floating, snapped and fullscreen gets
  its decoration mode and its new size in one configure batch.
- Debug outline screenshots show the bar band and the border as
  outlines; still no window contents.
- Output edges (owner, after M11b): windows the layout places on the
  output boundary (tiles, snapped and maximized windows) have no border
  and no resize zone on their edges that lie on the boundary; interior
  edges keep both. The edge pixels belong to the client. A plain
  floating window keeps all four borders wherever it is (its client
  size never changes by moving it). The snap triggers (8 px bands,
  64 px corners) are measured from the true output edge, unchanged; a
  test proves it.
- Known UX question (owner, after M11b): maximize, then restore, from a
  snapped half returns to the pre-snap rectangle, not to the half. Kept
  for now.
- sash-ui apps will draw the shared chrome themselves later (option 4);
  until a native Sash protocol tells them apart, every negotiating
  client is server-side.

## Layer-shell, trust, text stack (M12): investigate and propose first

- M12 is an investigation (owner, after M11b): report tables and short
  notes with sources in docs/m12-investigation.md and a summary in
  docs/plan.md, then STOP and ask. No code. M13 is not started.
- A. wlr-layer-shell: what Smithay 0.7 supports and what sash-wm must
  build (layers, anchors, margins, exclusive zones, keyboard
  interactivity none/exclusive/on-demand, layer popups, namespaces,
  output assignment); how the usable area feeds tiling coverage,
  maximize, snapping, the 64 px rule and the top-edge rule, and how the
  coverage invariants change; ordering against windows, bars, popups,
  drag icons and fullscreen; keyboard focus between layer surfaces and
  windows.
- B. Trust and privacy: a trust model for layer-shell (layers,
  exclusive keyboard focus) and later ext-session-lock, screen capture,
  idle-inhibit, virtual keyboard and input injection; a table with pros,
  cons and a recommendation. No new setting.
- C. The shared text stack in sash-ui (font loading, shaping and
  fallback, glyph atlas, rasterisation, hinting, scaling; bidi and IME
  as known limits); whether sash-wm can use sash-ui without becoming
  its own client; crate boundaries, licences, binary size, frame time.
- D. A milestone plan: layer-shell, the text stack, window titles in
  bars, then a minimal dock and launcher as the first layer-shell
  clients; smallest steps and what is deliberately left out.

## Protocol support and known limits

- xdg popups are supported from M7 (see "Popups (M7)").
- zwp_linux_dmabuf_v1 (version 5) and wp_cursor_shape_manager_v1 from M8.
  Smithay 0.7 renumbers dmabuf planes and checks neither a format's plane
  count nor the modifier, so sash-wm pre-checks zwp_linux_buffer_params_v1
  creates (incomplete, invalid_format) before Smithay sees them. No explicit
  sync (wp_linux_drm_syncobj_v1).
- `wl_data_device_manager` (version 3) from M10: clipboard and drag and
  drop (see "Clipboard and drag and drop (M10)"). No primary selection,
  no data-control.
- `zxdg_decoration_manager_v1` (version 1) from M11b: every negotiating
  client is answered server_side (see "Decorations (M11b, owner)").
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
- Input (owner decision after M8, the "middle ground"): fixed defaults
  for now. The keyboard keymap honours XKB_DEFAULT_LAYOUT,
  XKB_DEFAULT_VARIANT, XKB_DEFAULT_OPTIONS and the other XKB_DEFAULT_*
  variables; nothing is read from system configuration. Key repeat
  600 ms / 25 Hz, libinput's default pointer speed, natural scrolling
  off, tap-to-click on. An "input" group (keyboard layout, key repeat
  delay and rate, pointer speed, natural scrolling, tap-to-click) joins
  the closed settings list when the settings protocol is built; no
  input setting exists before then.

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

## Process and privacy rules (owner, after M7)

- Never signal processes by name: no pkill, killall or pkill -f. Only
  kill PIDs you started, and record them when you start them.
- Run real clients (Firefox, pavucontrol, anything with personal data)
  only with a throwaway profile or a throwaway config directory. Look
  at every screenshot before keeping it; delete it at once if it shows
  personal content; never commit it.
- Do not run anything that needs a password, a login or the owner's
  network accounts.
- Clients that can reach the network or report crashes (owner, after
  M10; Firefox especially, any browser): started with
  MOZ_CRASHREPORTER_DISABLE=1, a throwaway profile, telemetry and
  updates off, and no network (a proxy to nowhere) unless a test needs
  it. Clients that can show device names or personal data (pavucontrol,
  file managers) get no content screenshots at all. Enforced in tools/:
  tools/measure/common.sh (`start_firefox`; `start_client` refuses
  browsers and marks no-capture clients, whose presence makes every
  capture a no-op) and tools/check-clients.sh (a static check of
  tools/).
- The scratchpad is volatile (it is lost on reboot). Measurement and
  similarity scripts live in tools/, reviewed screenshots in
  docs/screenshots/; nothing in the repo depends on the scratchpad.

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

- Screenshots come only from sash-wm's own offscreen capture (debug
  builds): `SASH_CAPTURE_DIR=<dir>` in the environment, then
  `kill -USR1 <pid>` writes `<dir>/sash-wm-capture-<n>.png`. It re-renders
  the current output offscreen and never reads the host screen (works
  while the nested window is hidden). Every image is looked at before it
  is used. Never capture the host screen.
- Busy-client runs report a frame-interval histogram, not only render
  time.

## Milestone routine

- After each milestone: build/clippy/fmt/test output, trixie container
  build, push `main` to the private remote, then STOP for approval.
