#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Runs sash-text's tests, and sash-wm's window-title tests (with the bar goldens that
# need the fallback fonts, ignored elsewhere), in a debian:trixie container with the
# trixie fallback fonts
# (fonts-dejavu-core, fonts-noto-core, fonts-noto-cjk, fonts-noto-color-emoji; about
# 81 MB downloaded inside the container), so the fallback goldens (CJK, emoji) run
# instead of being skipped (owner, after M14 step 1). The container's Rust toolchain and
# build directory are kept in ~/.cache/sash/text.
#
# Usage: tools/text-goldens.sh [--bless] [--report DIR] [--long SECONDS] [--bench]
#   --bless        write the golden images instead of comparing (look at each one)
#   --report DIR   write the Arabic, Devanagari and Hebrew report images to DIR
#   --long S       also run the long proptest for S seconds (release build)
#   --bench        also run the timing and memory measurement (release build)
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
CACHE=$HOME/.cache/sash/text
mkdir -p "$CACHE"
bless="" report="" long="" bench=""
while [ $# -gt 0 ]; do
    case $1 in
        --bless) bless=1 ;;
        --report) report=$(realpath "$2"); mkdir -p "$report"; shift ;;
        --long) long=$2; shift ;;
        --bench) bench=1 ;;
        *) echo "unknown argument $1" >&2; exit 2 ;;
    esac
    shift
done
SRC=$(mktemp -d)
trap 'rm -rf -- "$SRC"' EXIT
cp -r "$ROOT"/Cargo.toml "$ROOT"/Cargo.lock "$ROOT"/rust-toolchain.toml "$ROOT"/sash-wm \
    "$ROOT"/sash-ui "$ROOT"/sash-text "$SRC"/
mounts=(-v "$SRC":/src:Z -v "$CACHE":/cache:Z)
[ -n "$report" ] && mounts+=(-v "$report":/report:Z)
podman run --rm "${mounts[@]}" -e BLESS="$bless" -e LONG="$long" -e BENCH="$bench" \
    -e REPORT="${report:+/report}" -e CARGO_HOME=/cache/cargo -e RUSTUP_HOME=/cache/rustup \
    -e CARGO_TARGET_DIR=/cache/target docker.io/library/debian:trixie bash -c '
set -e
apt-get update -qq > /dev/null
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends build-essential \
    curl ca-certificates fonts-dejavu-core fonts-noto-core fonts-noto-cjk \
    fonts-noto-color-emoji pkg-config libxkbcommon-dev libwayland-client0 libseat-dev \
    libinput-dev libudev-dev libgbm-dev > /dev/null 2>&1
[ -x /cache/cargo/bin/cargo ] || curl -sSf https://sh.rustup.rs |
    sh -s -- -y -q --default-toolchain none --profile minimal --no-modify-path > /dev/null 2>&1
export PATH=/cache/cargo/bin:$PATH
cd /src
env ${BLESS:+SASH_TEXT_BLESS=1} ${REPORT:+SASH_TEXT_REPORT=$REPORT} \
    cargo test --locked -p sash-text -- --nocapture 2>&1 | grep -E "^test |test result|^golden|skipped|report |wrote|panicked|FAILED" || true
env ${BLESS:+SASH_TEXT_BLESS=1} cargo test --locked -p sash-wm titles::bars_ -- --include-ignored 2>&1 |
    grep -E "^test |test result|wrote|panicked|FAILED" || true
if [ -n "$BLESS" ]; then
    mkdir -p /cache/blessed /cache/blessed-titles
    cp sash-text/tests/golden/*.png /cache/blessed/
    cp sash-wm/src/tests/fixtures/titles/*.png /cache/blessed-titles/
fi
if [ -n "$LONG" ]; then
    SASH_TEXT_LONG_SECONDS=$LONG cargo test --locked --release -p sash-text --test text \
        -- --ignored --nocapture long_run 2>&1 | grep -E "long run|test result|panicked"
fi
if [ -n "$BENCH" ]; then
    cargo test --locked --release -p sash-text --test bench -- --ignored --nocapture 2>&1 |
        grep -vE "^\s*$|Running|Compiling|Finished"
fi
' 2>&1 | grep -v '^info:\|^warn:'
if [ -n "$bless" ]; then
    cp "$CACHE"/blessed/*.png "$ROOT"/sash-text/tests/golden/
    cp "$CACHE"/blessed-titles/*.png "$ROOT"/sash-wm/src/tests/fixtures/titles/
    echo "blessed goldens copied to sash-text/tests/golden/ (look at each before committing)"
fi
