# SPDX-License-Identifier: Apache-2.0
#
# Scripted drags through the synthetic input socket, with log line marks per phase.
# Usage: python3 -I tools/measure/drive_drags.py LOG INPUT_SOCKET W H   (W x H = output size in logical px)
# Prints "phase START END" lines for analyze.py.
import math, socket, sys, time

log, path, W, H = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(path)
f = s.makefile("rw")

def cmd(c):
    f.write(c + "\n"); f.flush()
    r = f.readline().strip()
    if r != "ok": sys.exit(f"{c!r} -> {r}")

def lines():
    with open(log, "rb") as fh: return sum(1 for _ in fh)

PERIOD = 0.004  # 250 Hz pointer

def motions(seconds, at):
    t0 = time.monotonic(); n = 0
    while (t := time.monotonic() - t0) < seconds:
        x, y = at(t); cmd(f"motion {x:.1f} {y:.1f}"); n += 1
        time.sleep(max(0, PERIOD - (time.monotonic() - t0 - t)))
    return n

marks = []
cmd("key 56 press")  # Alt held for the whole run

# 1. Tiled resize: right cell of window 1 (0, 0, W/2, H) moves the vertical split.
x0, y0 = W * 0.45, H * 0.5
cmd(f"motion {x0} {y0}"); cmd("button right press")
a = lines()
n = motions(5, lambda t: (x0 + W * 0.2 * math.sin(2 * math.pi * 0.5 * t), y0))
marks.append(("tiled-resize", a, lines(), n)); cmd("button right release")
time.sleep(0.5)

# 2. Floating drag: window 2 (top right) floats on press and circles.
x0, y0 = W * 0.75, H * 0.25
cmd(f"motion {x0} {y0}"); cmd("button left press")
a = lines()
r = min(W, H) * 0.12
n = motions(5, lambda t: (x0 + r * math.cos(2 * math.pi * t) - r, y0 + r * math.sin(2 * math.pi * t)))
marks.append(("floating-drag", a, lines(), n)); cmd("button left release")
time.sleep(0.5)

# 3. Snap drag: take it to the left edge zone and move along it (preview shown).
cmd(f"motion {x0} {y0}"); cmd("button left press")
motions(0.3, lambda t: (x0 - (x0 - 2) * t / 0.3, y0 + (H * 0.5 - y0) * t / 0.3))
a = lines()
n = motions(5, lambda t: (2, H * 0.5 + H * 0.2 * math.sin(2 * math.pi * 0.5 * t)))
marks.append(("snap-drag", a, lines(), n)); cmd("button left release")
cmd("key 56 release")
time.sleep(0.5)
for name, a, b, n in marks: print(name, a, b, n)
