# M16 investigation: the shell and sash-ui

Investigation only (owner, after M15b): no code in the repository. Rules in CLAUDE.md, "Shell and sash-ui (M16, owner)". Sources are linked per row; "unverified" marks what could not be checked. Checked on 2026-11-05.

Measurements were made in throwaway crates outside the repository, under ~/.cache/sash/m16-* (not part of the repository; the sources are summarised here so they can be redone):
- `m16-measure`: tiny-skia drawing;
- `m16-a11y`: the size of AccessKit's adapter;
- `m16-empty`: a baseline binary.

All were built in release with the pinned toolchain on the development host (Ryzen 7 7700X, one core). `cargo deny check licenses` with the repository's deny.toml passed for both crate trees.

Decisions already made by the owner are not reopened here (the dock as the only chrome, three running-states, the right zone, the Windows-11-style launcher, the palette and accents, Manrope 13 px, the proportional radius, no GTK or Qt, the UX rules).

## Questions for the owner (summary)

| # | Question | Recommendation |
|---|---|---|
| Q1 | New crates: the dock and launcher binary, and Settings (CLAUDE.md: no crate without asking). Their names (CLAUDE.md allows only sash, sash-wm and sash-ui, and plain names for apps). | One binary for the dock and the launcher (one privileged process with two layer surfaces), and one for Settings. Names are the owner's choice. |
| Q2 | Drawing: CPU (tiny-skia into shm) or GPU. | CPU for v1 (A3), with damage tracking; sash-wm also advertises wl_shm ABGR8888 so no per-pixel swap is needed. |
| Q3 | Client plumbing: smithay-client-toolkit 0.21.1 (MIT), plus about 10 new crates. | Yes (A2). |
| Q4 | Accessibility: build AccessKit now, or record it as a known limit. | Build the hooks now (a semantic tree and the focus model in sash-ui, no D-Bus); the AccessKit adapter is a known limit, dated "before the v1 gates" (B1). |
| Q5 | Input methods. | Known limit, dated together with the open IME trust question; sash-ui's text field keeps the preedit and cursor-rectangle hooks (B2). |
| Q6 | Icon theme: WhiteSur is GPL-3.0 and not in Debian; no packaged theme has a licence on the crate allow list. | A first-party Apache-2.0 symbolic set for the chrome; app icons from each app's own hicolor files; adwaita-icon-theme (already a dependency for cursors) for folders and mimetypes; resvg for SVG (C2). |
| Q7 | Who starts apps: the dock itself, or sash-wm on request. | The dock, with no shell, a clean environment and its own session (C4). sash-wm spawning arbitrary apps clashes with the fixed-list rule. |
| Q8 | The window list: a standard action protocol (wlr) or a native one. | Smithay's ext-foreign-toplevel-list (privileged through its filter) plus a small native Sash protocol for actions and states (C6). |
| Q9 | D-Bus: zbus (pure Rust, MIT, about 100 crates with its async stack) for the tray, notifications, network and power. | Accept zbus once, in a later milestone; v1 of the dock has no tray (D). |
| Q10 | "Recent" in the launcher: recently-used.xbel (paths of every opened file) or recently launched apps only. | Recently launched apps only (C3). |
| Q11 | Contrast: 35% text fails 4.5:1 everywhere; 55% on #E5E5EA misses by 0.004; accent text on #2C2C2E fails. | Treat these as design rules (B4). The palette stays as decided. |

## A. sash-ui architecture

### A1. Crate layout

| Crate | Kind | Depends on | Notes |
|---|---|---|---|
| sash-text | lib (exists) | swash and its crates | unchanged |
| sash-ui | lib (stub today) | sash-text, smithay-client-toolkit, tiny-skia, (later) resvg | Wayland client plumbing, drawing, layout, widgets, theme tokens, animation; no app logic |
| dock and launcher | bin (new; Q1) | sash-ui | one privileged process: the dock layer surface and the launcher overlay layer surface; spawned by sash-wm from the fixed list (M13a) |
| Settings | bin (new; Q1) | sash-ui | a normal xdg toplevel client (not privileged); talks to sash-wm through the native settings protocol when it exists |
| sash-wm | bin (exists) | sash-text only | never sash-ui (CLAUDE.md) |

### A2. Wayland client plumbing: smithay-client-toolkit (SCTK)

