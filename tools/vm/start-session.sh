#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
#
# Runs as root inside the test guest: starts sash-wm for the user "sash" in a real
# logind session on VT 2 (as a display manager would), and switches to VT 2.
# Usage: start-session.sh BINARY [ARGS...]
# No --backend: sash-wm chooses DRM itself (no display variables, a login session).
# Environment: XKB_DEFAULT_* (passed on), CHVT_DELAY (seconds before the VT switch).
set -e
BIN=$1
shift
systemctl stop sash-session 2>/dev/null || true
# A login prompt on VT 2 would take the terminal back (as a display manager does, the
# session gets the VT to itself).
# VT 2 is reserved for the session, as a display manager reserves its VT: no login
# prompt may start there (logind starts one on a VT without a session, and a getty
# hangs up the terminal, which ends sash-wm).
systemctl mask --runtime --quiet getty@tty2.service autovt@tty2.service
systemctl stop getty@tty2.service autovt@tty2.service 2>/dev/null || true
sleep 1
systemctl reset-failed sash-session 2>/dev/null || true
: > /tmp/sash.log
chmod 666 /tmp/sash.log
# Only XKB_DEFAULT_* variables that are set and not empty are passed on: xkbcommon
# takes an empty one at its word (an empty layout has no letters).
set -- --setenv=RUST_LOG="${RUST_LOG:-sash_wm=debug}" "$BIN" "$@"
for var in XKB_DEFAULT_RULES XKB_DEFAULT_MODEL XKB_DEFAULT_LAYOUT XKB_DEFAULT_VARIANT \
    XKB_DEFAULT_OPTIONS; do
    value=$(printenv "$var" || true)
    [ -n "$value" ] && set -- --setenv="$var=$value" "$@"
done
systemd-run --unit=sash-session --collect \
    -p PAMName=login -p User=sash -p TTYPath=/dev/tty2 -p StandardInput=tty-force \
    -p StandardOutput=append:/tmp/sash.log -p StandardError=append:/tmp/sash.log \
    "$@"
# Switch to VT 2 once logind has registered the session there (after CHVT_DELAY
# seconds more, to test a compositor that starts on a VT that is not in front).
sleep "${CHVT_DELAY:-0}"
for _ in $(seq 50); do
    loginctl list-sessions --no-legend | grep -q ' tty2 ' && break
    sleep 0.1
done
chvt 2
