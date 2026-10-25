#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Popup drag (M7): pavucontrol floating, mod+right-drag from its top-left cell for 5 s
# without and with its "Show:" dropdown open, then frame statistics. Offscreen captures
# go to CAPDIR. pavucontrol gets a throwaway config directory, and the script switches to
# its Recording tab, but its Playback tab lists the audio streams of the session: look at
# every capture and delete any that shows personal content.
# Usage: tools/measure/run-popup.sh LOG CAPDIR
set -u
source "$(dirname "$0")/common.sh"
LOG=$1
export SASH_CAPTURE_DIR
SASH_CAPTURE_DIR=$(realpath "$2")
start_sash "$LOG"
start_client pavucontrol
sleep 4
read -r SASH_W SASH_H < <(sed 's/\x1b\[[0-9;]*m//g' "$LOG" \
    | grep -oP ' resize w=\K\d+ h=\d+' | tail -1 | tr -d 'h=')
echo "pid=$SASH_PID socket=$SASH_SOCKET output=${SASH_W}x$SASH_H"
python3 -I "$ROOT/tools/measure/drive_popup.py" "$LOG" "$SASH_INPUT" "$SASH_W" "$SASH_H" "$SASH_PID" \
    | while read -r name start end motions; do
        echo "== $name ($motions motions)"
        python3 -I "$ROOT/tools/measure/analyze.py" "$LOG" "$start" "$end"
    done
stop_all
