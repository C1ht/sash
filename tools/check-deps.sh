#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Crate boundaries (owner, M17): sash-wm never depends on sash-ui as a normal or build
# dependency (a dev-dependency used by tests only is allowed); sash-ui never depends on
# sash-wm; sash-text is the only crate they share.
#
# CLIENT_ONLY lists the crates (name@version) only sash-ui's normal and build tree brings
# in, i.e. what sash-wm does not use already. None of them may be in sash-wm's normal
# and build tree. Every crate of sash-ui's tree must be either in sash-wm's tree or on
# the list, so a new sash-ui dependency fails here until it is classified.
# Run by tools/check-release.sh and tools/check-all.sh. Usage: tools/check-deps.sh

set -euo pipefail
cd "$(dirname "$0")/.."

CLIENT_ONLY=(
    sash-ui@v0.1.0
    smithay-client-toolkit@v0.21.1
    calloop-wayland-source@v0.4.1
    wayland-protocols-experimental@v20251230.0.3
    tiny-skia@v0.11.4
    tiny-skia-path@v0.11.4
    strict-num@v0.1.1
    arrayref@v0.3.9
    arrayvec@v0.7.8
)

tree() {
    cargo tree --locked -p "$1" -e "$2" --prefix none --format '{p}' | awk '{print $1 "@" $2}' | sort -u
}
wm=$(tree sash-wm normal,build)
ui=$(tree sash-ui normal,build)
ui_all=$(tree sash-ui normal,build,dev)

failed=0
for crate in "${CLIENT_ONLY[@]}"; do
    if grep -qxF -- "$crate" <<< "$wm"; then
        echo "FAIL: sash-wm's normal or build tree contains the client-only crate $crate"
        failed=1
    fi
done
while read -r crate; do
    if ! grep -qxF -- "$crate" <<< "$wm" && ! printf '%s\n' "${CLIENT_ONLY[@]}" | grep -qxF -- "$crate"; then
        echo "FAIL: $crate is in sash-ui's tree but neither in sash-wm's nor in CLIENT_ONLY (classify it)"
        failed=1
    fi
done <<< "$ui"
if grep -q '^sash-wm@' <<< "$ui_all"; then
    echo "FAIL: sash-ui depends on sash-wm"
    failed=1
fi
# Control: the dev-dependency is there, so the edge check above is not vacuous.
if ! tree sash-wm normal,build,dev | grep -q '^sash-ui@'; then
    echo "FAIL: control: sash-wm has no sash-ui dev-dependency, the check is stale"
    failed=1
fi
# The shipped binary (the trixie build) is built with -p sash-wm, the configuration this
# check covers: a workspace build would unify features with sash-ui's.
if grep -E '^cargo build' tools/vm/build-iso.sh | grep -vq -- '-p sash-wm'; then
    echo "FAIL: tools/vm/build-iso.sh has a cargo build without -p sash-wm"
    failed=1
fi
if [ "$failed" -ne 0 ]; then
    exit 1
fi
echo "ok: sash-wm's normal and build tree holds no sash-ui and none of its ${#CLIENT_ONLY[@]} client-only crates; sash-ui does not depend on sash-wm"
