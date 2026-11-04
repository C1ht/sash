<!-- SPDX-License-Identifier: Apache-2.0 -->
# M9 stage C: the first runs on the real machine (checklist)

For the owner, on the daily machine (NVIDIA RTX 4070, HDMI-A-1, Hyprland on tty1).
The safety plan is in docs/m9-plan.md; this is the exact procedure. **Stage C is run
by the owner, never by Claude.** Every step that needs sudo is marked.

What the probe found (`python3 -I tools/drm-probe.py --all-modes`, after stage B): one
GPU (`/dev/dri/card1`, nvidia-drm); HDMI-A-1 connected to a 1190x340 mm (32:9)
monitor, 26 modes; a hardware cursor plane (up to 256x256); atomic modesetting.
Hyprland drives it at 5120x1440@240.

**Mode:** the selection rule (CLAUDE.md) picks the native resolution from the panel's
physical aspect, **5120x1440, at its highest rate, 240 Hz**, not the EDID's preferred
3840x1080@120. Before anything is committed, every candidate is checked with an atomic
TEST_ONLY commit, in this order: 5120x1440 at 240, 120, 60 Hz, then 3840x1080 at 240,
120, 60 Hz. The log lists them (`mode candidates`) and says which passed
(`mode candidate passed its test commit`) or why one was rejected (`mode candidate
rejected ... reason=`). If none passes, sash-wm exits with a message and **does not
touch the display**: the console stays as it is. Run 1 asks for the safest native mode
instead (`--mode 5120x1440@60`, debug builds only); the later runs use the selected
240 Hz mode.

## The tool

`tools/stage-c.sh SECONDS NAME [-- CLIENT ARGS...]`, run on a text console from the
repository:

- prints the escape instructions and waits for Enter (read them, then press Enter);
- runs the debug build with `--backend drm --exit-after SECONDS`, inside `timeout`
  (SIGTERM 5 s after that, SIGKILL 5 s after the SIGTERM);
- with `--mode WxH@R`, passes that on (a fixed mode instead of the selection rule);
- writes the log, fsynced line by line, to
  `~/.cache/sash/stage-c/<timestamp>-<NAME>.log`, so it survives a hang or a power
  cycle;
- writes sash-wm's PID to `~/.cache/sash/stage-c/sash-wm.pid` (for the escape steps);
- with `-- CLIENT ...`, starts that client on sash-wm once it listens, with a throwaway
  `XDG_CONFIG_HOME`;
- with `--firefox` instead, starts Firefox on about:blank through `start_firefox`
  (tools/measure/common.sh: a throwaway profile, no network, the crash reporter and
  telemetry off), stops it by its recorded PID and removes the profile;
- prints the exit status and the log path when the console is back.

It refuses to run inside Hyprland (WAYLAND_DISPLAY or DISPLAY set) or off a text
console. It was tested end to end in the stage B VM (tools/vm/scenario.py, check 16).

## Prepare first

1. **Save your work.** Hyprland keeps running on tty1, but a hang could need a power
   cycle.
2. **Keep this checklist on another device** (phone, laptop): the screen will belong to
   sash-wm.
3. **Check that a text login works:** Ctrl+Alt+F3, log in as yourself, `exit`, then
   Ctrl+Alt+F1 back to Hyprland. If this does not work, stop here.
4. **Optional, needs sudo (do it yourself, or tell me first):** widen SysRq for this
   boot only, so Alt+SysRq+R (take the keyboard back) and Alt+SysRq+B (reboot) work:
   `sudo sysctl kernel.sysrq=1`. It resets at the next reboot. Today it is 16 (sync
   only).
5. **Build the debug binary in Hyprland:** `cargo build -p sash-wm` in the repository.

## If the screen stays black or frozen

The tool prints these steps before every run. In this order, waiting a few seconds
between the steps:

1. **Wait** SECONDS + 10 seconds (the two watchdogs).
2. **Super+Shift+E** (quit), or **Ctrl+Alt+F1** (Hyprland) / **Ctrl+Alt+F3**:
   sash-wm handles the VT keys before anything else.
3. **Ctrl+Alt+F4**, log in, `kill $(cat ~/.cache/sash/stage-c/sash-wm.pid)`; still
   there after 5 s: `kill -9 $(cat ~/.cache/sash/stage-c/sash-wm.pid)`. When sash-wm is
   gone, logind gives the display back. Then Ctrl+Alt+F3 or F1.
