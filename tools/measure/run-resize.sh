#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Stale content during resizes: one client, floated, resized by keyboard N times; prints
# the time from each configure to the client's first buffer at the new size
# ("configure to commit"). The client gets a throwaway config directory; for Firefox
# pass its own throwaway profile in the command.
# Usage: tools/measure/run-resize.sh LOG N CLIENT [ARGS...]
set -u
source "$(dirname "$0")/common.sh"
LOG=$1 N=$2
shift 2
start_sash "$LOG"
start_client "$@"
sleep "${CLIENT_WAIT:-5}"
start=$(wc -l < "$LOG")
python3 -I "$ROOT/tools/measure/drive_resize.py" "$SASH_INPUT" "$N" 0.1
sleep 1
python3 -I "$ROOT/tools/measure/analyze.py" "$LOG" "$start" "$(wc -l < "$LOG")" | grep -E 'configure to commit'
echo "configures answered: $(tail -n +"$start" "$LOG" | grep -c 'caught up') of $N"
stop_all
