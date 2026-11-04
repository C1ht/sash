<!-- SPDX-License-Identifier: Apache-2.0 -->

# M12 investigation: layer-shell, trust, text stack

Investigation only (owner, after M11b): no code. Facts about Smithay come from the
0.7.0 sources in the cargo registry (paths below are inside `smithay-0.7.0/src`).
Sizes and times marked *estimate* are not measured; measuring them needs crates that
are not approved yet (section C).

## A. wlr-layer-shell

### What Smithay 0.7 gives, and what sash-wm builds

| Piece | Smithay 0.7 | sash-wm must build |
|---|---|---|
| Protocol objects, roles, cached state (layer, anchor, size, margins, exclusive zone, keyboard interactivity, namespace) | `wayland/shell/wlr_layer`: global **version 4** (`create_global(4, …)`); the protocol XML in wayland-protocols-wlr 0.3.12 is version 5 | nothing |
| Protocol errors (invalid layer, role, invalid size, invalid surface state, invalid anchor) | posted by `wlr_layer/handlers.rs` | nothing |
| `set_exclusive_edge` (v5: which edge a corner-anchored surface reserves) | not supported (v4 global) | wait, or patch; v4 is enough for a dock and a panel |
| Popups of layer surfaces (`get_popup`) | `new_popup` hook; the xdg popup machinery we already use | placement against the output (not the usable area), dismissal, grabs: our M7 code with a layer parent |
| Placement and exclusive zones | `desktop::LayerMap::arrange` (`desktop/wayland/layer.rs`) | **our own arrangement** (below) |
| Keyboard interactivity (none, exclusive, on_demand) | stored only | focus rules (below) |
| Output assignment | stored (`wl_output` or null) | one output today: null means ours |
| Namespace | stored string | logging only; never used for trust (any client can claim any name) |
| Hit-testing, ordering, rendering | `LayerMap::layer_under` | our stacking (below) |
| Global visibility per client | `WlrLayerShellState::new_with_filter` (a per-client filter) | the trust model (section B) |

Why not Smithay's `LayerMap::arrange`, read against the protocol text:

| Case | Protocol (v4/v5) | `LayerMap::arrange` |
|---|---|---|
| Exclusive zone on a surface anchored to a corner, to two parallel edges or to all four | treated as 0 | corner: reserves the left/top edge; two parallel edges (top+bottom): shrinks the width; all four: usable area becomes 0x0 |
| Area it lays out in | the output | the output's *mode* size, not sash-wm's layout area |
| Clamping | sizes never negative | `zone.size -= amount` without a clamp: a large zone gives a negative size |
| Order of reservations | compositor's choice | mapping order |

All four are small (one function), and the rules are ours to state, so sash-wm writes
its own pure arrangement module (no Smithay types, like `layout/`), tested with
property tests. Smithay's types and handlers are still used.

### Usable area and the layout invariants

The layout gets two rectangles: the **output** and the **usable area** (the output
minus exclusive zones). Proposal:

