#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Resize stress test for the nested (winit) backend under Hyprland.
#
# Launches sash-wm several times. After each launch it floats the window and
# runs resize loops; the first loop starts as soon as Hyprland maps the window.
# For each loop it reads sash-wm's debug log and reports:
#   requested  resize requests sent to Hyprland
#   resizes    resize events sash-wm received
#   frames     frames rendered
#   coalesced  resizes directly followed by another resize (one frame covers both)
#   missed     resizes never followed by a frame; must be 0
# A loop fails if missed > 0 or fewer resizes arrived than were requested.
#
# Usage: tools/stress-resize.sh [launches] [loops-per-launch] [resizes-per-loop] [delay-s]
# Env:   BIN (default target/release/sash-wm), LOGDIR (default: new temp dir)

set -euo pipefail

LAUNCHES=${1:-5}
LOOPS=${2:-4}
RESIZES=${3:-200}
DELAY=${4:-0.03}
BIN=${BIN:-target/release/sash-wm}
LOGDIR=${LOGDIR:-$(mktemp -d)}

# Prints "resizes frames coalesced missed" for log lines after line $2.
analyze() {
    sed 's/\x1b\[[0-9;]*m//g' "$1" | tail -n +"$(($2 + 1))" | python3 -I -c '
import sys
resizes = frames = coalesced = 0
pending = False
for line in sys.stdin:
    if " resize w=" in line:
        resizes += 1
        coalesced += pending
        pending = True
    elif " frame frame_time_us=" in line:
        frames += 1
        pending = False
print(resizes, frames, coalesced, int(pending))'
}

window_size() {
    hyprctl clients -j | python3 -I -c '
import json, sys
for c in json.load(sys.stdin):
    if c["pid"] == int(sys.argv[1]):
        print("%dx%d" % tuple(c["size"]))' "$1"
}

total_loops=0
bad_loops=0
echo "logs: $LOGDIR"
printf '%-6s %-4s %-9s %-8s %-7s %-9s %-6s %s\n' launch loop requested resizes frames coalesced missed result

for launch in $(seq "$LAUNCHES"); do
    log="$LOGDIR/launch$launch.log"
    RUST_LOG=sash_wm=debug "$BIN" >"$log" 2>&1 &
    pid=$!

    # Wait for Hyprland to map the window (poll every 10 ms, up to 5 s).
    for _ in $(seq 500); do
        [ -n "$(window_size "$pid")" ] && break
        sleep 0.01
    done
    hyprctl dispatch "hl.dsp.window.float({ action = \"set\", window = \"pid:$pid\" })" >/dev/null

    for loop in $(seq "$LOOPS"); do
        start_line=$(wc -l <"$log")

        for i in $(seq "$RESIZES"); do
            w=$((700 + (i % 2) * 40))
            hyprctl dispatch "hl.dsp.window.resize({ x = $w, y = 500, window = \"pid:$pid\" })" >/dev/null
            sleep "$DELAY"
        done
        sleep 0.5

        read -r r f c m < <(analyze "$log" "$start_line")
        result=ok
        if [ "$m" -ne 0 ] || [ "$r" -lt "$RESIZES" ]; then
            result=MISMATCH
            bad_loops=$((bad_loops + 1))
        fi
        total_loops=$((total_loops + 1))
        printf '%-6s %-4s %-9s %-8s %-7s %-9s %-6s %s\n' "$launch" "$loop" "$RESIZES" "$r" "$f" "$c" "$m" "$result"
    done

    kill "$pid"
    wait "$pid" 2>/dev/null || true
done

echo "loops: $total_loops, mismatches: $bad_loops"
[ "$bad_loops" -eq 0 ]
