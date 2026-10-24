#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Fails if debug-only code (offscreen capture, synthetic input) is in the release binary.
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

# Control: the same markers must be found in a debug build, or the check proves nothing.
cargo build --locked -p sash-wm
for marker in SASH_CAPTURE_DIR sash-wm-input-; do
    if ! grep -aqF -- "$marker" target/debug/sash-wm; then
        echo "FAIL: control: debug binary lacks $marker, the check is stale"
        exit 1
    fi
done
echo "ok: release binary has no debug-only capture or synthetic input code"
