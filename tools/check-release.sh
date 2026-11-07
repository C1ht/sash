#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Fails if debug-only code (offscreen capture, synthetic input, the --mode flag, mode
# fault injection, debug recording) is in the release binary, or if a release build
# with the debug-record feature compiles at all.
# Builds the release binary first. Usage: tools/check-release.sh

set -euo pipefail
cd "$(dirname "$0")/.."

# Crate boundaries (owner, M17): no sash-ui in sash-wm's normal or build tree.
tools/check-deps.sh

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
    # M13a: the stand-in privileged global exists in test builds only.
    "privileged stand-in global (tests only)"
    # M15: the title size override exists in debug builds only.
    SASH_DEBUG_FONT_PX
    # M18a: the gap and corner radius overrides exist in debug builds only (M15b's
    # SASH_DEBUG_GAP_PX is removed; it must not come back in any build).
    SASH_DEBUG_GAP_PX
    SASH_DEBUG_OUTER_GAP_PX
    SASH_DEBUG_INNER_GAP_PX
    SASH_DEBUG_RADIUS_PX
    # M13b: the layer-shell test client exists in test builds only.
    "layer-shell test client (tests only)"
    # M17: sash-ui is a test-only dev-dependency: neither its code nor the test panel.
    "sash-ui test panel (tests only)"
    "sash_ui::"
    "the compositor offers no wl_shm ABGR8888"
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
    SASH_DEBUG_REJECT_ALL_MODES "fault injection" SASH_DEBUG_FONT_PX \
    SASH_DEBUG_OUTER_GAP_PX SASH_DEBUG_INNER_GAP_PX SASH_DEBUG_RADIUS_PX; do
    if ! grep -aqF -- "$marker" target/debug/sash-wm; then
        echo "FAIL: control: debug binary lacks $marker, the check is stale"
        exit 1
    fi
done
# Control for the stand-in privileged global: it is in the test binary, and in no
# sash-wm binary (not even a debug one).
test_bin=$(cargo test --locked -p sash-wm --no-run --message-format=json 2> /dev/null |
    python3 -I -c 'import sys, json
for line in sys.stdin:
    m = json.loads(line)
    if m.get("reason") == "compiler-artifact" and m.get("profile", {}).get("test") and m.get("executable"):
        print(m["executable"])' | head -1)
if ! grep -aqF -- "privileged stand-in global (tests only)" "$test_bin"; then
    echo "FAIL: control: the test binary lacks the stand-in marker, the check is stale"
    exit 1
fi
if grep -aqF -- "privileged stand-in global (tests only)" target/debug/sash-wm; then
    echo "FAIL: the debug sash-wm binary contains the test-only stand-in global"
    exit 1
fi
# Control for the layer-shell test client and sash-ui's test panel (M17): their markers
# are in the test binary, and in no sash-wm binary.
for marker in "layer-shell test client (tests only)" "sash-ui test panel (tests only)" \
    "sash_ui::" "the compositor offers no wl_shm ABGR8888"; do
    if ! grep -aqF -- "$marker" "$test_bin"; then
        echo "FAIL: control: the test binary lacks $marker, the check is stale"
        exit 1
    fi
    if grep -aqF -- "$marker" target/debug/sash-wm; then
        echo "FAIL: the debug sash-wm binary contains test-only code ($marker)"
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

# glibc floor (owner, after M15): the shipped binary is the one built in the
# debian:trixie container (tools/vm/build-iso.sh puts it in the VM's ISO); its floor may
# not pass GLIBC_2.41. The host's release build is reported only (a newer host glibc
# versions some symbols newer). SASH_TRIXIE_RELEASE=<path> names the trixie binary;
# by default it is taken from ~/.cache/sash/vm/sash.iso.
floor() { objdump -T "$1" | grep -o 'GLIBC_[0-9.]*' | sort -Vu | tail -1; }
echo "glibc floor, host release build (reported only): $(floor "$bin")"
trixie=${SASH_TRIXIE_RELEASE:-}
iso=$HOME/.cache/sash/vm/sash.iso
if [ -z "$trixie" ] && [ -f "$iso" ] && command -v xorriso > /dev/null; then
    extracted=$(mktemp -d)
    trap 'rm -rf -- "$extracted"' EXIT
    if xorriso -osirrox on -indev "$iso" -extract /sash-wm-release "$extracted/sash-wm" \
        > /dev/null 2>&1; then
        trixie=$extracted/sash-wm
        echo "trixie release build: from $iso ($(date -r "$iso" '+%Y-%m-%d %H:%M'))"
    fi
fi
if [ -z "$trixie" ]; then
    echo "SKIPPED: glibc floor of the trixie build (no build: run tools/vm/build-iso.sh)"
else
    trixie_floor=$(floor "$trixie")
    echo "glibc floor, trixie release build: $trixie_floor (limit GLIBC_2.41)"
    if [ "$(printf '%s\nGLIBC_2.41\n' "$trixie_floor" | sort -V | tail -1)" != GLIBC_2.41 ]; then
        echo "FAIL: the trixie release build needs $trixie_floor, above GLIBC_2.41"
        exit 1
    fi
fi

echo "ok: release binary has no debug-only capture, synthetic input, --mode, fault injection, title size and gap overrides or debug recording code; the test-only privileged stand-in, layer-shell test client and sash-ui test panel are in tests only"
