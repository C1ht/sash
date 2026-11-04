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
- [ ] **Alt+right-drag on a tiled window** moves its splits instead (section 11).

## 4. Minimum and maximum size

- [ ] **Too small.** Resize a window with a minimum size (most GTK apps) smaller than its minimum. It stops at the minimum, and the opposite edge does not move.
- [ ] **Too large.** For a window with a maximum size, it stops at the maximum.
- [ ] **Keyboard resize.** Alt+Ctrl+Shift+H/J/K/L on a floating window (narrower, taller, shorter, wider; top-left fixed) also stops at the limits.

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

## 10. Snapping and fullscreen (M5)

Setup as above (debug build, mod = Alt). The zones are: the top edge (whole output, maximized), the left and right edges (halves), and the four corners (quarters). There is no bottom zone.

### Preview

- [ ] **Shows.** Alt+left-drag a floating window and move the pointer to the left edge. A blue rectangle appears over the left half: 30% fill and a 2 px border, sharp corners, no animation.
- [ ] **Placement.** The preview is drawn above all other windows but below the window you drag (the dragged window is not tinted).
- [ ] **Hides.** It disappears when the pointer moves out of the zone, and when the drag ends.

### Snapping by mouse: each of the seven zones

- [ ] **Top edge.** The window fills the whole output and is told it is maximized; foot's title bar shows the "restore" button.
- [ ] **Left edge.** Left half.
- [ ] **Right edge.** Right half.
- [ ] **Top-left corner.** Top-left quarter.
- [ ] **Top-right corner.** Top-right quarter.
- [ ] **Bottom-left corner.** Bottom-left quarter.
- [ ] **Bottom-right corner.** Bottom-right quarter.
- [ ] **Exact fit.** In each case the zone fits the output exactly, with no gap and no overlap with the other half or quarters.
- [ ] **Corners win.** Within 64 px of two edges you get the corner, not the edge.
- [ ] **Tiled window.** Alt+left-drag a tiled window into a zone. It floats and snaps, and its sibling fills its old tile.

### Restore on drag-out

- [ ] **Size.** Drag a snapped window away. It immediately takes its size from before snapping.
- [ ] **Grab point.** The spot you grabbed stays under the pointer.
- [ ] **Click without motion.** Alt+click on a snapped window without moving leaves it snapped.

### Edge snapping and the Shift override

- [ ] **Edges.** Drag a floating window near an output edge or near another window's edge. Within about 12 px it jumps to touch it.
- [ ] **Overlap only.** Only windows that overlap on the other axis attract it.
- [ ] **Resize.** The same happens for the dragged edges when resizing with Alt+right-drag.
- [ ] **Shift.** Hold Shift while dragging: there is no edge snapping (the zones still work).
- [ ] **Keyboard.** Keyboard moves (Alt+Ctrl+H/J/K/L) do not edge-snap.

### Keybind transitions (Alt + arrow keys)

Start from an unsnapped floating window. Check every cell of the table in docs/plan.md:

- [ ] **Unsnapped.**
  - Left → left half
  - Right → right half
  - Up → whole output (maximized)
  - Down → nothing
- [ ] **Left half.**
  - Left → bottom-left
  - Right → right half
  - Up → top-left
  - Down → restore
- [ ] **Right half.**
  - Left → left half
  - Right → bottom-right
  - Up → top-right
  - Down → restore
- [ ] **Top-left.**
  - Left → nothing
  - Right → top-right
  - Up → whole output
  - Down → left half
- [ ] **Top-right.**
  - Left → top-left
  - Right → nothing
  - Up → whole output
  - Down → right half
- [ ] **Bottom-left.**
  - Left → nothing
  - Right → bottom-right
  - Up → top-left
  - Down → left half
- [ ] **Bottom-right.**
  - Left → bottom-left
  - Right → nothing
  - Up → top-right
  - Down → right half
- [ ] **Whole output.**
  - Left → left half
  - Right → right half
  - Up → nothing
  - Down → restore
