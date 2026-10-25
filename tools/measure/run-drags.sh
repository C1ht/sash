#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Scripted drags on three foot windows (M6): a tiled resize, a floating drag and a snap
# drag with the preview, 5 s each through the synthetic input, then frame statistics.
# Needs a running Wayland session (sash-wm runs nested). Build first with
# `source tools/measure/common.sh && build_measure`.
# Usage: tools/measure/run-drags.sh LOG
set -u
source "$(dirname "$0")/common.sh"
LOG=$1
start_sash "$LOG"
for n in 1 2 3; do
    start_client foot -e sh -c "printf '\\n  foot %s\\n' $n; sleep 600"
    sleep 1.2
done
sleep 2
read -r SASH_W SASH_H < <(sed 's/\x1b\[[0-9;]*m//g' "$LOG" \
    | grep -oP ' resize w=\K\d+ h=\d+' | tail -1 | tr -d 'h=')
echo "pid=$SASH_PID socket=$SASH_SOCKET output=${SASH_W}x$SASH_H"
python3 -I "$ROOT/tools/measure/drive_drags.py" "$LOG" "$SASH_INPUT" "$SASH_W" "$SASH_H" \
    | while read -r name start end motions; do
        echo "== $name ($motions motions)"
        python3 -I "$ROOT/tools/measure/analyze.py" "$LOG" "$start" "$end"
    done
stop_all
