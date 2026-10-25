<!-- SPDX-License-Identifier: Apache-2.0 -->
# M9 plan: the DRM/KMS backend (docs only, no code yet)

sash-wm running directly on the hardware from a text console, on the owner's daily
computer: NVIDIA RTX 4070, open kernel module 615.78.08, one monitor on HDMI-A-1,
Hyprland on tty1. M9 starts with its own investigate-then-STOP pass.

## Pieces

- **Session and permissions: libseat.** sash-wm opens the DRM card and input devices
  through libseat. Here seatd is installed but inactive, and the login session is
  managed by systemd-logind, so libseat uses its logind backend: logind hands over the
  devices of the active VT to the session's controller. No group changes and no sudo
  are needed when sash-wm runs from a VT the owner is logged in on. (seatd would
  need `sudo systemctl enable --now seatd` and the `seat` group: not planned.)
- **Input: libinput** through Smithay's `backend_libinput` with the libseat session
  (devices opened via the session, not directly). Keyboard layout and repeat: the
  input-settings proposal (plan.md) decides; until then today's constants.
- **One GPU only:** the NVIDIA card (`/dev/dri/card1`, render node `renderD128`). No
  multi-GPU, no hotplug of GPUs.
- **GBM and EGL:** GBM device on the card (NVIDIA's egl-gbm is installed), EGL
  display from it, the same `GlesRenderer` as nested; scanout buffers allocated by GBM
  with the modifiers the plane supports. Smithay's `DrmCompositor` handles planes,
  damage and page flips.
- **Pacing: vblank.** Render when a frame is wanted and the previous page flip has
  completed; frame callbacks after the flip, as nested after the host's callback.
  **wp_presentation** feedback with the flip's timestamp (the real on-screen time) and
  refresh rate, which also answers M4's open pacing question.
- **Cursor:** a hardware cursor plane when the driver offers one (NVIDIA does), with
  the software cursor from M8 as the fallback (same code path as nested cursor
  surfaces).
- **VT switching:** Ctrl+Alt+F1..F12 are handled by sash-wm itself (libinput takes
  the keyboard away from the console) and call libseat's `switch_session`; on
  pause sash-wm stops rendering and releases DRM master and input; on resume it
  re-applies the mode and redraws.
- **New dependencies:** Smithay features `backend_drm`, `backend_gbm`,
  `backend_libinput`, `backend_session_libseat` (crates drm, gbm, input, libseat) and
  the trixie packages `libseat-dev`, `libinput-dev`, `libgbm-dev`, `libudev-dev`,
  `libdrm-dev`. Each needs the owner's approval and a cargo deny check.

## Testing safely on this machine

The risk is losing the display or the keyboard with no way back. The machine has no
SSH server, and SysRq is limited to sync (`kernel.sysrq = 16`): the "unraw" and
"reboot" keys are disabled.

### Ways back, built in from the first DRM run

1. **A watchdog:** the first runs are started as
   `timeout -s TERM 20 target/debug/sash-wm --backend drm` (coreutils, no code), and
   sash-wm also ends itself after `SASH_DRM_WATCHDOG` seconds if set (debug builds
   only, like synthetic input). Either way it exits cleanly: libseat releases the
   devices and logind gives the VT back to text mode.
2. **mod+Shift+E** quits (Super on DRM), from the very first build.
3. **Ctrl+Alt+F1..F12 always switch VTs**, handled before any keybind or client sees
   the keys, so the keyboard can never be trapped in sash-wm.
4. **Run from a second VT, never from tty1:** Hyprland stays on tty1. Switch with
   Ctrl+Alt+F3, log in on the text console, start sash-wm there. Ctrl+Alt+F1 returns
   to Hyprland.
5. **A third VT with a shell** as the last resort: from Ctrl+Alt+F4 the owner can
   `kill <pid>` the sash-wm process (its PID is printed at start and written to a file).

### What could leave the display black

- sash-wm crashing while holding DRM master. logind ends the session's device
  access when the process dies, but the VT can stay in graphics mode; switching VTs
  (Ctrl+Alt+F1) usually recovers, since the keyboard is back with the console and
  logind once sash-wm is gone.
- A failed or unsupported mode set (wrong modifier for the primary plane, a mode the
  monitor rejects): black until sash-wm exits; the watchdog ends it.
- Switching back to Hyprland while sash-wm still holds the device: sash-wm must
  release DRM master on pause (libseat's pause event).
- A GPU hang in the driver (rare): only a power cycle helps. The first runs avoid
  anything unusual (one primary plane, the monitor's preferred mode, no direct scanout).
- A keyboard stuck in raw mode after a crash: with SysRq unraw disabled, VT switching
  depends on logind restoring the console keyboard; the fallback is the power button
  (a clean shutdown via logind, as already used on this machine).

### What the owner should do first

1. Read this plan; decide whether SysRq should be widened for recovery
   (`kernel.sysrq = 1` or at least `+4` unraw, `+64` process signals). This **needs
   sudo** (`sudo sysctl kernel.sysrq=...` and a file in `/etc/sysctl.d/`): I will ask
   before doing it, or the owner does it.
2. Check that a text login works: Ctrl+Alt+F3, log in, `exit`, Ctrl+Alt+F1 back to
   Hyprland.
3. First M9 run is a **read-only probe**: open the session and the card through
   libseat, list connectors, modes and planes, print them, exit. No mode set.
4. Then a 10 s run with a mode set and a solid colour, under `timeout`, started from
   tty3; then input; then clients.

### Needs sudo (ask first)

- Widening SysRq (optional, recommended).
- Installing or enabling anything system-wide (seatd service, groups): not planned.
- Nothing else in the plan needs root: libseat/logind, DRM, GBM, libinput all work
  as the logged-in user on the active VT.
