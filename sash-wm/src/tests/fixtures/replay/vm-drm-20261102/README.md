<!-- SPDX-License-Identifier: Apache-2.0 -->
# Replay fixture: a DRM session in the test VM (2026-11-02)

Recorded by tools/vm/scenario.py (check 18) with the debug-record build on the DRM
backend in the Debian 13 VM (virtio-gpu, 1280x800, real libinput devices, foot
windows): three windows spawned by Super+Return, the third with the pointer over window 1
while window 2 has focus (rule A splits window 1; the old rule would split window 2), Super+F, a Super+drag, Super+Left, a click, Super+Q, then Super+F11.
Nothing was typed (no "printable" keys). `events.jsonl` and `state.json` of the
bundle, unchanged; the outline screenshots are left out.
