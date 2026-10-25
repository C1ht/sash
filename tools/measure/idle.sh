#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Idle CPU and memory of sash-wm with N idle foot windows over D seconds. Counts input
# events, commits and frames in the same period so disturbed runs are visible.
# Usage: tools/measure/idle.sh LOG N D [BIN]   (BIN defaults to target/release/sash-wm)
set -u
source "$(dirname "$0")/common.sh"
LOG=$1 N=$2 D=$3
RUST_LOG=sash_wm=trace start_sash "$LOG" "${4:-$ROOT/target/release/sash-wm}"
for n in $(seq "$N"); do
    start_client foot -e sh -c "printf '\\n  foot %s\\n' $n; sleep 900"
    sleep 1.2
done
sleep 3
T=$(getconf CLK_TCK)
a=$(awk '{print $14+$15}' "/proc/$SASH_PID/stat")
L=$(wc -l < "$LOG")
t0=$(date +%s.%N)
sleep "$D"
b=$(awk '{print $14+$15}' "/proc/$SASH_PID/stat")
t1=$(date +%s.%N)
slice=$(sed 's/\x1b\[[0-9;]*m//g' "$LOG" | tail -n +$((L + 1)))
python3 -I -c "print(f'CPU: {$b-$a} ticks over {$t1-$t0:.1f}s = {100*($b-$a)/$T/($t1-$t0):.3f}% of one core')"
echo "input events: $(grep -c 'event=Input' <<< "$slice")  commits: $(grep -c ' commit' <<< "$slice")  frames: $(grep -c 'frame_time_us' <<< "$slice")"
grep -E 'VmRSS|RssAnon|RssFile' "/proc/$SASH_PID/status"
grep -E '^Pss:' "/proc/$SASH_PID/smaps_rollup"
stop_all
