#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Builds sash-wm (release, debug, and debug with the debug-record feature) in a
# debian:trixie container from the working tree's
# sources, and an ISO with both binaries and the guest scripts for the test VM.
# Usage: tools/vm/build-iso.sh OUTDIR   (writes OUTDIR/sash.iso)
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
OUT=$(realpath "$1")
SRC=$(mktemp -d)
ISO_DIR=$(mktemp -d)
cp -r "$ROOT"/Cargo.toml "$ROOT"/Cargo.lock "$ROOT"/rust-toolchain.toml "$ROOT"/sash-wm \
    "$ROOT"/sash-ui "$ROOT"/sash-text "$ROOT"/sash-theme "$SRC"/
podman run --rm -v "$SRC":/src:ro,Z -v "$ISO_DIR":/out:Z docker.io/library/debian:trixie bash -c '
set -e
apt-get update -qq
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends build-essential \
    pkg-config curl ca-certificates libxkbcommon-dev libwayland-client0 libseat-dev \
    libinput-dev libudev-dev libgbm-dev > /dev/null
curl -sSf https://sh.rustup.rs | sh -s -- -y -q --default-toolchain none --profile minimal \
    > /dev/null 2>&1
. "$HOME/.cargo/env"
cp -r /src /work && cd /work
# -p sash-wm: a workspace build would unify features with those of sash-ui (M17 E2).
cargo build --locked --release -p sash-wm 2>&1 | tail -1
cargo build --locked -p sash-wm 2>&1 | tail -1
cp target/release/sash-wm /out/sash-wm-release
cp target/debug/sash-wm /out/sash-wm-debug
cargo build --locked -p sash-wm --features debug-record 2>&1 | tail -1
cp target/debug/sash-wm /out/sash-wm-debug-record
' 2>&1 | grep -v '^info:\|^warn:'
cp "$ROOT"/tools/vm/*.sh "$ROOT"/tools/stage-c.sh "$ROOT"/tools/drm-probe.py "$ISO_DIR"/
xorriso -as mkisofs -quiet -R -J -V SASH -o "$OUT/sash.iso" "$ISO_DIR"
rm -rf -- "$SRC" "$ISO_DIR"
echo "wrote $OUT/sash.iso"
