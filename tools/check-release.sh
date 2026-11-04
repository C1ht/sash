#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Fails if debug-only code (offscreen capture, synthetic input, the --mode flag, mode
# fault injection, debug recording) is in the release binary, or if a release build
# with the debug-record feature compiles at all.
# Builds the release binary first. Usage: tools/check-release.sh

set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release --locked -p sash-wm
bin=target/release/sash-wm

# Strings and symbol paths only the debug-only modules contain.
markers=(
    SASH_CAPTURE_DIR
    sash-wm-capture-
    sash-wm-input-
    "sash_wm::capture"
    "sash_wm::synthetic"
    "synthetic input"
    "--mode takes WxH@R"
    "--mode: only the requested mode is tried"
    "[--mode WxH@R]"
    SASH_DEBUG_REJECT_MODES
    SASH_DEBUG_REJECT_ALL_MODES
    "fault injection"
    SASH_DEBUG_RECORD
    SASH_DEBUG_KEYS
    "DEBUG RECORDING"
    "sash_wm::record"
    "debug bundle"
    # M11a: the debug-only cursor theme override (XCURSOR_THEME and XCURSOR_SIZE
    # themselves are in every build: winit's client-side cursor code reads them) and
    # the renderer fault injection.
    "cursor theme override"
    SASH_DEBUG_FAIL_SWAPS
    SASH_DEBUG_CONTEXT_LOST
    SASH_DEBUG_STOP_FLIPS
)
found=0
for marker in "${markers[@]}"; do
    if grep -aqF -- "$marker" "$bin"; then
        echo "FAIL: release binary contains debug-only marker: $marker"
        found=1
    fi
done
if [ "$found" -ne 0 ]; then
    exit 1
fi

# The release binary refuses --mode as an unknown argument (argument parsing comes before
# anything else: nothing is opened).
refused=$("$bin" --mode 1x1@1 2>&1 || true)
if ! grep -qF "unknown argument --mode" <<< "$refused"; then
    echo "FAIL: release binary accepts --mode"
    exit 1
fi

# Debug recording cannot be built into a release binary: the build must fail.
if cargo build --release --locked -p sash-wm --features debug-record > /dev/null 2>&1; then
    echo "FAIL: a release build with --features debug-record compiled"
    exit 1
fi

# Control: the same markers must be found in a debug build, or the check proves nothing.
cargo build --locked -p sash-wm
for marker in SASH_CAPTURE_DIR sash-wm-input- "--mode takes WxH@R" \
    "--mode: only the requested mode is tried" SASH_DEBUG_REJECT_MODES \
    SASH_DEBUG_REJECT_ALL_MODES "fault injection"; do
    if ! grep -aqF -- "$marker" target/debug/sash-wm; then
        echo "FAIL: control: debug binary lacks $marker, the check is stale"
        exit 1
    fi
done
# Control for the recorder: its markers are in a debug build with the feature.
cargo build --locked -p sash-wm --features debug-record
for marker in SASH_DEBUG_RECORD SASH_DEBUG_KEYS "DEBUG RECORDING" "sash_wm::record" "debug bundle"; do
    if ! grep -aqF -- "$marker" target/debug/sash-wm; then
        echo "FAIL: control: the debug-record build lacks $marker, the check is stale"
        exit 1
    fi
done
# Leave the default debug build in target/debug.
cargo build --locked -p sash-wm

echo "ok: release binary has no debug-only capture, synthetic input, --mode, fault injection or debug recording code"
