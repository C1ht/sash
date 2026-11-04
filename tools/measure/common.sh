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

# Clients that can show device names or personal data: no content captures while one
# runs (CLAUDE.md, "Process and privacy rules"). Their PIDs are exported in
# SASH_NO_CAPTURE_PIDS; every capture helper is a no-op while one of them is alive.
NO_CAPTURE_APPS="pavucontrol nautilus thunar dolphin nemo pcmanfm caja"
# Clients that reach the network or report crashes: only through start_firefox.
BROWSERS="firefox firefox-esr chromium chrome google-chrome brave epiphany"
export SASH_NO_CAPTURE_PIDS=""

# start_client CMD...: runs a client on sash-wm with a throwaway XDG_CONFIG_HOME and
# records its PID. Refuses browsers (use start_firefox).
start_client() {
    local config name
    name=$(basename "$1")
    if [[ " $BROWSERS " == *" $name "* ]]; then
        echo "start_client: $name reaches the network; use start_firefox" >&2
        return 1
    fi
    config=$(mktemp -d)
    WAYLAND_DISPLAY=$SASH_SOCKET XDG_CONFIG_HOME=$config GDK_BACKEND=wayland "$@" \
        > /dev/null 2>&1 &
    PIDS+=("$!")
    if [[ " $NO_CAPTURE_APPS " == *" $name "* ]]; then
        SASH_NO_CAPTURE_PIDS="$SASH_NO_CAPTURE_PIDS $!"
    fi
}

# start_firefox URL [--network]: Firefox with a new empty profile (removed by
# stop_all), the crash reporter off, telemetry, updates and other background traffic
# off, and no network (every request goes to a proxy that does not exist) unless
# --network is given. Its output goes to FFLOG if set. Records its PID.
start_firefox() {
    local url=$1 network=${2:-} profile
    profile=$(mktemp -d)
    PROFILES+=("$profile")
    {
        echo 'user_pref("toolkit.telemetry.enabled", false);'
        echo 'user_pref("toolkit.telemetry.unified", false);'
        echo 'user_pref("datareporting.healthreport.uploadEnabled", false);'
        echo 'user_pref("datareporting.policy.dataSubmissionEnabled", false);'
        echo 'user_pref("app.update.enabled", false);'
        echo 'user_pref("app.update.auto", false);'
        echo 'user_pref("app.normandy.enabled", false);'
        echo 'user_pref("extensions.update.enabled", false);'
        echo 'user_pref("browser.crashReports.unsubmittedCheck.autoSubmit2", false);'
        echo 'user_pref("browser.safebrowsing.malware.enabled", false);'
        echo 'user_pref("browser.safebrowsing.phishing.enabled", false);'
        echo 'user_pref("browser.shell.checkDefaultBrowser", false);'
        echo 'user_pref("browser.aboutwelcome.enabled", false);'
        echo 'user_pref("browser.startup.homepage_override.mstone", "ignore");'
        echo 'user_pref("browser.newtabpage.enabled", false);'
        echo 'user_pref("network.captive-portal-service.enabled", false);'
        echo 'user_pref("network.connectivity-service.enabled", false);'
        if [ "$network" != --network ]; then
            echo 'user_pref("network.proxy.type", 1);'
            echo 'user_pref("network.proxy.http", "127.0.0.1");'
            echo 'user_pref("network.proxy.http_port", 9);'
            echo 'user_pref("network.proxy.ssl", "127.0.0.1");'
            echo 'user_pref("network.proxy.ssl_port", 9);'
            echo 'user_pref("network.proxy.socks", "127.0.0.1");'
            echo 'user_pref("network.proxy.socks_port", 9);'
            echo 'user_pref("network.proxy.allow_hijacking_localhost", true);'
            echo 'user_pref("network.proxy.no_proxies_on", "");'
        fi
    } > "$profile/user.js"
    WAYLAND_DISPLAY=$SASH_SOCKET XDG_CONFIG_HOME=$(mktemp -d) MOZ_ENABLE_WAYLAND=1 \
        MOZ_CRASHREPORTER_DISABLE=1 MOZ_CRASHREPORTER_NO_REPORT=1 \
        firefox --no-remote --profile "$profile" "$url" > "${FFLOG:-/dev/null}" 2>&1 &
    PIDS+=("$!")
}
PROFILES=()

# capture: an offscreen capture (SIGUSR1 to sash-wm), unless a no-capture client runs.
capture() {
    local p
    for p in $SASH_NO_CAPTURE_PIDS; do
        if kill -0 "$p" 2> /dev/null; then
            echo "capture skipped: a client that shows device names is running" >&2
            return 0
        fi
    done
    kill -USR1 "$SASH_PID"
}

# Stops everything this script started, newest first.
stop_all() {
    local i
    for ((i = ${#PIDS[@]} - 1; i >= 0; i--)); do
        kill "${PIDS[i]}" 2> /dev/null
        wait "${PIDS[i]}" 2> /dev/null
    done
    PIDS=()
    for i in "${PROFILES[@]}"; do rm -rf "$i"; done
    PROFILES=()
    SASH_NO_CAPTURE_PIDS=""
}
