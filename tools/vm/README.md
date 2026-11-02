<!-- SPDX-License-Identifier: Apache-2.0 -->
# DRM backend tests in a VM (M9 stage B)

A Debian 13 guest under QEMU/KVM with a virtio GPU runs sash-wm's DRM backend. QEMU
runs as the normal user (`/dev/kvm`), shows nothing on the host display
(`-display none`), and is driven through its serial console (a root shell) and QMP
(keyboard and pointer input, `screendump` screenshots of the guest's own display).

- `vm.py`: starts or attaches to the guest; serial commands; QMP input and screenshots.
  UEFI (OVMF from `edk2-ovmf`); systemd credentials set the throwaway root password and
  skip the first-boot questions.
- `guest-setup.sh` (in the guest, once, as root): runtime packages and the user `sash`.
- `start-session.sh` (in the guest): sash-wm for `sash` in a logind session on VT 2,
  like a display manager would start it (no `--backend`: sash-wm chooses DRM itself).
- `foot.sh` (in the guest): a foot window on sash-wm.
- `build-iso.sh`: the trixie-built binaries, the guest scripts, `tools/stage-c.sh` and
  `tools/drm-probe.py` on an ISO.
- `scenario.py`: every check, PASS/FAIL, screenshots and logs: start (backend chosen
  from the environment), input, a drag with frame statistics and the refresh-rate cap,
  idle CPU and memory, VT switch, quit key, XKB layout, SIGTERM/SIGABRT/SIGKILL,
  `--exit-after`, a late VT, unplug and replug (the connector forced off and back
  through sysfs, then a change uevent: debugfs `trigger_hotplug` where the kernel has
  it, else `udevadm trigger`), and `tools/stage-c.sh` on a guest console.

## Run

1. Image (407 MB): `debian-13-nocloud-amd64.qcow2` from
   https://cloud.debian.org/images/cloud/trixie/latest/ (check `SHA512SUMS`).
2. Binaries, built in the `debian:trixie` container (as for the trixie build check,
   with `libseat-dev libinput-dev libudev-dev libgbm-dev`): `sash-wm-release`,
   `sash-wm-debug`.
3. An ISO with the binaries and the guest scripts:
   `xorriso -as mkisofs -R -J -V SASH -o sash.iso <dir>`.
4. First boot and setup: start the guest with `vm.py`, log in, mount the ISO
   (`mount -o ro /dev/sr0 /mnt/sash`) and run `sh /mnt/sash/guest-setup.sh`.
5. `python3 -I tools/vm/scenario.py <workdir> <image> <iso> <outdir>`.

The workdir holds the guest's disk overlay and UEFI variables; the sockets are in
`$XDG_RUNTIME_DIR/sash-vm/`. VM numbers do not predict the NVIDIA hardware.
