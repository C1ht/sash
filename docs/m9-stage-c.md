<!-- SPDX-License-Identifier: Apache-2.0 -->
# M9 stage C: the first run on the real machine (checklist)

For the owner, on the daily machine (NVIDIA RTX 4070, HDMI-A-1, Hyprland on tty1).
The safety plan is in docs/m9-plan.md; this is the exact procedure. Nothing here is run
by Claude: stage C starts only when the owner says so, and every step that needs sudo is
marked and asked for first.

What the stage A probe found, so you know what to expect: one GPU (`/dev/dri/card1`,
nvidia-drm), HDMI-A-1 connected, preferred mode **3440x1440 at 60 Hz** (the one sash-wm
uses), a hardware cursor plane (64x64 to 256x256), atomic modesetting.

## Prepare first

1. **Save your work.** Hyprland keeps running on tty1, but a hang could need a power
   cycle.
2. **Keep this checklist on another device** (phone, laptop): the screen will belong to
   sash-wm.
3. **Check that a text login works:** press Ctrl+Alt+F3, log in as yourself, type
   `exit`, then Ctrl+Alt+F1 to get back to Hyprland. If this does not work, stop here.
4. **Optional, needs sudo (ask me before, or do it yourself):** widen SysRq for this boot
   only, so Alt+SysRq+R (take the keyboard back) and Alt+SysRq+B (reboot) work:
   `sudo sysctl kernel.sysrq=1`. It resets at the next reboot. Today it is 16 (sync only).
5. **Build the debug binary in Hyprland** (debug builds have the watchdog flag):
   `cargo build -p sash-wm`, from the repository.

## Run 1: 20 seconds, no input (does the display come up and go back?)

On **tty3** (Ctrl+Alt+F3, logged in), in the repository:

```sh
RUST_LOG=sash_wm=debug timeout -s TERM 25 target/debug/sash-wm --backend drm \
    --exit-after 20 2> /tmp/sash-wm-c1.log &
echo $! > /tmp/sash-wm.pid
wait
```

- Expected: within 2 seconds the screen turns dark grey with a white arrow in the top
  left corner; after 20 seconds the text console of tty3 is back.
- `--exit-after 20` makes sash-wm quit by itself; `timeout ... 25` sends SIGTERM 5
  seconds later in case it did not.

## If the screen stays black or frozen

In this order, waiting a few seconds between steps:

1. **Wait 25 seconds** for the two watchdogs.
2. **Ctrl+Alt+F1** (Hyprland) or **Ctrl+Alt+F3**: sash-wm handles these keys before
   anything else, and the console handles them once sash-wm is gone.
3. **Ctrl+Alt+F4**, log in, then `kill $(cat /tmp/sash-wm.pid)`; if it is still
   there after 5 seconds, `kill -9 $(cat /tmp/sash-wm.pid)`. When the process is gone,
   logind gives the display back. Then Ctrl+Alt+F3 or F1.
4. **Only if you widened SysRq (step 4 above):** Alt+SysRq+R, then Ctrl+Alt+F1.
5. **Last resort:** a short press of the power button (logind shuts down cleanly, as on
   this machine before).

Then send me `/tmp/sash-wm-c1.log`.

## Run 2: input, quit key, VT switch (60 seconds)

```sh
RUST_LOG=sash_wm=debug timeout -s TERM 70 target/debug/sash-wm --backend drm \
    --exit-after 60 2> /tmp/sash-wm-c2.log &
echo $! > /tmp/sash-wm.pid
wait
```

- [ ] Move the mouse: the arrow follows.
- [ ] Ctrl+Alt+F1: Hyprland appears and works. Ctrl+Alt+F3: sash-wm is back.
- [ ] **Super+Shift+E** quits at once and tty3's console is back (before the 60 s).

## Run 3: windows (120 seconds)

```sh
RUST_LOG=sash_wm=debug timeout -s TERM 130 target/debug/sash-wm --backend drm \
    --exit-after 120 2> /tmp/sash-wm-c3.log &
echo $! > /tmp/sash-wm.pid
sleep 2; grep -o 'socket="[^"]*"' /tmp/sash-wm-c3.log
```

The second line prints sash-wm's socket, e.g. `wayland-2` (Hyprland has `wayland-1`).
Then, with that name:

```sh
WAYLAND_DISPLAY=wayland-2 foot &
WAYLAND_DISPLAY=wayland-2 XDG_CONFIG_HOME=$(mktemp -d) kitty --config NONE &
wait
```

- [ ] Both windows appear tiled; typing goes to the focused one; Super+H/L moves focus.
- [ ] **Explicit sync (to decide now):** in kitty, type and scroll quickly (`seq 100000`).
  Any flicker, old frames or garbage? This decides whether wp_linux_drm_syncobj_v1 is
  built (owner decision after M8).
- [ ] Super+Shift+E quits; the console is back.

Send me the three logs. Stage C continues from there.

## Needs sudo

- Only the optional `sudo sysctl kernel.sysrq=1` (prepare, step 4). Nothing else: the
  logind session on tty3 gives sash-wm the display and input devices.
