<!-- SPDX-License-Identifier: Apache-2.0 -->
# Replay fixture: the owner's first real-hardware session (2026-11-03)

Stage C run 4 on the owner's machine (NVIDIA, HDMI-A-1, 5120x1440@240), the
debug-record build of commit d097519, `SASH_DEBUG_RECORD_SESSION=1`, foot windows only,
from an empty desktop. Made with `tools/replay-fixture.py <bundle> <this directory>
20.0`: events before 20.0 s (before foot's first maximize request, which the replay
cannot reproduce), only the kinds the replay uses (layout snapshots, window commits,
buttons, modifier keys, pointer positions), downsampled (the last pointer position
before each button, key or window event, the first and every 25th one during a drag).
No typed keys, no device names, no screenshots; the full bundle is not in the
repository.

What it covers: four windows opened by Super+Return under rule A (the third with the
pointer parked over window 1 while window 2 has focus), then nine Super+drag results,
including snaps to the bottom-right and bottom-left quarters and the left half, with an
8 kHz mouse (17,830 pointer positions before 20.0 s in the original, 388 here). Layout snapshots 0 to
12 of the bundle.
