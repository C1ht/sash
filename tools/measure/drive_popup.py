# SPDX-License-Identifier: Apache-2.0
#
# M7 measurement: pavucontrol floating, mod+right-drag from its top-left cell (the
# window's origin moves) without and with its dropdown open.
# Usage: python3 -I tools/measure/drive_popup.py LOG INPUT_SOCKET W H PID   Prints "phase START END MOTIONS".
import math, os, signal, socket, sys, time

log, path, W, H, pid = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4]), int(sys.argv[5])
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(path)
f = s.makefile("rw")

def cmd(c):
    f.write(c + "\n"); f.flush()
    r = f.readline().strip()
    if r != "ok": sys.exit(f"{c!r} -> {r}")

def click(x, y):
    cmd(f"motion {x} {y}"); cmd("button left press"); cmd("button left release")

def lines():
    with open(log, "rb") as fh: return sum(1 for _ in fh)

def capture_allowed():
    """No content captures while a client that shows device names runs (CLAUDE.md)."""
    for p in os.environ.get("SASH_NO_CAPTURE_PIDS", "").split():
        try:
            os.kill(int(p), 0)
        except ProcessLookupError:
            continue
        print("capture skipped: a client that shows device names is running", flush=True)
        return False
    return True


def capture():
    time.sleep(0.4)
    if capture_allowed():
        os.kill(pid, signal.SIGUSR1)
    time.sleep(0.6)

PERIOD = 0.004
def drag(seconds):
    x0, y0 = W / 6, H / 6
    cmd("key 56 press"); cmd(f"motion {x0} {y0}"); cmd("button right press")
    a = lines(); t0 = time.monotonic(); n = 0
    while (t := time.monotonic() - t0) < seconds:
        x = x0 + 40 * math.sin(2 * math.pi * t); y = y0 + 40 * (1 - math.cos(2 * math.pi * t))
        cmd(f"motion {x:.1f} {y:.1f}"); n += 1
        time.sleep(max(0, PERIOD - (time.monotonic() - t0 - t)))
    b = lines()
    cmd("button right release"); cmd("key 56 release")
    return a, b, n

click(151, 56)                       # Recording tab (no personal content)
time.sleep(0.5)
cmd("key 56 press"); cmd("key 33 press"); cmd("key 33 release"); cmd("key 56 release")  # float
time.sleep(0.5)
marks = [("drag-no-popup", *drag(5))]
time.sleep(0.5)
click(W - 350, H - 30)               # the "Show:" dropdown
time.sleep(0.8)
capture()
marks.append(("drag-with-popup", *drag(5)))
time.sleep(0.5)
capture()
for m in marks: print(*m)