| Rule today | With layer-shell (proposal) |
|---|---|
| Tiles cover the output exactly | tiles cover the **usable area** exactly (same floor/remainder rule) |
| Snap zones are fractions of the output | fractions of the **usable area**; the triggers (8 px bands, 64 px corners) stay on the **true output edge**, so a drag into a dock still snaps |
| Maximize fills the output | fills the **usable area** (CLAUDE.md already says so for when layer-shell exists) |
| Fullscreen fills the output | unchanged: the whole output, over Top-layer panels |
| 64 px visibility rule | against the **usable area** (a bar under a panel can't be grabbed) |
| Top-edge rule ("usable area") | the usable area's top, as already written |
| No border on output-boundary edges | "boundary" becomes the **usable area's** boundary; an edge against a panel then has no border either. Alternative: keep a border there (it isn't a screen edge, so the edge-pixel argument doesn't hold). **Owner decision** |
| An output resize re-lays out | an exclusive-zone change goes through the same path (tiles recomputed, floating windows clamped, snapped ones recomputed from their zone) |
| Tiny output rule | applies when the usable area is tiny |
| New invariant | the usable area is never negative and lies inside the output; layer surfaces are never in the BSP tree; total reservations per axis are capped by a constant (proposal: half the output), so one client can't squeeze the windows to nothing |

### Stacking, bottom to top (proposal)

| # | What | Note |
|---|---|---|
| 1 | Background layer | wallpaper |
| 2 | Bottom layer | |
| 3 | tiled windows, then popups of tiled windows | as today |
| 4 | maximized, floating windows (each with its popups and decorations) | as today |
| 5 | snap preview | below the dragged window, as today |
| 6 | Top layer (panels, dock) | under a fullscreen window |
| 7 | fullscreen window (+ backdrop, popups) | |
| 8 | Overlay layer | above fullscreen (OSDs; session lock later replaces it) |
| 9 | popups of layer surfaces | directly above their parent layer |
| 10 | drag icon | above everything, as today |
| 11 | cursor | |

### Keyboard focus (proposal)

| Interactivity | Focus |
|---|---|
| none | never |
| on_demand | on a click into it; lost when a window is clicked or focused by a key |
| exclusive (Top, Overlay) | takes the keyboard while mapped; the topmost exclusive surface wins; windows get it back on unmap |
| exclusive (Bottom, Background) | treated as on_demand (the protocol allows it; a background can't hold the keyboard) |
| sash-wm's keybinds | still run first, as today; mod+Shift+E and the VT keys always work. Only a future session lock suspends keybinds. **Owner decision** |
| Clipboard | follows keyboard focus as today; a layer surface with focus can read the selection, so only trusted layer clients should get focus (section B) |

## B. Trust and privacy

Threat: any client can bind layer-shell, cover the screen with an Overlay surface and
take exclusive keyboard focus: a fake lock screen or password prompt. The same goes
for screen capture, virtual keyboards and input injection.

### How a client can be identified

| Method | How | Pros | Cons |
|---|---|---|---|
| **Spawned by sash-wm** (recommended) | sash-wm creates a socket pair, inserts its end as a client marked *trusted*, starts a fixed first-party binary from a fixed path with the other end as `WAYLAND_SOCKET` | the connection itself is the proof: no PID races, no name matching; works for every protocol | only programs sash-wm starts; a third-party panel cannot be trusted without a later rule |
| Peer credentials (SO_PEERCRED: pid, uid, gid; `Client::get_credentials` in wayland-server 0.31) | look up `/proc/<pid>/exe` against an allowlist | no change for the clients | uid is the same for every app of the user; pid reuse and exec races; a same-user process can start an allowed binary and inject into it |
| Executable allowlists (KWin `X-KDE-Wayland-Interfaces` in .desktop files; Hyprland `permission` rules by binary path) | path → allowed protocols | works for third-party tools | relies on the exe path; KWin's own review notes a user-writable .desktop could override a system one |
| Ask the user (Hyprland "ask" mode) | a prompt on first use | flexible | needs compositor-drawn prompts (text stack); prompt fatigue; a new setting-like state |
| wp_security_context_v1 (Smithay `security_context`) | sandboxes (Flatpak) tag the connections they create | standard; cheap with Smithay | only marks sandboxed clients; unsandboxed apps are untagged, so it can only *restrict*, not *trust* |
| A separate "privileged" socket file | a second socket | simple | any same-user process can connect to it |

### Per protocol (recommendation in bold)

| Protocol | Risk | Untrusted clients | Trusted (spawned) |
|---|---|---|---|
| layer-shell | fake lock screen, covering the screen, keyboard capture | **the global is hidden** (`new_with_filter`) | all layers and modes |
| ext-session-lock-v1 (Smithay `session_lock`, v1) | a fake lock, or a lock that never unlocks | **hidden** | only the sash-wm-started locker; if it dies, the screen stays locked with a compositor-drawn notice and the locker is restarted |
| screen capture (ext-image-copy-capture / wlr-screencopy; **not in Smithay 0.7**) | reading everything on screen | **none** | only a first-party capture tool, later with a compositor-drawn indicator |
| idle-inhibit (Smithay, v1) | keeps the screen on | **allowed**, but only while the inhibiting surface is visible (the protocol's rule) | same |
| virtual keyboard (Smithay, v1), input method v2 | typing into any window | **hidden** | only a first-party on-screen keyboard / IME |
| input injection (any) | full control | **none** (sash-wm's synthetic input stays debug-only) | none |
| data-control | reads the clipboard | none (owner, M10) | none |

Recommendation: **spawned-by-sash-wm only, with the globals hidden from everyone
else**. No setting. Honest limit: a program running as the same user can still read
the user's files, ptrace other processes and use D-Bus; the trust model stops Wayland
spoofing and keyboard capture, not a malicious local program (niri's security notes
make the same point). Sandboxed apps get more protection once security-context is
used to restrict them further.

## C. The shared text stack (sash-ui)

### Candidates (crates.io, checked 2026-11-04)

| Crate | Version | Licence | Role |
|---|---|---|---|
| cosmic-text | 0.19.0 (2026-04-22) | MIT OR Apache-2.0 | layout, line breaking, bidi, font fallback, shaping |
| harfrust | ^0.5 (dep) | MIT | shaping (a Rust HarfBuzz port) |
| skrifa / read-fonts | ^0.40 (dep) | MIT OR Apache-2.0 | font parsing, outlines, hinting |
| fontdb | ^0.23 (dep) | MIT | font discovery |
| swash | ^0.2 (optional) | Apache-2.0 OR MIT | CPU rasterisation to alpha masks |
| unicode-bidi, -linebreak, -script, -segmentation | dep | MIT/Apache, Apache-2.0 | |
| self_cell | dep | Apache-2.0 OR GPL-2.0-only | choose Apache |

All licences are in deny.toml's allow list today; cargo deny confirms it in the first
step. About 20 new crates in total: all need the owner's approval.

### Proposal

| Topic | Proposal | Known limits |
|---|---|---|
| Fonts | fixed directories only (`/usr/share/fonts`), no fontconfig, no `~/.fonts`; default DejaVu Sans; Debian Depends: fonts-dejavu-core 2.37-8 (840 KB). Noto (fonts-noto-core, 12 MB) only as a fallback set if installed | no user fonts; no setting |
| Shaping and fallback | cosmic-text | |
| Rasterisation | CPU (swash) into alpha masks, packed in a glyph atlas, uploaded once; the GPU only composites, like the cursor and the bar glyphs now | no GPU text rendering |
| Hinting | light/slight from skrifa or swash at integer sizes | |
| Scaling | integer output scale only; fractional scale comes with viewporter/fractional-scale (roadmap) | |
| Bidi | cosmic-text handles it for display | right-to-left UI layout not designed |
| IME | none | text-input-v3 / input-method later |
| Colour emoji | not in the first step | |

### sash-wm without becoming its own client

sash-ui gets a renderer-agnostic **text module** behind a cargo feature (`text`):
fonts in, shaped lines and glyph alpha masks out. No Wayland client code in it.
sash-wm depends on `sash-ui` with `default-features = false, features = ["text"]` and
draws the masks as memory textures, as it draws its glyphs today. sash-ui's client
side (buffers, layer surfaces, input) lives behind another feature that sash-wm never
enables. No new crate is needed. The alternative, a separate `sash-text` crate, would
make the boundary explicit but needs the owner's approval.

### Cost

| Item | Value |
|---|---|
| sash-wm release binary today | 11.9 MB (measured) |
| added by the text stack | *estimate* 2 to 4 MB (shaping and font parsing tables) |
| per frame | one more texture per bar (the title), cached until the title changes; for scale, all bar pieces of 10 windows cost about 180 µs today (M11b full frame p50: 557 µs with bars, 377 µs without) |
| per title change | *estimate* tens of µs to shape and rasterise a short line |
| startup | *estimate* a few ms to scan the font directories; lazy until the first title |

Step 1 of the text milestone measures these in a throwaway crate after approval.

## D. Milestone plan

| Milestone | Contents | Deliberately not included |
|---|---|---|
| M13a Trust and spawning | sash-wm-spawned clients through `WAYLAND_SOCKET`, a trusted flag per client, global filters; first-party binaries from a fixed path; tests that untrusted clients cannot see the filtered globals; recorder events | permission prompts, third-party trust |
| M13b Layer-shell | v4 global for trusted clients; our arrangement; usable area into the layout (the invariant changes above); stacking; keyboard modes; layer popups; property and headless tests with a trusted test client; VM check | exclusive_edge (v5), multiple outputs |
| M14 Text stack | sash-ui `text` feature: font loading, shaping, atlas, masks; golden-mask tests; measured size and frame time; cargo deny | IME, colour emoji, fractional scale, user fonts |
| M15 Window titles | the xdg title in the bar after the buttons, ellipsised, cached per window; never recorded by the debug recorder, no titles in outline screenshots | title styling, centered titles, a menu |
| M16 Dock and launcher | first sash-ui clients (layer-shell, Top layer): a dock with running windows (needs a small native Sash protocol to list and activate windows) and a launcher (Overlay, exclusive keyboard while open, apps from /usr/share/applications, typing filters) | notifications, tray, workspaces, minimize (proposal), lock screen, screenshots, settings app, animations, blur |

## Sources

- Smithay 0.7.0: `wayland/shell/wlr_layer/{mod,handlers,types}.rs`, `desktop/wayland/layer.rs`, `wayland/{session_lock,security_context,idle_inhibit,virtual_keyboard,input_method}`.
- wlr-layer-shell XML: wayland-protocols-wlr 0.3.12, `wlr-protocols/unstable/wlr-layer-shell-unstable-v1.xml` (interfaces at version 5).
- wayland-server 0.31.14, `Client::get_credentials` (`src/client.rs`).
- [Hyprland permissions](https://wiki.hypr.land/Configuring/Advanced-and-Cool/Permissions/)
- [KWin: Allow blacklisting some wayland interfaces](https://commits.kde.org/kwin/f324776137342814de98c0c466f43169dbbe0f7e), [review discussion](https://mail.kde.org/pipermail/kwin/2019-July/000640.html), [serviceutils.h](https://lxr.kde.org/source/plasma/kwin/src/utils/serviceutils.h)
- [Weston issue 206: authenticating clients](https://gitlab.freedesktop.org/wayland/weston/-/issues/206)
- [niri security model](https://niri-wm.github.io/niri/Security-Model.html)
- [RFC: Wayland Security Modules (wayland-devel, 2025)](https://lists.freedesktop.org/archives/wayland-devel/2025-May/044266.html)
- crates.io API: cosmic-text 0.19.0 and its dependencies, harfrust, skrifa, read-fonts, fontdb, swash, unicode-linebreak, self_cell (2026-11-04).
- Debian trixie (container, apt-cache): fonts-dejavu-core 2.37-8, fonts-noto-core 20201225-2, fontconfig 2.15.0-2.3.