- **Version:** 0.21.1 (2026-07-23), MIT, MSRV 1.86 ([crates.io](https://crates.io/api/v1/crates/smithay-client-toolkit), [CHANGELOG](https://github.com/Smithay/client-toolkit/blob/master/CHANGELOG.md)).
- **Compatibility:** it needs wayland-client ^0.31.14, wayland-protocols ^0.32.9, -wlr ^0.3.1 and -misc ^0.3.6, so the versions already in Cargo.lock fit ([Cargo.toml](https://docs.rs/crate/smithay-client-toolkit/0.21.1/source/Cargo.toml.orig)).
- **New crates it brings:**
  - wayland-protocols-experimental (unconditional);
  - wayland-csd-frame;
  - cursor-icon;
  - xkeysym;
  - calloop-wayland-source;
  - memmap2: this maps shm pools, not fonts, so the no-mmap font rule is untouched;
  - the xkbcommon crate (feature `xkbcommon`): it binds the C libxkbcommon (trixie 1.7), which sash-wm already links through Smithay.
- All of these are MIT or MIT/Apache/Zlib.

| Need | SCTK 0.21.1 | Source |
|---|---|---|
| shm buffers | yes: `shm::slot::SlotPool`, which refuses to draw into a slot that is still attached (until wl_buffer.release) | [docs](https://docs.rs/smithay-client-toolkit/0.21.1/smithay_client_toolkit/shm/index.html) |
| xdg-shell windows, popups | yes | [docs](https://docs.rs/smithay-client-toolkit/0.21.1/smithay_client_toolkit/shell/index.html) |
| zwlr_layer_shell_v1 (client) | yes, `shell::wlr_layer` | same |
| wp_cursor_shape_v1 | yes (`ThemedPointer` uses it when offered) | [docs](https://docs.rs/smithay-client-toolkit/0.21.1/smithay_client_toolkit/seat/pointer/index.html) |
| keyboard and key repeat | yes, with the `xkbcommon` and `calloop` features | [docs](https://docs.rs/smithay-client-toolkit/0.21.1/smithay_client_toolkit/seat/struct.SeatState.html) |
| xdg-activation-v1 | yes, `activation` | [docs](https://docs.rs/smithay-client-toolkit/0.21.1/smithay_client_toolkit/) |
| ext-foreign-toplevel-list | yes | same |
| data device (clipboard) | yes | same |
| wp_fractional_scale_v1 | **no**: integer `scale_factor_changed` only; bind the protocol directly | [docs](https://docs.rs/smithay-client-toolkit/0.21.1/smithay_client_toolkit/compositor/trait.CompositorHandler.html) |
| wp_viewporter | **no**; bind directly | same |
| zwp_text_input_v3 | **no**; bind directly | same |

Event loop: calloop, also used by sash-wm. The wayland queue is a calloop source, and timers are `calloop::timer::Timer` (no system resource; [docs](https://docs.rs/calloop/latest/calloop/timer/index.html)). Idle means no armed timer and no frame callback requested. Fractional scale and viewporter are on the compositor roadmap; until then sash-ui draws at integer scale.

### A3. CPU or GPU drawing

**Measured** in `m16-measure`: tiny-skia 0.11.4 (default features, SSE2 baseline, one core). 0.12.0 came out on 2026-02-02 ([crates.io](https://crates.io/api/v1/crates/tiny-skia)) and was not measured. 200 frames each; p50 / p99.

| Surface | Pixels | Full draw | RGBA to BGRA swap | Plain copy |
|---|---|---|---|---|
| dock 5120x64, scale 1, 16 icons | 0.33 M | 917 / 1048 us | 64 / 112 us | |
| dock 10240x128, scale 2 | 1.31 M | 3441 / 3558 us | 283 / 413 us | |
| launcher 800x720, scale 1, 16 rows | 0.58 M | 124 / 134 us | 119 / 140 us | 33 / 51 us |
| launcher 1600x1440, scale 2 | 2.3 M | 342 / 427 us | 463 / 568 us | 144 / 281 us |
| page 1920x1080 | 2.1 M | 302 / 330 us | 424 / 514 us | 131 / 227 us |
| page 3840x1080 | 4.1 M | 666 / 1245 us | 944 / 1449 us | 635 / 1259 us |
| page 5120x1440 (a maximized Settings) | 7.4 M | 1423 / 2288 us | 2007 / 2735 us | 1419 / 2290 us |
| page 5120x1440, one 5088x40 row redrawn | 0.2 M | 517 / 552 us (with a full-size clip mask) | | |

What the numbers mean:
- **The dock draws slowly for its size.** Each icon goes through a full pixmap draw, so it costs about 0.9 ms at scale 1. The dock only redraws on change (a running-state, the hover, the clock once a minute), so this is not per frame.
- **Clip masks cost too.** A full-size mask costs more than the drawing it clips. Damage should be drawn into small pixmaps, or with a mask of the damaged size.
- **Full redraws at 240 Hz are not realistic on the CPU.**
  - A 5120x1440 frame is 28.1 MiB.
  - The client writes about 7.1 GB/s at 240 Hz.
  - sash-wm then uploads the same amount to the GPU.
  - The draw plus swap at 5120x1440 is about 3.4 ms of the 4.17 ms budget, before the upload.
  - A full-screen animation on the CPU would therefore drop frames. The ~180 ms animations must be small-area (a hover, an opening overlay of launcher size) or run at a lower rate.
- **The pixel swap.**
  - tiny-skia writes RGBA (premultiplied); wl_shm's mandatory ARGB8888 is BGRA in memory.
  - sash-wm advertises only the two mandatory formats today: `ShmState::new(display, Vec::new())` in sash-wm/src/state.rs.
  - Advertising ABGR8888 (which GLES imports) is a one-line compositor change that removes the swap. It is proposed for the first sash-ui milestone.
- **Not measured:** sash-wm's upload of a full 5120x1440 shm buffer per frame. It is measured in step E1.

| Option | Licence | Size and crates | Memory | Pitfalls |
|---|---|---|---|---|
| tiny-skia into shm | BSD-3-Clause | +0.24 MB stripped binary over an empty one (575,744 vs 336,704 bytes), 18 crates in the tree (with png) | 2 buffers of the surface size: 56 MiB for 5120x1440, 2.5 MiB for a 5120x64 dock, 4.4 MiB for an 800x720 launcher | full redraws expensive at 5K; no GPU effects (blur) |
| GLES via glow + khronos-egl + wayland-egl | MIT/Apache/Zlib | a few crates; wayland-egl forces the C libwayland-client ([Cargo.toml](https://docs.rs/crate/wayland-egl/0.32.11/source/Cargo.toml.orig)) | a GL context per process: about 7 MB measured by Mozilla for Mesa ([bug 1710958](https://bugzilla.mozilla.org/show_bug.cgi?id=1710958)); NVIDIA unverified | swap interval 1 blocks while sash-wm sends no frame callbacks (hidden output) |
| femtovg / wgpu / vello | MIT OR Apache-2.0 | wgpu is a large tree (size unverified) | as above or more | startup cost unverified |

**Recommendation (Q2): CPU drawing with tiny-skia for v1.**
- Redraw only on change, with per-widget damage.
- Use `damage_buffer` for the changed rectangles only.
- Double buffering through SlotPool. With buffer-age damage, a frame repaints the union of its own damage and the previous frame's.
- Animations are small-area and capped at the output's refresh.
- GPU drawing stays an option for later (blur needs it), behind the same draw API.

Idle is 0 frames: no frame callback is requested and no timer is armed unless something changes. The clock's one wake per minute is the only periodic work (D1).

### A4. Layout, theme tokens, animation

- **Layout:** a hand-written row / column / stack with fixed, content and flex sizes, in integer logical pixels. That is enough for the dock, the launcher and settings pages, exact and easy to test. taffy 0.15.1 (MIT, [crates.io](https://crates.io/api/v1/crates/taffy)) is the alternative if grids get complex; it is not proposed.
- **Theme tokens:** one struct of named values: the neutral surfaces (bg / surface / raised), text alphas 0.92 / 0.55 / 0.35, border alpha 0.10, the accent preset, the radius rule, Manrope 13 px and weights 400 / 500. Dark and light are two instances. The appearance and accent settings later only pick the instance; no other token is a setting.
- **Animation** (the model and presentation split of Core Animation, [Apple](https://lists.apple.com/archives/Cocoa-dev/2008/Jan/msg00997.html)):
  - Logical state (focus, selection, open or closed, hit-testing) changes at once.
  - The animator only interpolates drawn rectangles and opacities toward it over ~180 ms.
  - Input during an animation acts on the new state.
  - Tests check state without waiting for animation frames.

### A5. Widgets

| Needed by | Widgets |
|---|---|
| dock | panel surface, icon button (with the three running-states and a hover), separator, right zone with clock text; tooltip (a popup) |
| launcher | overlay surface, search field (single line, no IME in v1), grid of icon tiles (pinned), list rows (recent, all apps, with a scroll view), section header, footer buttons (Files, Downloads, Settings, power), a power menu (popup) |
| Settings, first page | window with a sidebar list, page header, picker (enumerable values: a segmented control or a dropdown list), switch, label rows |
| all | keyboard focus ring, text (sash-text), icon image, scroll view |

**Not in v1:**
- multi-line text editing, text selection with the mouse, rich text;
- IME;
- drag and drop between widgets;
- blur and transparency effects;
- custom themes;
- right-to-left layout mirroring;
- touch gestures;
- fractional scale (until the compositor has it);
- GPU drawing.

## B. Accessibility and input methods

### B1. AT-SPI through AccessKit

| Item | Finding | Source |
|---|---|---|
| Versions | accesskit 0.25.2, accesskit_unix 0.24.1, accesskit_consumer 0.39.2; MIT OR Apache-2.0; MSRV 1.87 | [crates.io](https://crates.io/api/v1/crates/accesskit_unix) |
| Toolkit fit | `accesskit_unix::Adapter::new(activation, action, deactivation)` has no winit dependency; `update_if_active` sends tree updates only while an assistive technology is listening | [docs](https://docs.rs/accesskit_unix/latest/accesskit_unix/struct.Adapter.html) |
| Size (measured) | +4.04 MB stripped (4,379,768 bytes vs 336,704 for an empty binary); 103 crates in the tree (zbus 5, async-io stack, atspi, serde) | `m16-a11y` |
| Runtime | the session D-Bus plus at-spi2-core (trixie 2.56.2: bus launcher and registry) | [packages.debian.org](https://packages.debian.org/trixie/amd64/at-spi2-core/filelist) |
| Threads | the adapter runs its own executor thread; handlers are called from it | [source](https://docs.rs/crate/accesskit_unix/latest/source/src/context.rs) |
| Users | egui, Slint, Bevy (through accesskit_winit) | [reverse deps](https://crates.io/api/v1/crates/accesskit_winit/reverse_dependencies) |

Limits:
- Wayland gives no global coordinates, so `set_root_window_bounds` does nothing there.
- Orca's own key commands on Wayland need compositor help: at-spi2-core 2.55.90's `org.freedesktop.a11y.KeyboardMonitor` ([news](https://download.gnome.org/sources/at-spi2-core/2.55/at-spi2-core-2.55.90.news)), or COSMIC's `cosmic-atspi-v1` ([protocol](https://wayland.app/protocols/cosmic-atspi-unstable-v1)). For sash-wm that is another privileged path.

**Recommendation (Q4):**
- Build the hooks now, inside sash-ui and with no dependency: every widget has a role, a label, a state (focused, selected, checked, disabled) and its actions, kept as plain data in a tree with stable ids. Keyboard focus moves only through that tree.
- Tests use the tree: focus order, labels present, every action reachable by keyboard.
- AccessKit maps onto this tree later as a thin adapter.
- Record the adapter as a known limit, dated "before the v1 gates" (docs/v1.md), together with the compositor's keyboard-monitor path for Orca.

### B2. Input methods (text-input-v3)

- **Client side:** zwp_text_input_v3 ([protocol](https://wayland.app/protocols/text-input-unstable-v3)): enable and disable, set_surrounding_text (at most 4000 bytes), set_content_type, set_cursor_rectangle, commit; events preedit_string, commit_string, delete_surrounding_text, done. Not in SCTK; bound directly. An estimated 300 to 600 lines plus tests in sash-ui (unverified).
- **Compositor side:** Smithay 0.7.0 has `wayland::text_input` (v3, no client filter: every app needs it), `wayland::input_method` (zwp_input_method_v2, with a client filter) and `wayland::virtual_keyboard` (with a filter). Checked in the Smithay 0.7.0 source (src/wayland/).
- **The IME itself:** fcitx5 (trixie 5.1.12) and ibus (1.5.32, `ibus-wayland`) both use input-method-v2 on non-GNOME compositors ([fcitx](https://fcitx-im.org/wiki/Using_Fcitx_5_on_Wayland), [ibus](https://www.fedoraproject.org/wiki/Changes/IBus_1.5.32)). That is the open trust question already in docs/plan.md (an IME sees every key).

**Recommendation (Q5):**
- Record it as a known limit, dated "decided together with the IME trust question, before the first app with free text entry (Text Editor)".
- The only v1 text field, the launcher's search, has no IME.
- sash-ui's text field model keeps a preedit range and reports its cursor rectangle from the start, so the protocol can be added without reworking it.

### B3. Keyboard navigation (a rule from day one)

Following the WAI-ARIA Authoring Practices ([keyboard](https://www.w3.org/WAI/ARIA/apg/practices/keyboard-interface/), [menus](https://www.w3.org/WAI/ARIA/apg/patterns/menubar/), [dialogs](https://www.w3.org/WAI/ARIA/apg/patterns/dialog-modal/)), the GNOME HIG ([keyboard](https://developer.gnome.org/hig/guidelines/keyboard.html)) and WinUI ([keyboard](https://learn.microsoft.com/en-us/windows/apps/design/input/keyboard-interactions)). Proposed rules:
- **Tab and Shift+Tab** move between groups in reading order (left to right, top to bottom). Each group (the icon row, a list, a grid, the footer) is one tab stop.
- **Arrow keys** move inside a group (roving focus). Home and End go to the first and last item. Wrapping only in menus.
- **Keys that act:** Enter activates, Space toggles, Escape closes the innermost overlay or popup, and focus returns to what opened it.
- **Focus is never lost:** after a close or a removal it moves to the nearest item.
- **The focus ring** is always visible on keyboard focus: 2 px in the accent, with at least 3:1 against both neighbours (WCAG 2.4.7 and 2.4.13). It is distinct from selection, which is weight 500.
- **Focus order is part of the widget tree.** A test walks every screen by Tab and arrows alone and reaches every control (it follows "every mouse action has a keybind").

### B4. High contrast and large text as design constraints

- **Thresholds:** WCAG 2.2 ([spec](https://www.w3.org/TR/WCAG22/)):
  - text 4.5:1;
  - large text (18 pt, or 14 pt bold) 3:1;
  - non-text (icons, borders, the focus ring) 3:1.
  - Manrope 13 px is normal text.
- **Portal keys** that clients may read later: `org.freedesktop.appearance` `color-scheme`, `accent-color`, `contrast` (1 = higher) and `reduced-motion` ([portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Settings.html)).

Contrast of the decided palette (text blended at its alpha over the surface):

| Text | over #1C1C1E | over #2C2C2E | over #3A3A3C |
|---|---|---|---|
| white 92% | 14.53 | 12.01 | 9.87 |
| white 55% | 5.98 | 5.33 | 4.67 |
| white 35% | 3.20 | 3.07 | **2.84** |

| Text | over #F2F2F7 | over #FFFFFF | over #E5E5EA |
|---|---|---|---|
| black 92% | 16.64 | 18.42 | 14.91 |
| black 55% | 4.63 | 4.74 | **4.4957** |
| black 35% | **2.42** | **2.43** | **2.38** |

| Accent pair | Ratio |
|---|---|
| #0A84FF on #1C1C1E | 4.66 |
| #0A84FF on #2C2C2E | 3.82 |
| #007AFF on #F2F2F7 | 3.60 |
| #007AFF on #FFFFFF | 4.02 |
| white on #0A84FF | 3.65 |
| white on #007AFF | 4.02 |

Proposed constraints (Q11; the palette itself is unchanged):
- 92% for all body text.
- 55% only for secondary text, and in light mode never on #E5E5EA (it misses 4.5:1 by 0.004).
- 35% only for disabled items and non-text marks on the dark surfaces, never in light mode.
- Accent only as fills, rings and icons (3:1 everywhere), not as 13 px text. White text on an accent fill only at 14 pt bold or larger.
- Every size, gap and radius comes from tokens in logical pixels, so large text is a different token set, not a code change.
- A test computes these ratios from the tokens and fails on a regression.

## C. Launching and the shell's data

### C1. .desktop files

- **Spec:** Desktop Entry Specification 1.5 ([spec](https://specifications.freedesktop.org/desktop-entry/latest/)).
- **Keys used:** Type=Application, Name, GenericName, Comment, Icon, Exec, TryExec, Path, Terminal, NoDisplay, Hidden, OnlyShowIn and NotShowIn (against XDG_CURRENT_DESKTOP), Categories, Keywords (search only), StartupNotify, StartupWMClass, Actions with `[Desktop Action x]` ([keys](https://specifications.freedesktop.org/desktop-entry/latest/recognized-keys.html)). DBusActivatable entries need D-Bus (D); until then they are launched through Exec if they have one.
- **Localisation:** `lang_COUNTRY@MODIFIER`, then `lang_COUNTRY`, `lang@MODIFIER`, `lang`, then the plain key, from LC_MESSAGES with the encoding dropped ([spec](https://specifications.freedesktop.org/desktop-entry/latest/localized-keys.html)).
- **Exec** ([spec](https://specifications.freedesktop.org/desktop-entry/latest/exec-variables.html)):
  - The general unescape comes first, then double-quote splitting.
  - Field codes: %f %F %u %U are removed when launching without files; %i, %c and %k are expanded; %% is a literal percent sign; the deprecated codes are dropped.
  - An unknown code makes the entry invalid (not run).
- **Lookup:** `$XDG_DATA_HOME/applications`, then each of `$XDG_DATA_DIRS`. The desktop-file id is the relative path with `/` turned into `-`, and the first occurrence of an id shadows the rest (`Hidden=true` in the user directory removes a system entry).
- **Crates:** freedesktop-desktop-entry 0.8.4 (MPL-2.0, 6 new crates; keeps the last of duplicate keys, which the spec forbids) and freedesktop_entry_parser 2.0.1 (MPL-2.0, nom) ([crates.io](https://crates.io/api/v1/crates/freedesktop-desktop-entry)).

**Recommendation: our own parser in sash-ui (or the dock crate).**
- The format is small.
- Files in ~/.local/share are untrusted input. They need limits (file size, line length, key count, entries per directory), a no-panic guarantee, a fuzz and property test, and Name, Comment and Keywords held as sash-text client text (never logged).

### C2. Icon themes

Lookup follows the Icon Theme Specification ([spec](https://specifications.freedesktop.org/icon-theme/latest/)): index.theme with Directories, Size, Scale, Type (Fixed, Scalable, Threshold) and Inherits; then hicolor; then /usr/share/pixmaps. As for cursors (M11a), the lookup could be limited to /usr/share/icons and /usr/share/pixmaps. That is an owner choice: Flatpak and user-installed apps put their icons in XDG_DATA_HOME and the Flatpak exports.

| Theme | Licence | In trixie | Notes |
|---|---|---|---|
| WhiteSur (the earlier choice) | **GPL-3.0** ([GitHub](https://api.github.com/repos/vinceliuice/WhiteSur-icon-theme)) | **no** ([search](https://packages.debian.org/search?keywords=whitesur&searchon=all&suite=all)) | macOS look; Apple design and trademark questions unverified |
| adwaita-icon-theme | LGPL-3.0-only OR CC-BY-SA-3.0 | 48.1-1 | already a Depends (cursors); mostly symbolic, full-colour devices, mimetypes, places; no app icons |
| papirus-icon-theme | GPL-3.0 | 20250501-1, 187 MB installed ([package](https://packages.debian.org/trixie/papirus-icon-theme)) | full-colour, covers many apps |
| breeze (kf6) | LGPL-3.0-or-later | 6.13.0 | |
| elementary-icon-theme | GPL-3.0-or-later | 8.1.0-1 | |
| Tela, Fluent, Colloid | GPL-3.0 | no | |
| hicolor-icon-theme | (directory skeleton) | 0.18-2 | where apps install their own icons |

A theme is data read at run time, not linked code, so cargo deny does not cover it. Whether a GPL or LGPL theme may ship as a Depends of an Apache-2.0 desktop is the owner's decision; this is not legal advice.

On the development host (Arch, not trixie), 50 desktop entries name a theme icon:
- 32 have a PNG in hicolor or pixmaps;
- 3 have only an SVG;
- 15 are in no hicolor file at all, so they rely on a theme (Adwaita here).

**Recommendation (Q6):**
- Drop WhiteSur.
- A first-party symbolic set (Apache-2.0, drawn for sash-ui: chrome, footer, power, the right zone) is needed anyway.
- App icons come from hicolor, then adwaita-icon-theme (already installed, used for folders and mimetypes), then a first-party generic app icon.
- SVG through resvg 0.49.0 (Apache-2.0 OR MIT, about 15 new crates, tiny-skia shared). usvg's `text`, `system-fonts` and `memmap-fonts` features stay off ([deps](https://crates.io/api/v1/crates/resvg/0.49.0/dependencies)).
- Icons are rasterised once per size and scale and cached with a cap.

### C3. Recent files

- `$XDG_DATA_HOME/recently-used.xbel` (XBEL, the [Desktop Bookmark spec](https://wiki.freedesktop.org/www/Specifications/desktop-bookmark-spec/)) is written by GTK and KDE apps.
- It holds the full paths and URIs of opened documents. Showing it in an overlay shows them on screen (and to any screen share), and reading it needs an XML parser.

**Recommendation (Q10):** "Recent" lists the launcher's own recently launched desktop ids (a small file in XDG_STATE_HOME with ids only, capped). File recents wait for Files.

### C4. Starting apps without passing privilege on

Facts:
- The dock is privileged through its WAYLAND_SOCKET descriptor.
- libwayland 1.23.1 sets FD_CLOEXEC on that descriptor and unsets the variable ([wayland-client.c](https://sources.debian.org/data/main/w/wayland/1.23.1-3/src/wayland-client.c), lines 1326-1340). wayland-rs does the same ([conn.rs](https://github.com/Smithay/wayland-rs/blob/master/wayland-client/src/conn.rs)).
- The existing M13a test proves that a child connecting through the public socket is a normal client.

Proposed launch procedure (in the dock, no shell; Q7):
1. Parse Exec into argv ourselves (C1); never `sh -c`.
2. The program is resolved in PATH. These are unprivileged user apps, so the fixed root-owned list (which covers what sash-wm spawns) does not apply.
3. The child calls setsid and gets its own process group. The dock reaps it at once (it is never the dock's long-lived child).
4. The child gets a fresh environment built from an allow-list, not by subtracting from the dock's: WAYLAND_DISPLAY (the public socket), XDG_RUNTIME_DIR, HOME, PATH, LANG and LC_*, XDG_CURRENT_DESKTOP, XDG_SESSION_TYPE, XDG_DATA_DIRS, DBUS_SESSION_BUS_ADDRESS and XDG_ACTIVATION_TOKEN.
5. It never gets WAYLAND_SOCKET, SASH_* or SASH_DEBUG_*, or DISPLAY (no XWayland).
6. Defence in depth before exec:
   - `close_range(3, ~0, CLOSE_RANGE_CLOEXEC)`;
   - stdin from /dev/null;
   - the signal mask and dispositions reset (ignored signals survive execve; sash-wm ignores SIGUSR1 in release builds).
7. The working directory is `Path`, else $HOME.
8. Terminal=true entries are hidden until a terminal ships.
9. Optional, later: `systemd-run --user` to put each app in an `app-*.scope` or service (cgroup, journal; [systemd](https://systemd.io/DESKTOP_ENVIRONMENTS/)). Not in v1.

Tests:
- a launched child never sees the privileged globals;
- no descriptor above 2 survives;
- the environment equals the allow-list;
- Exec parsing over the spec's examples, with a property test.

sash-wm launching for the dock was considered and is not proposed. It would make the compositor the parent of arbitrary programs (against the fixed-list rule), and it gives nothing over what the dock can already do.

### C5. Startup notification (xdg-activation-v1)

- The dock asks for a token with the click's serial, its own surface and the app id, and passes it as XDG_ACTIVATION_TOKEN. The app calls `activate(token, surface)` ([protocol](https://wayland.app/protocols/xdg-activation-v1)).
- GTK4, Qt 6, Firefox and foot read the variable and unset it.
- Smithay 0.7.0 has `wayland::xdg_activation` (`XdgActivationState`, `request_activation`, `token_created`, `create_external_token`) ([docs](https://docs.rs/smithay/0.7.0/smithay/wayland/xdg_activation/)).

What sash-wm builds:
- The global, for every client.
- Tokens valid only with a recent input serial of the focused client (or from a privileged client), single use, expiring after a constant time.
- An activation of an existing window focuses and raises it. A refused one marks the window urgent, which the dock shows.
- New windows already take focus on map (M9e).
- The token's app id lets the dock turn "launching" into "running" exactly.

### C6. The list of running windows

| Protocol | Gives | Smithay 0.7.0 | Fit |
|---|---|---|---|
| ext-foreign-toplevel-list-v1 (staging) | handles with title, app_id, a unique identifier; read-only | **yes**: `ForeignToplevelListState::new_with_filter(dh, can_view)` (checked in the source) | the filter makes it privileged-only, as M13a requires |
| zwlr-foreign-toplevel-management-v1 | title, app_id, states (maximized, minimized, activated, fullscreen), activate, close, minimize | no (bindings in wayland-protocols-wlr 0.3.12) | its "minimized" has no meaning in Sash |
| cosmic-toplevel-info and management | extensions on the ext handle | no | COSMIC only |
| ext-foreign-toplevel-state and -management drafts | the same, upstream | no; drafts ([wayland-protocols MRs](https://gitlab.freedesktop.org/wayland/wayland-protocols/-/merge_requests/196)) | |
| ext-workspace-v1 (staging) | workspace groups and workspaces with state and activation | no (bindings in wayland-protocols 0.32.13) | later |

**Recommendation (Q8):** Smithay's ext-foreign-toplevel-list, privileged through its filter, plus a small native Sash protocol on its handle:
- a state event: focused, urgent, fullscreen, tiled or floating;
- requests: activate and close.

This is the first piece of the native Sash protocol, versioned and tested headless.

Titles and app ids are untrusted:
- sash-wm sends them capped (the 1024-byte client text) and never logs them;
- the dock holds them as client text;
- an app id groups windows and picks an icon, and never grants anything (any client can claim any app id).

Matching a window to a .desktop entry:
1. the activation token's app id;
2. app_id equal to the desktop id;
3. app_id equal to StartupWMClass;
4. a case-insensitive id match;
5. otherwise a generic icon.

Known failures: Electron and Chromium app ids, wrappers and terminals, app ids changed after map, and several entries sharing one StartupWMClass. GNOME's tracker uses the same order ([shell-window-tracker.c](https://github.com/GNOME/gnome-shell/blob/main/src/shell-window-tracker.c)).

Minimize does not exist. docs/plan.md already has the proposal "Hiding windows (minimize) through the dock", for the owner.

The three running-states, as I read the decision (to be confirmed): pinned but not running, running, and running with focus. "Urgent" from a refused activation would be a marker on top, not a fourth state.

Workspaces later:
- ext-workspace-v1 in the same privileged way;
- the native state event gains a workspace id;
- the right zone shows them.
- Workspaces do not exist yet, and they would need their own rules (they are not a layout mode).

## D. The dock's right zone

| Item | Smallest useful version | Talks to | Privilege | Risks | Crates (licence) | trixie |
|---|---|---|---|---|---|---|
| Clock | local HH:MM, the date in a tooltip | /etc/localtime, TZ, /usr/share/zoneinfo; a timerfd on CLOCK_REALTIME with TFD_TIMER_CANCEL_ON_SET at the next minute ([man](https://man7.org/linux/man-pages/man2/timerfd_create.2.html)); inotify on /etc for a zone change | none | low | jiff 0.2.39 (MIT option of Unlicense OR MIT), or our own TZif reader; nix (already a dependency of sash-wm; its timerfd and inotify features unchecked) | tzdata 2026c |
| Tray (StatusNotifierItem) | watcher and host, IconName or IconPixmap, Activate on click, a dbusmenu menu | session D-Bus (`org.kde.StatusNotifierWatcher`, the deployed name; unverified) ([spec](https://specifications.freedesktop.org/status-notifier-item/0.1/)) | none, but any session client can register an item | untrusted titles, tooltips and pixmaps from any process; a D-Bus connection in a privileged process; Electron apps all share one id | zbus 6.2.1 (MIT); system-tray 0.8.9 (MIT, pulls tokio); ksni is Unlicense only (outside the list) | n/a |
| Notifications | `org.freedesktop.Notifications` 1.3 with the `body` capability: Notify, CloseNotification, GetCapabilities, GetServerInformation, NotificationClosed; urgency, replaces_id, expiry; plain text ([spec](https://specifications.freedesktop.org/notification/latest/)) | session D-Bus; drawn on a privileged layer surface | owning the name (none) | contents are private (never logged, recorded or in screenshots); image-data has no limit in the spec (cap it); a per-sender rate limit | zbus | portal notifications reach it through xdg-desktop-portal-gtk 1.15.3 ([source](https://raw.githubusercontent.com/flatpak/xdg-desktop-portal-gtk/main/src/fdonotification.c)) |
| Volume | default output volume slider and mute | PipeWire: spawning /usr/bin/wpctl (fixed root-owned path; no change events, so poll while the popup is open) or pipewire-rs 0.10.1 (MIT; binds libpipewire, Expat; needs bindgen and libclang at build time, unverified) | the user session | device names are personal (no content screenshots, never recorded); wpctl's output format is not promised stable; libpulse is LGPL (flag) | | pipewire 1.4.2, wireplumber 0.5.8 |
| Network | read-only state: connected or not, type, Wi-Fi strength | system D-Bus `org.freedesktop.NetworkManager` (reads need no authorisation; [API](https://networkmanager.dev/docs/api/latest/gdbus-org.freedesktop.NetworkManager.html)) | reading: none; connecting or toggling Wi-Fi: allowed for the active local session by the default polkit policy; Wi-Fi passwords need a secret agent (and a keyring) | SSIDs and BSSIDs reveal location (never logged or recorded); NetworkManager is absent on a non-desktop trixie install (hide the indicator) | zbus | network-manager 1.52.1 |
| Power (launcher footer) | power off, reboot, suspend; each shown only when `Can*` says yes | system D-Bus `org.freedesktop.login1` ([man](https://www.freedesktop.org/software/systemd/man/latest/org.freedesktop.login1.html)) | allowed for the active local session by default (no polkit agent needed) | power off is destructive: a confirmation (the UX rule) | zbus, or spawning /usr/bin/systemctl (fixed path) | systemd 257 |
| Polkit agent | (note) | | handles the user's password | the highest trust weight; libpolkit-agent-1 is LGPL; not needed for the defaults above | | polkitd 126 |
| Lock | (later) | ext-session-lock-v1 (Smithay 0.7.0 `session_lock`) and logind Lock / PrepareForSleep | a privileged spawned client | the existing rule disables keybinds while locked | | |

Notes:
- **zbus is the one shared dependency** for the tray, notifications, network, power and later AccessKit (whose tree includes zbus 5).
- **What zbus costs:** in `m16-a11y` the whole AccessKit tree (zbus with its async stack) is 103 crates and +4.0 MB. zbus alone was not measured.
- **Recommendation (Q9):** zbus is accepted once, in its own milestone after the first dock; the first dock has the clock only.
- **Without D-Bus**, power works by spawning `/usr/bin/systemctl poweroff|reboot|suspend` (a fixed root-owned path), which would make the launcher's power menu possible earlier.
- **12 or 24 hours:** not a setting. The proposal is to read LC_TIME's `T_FMT` through `nl_langinfo` (12 h if it contains `%p` or `%r`; [man](https://man7.org/linux/man-pages/man3/nl_langinfo.3.html)); a fixed 24 h is the simpler alternative.

## E. Milestone plan

The smallest steps, each with a report and a STOP. Every step includes the standard checks (tools/check-all.sh, trixie, fresh clone) and the "nothing on the real display" rule. Headless tests run the client against sash-wm's test harness.

| Step | Builds | Tests | Excluded |
|---|---|---|---|
| E1 | sash-ui's first draw: SCTK layer surface, SlotPool, tiny-skia, sash-text; a rectangle and a line of text in a test panel (a layer surface with exclusive zone = visible size + margin, test builds only). sash-wm advertises wl_shm ABGR8888. Measure: sash-wm's upload of a full 5120x1440 shm buffer per frame, client draw times, memory | golden images of the panel at scales 1 and 2 (CPU, everywhere); a headless client harness (the client connects to the test harness's socket, maps, draws); idle: 0 frames and 0 wakeups over 10 s with nothing changing; damage: a change redraws only its rectangle | widgets, input |
| E2 | Widget tree, layout (row, column, stack), theme tokens (dark and light), the semantic tree and focus model (B1, B3), pointer and keyboard input, the focus ring, damage per widget, the animation model | layout unit and property tests (exact integer rectangles); hit-testing; Tab and arrow focus order; contrast ratios from the tokens (B4); animation never delays state (state checked before any animation frame); goldens | icons, apps |
| E3 | Icon lookup (Icon Theme Spec, hicolor, adwaita, pixmaps), resvg for SVG, the first-party symbolic set, a raster cache with a cap | lookup against fixture themes (Inherits, sizes, scales, Threshold, missing icons); goldens of icons at scales 1 and 2; cache cap and eviction; broken or huge SVG and PNG files never panic (limits as constants) | |
| E4 | .desktop parsing: lookup and shadowing, localisation, Exec into argv, Actions, NoDisplay, Hidden, OnlyShowIn, TryExec; limits; untrusted strings as client text | the spec's examples; property and fuzz tests (no panic, limits); shadowing; a leak test (names never logged) | launching |
| E5 | sash-wm: ext-foreign-toplevel-list (privileged through the filter), the native Sash window protocol (state event; activate, close), xdg-activation (tokens, validation, urgent) | headless: only privileged clients see the list; titles capped and never logged; open, close and focus events in order; activate and close act; a token from an old serial is refused (urgent); the leak test covers the protocol | minimize, workspaces |
| E6 | The dock: pinned apps (a fixed default list until the settings protocol), running windows matched to entries, the three running-states, click to launch (C4) or focus, the clock in the right zone | headless end to end: launch a test client, it appears running, focus moves, close removes it; the launched child gets no privilege, no inherited descriptors, the allow-listed environment; goldens; idle 0 frames except one per minute; the VM scenario | tray, notifications, volume, network |
| E7 | The launcher: an overlay layer surface (exclusive keyboard), pinned grid, recent (launched ids), all apps with search, the footer (Files and Downloads hidden until Files exists, Settings, power through systemctl with a confirmation for power off) | keyboard-only use of every part; Escape returns focus; search over Name and Keywords; goldens; the VM scenario launches foot | file recents, IME |
| Later | zbus: power through logind, network read-only, notifications daemon, tray; AccessKit adapter; Settings app; text-input-v3 | | |

Order rationale:
- The icon theme (E3) and .desktop parsing (E4) are pure data with no Wayland code, so they are tested in isolation before the dock needs them.
- The foreign-toplevel protocol (E5) comes before the dock (E6), so the dock is built against a tested server.
- The test panel lives in E1 and stays as the harness client for E2 to E7.
