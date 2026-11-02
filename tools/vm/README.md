<!-- SPDX-License-Identifier: Apache-2.0 -->
# DRM backend tests in a VM (M9 stage B)

A Debian 13 guest under QEMU/KVM with a virtio GPU runs sash-wm's DRM backend. QEMU
runs as the normal user (`/dev/kvm`), shows nothing on the host display
(`-display none`), and is driven through its serial console (a root shell) and QMP
(keyboard and pointer input, `screendump` screenshots of the guest's own display).

Everything lives in `~/.cache/sash/vm` (`$XDG_CACHE_HOME/sash/vm`): the image, the
guest (`guest/`: disk overlay, UEFI variables), the ISO, a throwaway SSH key for the
guest (`ssh/`) and the results (`out/`). Nothing depends on a temporary directory.

- `vm.py`: starts or attaches to the guest; serial commands; QMP input and screenshots.
  UEFI (OVMF from `edk2-ovmf`); systemd credentials set the throwaway root password and
  skip the first-boot questions. The image is pinned to one dated Debian build and its
  SHA512 (`IMAGE_VERSION`, `IMAGE_SHA512`); it is checked when downloaded and before
  every run.
- `prepare.py`: downloads the image (or verifies the one there) and sets up the guest
  once (`--fresh` again).
- `guest-setup.sh` (in the guest, run by `prepare.py`): runtime packages, sshd, and the
  user `sash`.
- `start-session.sh` (in the guest): sash-wm for `sash` in a logind session on VT 2,
  like a display manager would start it (no `--backend`: sash-wm chooses DRM itself).
- `foot.sh` (in the guest): a foot window on sash-wm.
- `build-iso.sh`: the trixie-built binaries, the guest scripts, `tools/stage-c.sh` and
  `tools/drm-probe.py` on an ISO.
- `scenario.py`: every check, PASS/FAIL, screenshots and logs: start (backend chosen
  from the environment; the mode by the selection rule, validated with a test commit),
  the session variables of a console, a serial and an SSH login (and DRM refused over
  SSH), input, a drag with frame statistics and the refresh-rate cap, idle CPU and
  memory, VT switch, quit key, XKB layout, SIGTERM/SIGABRT/SIGKILL, `--exit-after`,
  `--mode` (offered and not offered), mode fault injection (the first candidates
  rejected; all rejected, with the kernel's DRM ioctl trace showing that sash-wm made
  no state-changing ioctl), a late VT, unplug and replug (the connector forced
  off and back through sysfs, then a change uevent: debugfs `trigger_hotplug` where the
  kernel has it, else `udevadm trigger`), and `tools/stage-c.sh` on a guest console.

## Run

```sh
tools/vm/build-iso.sh ~/.cache/sash/vm      # trixie container build + ISO
python3 -I tools/vm/prepare.py              # image (about 410 MB, once) and guest setup
python3 -I tools/vm/scenario.py             # results in ~/.cache/sash/vm/out
```

The guest setup installs about 150 MB of Debian packages inside the guest. The SSH port
is forwarded on 127.0.0.1:22422 while the guest runs; the QMP and serial sockets are in
`$XDG_RUNTIME_DIR/sash-vm/`. VM numbers do not predict the NVIDIA hardware.
