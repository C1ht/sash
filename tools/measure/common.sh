#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Shared helpers for the measurement scripts; source it, do not run it.
#
# Processes are only ever stopped by the PIDs these helpers recorded when they started
# them (never by name). Real clients get a throwaway config directory.

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
# The measurement build: optimized, with debug assertions on, so the debug-only
# synthetic input and offscreen capture exist. See `build_measure`.
MEASURE_BIN=${SASH_BIN:-"$ROOT/target/measure/release/sash-wm"}
PIDS=()

build_measure() {
    cargo build --manifest-path "$ROOT/Cargo.toml" --release -p sash-wm \
        --config 'profile.release.debug-assertions=true' --target-dir "$ROOT/target/measure"
}

# start_sash LOG [BIN]: starts sash-wm (nested) logging to LOG and sets SASH_PID,
# SASH_SOCKET (Wayland socket name), SASH_INPUT (synthetic input socket, debug
# builds) and SASH_W/SASH_H (output size once the host has sized the window).
start_sash() {
    local log=$1 bin=${2:-$MEASURE_BIN}
    : > "$log"
    RUST_LOG=${RUST_LOG:-sash_wm=debug} "$bin" > "$log" 2>&1 &
    SASH_PID=$!
    PIDS+=("$SASH_PID")
    SASH_SOCKET=
    for _ in $(seq 100); do
        SASH_SOCKET=$(sed 's/\x1b\[[0-9;]*m//g' "$log" | grep -oP 'socket="\K[^"]+' | head -1)
        [ -n "$SASH_SOCKET" ] && break
        sleep 0.05
    done
    sleep 1
    SASH_INPUT=$(sed 's/\x1b\[[0-9;]*m//g' "$log" | grep -oP 'synthetic input.*?path=\K\S+' | head -1)
    read -r SASH_W SASH_H < <(sed 's/\x1b\[[0-9;]*m//g' "$log" \
        | grep -oP ' resize w=\K\d+ h=\d+' | tail -1 | tr -d 'h=')
}

# start_client CMD...: runs a client on sash-wm with a throwaway XDG_CONFIG_HOME and
# records its PID.
start_client() {
    local config
    config=$(mktemp -d)
    WAYLAND_DISPLAY=$SASH_SOCKET XDG_CONFIG_HOME=$config GDK_BACKEND=wayland "$@" \
        > /dev/null 2>&1 &
    PIDS+=("$!")
}

# Stops everything this script started, newest first.
stop_all() {
    local i
    for ((i = ${#PIDS[@]} - 1; i >= 0; i--)); do
        kill "${PIDS[i]}" 2> /dev/null
        wait "${PIDS[i]}" 2> /dev/null
    done
    PIDS=()
}
