<!-- SPDX-License-Identifier: Apache-2.0 -->
# M11 investigation: cursor themes, xdg-decoration, and the nested window going blank

Investigation only (CLAUDE.md, "Cursor themes, xdg-decoration (M11)"). Nothing here is
built; the decisions at the end are the owner's. Measured on the owner's machine
(Arch, NVIDIA RTX 4070, driver 615.78.08, Hyprland host) and in a debian:trixie
container (packages only, nothing installed on the host). Scripts:
tools/measure/probe-nested-egl.sh; the theme analysis is reproducible with the commands
in the notes.

## 0. The nested window stops drawing once Firefox runs (after M10)

### Reproduction (m10 code, tools/measure/probe-nested-egl.sh, 15 to 20 s per run)

| Client in nested sash-wm | Runs | Rendering failed | Frames after the failure |
|---|---|---|---|
| foot | 2 | 0 | |
| kitty (OpenGL) | 1 | 0 | |
| mpv (gpu output) | 1 | 0 (inconclusive: the test source may not have played) | |
| Firefox, GPU (WebRender, dmabuf buffers) | 6 (plus 3 the day before, all failed) | 3 (plus 3) | 0 in every case |
| Firefox, software rendering (shm buffers) | 1 | 0 | |
| Firefox, m9e build (before M10) | 1 | 1 | 0 |

### What fails (temporary instrumentation, reverted)

- The swap, not the draw: `submit` fails with `ContextLost(EGLCreateSurface(BadAlloc))`.
  During the swap Smithay recreates the EGL window surface; `eglCreateWindowSurface`
  returns `EGL_BAD_ALLOC`. Smithay 0.7 reports every swap error except
  `EGL_BAD_SURFACE` as "context lost", so the context itself may well be intact.
- It happens early, after Firefox's first few GPU buffers (4 dmabuf imports in the two
  instrumented failing runs, ARGB8888 with an NVIDIA block-linear modifier; 30 to 42 in
  runs that survive). About half of the GPU Firefox runs fail. GPU memory stays at about 1.1 of 12 GB: not a real shortage.
- After the failure the nested window never draws again: a redraw requested 0.5 s later
  produces no frame. The host window waits for the frame callback of a frame that was
  never committed, so winit sends no more redraw events. Clients get no frame callbacks
  either (CLAUDE.md: none while the output is not presenting), so Firefox stops
  painting too.
- The host (Hyprland): its log has no lines about the nested window at its current log
  level. More would need Hyprland's debug logging (the owner's call).

### What the backends do with a lost context today

| | Nested (winit) | DRM |
|---|---|---|
| On a render or swap error | logs "rendering failed", returns | logs, the frame is not queued, idle frame callbacks one refresh later, retried at the next redraw |
| Retry | none: no redraw event comes again (above) | every redraw retries with the same renderer |
| Renderer recreated | never | never |
| Reset detection | none | none: Smithay 0.7 creates contexts without the robustness extensions (no reset notification, no reset status query) |
| What the user sees | a frozen nested window | after a real GPU reset (NVIDIA Xid): the last frame frozen, input and clients alive; Super+Shift+E and VT switches still work. Depending on the driver, GL calls may also fail in other ways |

### Proposed recovery rule (not built)

1. A failed frame is retried by a timer at the refresh period, not by waiting for a
   frame callback that cannot come (fixes the nested stall even when the failure is
   transient). Clients keep getting their idle frame callbacks.
2. Three consecutive failures, or one "context lost": the rendering stack is recreated
   once. DRM: a new EGL display, renderer and DRM compositor, as after a resume (state
   reset, buffers reset), then a full redraw; client buffers are imported again on their
   next commit. Nested: a new EGL surface on the same window first; if that fails too, a
   new context and renderer.
3. If recreation fails, or frames keep failing for 5 s after it: nested sash-wm exits
   with a message (the host is unaffected); DRM sash-wm exits cleanly with the console
   restored, because a frozen screen with live input traps the user.
4. Every step is logged and recorded (recorder events, no data).
5. Later, when Smithay supports it: robust contexts with reset notification, so a GPU
   reset is detected at once instead of after three failures.

## A. Cursor themes

### What the code base already has

