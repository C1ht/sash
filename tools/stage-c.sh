#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# M9 stage C: one run of sash-wm on the real display, started by the owner from a text
# console (never by Claude). See docs/m9-stage-c.md for the runs and what to check.
#
# - Prints the escape instructions first and waits for Enter.
# - Runs the debug build with --exit-after SECONDS, inside `timeout` (SIGTERM 5 s after
#   that, SIGKILL 5 s after the SIGTERM).
# - Writes everything sash-wm logs to ${XDG_CACHE_HOME:-~/.cache}/sash/stage-c/
#   <timestamp>-<name>.log, fsynced line by line, so a hang or a power cycle keeps it.
# - Records sash-wm's PID for the escape steps.
# - Optionally starts one client once sash-wm is listening (everything after `--`), with
#   a throwaway XDG_CONFIG_HOME; its output goes to the same log.
#
# Usage: tools/stage-c.sh SECONDS NAME [-- CLIENT ARGS...]
#   e.g. tools/stage-c.sh 20 run1
#        tools/stage-c.sh 120 run3 -- kitty --config NONE
# Needs no sudo.
set -u

ROOT=$(cd "$(dirname "$0")/.." && pwd)
# STAGE_C_BIN: another binary (the VM test of this script uses it).
BIN="${STAGE_C_BIN:-$ROOT/target/debug/sash-wm}"
SECONDS_ARG=${1:-}
NAME=${2:-}
shift 2 2>/dev/null || true
CLIENT=()
if [ "${1:-}" = "--" ]; then
    shift
    CLIENT=("$@")
fi

fail() {
    echo "stage-c: $*" >&2
    exit 2
}
case "$SECONDS_ARG" in
    '' | *[!0-9]*) fail "usage: tools/stage-c.sh SECONDS NAME [-- CLIENT ARGS...]" ;;
esac
[ "$SECONDS_ARG" -ge 5 ] && [ "$SECONDS_ARG" -le 600 ] || fail "SECONDS must be 5..600"
case "$NAME" in
    '' | *[!A-Za-z0-9_-]*) fail "NAME: letters, digits, - and _ only (e.g. run1)" ;;
esac
[ -z "${WAYLAND_DISPLAY:-}" ] && [ -z "${DISPLAY:-}" ] ||
    fail "WAYLAND_DISPLAY or DISPLAY is set: run this on a text console (Ctrl+Alt+F3), not inside Hyprland"
case "$(tty)" in
    /dev/tty[0-9]*) ;;
    *) fail "not on a text console ($(tty)); log in on one with Ctrl+Alt+F3" ;;
esac
[ -x "$BIN" ] || fail "$BIN is missing; build it first (in Hyprland): cargo build -p sash-wm"
command -v python3 > /dev/null || fail "python3 is needed for the fsynced log"

DIR="${XDG_CACHE_HOME:-$HOME/.cache}/sash/stage-c"
mkdir -p "$DIR" || fail "cannot create $DIR"
LOG="$DIR/$(date +%Y%m%d-%H%M%S)-$NAME.log"
PIDFILE="$DIR/sash-wm.pid"
FIFO="$DIR/.log-fifo-$$"
TTY=$(tty)

cat << EOF

=== sash-wm stage C: $NAME, $SECONDS_ARG seconds ===

The screen will belong to sash-wm. It quits by itself after $SECONDS_ARG s; if it does not,
SIGTERM follows 5 s later and SIGKILL 5 s after that. If the screen stays black or frozen,
in this order, waiting a few seconds between the steps:

  1. Wait $((SECONDS_ARG + 10)) seconds (the two watchdogs).
  2. Super+Shift+E (quit), or Ctrl+Alt+F1 (Hyprland) / Ctrl+Alt+F3.
  3. Ctrl+Alt+F4, log in, then:   kill \$(cat $PIDFILE)
     still there after 5 s:       kill -9 \$(cat $PIDFILE)
     When sash-wm is gone, logind gives the display back: Ctrl+Alt+F3 or F1.
  4. Only if SysRq was widened (docs/m9-stage-c.md): Alt+SysRq+R, then Ctrl+Alt+F1.
  5. Last resort: a short press of the power button.

