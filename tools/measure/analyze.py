# SPDX-License-Identifier: Apache-2.0
#
# Frame statistics for log lines [start, end): python3 -I tools/measure/analyze.py LOG START END
import sys, re, statistics as st, datetime as dt
log, start, end = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
ansi = re.compile(r"\x1b\[[0-9;]*m")
lines = [ansi.sub("", l) for l in open(log).read().splitlines()[start:end]]
def ts(l): return dt.datetime.fromisoformat(l[:27].replace("Z", "+00:00")).timestamp()
frames = [l for l in lines if " frame frame_time_us=" in l]
def num(l, k):
    m = re.search(k + r"=(?:Some\()?(\d+)", l); return int(m.group(1)) if m else None
def stats(name, v, unit="us"):
    if not v: print(f"  {name}: none"); return
    v = sorted(v); q = lambda p: v[min(len(v)-1, int(p*len(v)))]
    print(f"  {name}: n={len(v)} p50={q(.5)}{unit} p95={q(.95)}{unit} p99={q(.99)}{unit} max={v[-1]}{unit} mean={st.mean(v):.0f}{unit}")
if frames:
    span = ts(frames[-1]) - ts(frames[0])
    print(f"  frames: {len(frames)} over {span:.2f}s = {len(frames)/max(span,1e-9):.1f} fps")
stats("frame time", [num(l, "frame_time_us") for l in frames])
stats("render", [num(l, "render_us") for l in frames])
stats("input to frame", [x for x in (num(l, "input_to_frame_us") for l in frames) if x is not None])
stats("configure to commit", [num(l, "configure_to_commit_us") for l in lines if "client caught up" in l])
keys = {}
for l in lines:
    if " keybind action=" in l:
        keys.setdefault(re.search(r"action=(\w+)", l).group(1), []).append(num(l, "layout_us"))
for a, v in keys.items(): stats(f"keybind {a} layout", v)
iv = [(ts(b) - ts(a)) * 1000 for a, b in zip(frames, frames[1:])]
if iv:
    print("  frame intervals (ms):")
    edges = [0, 8, 12, 15, 16, 17, 18, 20, 25, 34, 50, 100, 1e9]
    for lo, hi in zip(edges, edges[1:]):
        n = sum(1 for x in iv if lo <= x < hi)
        if n: print(f"    [{lo:>4}, {'inf' if hi > 1e8 else hi:>4}) {n:5d} {'#' * min(60, n)}")