| Piece | Provides | Status |
|---|---|---|
| Smithay 0.7 | cursor-shape protocol; `CursorImageStatus::Named(CursorIcon)` | used (M8): nested, the shape goes to the host; DRM draws the built-in arrow for every shape |
| cursor-icon 1.2.0 (via Smithay) | the 36 shape names (`name()`, CSS names) and the old X11 names (`alt_names()`) | in the tree |
| xcursor 0.3.11 (MIT, via Smithay's wayland-cursor) | theme lookup with inheritance (`CursorTheme::load`, `load_icon`, honours XCURSOR_PATH), file parser (`parse_xcursor`: images with nominal size, hotspot, frame delay, RGBA pixels) | in the tree; a direct dependency needs the owner's approval. Alternative: sash-wm's own parser (the format is a small table of contents plus image chunks, about 60 lines) |

### Themes Debian 13 packages (inspected in a debian:trixie container)

| Theme (package, version) | Licence of the cursor files | Shapes covered (of 36) | Nominal sizes | Size installed |
|---|---|---|---|---|
| Adwaita (adwaita-icon-theme 48.1-1) | CC-BY-SA-3.0 or LGPL-3 (part CC-BY-SA-4.0) | 36 (35 by CSS name) | 24, 30, 36, 48, 72, 96 | cursors 11.7 MB (package 13.4 MB) |
| Breeze (breeze-cursor-theme 6.3.5) | LGPL-3+ | 36 (34 by CSS name) | 12 to 72 in steps of 6 | 14.8 MB |
| Bibata, 6 variants (bibata-cursor-theme 2.0.6) | GPL-3.0 | 36 (35) | 16 to 96 | 28 to 32 MB each, 178 MB package |
| DMZ White/Black (dmz-cursor-theme) | CC-BY-SA-3.0 | 31 (6 by CSS name; no context-menu, vertical-text, all-scroll, zoom-in, zoom-out) | 24, 32, 48 | 1.7 MB |
| X.org redglass, whiteglass, handhelds (xcursor-themes 1.0.5) | MIT/X11 | 19 or 20 | up to 48 | 2 MB |
| WhiteSur | not packaged in Debian 13 (upstream: GPL-3.0, not verified here) | | | would have to be bundled |

### Cost (all 36 shapes, read and parsed; Python, so an upper bound)

| Theme | At 24 px: frames, pixels, time | At 48 px |
|---|---|---|
| Adwaita | 154 frames, 346 KiB, 3.2 ms | 154 frames, 1386 KiB, 2.7 ms |
| Breeze | 80, 320 KiB, 1.5 ms (largest image 32 px) | 80, 1280 KiB (largest 64 px) |
| DMZ-White | 91, 205 KiB, 0.6 ms (31 shapes) | 91, 819 KiB |
| Bibata Modern Classic | 158, 356 KiB, 4.0 ms | 158, 1422 KiB |

### Notes

- Names: a shape is looked up by its CSS name (`CursorIcon::name()`), then its old X11
  names (`alt_names()`), then the theme's "default", then the built-in arrow. Adwaita
  needs only the first step for 35 of 36 shapes.
- Scaling at 5120x1440: the panel is 1190 mm wide, about 109 dpi, so a 24 px cursor is
  about 5.6 mm (24 px at 96 dpi is 6.4 mm): size 24 at scale 1 is right. Adwaita's 30,
  36, 48, 72 and 96 px images cover scales 1.25, 1.5, 2, 3 and 4 when fractional
  scaling comes (roadmap). Every image fits the NVIDIA cursor plane (256x256); the VM's
  plane is 64x64, so sizes above 64 would be composited there.
- Loading and caching: the default arrow at startup; other shapes on first use, kept for
  the session per (shape, size); a few milliseconds and under 0.5 MiB at 24 px.
- Animated shapes (wait, progress: most of the 154 frames) need frames at the
  animation's rate while shown: an exception to "no motion, no frame" that needs a rule.
- Fallback: no theme found, the built-in arrow for every shape (today's behaviour); a
  shape missing, the theme's default, then the built-in arrow.
- Nested: shapes keep going to the host's cursor (host theme); the theme matters on DRM.
- XCURSOR_THEME and XCURSOR_SIZE in debug builds only: useful to check other themes and
  the hi-dpi sizes without a setting. Release builds use the fixed theme and size.

### Proposal A

The system's Adwaita, loaded at runtime from /usr/share/icons (the Debian package
adwaita-icon-theme, a runtime dependency of sash-wm's future package), size 24, not
copied into the repository (no CC-BY-SA or LGPL files in an Apache-2.0 tree), with the
built-in arrow as the fallback. xcursor 0.3.11 as a direct dependency (or an own parser).

## B. xdg-decoration

### What clients do (from their documentation and known behaviour; foot's bar seen in M10 captures)

| Client | Uses xdg-decoration | Asks for | Without server-side decorations |
|---|---|---|---|
| foot | yes | server-side (csd.preferred=server) | draws its own bar with minimize, maximize, close (seen in sash-wm today) |
| kitty | yes | server-side | its own simple bar |
| Qt 6 apps | yes | server-side | Qt's own bar |
| GTK 3 and 4 (pavucontrol, Firefox, GNOME apps) | no | | always their own header bar |
| Chromium and Electron | yes | client-side by default | their own bar |
| SDL3, mpv | yes | server-side | plain bar |

Requests clients send today: `set_maximized` and `unset_maximized` (handled: the maximized
snap), `set_fullscreen` (handled), `set_minimized` (ignored: sash-wm has no minimized
state, so a client's minimize button does nothing), `show_window_menu` (ignored; GTK
sends it on a right-click of its header bar).

### Options

| | (1) Always client-side | (2) Server-side bars on every window | (3a) Server-side only for floating windows that ask or draw none | (3b) Server-side mode for every client that supports it; sash-wm draws a bar only on floating windows | (4) As 3b, but sash-ui apps client-side (shared chrome) |
|---|---|---|---|---|---|
| Work | none | most | medium | medium | 3b plus a native Sash protocol signal |
| Tiled windows | foot, kitty, Qt show their own bars in every tile, with a minimize button that does nothing | a bar in every tile: about 30 px per tile lost | as (1) | no bar: clean tiles; GTK and Firefox keep their header bars | as 3b |
| Floating windows | client bars | sash-wm bars | sash-wm bars for clients that ask | sash-wm bars | sash-wm bars; sash-ui apps draw the same chrome |
| Look | mixed | uniform for clients that support it; GTK still draws its own (two styles) | mixed in tiles | uniform where possible; GTK and Firefox differ | uniform for sash-ui and supporting clients |
| No hidden controls | dead minimize buttons (visible controls that do nothing) | every window has visible move, maximize and close | partly | tiled windows: no mouse controls on screen, keybinds and mod+drag only | as 3b |
| Every mouse action has a keybind | yes | bar drag = mod+left-drag, maximize button = mod+Up, close = mod+Q, double-click = mod+Up | yes | yes | yes |
| Closed settings list | no setting | no setting | no setting | no setting | no setting |
| Hit-testing, resize edges | client's | the bar is part of the window; resize zones around floating windows (the M4 known limit) | as (2) for those windows | as (2) for floating windows | as 3b |
| Snapping, fullscreen | unchanged | bars on snapped windows; none in fullscreen | unchanged | owner decides: bar on halves and quarters or not; none when maximized or fullscreen | as 3b |
| 64 px visibility, top edge | unchanged | the bar counts as the window's top; the top-edge rule keeps it on screen | as (2) | as (2) | as (2) |
| Drawing cost | none | rectangles plus text for every window | for floating windows | for floating windows | for floating windows |
| Title text | client's | needs a font rasterizer (a new crate) or sash-ui's text stack | same | same, or bars with buttons only until sash-ui text exists | same |
| Window menu, minimize | ignored, dead buttons | not drawn | partly | not drawn on sash-wm bars; GTK's right-click menu request stays ignored | as 3b |

Recommendation: (4), built as 3b now (server-side mode for every client that supports
it; a bar with maximize and close only on floating windows, no minimize, no window menu)
and the sash-ui split when sash-ui apps exist. It removes dead buttons from tiles,
keeps every bar action a keybind, adds no setting, and limits drawing to floating
windows. Open points: bars on snapped halves and quarters; title text now (a font
crate) or buttons only until sash-ui has text; a visible 1 px border so resize edges
are not hidden controls.

## C. What changes for M12 (layer-shell)

| Topic | Effect on M12 |
|---|---|
| Cursor themes | Panels and launchers ask for shapes (pointer, text) through cursor-shape; on DRM they show only the arrow until themes exist: do A before or with M12 |
| Usable area | Exclusive zones shrink the area used for tiling, maximize, snap zones and the top-edge rule (already worded as "usable area"); a floating window's bar stays below a top panel |
| Z-order | Background and bottom layers below windows; top and overlay layers above floating windows and bars; the snap preview, drag icon and cursor above all (owner to confirm the preview's place) |
| Hit-testing | Top and overlay layers before windows; bars are part of their window |
| Focus | Layer surfaces with keyboard interactivity take focus; the focus-on-map rule and the clipboard's focus rule apply to them too (a launcher's text field can paste) |
| Text | Both bar titles and the shell need text; one text stack (sash-ui) for both suggests the order: M12 with sash-ui's text, then bar titles |
| Decorations and the panel | With 3b, tiled windows have no bars, so the panel (a sash-ui client) is where a mouse user finds window controls (list, close): worth planning in M12's shell scope |
