# M19 investigation: focus follows the cursor, placement at the pointer

Investigation only (owner, M19 brief): no code. The decisions are in docs/plan.md
("Window manager requirements, after the design phase") and CLAUDE.md ("Window manager
requirements (owner, after the design phase)"). Code references are to commit e5ce776.

## A. Current behaviour

### A1. How focus changes today

| Trigger | Where | What happens |
|---|---|---|
| A window maps (first buffer) | `windows.rs` `toplevel_committed` (the `!mapped && has_buffer` branch) | `layout.focus(id)`: the new window takes focus and is raised; the last one to map wins (M9e). Before the map, focus stays where it was (`open_unfocused`). |
| A plain click on a window | `seat.rs` `pointer_button`, `Action::ClickFocus` | `focus_window`: an on_demand layer surface loses the keyboard, then focus and raise; the click is delivered. |
| A click on decorations | `decorations.rs` `decoration_press` | `focus_window` (focus and raise); never delivered. |
| A click on a layer surface | `layer_shell.rs` `layer_clicked` | on_demand (and exclusive on bottom or background) take the keyboard; the layout's focus is unchanged. |
| mod+H/J/K/L | `windows.rs` `run_action`, `Action::Focus` | `focus_window(neighbour)`. |
| mod+Tab | `cycle` / `commit_cycle` | `preview_focus` while mod is held (raise, no history), `commit_focus` on release. |
| mod+drag start | `begin_drag` | `layout.focus(id)`. |
| Close of the focused window | `MonitorLayout::close` | tiled: the most recent window of the sibling subtree; floating: the most recent overall. |
| Fullscreen | `commit_focus` | focusing any other window ends fullscreen. |
| Popups | `popups.rs` `popup_focus` | a grabbed chain takes the keyboard while its toplevel is the layout's focus; when that toplevel loses focus the grab ends. Pointer events go only to the grabbing client (`pointer_moved_to`). |
| Layer surfaces | `layer_shell.rs` `exclusive_layer`, `layer_keyboard_target` | an exclusive top or overlay surface takes the keyboard first; window keybinds do nothing then. `sync_keyboard_focus` resolves layer, then layout focus, then popup grab. |
| Move/resize grabs, a held button | Smithay `PointerHandle::is_grabbed` | a drag grab or the implicit click grab is active; nothing in sash-wm changes focus on motion. |

The keyboard always follows `MonitorLayout::focused` through `sync_keyboard_focus`,
called at the end of every `arrange`. `focus` always raises (`preview_focus` calls
`floating.raise`).

Pointer motion never changes focus today: `pointer_moved_to` (`seat.rs`) only updates
the wl_pointer focus (`pointer.motion`), the cursor, the decoration hover and the
recorder.

### A2. Rule A and "the pointer counts after its first movement"

| Piece | Where | Today |
|---|---|---|
| Moved flag | `seat.rs` `pointer_moved_to`: `pointer_has_moved \|= current_location != location` | Set by any position change, whether from the backend, from synthetic input or from the harness. The same position again does not count as moving. |
| Pointer for a new window | `toplevel_committed`, initial commit | `pointer` is the floored position, or None until the pointer has moved. |
| Target tile | `MonitorLayout::open_unfocused` → `tile_at(x, y)` | The tiled cell (the layout rectangle, gaps included) containing the point, whatever is drawn above it, so floating windows are already not targets. Otherwise the focused window's leaf (it may be a floating placeholder). Otherwise the last leaf. |
| Split | `BspTree::insert_with_mode` | Along the target's longer side (`w >= h` → side by side), `Ratio::HALF`. The new leaf is always the second child. |
| Forced floating | `classify` | A window with a parent or a fixed size floats, centred on its parent or the usable area. |

Tests: `tests/tiling.rs` `an_unmoved_pointer_counts_as_no_pointer`; `monitor.rs`
`new_window_placement_table`.

### A3. Where pointer motion is handled

- **Relative motion (libinput):** `handle_input` (`InputEvent::PointerMotion`) → `pointer_moved_by`, which clamps to the output, → `pointer_moved_to`.
- **Absolute motion (winit, the VM tablet):** `PointerMotionAbsolute` → `pointer_moved_to`.
- **Synthetic input (debug socket) and the harness:** `pointer_moved_to` directly.

`pointer_moved_to` does the following in order:

1. `surface_at`, which runs `hit` over the visible windows.
2. The popup-grab filter.
3. `pointer.motion`. During a grab, Smithay routes it to the grab: `grab.rs` for move and resize, `data_device/drag.rs` for drag and drop.
4. The cursor reset on a focus change.
5. `decoration_hover`, which runs `hit` again.
6. The recorder, which calls `window_at`.
7. A cursor frame, if the cursor is drawn.

### A4. A stationary pointer under changing geometry

Nothing re-evaluates the pointer after a layout change:

- **Keyboard focus:** unchanged. This is the desired rule.
- **wl_pointer focus:** stale. The client that was under the pointer keeps it until the next motion, and the window now under the pointer gets no enter.
- **Decoration hover and cursor image:** stale until the next motion.
- **A click with no motion in between:** sash-wm focuses `window_at`, which is the new window. But Smithay delivers the button to `pointer.current_focus()`, which is still the old surface, with the old surface-local position.

**Finding:** the stale click is an existing defect, not caused by M19. M19a should fix it (B5): placement at the pointer makes it routine, because a new tile appears right under the pointer.

### A5. Other findings

- **The xdg_toplevel `activated` state is never sent** (`arrange` sets tiled, maximized, fullscreen and resizing only). Clients learn focus only through wl_keyboard enter and leave, so a terminal or browser may keep its "active" look while unfocused.
  - Not part of the brief; listed for the owner (question 7).
- **Replay compares focus:** replay snapshots record the focused window (`record/snapshot.rs`). Bundles recorded with click-to-focus would replay differently with focus following the cursor (D, row 10).

### A6. Measured cost of a motion event today

Release build, the headless harness:

- **Scene:** 5120x1440 with gaps 13/8 and radius 13; 10 tiled windows, then the same 10 floated.
- **Method:** a throwaway test in a scratchpad clone (never committed), 200 000 motions sweeping the whole output.

| Scene | `pointer_moved_to` p50 / p95 / p99 | `MonitorLayout::window_at` p50 / p95 / p99 | `focus_window` p50 / p95 / p99 |
|---|---|---|---|
| 10 tiled | 5.4 / 5.8 / 8.4 µs | 0.48 / 0.55 / 0.79 µs | 7.0 / 7.7 / 11.0 µs |
| 10 floating | 3.8 / 4.0 / 5.6 µs | 0.20 / 0.25 / 0.32 µs | 4.4 / 4.5 / 5.1 µs |

- **Pointer motion:** at 1000 Hz, 5.4 µs per event is about 5.4 ms per second, 0.54% of one core.
- **Focus changes:** `focus_window` (an arrange of 10 windows plus keyboard enter and leave) runs only when focus actually changes, not on every motion.

## B. Proposed design: pointer-driven focus

### B1. Data flow

```
backend motion → pointer_moved_to(location)
  before = window_at(old location)   ← current geometry
  after  = window_at(new location)   ← (already computed by hit; reused)
  pointer.motion(...)                ← unchanged
  if focus_follows_cursor
     && real motion (location changed)
     && after is Some && after != before
     && no hold (B4)
     && after != layout.focused
  then pointer_focus(after)          ← focus without raise (below)
```

- **`window_at` counts popups and decorations:** it returns the window whose popup, surface or decoration is topmost at the point; None over a gap, the wallpaper or a layer surface above the windows (the dock).
- **Gaps, the wallpaper and the dock keep focus:** `after` is None there.
- **`pointer_focus(id)`:** a new `MonitorLayout` call. It sets focus and records it in the history (so closing a window falls back to what the user last pointed at), but never raises (open question c, proposal: no) and never ends fullscreen (B4). Then `layer_focus_lost` and `sync_keyboard_focus`.
  - It does not need a full `arrange`, because no configure changes.
  - It needs a redraw, because the bar tone, outline and dim of two windows change.
- **The setting:** a value in sash-wm's state, default on, with no environment variable (as the dim, M18b), until the settings protocol exists. Off means click-to-focus as today.

### B2. Hysteresis: a real motion into a different window

- **Focus changes only when one motion crosses into a different window.** Both ends are measured with the current geometry: `before` is the window under the previous position, `after` the window under the new one. This is stateless.
- **Geometry changes never change focus.** No code runs without motion. A window that slid under a still pointer is "before" and "after" at once on the next 1 px bump, so `before == after` and focus stays.
- **No pointer warp:** nothing moves the pointer.
- **The same position again is no motion:** the clamped position at an edge is unchanged, as `pointer_moved_by` and the edge-frame rule already define.

Alternative, for the owner (question 2): a stateful "last entered window", re-baselined after every arrange. It differs only in one case: pointer goes from a window into a gap and back into the same window. With the stateless rule that refocuses the window (it is an entry from nothing); the stateful rule does not. Recommendation: stateless, the classic sloppy-focus meaning (an enter refocuses).

### B3. Keyboard focus moves stay

mod+H/J/K/L, mod+Tab, a close fallback and a map all change `focused` without moving the pointer.

- **Motion inside the same window:** `before == after`, so nothing changes. Focus stays where the keyboard put it.
- **Entering any window:** focus goes to it. That includes the window the keyboard left, if the pointer leaves it and comes back (B2).
- No extra state is needed.

### B4. What holds focus (motion never moves it)

| Hold | Test today | Why |
|---|---|---|
| Move or resize grab, drag and drop, a held button (implicit grab, e.g. a text selection that leaves its window) | `pointer.is_grabbed()` | the brief; a selection drag must not hand the keyboard to another window |
| A grabbed popup chain | `popups.grab_root().is_some()` | the brief; other clients get no pointer events anyway |
| A layer surface holding the keyboard (exclusive, or on_demand chosen by a click: the launcher, a dock with keyboard focus) | `layer_keyboard_target().is_some()` | the brief (the launcher); a click still moves focus as today |
| The lock screen | (ext-session-lock does not exist yet) | the brief; the rule is written so the lock is one more hold |
| A mod+Tab cycle in progress | `keybinds.cycle.is_some()` | proposal: the cycle owns focus until mod is released |
| Fullscreen | `layout.fullscreen().is_some()` and `after` != the fullscreen window | nothing else is hit above it, and the backdrop hits nothing; the guard makes it explicit that pointer focus never ends fullscreen |

### B5. The stale pointer after geometry changes (A4)

Proposal for M19a:

- At the end of every `arrange`, and when a layer surface maps or unmaps, re-run the pointer's surface lookup at its current position with a *refresh* flag. The refresh updates the wl_pointer focus (enter and leave, a fresh serial), the hover and the cursor.
- The refresh never counts as motion: it does not set `pointer_has_moved` and never changes keyboard focus.
- Clicks then always go to the surface under the pointer.
- Grabs: while a grab is active the refresh is skipped. The grab owns the pointer, as today.

### B6. Multi-monitor

There is one output today; hotplug of other monitors is out of scope (M9).

- `window_at` and the crossing test work on global coordinates, so the rule needs nothing per monitor: crossing from one monitor's window into another's is an ordinary crossing.
- When workspaces and more outputs exist, `window_at` must look at the output under the pointer. This is noted for that milestone; there is no M19 work.

### B7. Cost per motion event

| | Today | Added by M19 |
|---|---|---|
| Every motion | 5.4 µs p50 (10 tiles, 5120x1440) | one `window_at` for `before`: +0.5 µs; `after` reuses the hit already done; a few compares |
| A crossing that changes focus | none | `pointer_focus`: keyboard enter and leave and a redraw; under the measured `focus_window` (7 µs), since there is no arrange |
| B5 refresh | none | one lookup per arrange (not per motion) |

- **Budget for a 1000 Hz mouse:** at most 1 ms per event is the hard limit. The proposal targets under 10 µs p99 per motion with 10 windows at 5120x1440, about 1% of a core at 1000 Hz. Today's p99 is 8.4 µs.
- **Frames:** a focus change costs at most one frame (merged with the cursor frame); a sweep across 10 windows gives at most 10.
- **Optional, not proposed:** today `hit` runs three times per motion (surface, hover, recorder). It could run once, if the budget needs it.

## C. Placement at the pointer

### C1. The zone function (pure, in `layout/`)

```
fn drop_side(tile: Rect, x: i32, y: i32) -> Side   // Left | Right | Top | Bottom | Centre
```

1. **Normalise the point:** u = (x - tile.x + 0.5) / tile.w and v = (y - tile.y + 0.5) / tile.h, using pixel centres. Exact integer arithmetic (cross-multiplied) avoids float ties.
2. **Centre zone:** if |u - 0.5| < c/2 and |v - 0.5| < c/2, the result is Centre. Here c = 0.236 (23.6%), as exact integer comparisons: 236 / 1000.
3. **Otherwise, the diagonals:** the side is the nearest edge in normalised units, min(u, 1 - u, v, 1 - v). Ties on a diagonal go to a fixed order (Left, Right, Top, Bottom), so the function is total and deterministic.

From the side to the split:

| Side | Split | New window |
|---|---|---|
| Left | side by side | first child |
| Right | side by side | second child |
| Top | stacked | first child |
| Bottom | stacked | second child |
| Centre | along the longer side (w >= h → side by side), as rule A today | second child |

- The ratio is always `Ratio::HALF`: 50:50, the first child gets the floor.
- `BspTree::insert_with_mode` gains the orientation and the child position as parameters (today the new window is always second).
- **Open, for the owner (question 1):** "a centre zone (23.6% each way)" can be read two ways.
  - Reading (a): a centred box 23.6% of the width by 23.6% of the height.
  - Reading (b): 23.6% from the centre in each direction, a box 47.2% wide.
  - The function is written with c as one constant; the proposal is (a).

### C2. Fall-backs, in order

| Pointer | Target | Side |
|---|---|---|
| never moved since startup (`pointer_has_moved` false) | the focused tile | Centre |
| over a tiled cell (also under a floating, maximized or fullscreen window drawn above it: floating windows are not targets) | that tile (`tile_at`, as today) | `drop_side` |
| in a gap (inner or outer) | the cell containing the point. Cells cover the usable area exactly and each inner gap is split between its two cells (floor and rest), so this is the nearest tile by construction | `drop_side`, which gives the side the gap is on |
| outside the usable area (over the dock or a panel) or over any layer surface above the windows (the launcher) | the focused tile | Centre |
| no tiled window at all | fills the usable area | n/a |

- **"The focused tile":** the focused window if it is tiled. Otherwise the most recently focused tiled window. Otherwise the last leaf, as today. Today a floating focused window's placeholder is split, which gives a tile in an arbitrary place; the proposal fixes that.
- **Layer surfaces inside the usable area (open, question 3):** "over any layer surface" is a proposal. A launcher overlay opened over the tiles would otherwise make the tile under the launcher the target of every app started from it.

### C3. The minimum tile (233 x 144) at insertion

- **Measured on the visible rectangle:** what the window shows, which is `gaps::visible` of the would-be cells. The one gap function computes it; nothing else computes a gap.
- **Steps:**
  1. Compute the two would-be cells of the chosen split.
  2. If both visible rectangles are at least 233 x 144, and at least each window's own minimum (C5), split.
  3. Otherwise try the other axis on the same tile. The side comes from the pointer on that axis (v < 0.5 → Top, else Bottom; u < 0.5 → Left, else Right); a Centre pick or a fall-back uses the second child.
  4. Otherwise the window opens floating: its client size, cascaded as other floating windows. Its placeholder leaf goes next to the target, so tiling it later (mod+F) tries the rule again.
- **The tiled-floor check:** the would-be cells come from `BspTree` (rects with the hypothetical leaf) and are checked with the same `gaps::visible`. A pure function `fits(cell, usable, gaps, min) -> bool` sits next to `visible` in `layout/gaps.rs`.

### C4. Replacing the 64 px tiled-resize floor (M6) with MIN_TILE_SIZE

- **Today:** `bsp.rs` `MIN_TILE = 64`, applied in `Node::min_length` (per leaf, per axis, on cells), used by `set_split_first` (keyboard and mouse tiled resize). If the split is too small, nothing changes (the tiny-output rule).
- **Proposal:**
  - Remove `MIN_TILE`. `min_length(orientation)` returns, for a tiled leaf, MIN_TILE_SIZE.w (side by side) or .h (stacked), plus the insets that leaf's edges get from `gaps::visible`: the outer gap on the usable area's boundary, half the inner gap inside.
  - The leaf's position in the walk decides which edges are boundary edges. The walk already knows the area of every node, so the floor applies to the visible rectangle, the same measure as C3.
  - At scale 1 the bound is exact. At other scales the physical rounding of gaps (M18a) changes the result by at most one pixel; the floor is logical.
  - The property test "a tiled resize never makes a tile smaller than min(MIN_TILE, its size before)" becomes min(MIN_TILE_SIZE per axis, its visible size before).
  - The test expectations at 64 (`bsp.rs` around line 962) move to 233 and 144.
- **When the area is too small for the minimum:**
  - **On insertion:** the window floats (C3).
  - **On a shrink (an output resize, a mode change, or a panel appearing) where the tree no longer fits every tile at the minimum:**
    - Proposal (option A): the newest tiled windows float one by one, newest first, until the rest fit. They get a "floated for size" flag, like the forced flag. They return to their leaves by themselves when the area grows enough. Tile-all re-tiles only those that fit.
    - Option B: keep today's tiny-output rule for shrinks. Tiles may go below the minimum only through an area change; the minimum applies to insertions and resizes.
    - Lossless toggling is kept in both, because the placeholder keeps the leaf.
    - The 640x360 floor on the usable area (M13b) always fits 2 x 2 tiles at the minimum: (640 - 26 - 8) / 2 = 303 >= 233 and (360 - 26 - 8) / 2 = 163 >= 144.
    - Recommendation: A, since the brief says "the tile floats". Owner question 4.
- **A client's own minimum size larger than the tile (known limit since M3: min and max hints are ignored while tiled):**
  - **On insertion:** the effective minimum on each axis is max(MIN_TILE_SIZE, the client's min plus its border). The client's min is known at the initial commit, when `classify` reads it. If neither axis fits, the window floats.
  - **On a resize:** the split stops at that effective minimum (`min_length` takes the leaf's limits).
  - **A client that raises its min after mapping:** it stays tiled and is clipped, as today. Floating it on a commit would make windows jump; this keeps the known limit.
  - The client's max size stays ignored while tiled.

### C5. Drag and drop

"The same rule applies to drag and drop" is read as: dropping a dragged window onto a tile inserts it there, by the same `drop_side`, minimum and fall-backs.

- **Today there is no drop into tiling:** a move drag floats a tiled window on the press (M4), and its release snaps (a zone) or leaves it floating.
- **The clipboard's drag and drop** (`drop_target_at`, `gap_owner`) is a different thing and stays as it is.
- **Proposal:**
  - **Trigger:** owner question 5 (below).
  - **During the drag:** the target tile's would-be half is shown with the snap-preview look (accent 12% fill, 1 px accent edge at 60%, rounded, through `gaps::visible`).
  - **On release:** the window leaves its old leaf (its sibling takes the space) and is inserted at the target by `drop_side`.
  - **Not allowed:** a drop onto its own placeholder's sibling only, or a drop that fails the minimum. Either way the window stays floating where it was released.
  - **Snap zones win at the output edge:** their triggers are checked first, as today.
- **Options for the trigger:**
  - (a) A drag that started on a tiled window re-tiles on release over a tile. To keep it floating, release it over nothing, or float it with mod+F. This changes M4's "a dragged tile floats".
  - (b) Only while a key is held at release, e.g. Ctrl. Mouse rows ignore Shift because Shift turns edge snapping off; Ctrl is free.
  - (c) Only when the monitor's default mode is tiled.
  - Recommendation: (b), because it changes no existing behaviour. The mouse row gets a keybind equivalent for the "every mouse action has a keybind" rule: mod+Shift+H/J/K/L (swap) already moves windows between tiles.

## D. Edge cases and risks

| # | Case | Expected | Test |
|---|---|---|---|
| 1 | A window maps under a stationary pointer (placement puts it there) | It takes focus by the map rule (interim, below), not by the pointer. A 1 px bump inside it changes nothing; moving into a neighbour focuses the neighbour. | harness: park the pointer, map, assert focus; bump 1 px, assert the same; cross into the neighbour, assert the neighbour |
| 2 | Two windows map in the same event-loop turn | Both are placed by the same pointer, in commit order (the second splits the half the first got under the pointer); focus ends on the last to map (M9e). | harness: two clients commit before one dispatch; assert the tree and the focus |
| 3 | A client maps, then resizes itself (a new buffer size or a new min size) | Placement was decided at the initial commit. A later size change never re-places the window and never changes focus. A raised min leaves it tiled and clipped (C4). | harness: map, commit a larger buffer and set_min_size; assert the leaf is unchanged and the focus too |
| 4 | Fullscreen window, pointer moves | Focus stays on it. Popups and windows below are not hit; the backdrop hits nothing. Pointer focus never ends fullscreen. | harness: fullscreen, sweep the pointer across the output, assert focus and fullscreen; the same with a fixed-size fullscreen window and its backdrop |
| 5 | The focused window closes while the pointer is over another window | The close fallback (M3/M4) decides. The pointer does not take over until it moves into a different window. | harness: pointer resting on B, focus A, close A; assert the fallback; bump within B: B stays unfocused if the fallback chose C; cross into C and back into B: B |
| 6 | The dim (M18b) | Each focus change moves the dim: two windows redraw, one frame; no animation (the spec has none). The dim is never on the window the pointer focused, and never over a bar or a fullscreen window. Sweeping across N windows gives at most N frames. | goldens are not needed (the M18b dim goldens cover the look); a harness test counts frames during a scripted sweep and checks the dim follows `focused` |
| 7 | A geometry change under a still pointer (swap, a close refill, a panel appearing, an output resize) | Keyboard focus is unchanged. The wl_pointer focus is refreshed (B5), so a click lands in the window now under the pointer. | harness: swap with mod+Shift+L under a parked pointer; assert focus unchanged and wl_pointer enter on the new surface; click without motion; assert the click reaches the new surface |
| 8 | Holds (B4): a move drag across windows, a text selection drag out of a window, an open menu, an on_demand or exclusive layer surface, a mod+Tab cycle | No focus change during the hold. After the hold, the next crossing changes focus. | harness: one test per hold |
| 9 | The pointer on decorations or a resize zone of another window | That window gets focus (decorations count as the window). The resize cursor still shows. | harness: cross onto a border zone |
| 10 | Replays of bundles recorded before M19 | They replay with focus follows cursor off (the bundle does not carry the setting, so it is taken as off). New bundles record the setting. | replay: the existing fixtures still match; a new fixture with the setting on |
| 11 | Old harness tests that move the pointer for clicks or rule A | The harness starts with the setting off, as it starts with gaps NONE and radius 0. The M19 tests turn it on, and the layout property tests run with it on. | the existing suite unchanged; the new tests on |
| 12 | Focus stealing (interim, below) | Recorded as a known risk until E5. | none (behaviour as M9e) |
| 13 | Pointer in a gap at a cell corner (inner gaps meet) | `tile_at` picks the one cell that contains it (cells do not overlap); `drop_side` picks a side by the diagonals. Deterministic. | property test: every point of the usable area maps to exactly one cell and one side |
| 14 | A tile too small on both axes for a split (5120x1440 after many splits; 640x360 usable) | The window floats, cascaded. | unit and property: no tile below the minimum after any insertion |
| 15 | Placement during a move drag or with a popup open | Placement uses the pointer as it is. The window opens unfocused; when it maps, focus moves to it, which ends a grabbed chain on the old window (popup_done), as today. | harness: open a window while a menu is open; assert the popup_done and the new focus |

### The interim state and the focus-stealing risk

Until E5 and E6 exist (the shell, xdg-activation tokens), every new tiled window is placed at the pointer and takes focus when it maps, as focus-at-map does today (M9e). That includes windows the user did not start, for example a client that opens a second window by itself, or a slow app that maps long after it was launched.

- **Risk:** keys typed into the old window after a background window maps go to the new window, passwords included. The user's tile is split under the pointer without their asking.
- **No mitigation is built in M19:** the user-started rule needs the token.
- Recorded in docs/plan.md.

## E. Milestones

| Step | Contents | Tests | Excluded |
|---|---|---|---|
| **M19a: focus follows the cursor** | `pointer_focus` (no raise, never ends fullscreen); the crossing rule (B2); the holds (B4); the setting as a value in state, default on; the wl_pointer refresh after layout changes (B5); the recorder's focus reason "pointer"; replay with the setting from the bundle (off for old bundles) | headless harness with scripted pointer motion: every row of D that concerns focus (1, 4, 5, 6, 7, 8, 9, 10, 15); a property test over random motions, layout operations and keybinds: focus changes only on a motion whose `before` and `after` differ, never inside a hold, never on an arrange; the motion cost measured again (p50/p95/p99, 10 windows at 5120x1440, budget B7) | placement; raise on focus; the activated state (owner question 7); multi-monitor; the lock screen (does not exist) |
| **M19b: placement at the pointer** | `drop_side` (C1); the fall-backs (C2); the minimum at insertion (C3) through `gaps::visible`; `MIN_TILE` replaced by MIN_TILE_SIZE for resizes, with the client's minimum (C4); the shrink rule (owner question 4); the insert-as-first-child option in `BspTree` | unit: the side table at every edge, corner, diagonal and centre boundary; property tests: `drop_side` is total, deterministic and mirror-symmetric (mirroring the point mirrors the side), points within 1 px of an edge give that edge outside the centre box; every insertion keeps coverage and every visible tile is at least 233 x 144 or the window floated; tiled resizes never go below the minimum; harness: the C2 fall-backs end to end, a window that does not fit floats, rows 2, 3, 13 and 14 of D; goldens: none (nothing new is drawn) | drag and drop into tiles; user-started windows (E5) |
| **M19c: drop into tiling** | the trigger (owner question 5), the preview, the release (C5) | unit: a drop target from the zone function; harness: a scripted drag onto each side of a tile, a drop that does not fit, a drop on an edge trigger (snap wins), Escape and a focus keybind cancel; goldens: the drop preview at scales 1, 1.5 and 2, dark and light, because it is visual | workspaces, multi-monitor drops |

Each step: tools/check-all.sh, trixie, the VM scenario, the fresh clone and similarity; then a report and a STOP. No tag without the owner.

## Questions for the owner

1. Centre zone: (a) a centred box 23.6% of the width by 23.6% of the height (proposal), or (b) 23.6% from the centre each way?
2. Hysteresis: stateless crossing (proposal: going from a window into a gap and back refocuses it), or a stateful "last entered window"?
3. A pointer over a layer surface inside the usable area (the launcher) counts as outside: the focused tile? (Proposal: yes.)
4. When a shrink makes the tiles too small: (A) the newest tiles float until the rest fit and return when there is room (proposal), or (B) today's tiny-output rule for shrinks?
5. The drop-into-tiling trigger: (a) every drag that started on a tile, (b) Ctrl held at release (proposal), or (c) only in the tiled default mode?
6. The minimum measured on the visible rectangle, which takes the gaps out (proposal), or on the cell?
7. Send the xdg_toplevel `activated` state with focus (not built today; a separate small change)?
8. A mod+Tab cycle holds focus against the pointer (proposal: yes)?