4. **Only if SysRq was widened (prepare, step 4):** Alt+SysRq+R, then Ctrl+Alt+F1.
5. **Last resort:** a short press of the power button (logind shuts down cleanly).

After every run, send me the log the tool printed.

## Run 1: 20 seconds at 5120x1440@60, no input (does the display come up and go back?)

On **tty3** (Ctrl+Alt+F3, logged in), in the repository:

```sh
tools/stage-c.sh 20 run1 --mode 5120x1440@60
```

- [ ] Within 2 seconds the screen turns dark grey, sharp (native resolution), with a
  white arrow in the top left corner.
- [ ] After 20 seconds the console of tty3 is back and the tool prints exit status 0.
- [ ] The log has `--mode: only the requested mode is tried` and
  `mode candidate passed its test commit ... mode="5120x1440@60.000"`.

## Run 2: the selected mode (240 Hz); input, VT switch, quit key (60 seconds)

```sh
tools/stage-c.sh 60 run2
```

- [ ] The log has `mode candidates ... ["5120x1440@240.000", ...]` and
  `mode candidate passed its test commit n=0 mode="5120x1440@240.000"`. If the first
  candidates were rejected, the log says why and which one passed; that is a result to
  send me, not a failure.
- [ ] The arrow moves smoothly (240 Hz).
- [ ] Move the mouse: the arrow follows.
- [ ] Ctrl+Alt+F1: Hyprland appears and works. Ctrl+Alt+F3: sash-wm is back.
- [ ] **Super+Shift+E** quits at once (before the 60 s); exit status 0.

## Run 3: windows and explicit sync (120 seconds)

```sh
tools/stage-c.sh 120 run3 -- kitty --config NONE
```

- [ ] kitty appears (it fills the screen). **Super+Return** opens foot next to it
  (debug builds); both are tiled; typing goes to the focused one; Super+H/L moves focus.
- [ ] **Explicit sync (to decide now):** in kitty, type and scroll quickly
  (`seq 100000`). Any flicker, old frames or garbage? This decides whether
  wp_linux_drm_syncobj_v1 is built (owner decision after M8).
- [ ] Super+Shift+E quits; the console is back.

## Run 4: monitor power off and on (120 seconds)

```sh
tools/stage-c.sh 120 run4 -- kitty --config NONE
```

1. Super+Return for a second window; note where both windows are.
2. **Turn the monitor off with its power button.** Wait 10 seconds.
3. **Turn it on again.** Wait up to 10 seconds.

Expected:

- [ ] sash-wm keeps running: no crash, no panic, the tool does not report a non-zero
  exit status at the end.
- [ ] After power-on the picture comes back by itself (with or without a short black
  flicker) with **the same windows in the same places**; the mouse and typing work.
- [ ] The log shows what the monitor did. Two normal outcomes:
  - the monitor drops hotplug when off: `udev event`, `connector probed
    state=Disconnected`, `display disconnected: rendering stops, windows are kept`;
    then on power-on `connector probed state=Connected` and `display connected again:
    full modeset`;
  - the monitor keeps hotplug while off (stays "connected"): only `udev event` /
    `connector probed state=Connected` lines, or none; sash-wm keeps rendering.
- [ ] Not expected (report it): the picture stays black after power-on, or the mouse
  moves but nothing redraws. Use Super+Shift+E, then the escape steps if needed.

## Run 5: cable pull (120 seconds)

```sh
tools/stage-c.sh 120 run5 -- kitty --config NONE
```

1. Super+Return for a second window; note where both windows are.
2. **Pull the HDMI cable at the graphics card** (the monitor stays on). Wait 10
   seconds. The keyboard still works: Super+Return opens a third window you cannot see.
3. **Plug it back in.** Wait up to 10 seconds.

Expected:

- [ ] sash-wm keeps running: no crash, no panic.
- [ ] The log shows `display disconnected: rendering stops, windows are kept` after the
  pull, and `display connected again: full modeset` after plugging back in (sash-wm
  clears the CRTC and commits a full modeset at the previous mode, or, if the monitor
  no longer offers it, at the first mode of the selection rule that passes its test
  commit; the windows are then re-clamped to the new size).
- [ ] The picture comes back with the same windows in the same places, plus the third
  one opened while unplugged.
- [ ] Not expected (report it): no picture after plugging back in. Super+Shift+E, then
  the escape steps.

Send me the five logs. Stage C continues from there.

## Needs sudo

- Only the optional `sudo sysctl kernel.sysrq=1` (prepare, step 4). Nothing else: the
  logind session on tty3 gives sash-wm the display and input devices.
