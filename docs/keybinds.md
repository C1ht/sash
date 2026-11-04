<!-- SPDX-License-Identifier: Apache-2.0 -->
<!-- Generated from sash-wm/src/keybinds.rs. Do not edit: run
     SASH_WRITE_KEYBINDS_DOC=1 cargo test keybinds_doc -->

# Keybinds

**mod** is Alt when sash-wm runs nested (the host keeps Super) and Super on a real session. Keys that trigger a binding are not passed to the focused app, and neither are mod+clicks.

| Chord | Modifier set | Action | Builds |
|---|---|---|---|
| mod+H | mod | Focus the window to the left | all |
| mod+J | mod | Focus the window below | all |
| mod+K | mod | Focus the window above | all |
| mod+L | mod | Focus the window to the right | all |
| mod+Tab | mod | Cycle windows, most recent first (hold mod; release to choose) | all |
| mod+Shift+Tab | mod+Shift | Cycle windows backwards (hold mod) | all |
| mod+Shift+H | mod+Shift | Swap the tiled window with its neighbour to the left | all |
| mod+Shift+J | mod+Shift | Swap the tiled window with its neighbour below | all |
| mod+Shift+K | mod+Shift | Swap the tiled window with its neighbour above | all |
| mod+Shift+L | mod+Shift | Swap the tiled window with its neighbour to the right | all |
| mod+F | mod | Toggle floating for the focused window | all |
| mod+Shift+F | mod+Shift | Float all windows on the monitor | all |
| mod+Shift+T | mod+Shift | Tile all windows on the monitor | all |
| mod+Ctrl+H | mod+Ctrl | Move the window 40 px left (floats a tiled window) | all |
| mod+Ctrl+J | mod+Ctrl | Move the window 40 px down (floats a tiled window) | all |
| mod+Ctrl+K | mod+Ctrl | Move the window 40 px up (floats a tiled window) | all |
| mod+Ctrl+L | mod+Ctrl | Move the window 40 px right (floats a tiled window) | all |
| mod+Ctrl+Shift+H | mod+Ctrl+Shift | Make the window 40 px narrower (tiled: moves the split at its right edge) | all |
| mod+Ctrl+Shift+J | mod+Ctrl+Shift | Make the window 40 px taller (tiled: moves the split at its bottom edge) | all |
| mod+Ctrl+Shift+K | mod+Ctrl+Shift | Make the window 40 px shorter (tiled: moves the split at its bottom edge) | all |
| mod+Ctrl+Shift+L | mod+Ctrl+Shift | Make the window 40 px wider (tiled: moves the split at its right edge) | all |
| mod+Left | mod | Snap: left half, then bottom-left (see the snap table) | all |
| mod+Right | mod | Snap: right half, then bottom-right | all |
| mod+Up | mod | Snap: maximize, or the top quarter of a half | all |
| mod+Down | mod | Snap: quarter to half, then restore | all |
| mod+Y | mod | Snap to the top-left quarter (a tiled window floats first) | all |
| mod+U | mod | Snap to the top-right quarter | all |
| mod+B | mod | Snap to the bottom-left quarter | all |
| mod+N | mod | Snap to the bottom-right quarter | all |
| mod+Q | mod | Close the focused window | all |
| mod+Shift+E | mod+Shift | Quit sash-wm | all |
| mod+Return | mod | Start foot | debug only |
| mod+Ctrl+D | mod+Ctrl | Toggle the monitor's default mode | debug only |
| mod+left drag | mod | Move the window; a tiled window floats; hold Shift to turn edge snapping off (keyboard: mod+Ctrl+H/J/K/L, mod+arrows) | all |
| mod+right drag | mod | Resize from the pointer's cell of a 3x3 grid; tiled: moves splits (keyboard: mod+Ctrl+Shift+H/J/K/L) | all |
| click | no mod | Focus and raise the window; the click is delivered (keyboard: mod+H/J/K/L, mod+Tab) | all |

## Window decorations

The bar and border sash-wm draws (M11b) are used with the plain mouse. They add no actions: every one has a keybind above.

| Mouse | Action | Keybinds |
|---|---|---|
| click on the close button | Close the window | mod+Q |
| click on the maximize button | Maximize; on a maximized window, restore | mod+Up, mod+Down |
| left drag on the bar | Move the window, with edge snapping and snap zones (as mod+left drag) | mod+Ctrl+H, mod+Ctrl+J, mod+Ctrl+K, mod+Ctrl+L, mod+left drag |
| left drag on a resize zone (edges and corners) | Resize from that edge or corner; tiled: moves the split (as mod+right drag) | mod+Ctrl+Shift+H, mod+Ctrl+Shift+J, mod+Ctrl+Shift+K, mod+Ctrl+Shift+L, mod+right drag |
| any press on the bar or border | Focus and raise the window (never delivered to it) | mod+H, mod+J, mod+K, mod+L, mod+Tab |
