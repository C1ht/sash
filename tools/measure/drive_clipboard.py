# SPDX-License-Identifier: Apache-2.0
#
# M10 by-hand run, scripted: copy and paste and drag and drop between foot and a local
# Firefox page on nested sash-wm, through the synthetic input socket. Checks the
# clipboard with wl-paste on sash-wm's display, takes offscreen captures (SIGUSR1) and
# prints "phase START END" lines (log line numbers) for analyze.py.
# Usage: python3 -I tools/measure/drive_clipboard.py LOG INPUT SOCKET W H PID WORKDIR
import os, signal, socket, subprocess, sys, time

log, path, display, W, H, pid, work = sys.argv[1:8]
W, H, pid = int(W), int(H), int(pid)
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(path)
f = s.makefile("rw")

ALT, CTRL, SHIFT, ESC, TAB, SPACE = 56, 29, 42, 1, 15, 57
KEY = {"c": 46, "v": 47, "h": 35, "l": 38, "left": 105, "right": 106}


def cmd(c):
    f.write(c + "\n"); f.flush()
    r = f.readline().strip()
    if r != "ok":
        sys.exit(f"{c!r} -> {r}")


def chord(*keys):
    for k in keys:
        cmd(f"key {k} press")
    for k in reversed(keys):
        cmd(f"key {k} release")
    time.sleep(0.3)


def lines():
    with open(log, "rb") as fh:
        return sum(1 for _ in fh)


def capture(name):
    os.kill(pid, signal.SIGUSR1)
    time.sleep(0.6)
    print(f"capture {name}", flush=True)


def nested(args, stdin=None, output=True):
    env = dict(os.environ, WAYLAND_DISPLAY=display)
    # wl-copy stays in the background serving the selection: its output must not be a
    # pipe this script waits on.
    out = subprocess.PIPE if output else subprocess.DEVNULL
    r = subprocess.run(args, env=env, stdin=stdin, stdout=out, stderr=subprocess.DEVNULL,
                       timeout=20)
    return r.stdout or b""


def drag(x0, y0, x1, y1, steps=60, then_escape=False):
    cmd(f"motion {x0} {y0}")
    time.sleep(0.2)
    cmd("button left press")
    for i in range(1, steps + 1):
        x = x0 + (x1 - x0) * i / steps
        y = y0 + (y1 - y0) * i / steps
        cmd(f"motion {x:.1f} {y:.1f}")
        time.sleep(0.008)
    time.sleep(0.3)
    if then_escape:
        cmd(f"key {ESC} press"); cmd(f"key {ESC} release")
        time.sleep(0.2)
    cmd("button left release")
    time.sleep(0.8)


# Layout: foot tiled on the left, Firefox (last to map, focused) on the right; its text
# field is focused and selected.
chord(CTRL, KEY["c"])
time.sleep(0.5)
print("copied text read by wl-paste:", nested(["wl-paste", "-n"]).decode(errors="replace"), flush=True)
# Paste into foot (Alt+H, Ctrl+Shift+V).
chord(ALT, KEY["h"])
chord(CTRL, SHIFT, KEY["v"])
time.sleep(0.5)
capture("text-pasted-into-foot")
# The image: Firefox's button (Tab from the text field, Space).
chord(ALT, KEY["l"])
chord(TAB)
chord(SPACE)
time.sleep(1.0)
types = nested(["wl-paste", "--list-types"]).decode(errors="replace").split()
image = nested(["wl-paste", "-t", "image/png"]) if "image/png" in types else b""
print(f"after 'copy image': types {types}, image/png {len(image)} bytes", flush=True)
# Drag the file link (top of the page) onto foot; latency is logged while it runs.
start = lines()
drag(3 * W // 4, 90 + (H - 90) // 5, W // 4, H // 2)
print(f"dnd {start} {lines()} 60", flush=True)
capture("link-dropped-on-foot")
# A drag cancelled with Escape.
drag(3 * W // 4, 90 + (H - 90) // 5, W // 4, H // 2, steps=30, then_escape=True)
capture("after-escape")
# 10 MB through wl-copy and wl-paste on sash-wm.
big = os.path.join(work, "big")
with open(big, "wb") as out:
    out.write(os.urandom(10 * 1024 * 1024))
with open(big, "rb") as src:
    nested(["wl-copy"], stdin=src, output=False)
time.sleep(0.5)
t0 = time.monotonic()
got = nested(["wl-paste"])
took = time.monotonic() - t0
with open(big, "rb") as src:
    data = src.read()
# wl-paste may add a newline after data it takes for text.
same = got.startswith(data) and len(got) - len(data) <= 1
print(f"10 MB wl-paste: {took * 1000:.0f} ms (whole command), intact: {same}", flush=True)
os.remove(big)