- [ ] **Repeated Down.** Repeated Alt+Down always ends at the pre-snap rectangle.
- [ ] **Tiled window, Up.** It covers the output but keeps its tile (the other tiles do not move). Alt+Down returns it to its tile.
- [ ] **Tiled window, Left/Right.** It floats and snaps to the half.
- [ ] **Manual edits.** Keyboard move or resize of a snapped window unsnaps it and continues from where it is.
- [ ] **Keys stay with sash-wm.** None of these keys reach the app.

### Fullscreen from a client

- [ ] **Open.** `foot --fullscreen` (or a video player's fullscreen button) covers the whole output above everything, without a title bar.
- [ ] **Leave.** Leaving fullscreen (the app's own control) returns the window to exactly where it was. A tiled one goes back to its tile, and the other tiles never moved.
- [ ] **Replace.** A second window going fullscreen replaces the first, which returns to its previous state.
- [ ] **Focus ends it.** Alt+H/J/K/L to another window, or a click when part of another window is visible, ends fullscreen.
- [ ] **Alt+Tab.** Alt+Tab past it and back without releasing Alt does not end it; releasing on another window does.
- [ ] **Close.** Closing a fullscreen window brings the layout back as it was.
- [ ] **Fixed size.** A fixed-size window that asks for fullscreen sits centered over a black backdrop, if you have such an app.
- [ ] **Output resize.** Resize the nested window while something is fullscreen. The fullscreen window follows the new size.

### Tile-all round trip

- [ ] **Snap.** Snap a window to a quarter.
- [ ] **Tile all.** Alt+Shift+T: it returns to its tile.
- [ ] **Resize the output.** Resize the nested sash-wm window.
- [ ] **Float all.** Alt+Shift+F: the window goes back into the same quarter, recomputed for the new output size.
- [ ] **Restore.** Alt+Down twice: half, then its rectangle from before it was snapped.

Notes:

## 11. Tiled resize (M6)

Setup as above, default mode tiled. Open three windows: 1 | (2 / 3). All keybinds are in docs/keybinds.md.

### Keyboard

- [ ] **Wider.** Focus window 1 (Alt+H) and press Alt+Ctrl+Shift+L. The split between 1 and the right column moves 40 px right. Window 1 stays tiled; 2 and 3 shrink together.
- [ ] **Narrower.** Alt+Ctrl+Shift+H moves it back 40 px.
- [ ] **Taller and shorter.** Focus window 2 and press Alt+Ctrl+Shift+J, then K. The split between 2 and 3 moves 40 px down, then up. Window 1 does not move.
- [ ] **Output boundary.** Focus window 3. None of Alt+Ctrl+Shift+H/J/K/L changes anything: its right and bottom edges are the output's.
- [ ] **Minimum.** Hold Alt+Ctrl+Shift+L on window 1. It stops when the right column is 64 px wide; window 1 never covers it.
- [ ] **Keys stay with sash-wm.** None of these keys reach the app.

### Mouse (Alt+right-drag, 3x3 grid)

- [ ] **Right cell of window 1.** Moves the vertical split; the windows follow the pointer with no lag you can see.
- [ ] **Left cell of window 3.** Moves the same vertical split (its left edge).
- [ ] **Top cell of window 3.** Moves the split between 2 and 3.
- [ ] **Corner.** Top-left cell of window 3 moves both splits at once.
- [ ] **Center cell of window 2.** Moves its bottom split only (its right edge is the output's).
- [ ] **Boundary.** Top-left cell of window 1 (both edges on the output boundary): nothing happens, and the click does not reach the app.
- [ ] **Minimum.** Drag far past a neighbour. Each tile stops at 64 px.
- [ ] **Clipping.** While dragging fast, no window draws over its neighbour, and the stale content of a window is cut off at its new tile.
- [ ] **Release.** After releasing, the split stays where you dropped it, and clicks go to clients again.

### Ratios are kept

- [ ] **Toggle.** After a resize, Alt+F twice on any window: the tiling looks exactly as after the resize.
- [ ] **Float all / tile all.** Alt+Shift+F, then Alt+Shift+T: same.
- [ ] **Output resize.** Resize the nested sash-wm window. The splits stay at the same fraction of the output (a 520 px split on an 800 px output becomes 650 px at 1000 px).

### Edge cases

- [ ] **Close during a resize.** Start an Alt+right-drag on window 2's bottom split and close window 3 from a terminal (`kill`). Nothing crashes; window 2 fills the column and further motion does nothing.
- [ ] **Hidden output.** Start a tiled resize, then switch the host to another workspace while still dragging (if your host allows it). Back on the workspace, the windows have the size of the last pointer position.
- [ ] **Client resize.** Dragging a tiled window's own border (apps with client-side borders) does nothing; tiled resize is Alt+right-drag only.

Notes:

## 12. Popups (M7)

Setup as above (debug build, mod = Alt). Use an app with menus, for example `pavucontrol` (its "Show:" dropdown at the bottom and its tooltips) or a GTK or Qt app with a menu bar. Start it with `WAYLAND_DISPLAY=<socket> pavucontrol`.

### Menus

- [ ] **Open and close.** Click a dropdown or menu button. The menu appears next to it, with no flicker, and takes the keyboard (arrow keys move the highlight). Choosing an item closes it.
- [ ] **Nested submenu.** In an app with submenus, hover an entry with an arrow. The submenu opens beside it. Both stay open while you move between them.
- [ ] **Click outside.** With a menu open, click elsewhere (on the same window or another one). The whole menu chain closes, innermost first, and that click does nothing else: no button is pressed, no window is focused.
- [ ] **Escape.** Escape closes the menu (the app does this) and the keyboard goes back to the window.
- [ ] **Tooltip.** Hover a control with a tooltip. The tooltip appears and never takes the keyboard: typing still goes to the window.

### Placement near each output edge

- [ ] **Bottom.** A dropdown near the bottom edge opens above its button (flipped) or slides up; it is never cut off by the output edge.
- [ ] **Right.** A menu near the right edge slides or flips to the left.
- [ ] **Left and top.** Same near the left and top edges.

### Popups from each kind of window

- [ ] **Tiled.** A menu from a tiled window can extend over the neighbouring tile; it is drawn on top and can be clicked there.
- [ ] **Floating.** A menu from a floating window can extend beyond the window.
- [ ] **Snapped.** Same for a snapped (half or quarter) window.
- [ ] **Fullscreen.** A menu from a fullscreen window is drawn above it.
- [ ] **Window being dragged.** A tooltip shown while you move a window with its own title bar moves with the window.

### Keybinds and popups

- [ ] **Keybinds close menus.** With a menu open, Alt+H/J/K/L, Alt+Tab, Alt+F, Alt+arrow, Alt+Ctrl+H/J/K/L and Alt+Q each close the menu first, then do their action.
- [ ] **Resize keeps them.** With a menu open, Alt+Ctrl+Shift+H/J/K/L or Alt+right-drag resizes the window and the menu stays open and follows.
- [ ] **Alt+left-drag** on the window closes the menu and moves the window.
- [ ] **Output resize.** Resize the nested sash-wm window with a menu open: most menus (non-reactive) close; menus the app marks reactive stay and are moved onto the output.

Notes:

## 13. GPU buffers (linux-dmabuf) and cursors (M8)

Setup as above (debug build, mod = Alt). sash-wm's log should show `dmabuf self-test passed` at start. Start GL apps with a throwaway config, for example `XDG_CONFIG_HOME=$(mktemp -d) WAYLAND_DISPLAY=<socket> kitty --config NONE`; for Firefox use a new empty profile (`firefox --new-instance --no-remote --profile $(mktemp -d)`).

### GPU rendering

- [ ] **kitty.** Opens without `MESA-EGL` warnings in its output (it renders on the NVIDIA GPU, not in software). Typing and scrolling look normal.
- [ ] **Firefox.** A page with animation (CSS or video) plays smoothly, at the same speed as on the host.
- [ ] **pavucontrol** and other GTK 4 apps look the same as before; menus open normally.
- [ ] **foot** still works (it never uses the GPU; nothing changes for it).

### Flicker and stale frames (no explicit sync yet)

Watch closely for each GPU app above:

- [ ] **No flicker:** no frame shows old or half-drawn content while typing, scrolling or animating.
- [ ] **No tearing or garbage** when the window is resized (Alt+right-drag, Alt+Ctrl+Shift+H/J/K/L) or when it is tiled next to others.
- [ ] **Popups and tiles:** a GPU app's menus, tiled clipping, floating, snapping and the snap preview behave exactly as with foot.

If you see flicker or stale frames, note which app and what you did: that is the case explicit sync (wp_linux_drm_syncobj_v1) would fix.

### Cursors

- [ ] **Shapes.** Over foot's text the cursor becomes the text cursor (I-beam); over pavucontrol's buttons it is the arrow; resize borders of a client (if it draws them) show resize cursors.
- [ ] **Reset.** Moving from foot to another window or to empty space gives the arrow back at once.
- [ ] **Hidden.** An app that hides the cursor (a video player in fullscreen, or kitty while typing if configured) hides it over sash-wm.
- [ ] **Cursor surfaces.** An older app that sets its own cursor image (not a shape) shows that image at the pointer, with its hot spot where you click, and moves smoothly with the pointer.

Notes:

## 14. Quarter chords (M8.1)

Setup as above (debug build, mod = Alt). Open two windows (foot).

- [ ] **Each quarter.** On the focused window: Alt+Y top-left, Alt+U top-right, Alt+B bottom-left, Alt+N bottom-right. The window fills exactly that quarter of the output, with no gap at the edges or the middle.
- [ ] **From tiled.** On a tiled window, a chord floats it and snaps it; its neighbour fills the freed space.
- [ ] **From a half or the whole output.** Alt+Left, then Alt+U: the window goes straight to the top-right quarter. Same from Alt+Up (whole output).
- [ ] **Same quarter.** Pressing the chord of the quarter the window is already in changes nothing (no flicker).
- [ ] **Restore.** Alt+Down twice goes back through the half to the rectangle the window had before it was first snapped (for a tiled window: its tile size and position, floating).
- [ ] **Arrows unchanged.** The mod+arrow transitions still behave as in section 10.
- [ ] **Fullscreen.** A fullscreen window (`foot --fullscreen`) ignores all four chords.
- [ ] **Keys stay with sash-wm.** None of Y, U, B, N reaches the app when pressed with Alt.

Notes:

## 15. Clipboard and drag and drop (M10)

Setup as above (debug build, mod = Alt, nested). Use throwaway profiles only and keep
anything private off the screen: `foot`, `pavucontrol` with `XDG_CONFIG_HOME` set to a
fresh temporary directory, and Firefox with `--profile` on a new empty directory and a
local test page (no network). Rules: CLAUDE.md, "Clipboard and drag and drop (M10)".

### Clipboard

- [ ] **Text, foot to foot.** Select text in one foot, Ctrl+Shift+C; focus another foot, Ctrl+Shift+V: the text is pasted.
- [ ] **Text, foot to Firefox and back.** Copy in foot, paste into a text field on the test page (Ctrl+V); copy in the page (Ctrl+C), paste into foot.
- [ ] **Image.** Right-click an image on the test page, "Copy Image"; paste into the page's editable area (or another app that takes images): the image arrives.
- [ ] **Only the focused app reads it.** Copy in foot; the clipboard is not offered to an app until it has keyboard focus (paste works right after focusing it, never in a window you did not focus).
- [ ] **The copying app closes.** Copy in foot, close that foot (Alt+Q): pasting elsewhere gives nothing (the selection ended with its app), and nothing hangs.
- [ ] **Large copy.** Copy a big block (for example the output of `seq 1000000` selected in foot): pasting does not freeze sash-wm (the pointer keeps moving smoothly while it transfers).
- [ ] **No middle-click paste.** Selecting text and middle-clicking elsewhere pastes nothing (no primary selection, a privacy decision).
- [ ] **Nested limit.** Copying in Hyprland and pasting into a window inside sash-wm does nothing, and the other way round (no host clipboard bridge).

### Drag and drop

- [ ] **A file name.** Drag a file from a file list (or a link from the test page) into foot or a text field: its name or address is inserted.
- [ ] **Text between apps.** Select text on the test page and drag it into foot or another text field.
- [ ] **The icon.** While dragging, the drag image (if the app provides one) follows the pointer, above every window, popup and the snap preview; it never blocks the window under it from receiving the drop.
- [ ] **Escape.** Start a drag, press Escape: the drag ends, nothing is dropped, Escape does not reach the app.
- [ ] **A focus keybind.** Start a drag, press Alt+H (or Alt+Tab): the drag ends and focus moves.
- [ ] **Drop on nothing.** Drop on an empty part of the output (with floating windows) or on a window that does not take the data: nothing happens, both apps keep working.
- [ ] **Edge.** Drag to the very edge of the output over a window and drop: the last pixel is the window's 1 px border (M11b), so the drop is cancelled; one pixel in, it goes to that window.
- [ ] **Popups.** Drag over an open menu of another app: the menu gets the drag (only if it accepts drops; otherwise the drop is cancelled).
- [ ] **Alt+drag is separate.** Alt+drag still moves windows and shows the snap preview as before; a drag and drop cannot start during it, and Alt+drag does nothing during a drag and drop.
- [ ] **An app closes mid-drag.** Start a drag from foot to Firefox, close the source (from another terminal: `kill <pid of that foot>`) or the target: the drag ends, the pointer works again at once.

Notes:

## 16. Decorations (M11b)

Setup as above (debug build, mod = Alt, nested), with foot (it negotiates
decorations) and an app that draws its own title bar and does not negotiate (for
example pavucontrol with `XDG_CONFIG_HOME` set to a fresh temporary directory; no
screenshots of it). Rules: CLAUDE.md, "Decorations (M11b, owner)".

- [ ] **Tiled.** Two foot windows side by side: each has a 1 px border (blue on the focused one, grey on the other) and no bar.
- [ ] **Floating.** Alt+F on one: a 32 px dark bar appears with close (x) and maximize (square) on the left; the window keeps its outer size.
- [ ] **Focus without colour.** Click the other window: the first one's border turns grey and its glyphs dim; both change together.
- [ ] **Hover.** Over close: a red square appears behind the x; over maximize: a grey square behind the square; leaving removes them.
- [ ] **Close.** Press close, move off it and release: nothing. Click it: the window closes (as Alt+Q).
- [ ] **Maximize.** Click maximize: the window fills the output with its bar; the glyph becomes two squares. Click again: back to where it was. From a half (Alt+Left), maximize then restore returns to the rectangle before the snap.
- [ ] **Bar drag.** Drag the bar: the window moves; to the left edge it shows the preview and snaps on release, as Alt+drag does. A tiny move of the pointer while pressing does not jump the window.
- [ ] **Resize zones.** Near each edge and corner (7 px from an edge, 16 px squares at corners) the cursor shows the matching resize arrow; dragging resizes from there and stops at the window's min and max size. Next to the close button the button wins.
- [ ] **Tiled zones.** On the shared edge of two tiles the cursor shows a resize arrow and dragging moves the split; on the output's edges there is none.
- [ ] **Nothing reaches the app.** Clicks and scrolling on the bar, border and zones do nothing inside foot (no selection starts, no scroll).
- [ ] **Fullscreen.** A fullscreen window (foot: F11 if bound, or a video player) has no bar and no border.
- [ ] **An app that draws its own title bar.** It keeps it; sash-wm adds only the 1 px border, no bar.
- [ ] **Menus over the bar.** A menu or tooltip that opens over a window's bar is on top and takes the pointer there.
