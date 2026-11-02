#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Replays a debug-recording bundle (docs/DEBUG.md) in the headless test harness and
# compares its layout snapshots with the recorded ones. Runs nothing on the display.
#
# Usage: tools/replay.sh <bundle directory>
set -euo pipefail
bundle=$(realpath "${1:?usage: tools/replay.sh <bundle directory>}")
[ -f "$bundle/events.jsonl" ] || { echo "replay: $bundle has no events.jsonl" >&2; exit 2; }
cd "$(dirname "$0")/.."
SASH_REPLAY_BUNDLE="$bundle" cargo test -q -p sash-wm --features debug-record \
    replay_a_bundle -- --ignored --nocapture 2>&1 | grep -v "^$"
