#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# What real clients do with the xdg activated state (owner, M19a). Starts the ignored
# harness test real_clients_and_the_activated_state (headless: nothing on any display),
# then foot (showing nothing but `sleep`) and Firefox on about:blank (start_firefox:
# throwaway profile, no network, software rendering into wl_shm) on its socket. The
# test writes PNGs of every phase to OUT_DIR; look at each before keeping any.
# Usage: tools/measure/run-activated.sh OUT_DIR
set -u
source "$(dirname "$0")/common.sh"
out=${1:?usage: run-activated.sh OUT_DIR}
mkdir -p "$out"
socket_file="$out/socket"
rm -f "$socket_file"

SASH_REAL_CLIENTS_OUT=$out SASH_REAL_CLIENTS_SOCKET=$socket_file \
    cargo test --manifest-path "$ROOT/Cargo.toml" --release -q -p sash-wm \
    real_clients_and_the_activated_state -- --ignored --nocapture > "$out/test.log" 2>&1 &
PIDS+=("$!")
test_pid=$!
for _ in $(seq 600); do
    [ -s "$socket_file" ] && break
    sleep 0.2
done
SASH_SOCKET=$(cat "$socket_file")
echo "harness socket: $SASH_SOCKET"

start_client foot sleep 120
sleep 2
FIREFOX_PREFS='user_pref("gfx.webrender.software", true);
user_pref("widget.dmabuf.force-enabled", false);
user_pref("gfx.webrender.compositor", false);' FFLOG="$out/firefox.log" \
    start_firefox about:blank

wait "$test_pid"
status=$?
stop_all
cat "$out/test.log"
exit "$status"
