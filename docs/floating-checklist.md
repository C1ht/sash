<!-- SPDX-License-Identifier: Apache-2.0 -->
# Floating checklist (manual)

A hands-on check of how floating feels. Run it in the nested (winit) backend.

## Setup

```sh
cargo build                     # debug build: mod+Return and mod+Ctrl+D exist
RUST_LOG=sash_wm=debug target/debug/sash-wm 2> /tmp/sash-wm.log
```

- **mod** is **Alt** when nested. Click the sash-wm window once so it has the host's keyboard focus.
- Open windows with **Alt+Return** (foot).
- For each item, mark **pass** or **fail** and write down what you saw.

## 1. Placement

- [ ] **First floating window.** Press Alt+Ctrl+D (default mode becomes floating), then Alt+Return. The window opens at foot's own size, centered on the output.
- [ ] **Cascade.** Alt+Return three more times. Each new window is 32 px further right and down than the previous one.
- [ ] **Wrap.** Keep opening windows. When the next one would leave the output, it goes back to the center.
- [ ] **New window in floating mode.** None of the tiled windows (if any) moves or resizes when it opens.
- [ ] **Dialog.** In an app with a dialog (for example a GTK app's file chooser), the dialog opens floating and centered over its parent, even in tiled mode. Alt+F does nothing on it, and Alt+Shift+T leaves it floating.
- [ ] **Fixed size.** A fixed-size window (min = max) opens floating and centered on the output.
- [ ] Press Alt+Ctrl+D again to go back to tiled mode.

## 2. Moving (drag smoothness)

- [ ] **Alt+left-drag a floating window.** It follows the pointer without lag you can see, with no jumps or flicker, and the cursor stays at the same spot on the window.
- [ ] **Fast drag in circles for 5 seconds.** Smooth? Afterwards, check `/tmp/sash-wm.log`: the `frame_time_us` and `input_to_frame_us` values should stay low.
- [ ] **Off the output.** Drag a window off each edge. At least 64 px of it (or all of it, if it is smaller) stays visible and can still be grabbed.
- [ ] **Alt+left-drag a tiled window.** It floats as soon as you press and then follows the pointer. Its sibling fills the gap.
- [ ] **Title bar.** Drag foot's own title bar (no Alt). Same result as the previous item, for tiled and floating windows.
- [ ] **Release.** After releasing the button, the window stays where you dropped it, and clicks go to clients again.

## 3. Resizing from each cell

Alt+right-drag a floating window starting in each cell of a 3x3 grid over it.

- [ ] Top-left: the top-left corner moves.
- [ ] Top: the top edge moves.
- [ ] Top-right: the top-right corner moves.
- [ ] Left: the left edge moves.
- [ ] Center: the bottom-right corner moves.
- [ ] Right: the right edge moves.
- [ ] Bottom-left: the bottom-left corner moves.
- [ ] Bottom: the bottom edge moves.
- [ ] Bottom-right: the bottom-right corner moves.
- [ ] **Opposite edges stay put.** In every case, the edges you are not dragging do not move or wobble, including while the client is still redrawing.
- [ ] **Alt+right-drag on a tiled window** does nothing.

## 4. Minimum and maximum size

- [ ] **Too small.** Resize a window with a minimum size (most GTK apps) smaller than its minimum. It stops at the minimum, and the opposite edge does not move.
- [ ] **Too large.** For a window with a maximum size, it stops at the maximum.
- [ ] **Keyboard resize.** Alt+Ctrl+Shift+H/J/K/L (narrower, taller, shorter, wider; top-left fixed) also stops at the limits.

## 5. Raise on click and floating above tiled

- [ ] **Raise on click.** With two overlapping floating windows, click the lower one. It comes on top and gets keyboard focus, and the click also reaches the app (for example, it places the text cursor).
- [ ] **Focus on click.** Click a tiled window. It gets keyboard focus.
- [ ] **Floating above tiled.** Floating windows are always drawn above tiled ones, even right after clicking a tiled window.
- [ ] **No clipping.** A floating window is never cut off at tile borders.
- [ ] **Tiled windows are clipped.** A tiled window never draws over its neighbours, even while it is still redrawing at a new size.

## 6. Tiled to floating and back

- [ ] **Float.** Alt+F on a tiled window. It floats exactly where its tile was, and nothing else jumps except its sibling filling the space.
- [ ] **Change it.** Move and resize it (Alt+Ctrl+H/J/K/L, Alt+Ctrl+Shift+H/J/K/L).
- [ ] **Back.** Alt+F returns it to the same leaf. The tiling looks exactly as before.
- [ ] **Float again.** Alt+F once more. It comes back where you last left it as a floating window.

## 7. Float-all and tile-all

- [ ] **Float all.** With 3–4 tiled windows, press Alt+Shift+F. Every window floats exactly at its tile, and nothing moves on screen.
- [ ] **Change things.** Move, resize, and close one of the floating windows. Open a new one.
- [ ] **Tile all.** Alt+Shift+T. The tiling is the same as before, with the edits applied as if all windows had stayed tiled. Forced-floating windows (dialogs, fixed size) stay floating.
- [ ] **With an output resize.** Resize the nested sash-wm window in the host while windows float, then Alt+Shift+T. Same structure, recomputed sizes.

## 8. A covered window reached by keyboard

- [ ] **Alt+Tab while holding Alt.** Move a floating window so it fully covers a tiled one. Hold Alt and press Tab repeatedly. Each press focuses and raises the next window in most-recently-used order, including the covered tiled one (it gets keyboard focus even though it is hidden).
- [ ] **Release.** Releasing Alt keeps the chosen window. A single Alt+Tab afterwards goes back to the window you started from.
- [ ] **Shift.** Alt+Shift+Tab steps backwards.
- [ ] **Directions.** Alt+H/J/K/L moves focus by direction, and floating windows are candidates.

## 9. Stale content during a floating resize

**Question:** during a floating resize, does a client ever show stale or empty content, and for how long?

**Watch for:**
- [ ] Old content stretched or offset.
- [ ] A frame where part of the new rectangle is empty (background showing).
- [ ] The window briefly drawn at its old size.

**Measure:**
- [ ] After the resize, find the `client caught up ... configure_to_commit_us=` lines in `/tmp/sash-wm.log`. Each one is the time from sending the new size to the client's first buffer at that size: the window shows old content for that long.
- [ ] Note p50 and the worst case for: foot, a GTK app, and a slow app.

**Numbers so far** (foot, 200 keyboard resize steps, release build):

| Measure | Value |
|---|---|
| p50 | 17.7 ms |
| p95 | 34.1 ms |
| max | 45.7 ms |
| Configures answered | 54 of 200 (the others were superseded by newer ones) |

That is roughly 1–3 frames of stale content per resize step.

**Your verdict:** is this acceptable, or should resize transactions (docs/plan.md) be revisited?

Notes:

