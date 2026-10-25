# SPDX-License-Identifier: Apache-2.0
#
# Stale-content run: floats the focused window (Alt+F) and resizes it by keyboard
# (Alt+Ctrl+Shift+L, then H, alternating) N times, SPACING seconds apart.
# Usage: python3 -I tools/measure/drive_resize.py INPUT_SOCKET N SPACING
import socket, sys, time

path, n, spacing = sys.argv[1], int(sys.argv[2]), float(sys.argv[3])
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(path)
f = s.makefile("rw")

def cmd(c):
    f.write(c + "\n"); f.flush()
    r = f.readline().strip()
    if r != "ok": sys.exit(f"{c!r} -> {r}")

ALT, CTRL, SHIFT, F, H, L = 56, 29, 42, 33, 35, 38

def chord(*keys):
    for k in keys: cmd(f"key {k} press")
    for k in reversed(keys): cmd(f"key {k} release")

chord(ALT, F)
time.sleep(1)
for i in range(n):
    chord(ALT, CTRL, SHIFT, L if i % 2 == 0 else H)
    time.sleep(spacing)