Log (fsynced): $LOG
EOF
if [ ${#CLIENT[@]} -gt 0 ]; then
    echo "Client once sash-wm listens: ${CLIENT[*]}"
fi
echo
read -r -p "Press Enter to start (Ctrl+C cancels) " _

# The log writer: every line written and fsynced at once.
mkfifo -m 600 "$FIFO" || fail "cannot create $FIFO"
python3 -I -c '
import os, sys
out = open(sys.argv[1], "ab", buffering=0)
fd = out.fileno()
with open(sys.argv[2], "rb", buffering=0) as fifo:
    for line in fifo:
        out.write(line)
        os.fsync(fd)
' "$LOG" "$FIFO" &
WRITER=$!
exec 3> "$FIFO"
rm -f "$FIFO"
{
    echo "== stage C $NAME: $(date -Is), $SECONDS_ARG s, $TTY"
    echo "== $(uname -r); sash $(git -C "$ROOT" describe --always --dirty 2> /dev/null)"
    echo "== client: ${CLIENT[*]:-none}"
} >&3

RUST_LOG=${RUST_LOG:-sash_wm=debug} timeout -k 5 -s TERM $((SECONDS_ARG + 5)) \
    "$BIN" --backend drm --exit-after "$SECONDS_ARG" >&3 2>&3 &
TIMEOUT_PID=$!
# sash-wm is the child of timeout: its PID is the one to signal.
SASH_PID=""
for _ in $(seq 50); do
    SASH_PID=$(ps -o pid= --ppid "$TIMEOUT_PID" | tr -d ' ')
    [ -n "$SASH_PID" ] && break
    sleep 0.1
done
echo "${SASH_PID:-$TIMEOUT_PID}" > "$PIDFILE"
echo "== sash-wm pid ${SASH_PID:-unknown}, timeout pid $TIMEOUT_PID" >&3

CLIENT_PID=""
if [ ${#CLIENT[@]} -gt 0 ]; then
    SOCKET=""
    for _ in $(seq 100); do
        SOCKET=$(sed 's/\x1b\[[0-9;]*m//g' "$LOG" | grep -o 'socket="[^"]*"' | head -1 |
            cut -d'"' -f2)
        [ -n "$SOCKET" ] && break
        kill -0 "$TIMEOUT_PID" 2> /dev/null || break
        sleep 0.1
    done
    if [ -n "$SOCKET" ]; then
        CONFIG=$(mktemp -d)
        echo "== starting the client on $SOCKET (XDG_CONFIG_HOME=$CONFIG)" >&3
        WAYLAND_DISPLAY="$SOCKET" XDG_CONFIG_HOME="$CONFIG" "${CLIENT[@]}" >&3 2>&3 &
        CLIENT_PID=$!
    else
        echo "== no socket found in the log; the client is not started" >&3
    fi
fi

wait "$TIMEOUT_PID"
STATUS=$?
if [ -n "$CLIENT_PID" ]; then
    # The client lost its compositor; give it a moment, then end it (our own PID).
    sleep 1
    kill "$CLIENT_PID" 2> /dev/null
    wait "$CLIENT_PID" 2> /dev/null
    rm -rf -- "$CONFIG"
fi
case $STATUS in
    0) MEANING="sash-wm quit by itself (Super+Shift+E or --exit-after)" ;;
    124) MEANING="sash-wm did not quit in time: timeout sent SIGTERM" ;;
    137) MEANING="sash-wm had to be killed (SIGKILL)" ;;
    *) MEANING="sash-wm exited with status $STATUS" ;;
esac
echo "== exit status $STATUS: $MEANING" >&3
exec 3>&-
wait "$WRITER"
rm -f "$PIDFILE"
sync "$LOG" 2> /dev/null

echo
echo "Exit status $STATUS: $MEANING"
echo "Log: $LOG ($(wc -l < "$LOG") lines)"
