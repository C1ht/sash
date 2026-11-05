#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Fetches the pinned similarity reference into DEST (DEST/smallvil, DEST/anvil): Smithay
# v0.7.0 from GitHub, checked against the pinned commit and source hash (pinned.py).
# Usage: tools/similarity/reference.sh DEST PYTHON   (PYTHON: the venv with copydetect)
set -euo pipefail
dest=$1
python=$2
here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
tag=$("$python" -I -c "import sys; sys.path.insert(0, '$here'); import pinned; print(pinned.SMITHAY_TAG)")
commit=$("$python" -I -c "import sys; sys.path.insert(0, '$here'); import pinned; print(pinned.SMITHAY_COMMIT)")
git clone -q --depth 1 --branch "$tag" https://github.com/Smithay/smithay.git "$tmp/smithay" 2>/dev/null
got=$(git -C "$tmp/smithay" rev-parse HEAD)
if [ "$got" != "$commit" ]; then
    echo "FAIL: $tag is $got, pinned $commit" >&2
    exit 1
fi
mkdir -p "$dest"
rm -rf "$dest/smallvil" "$dest/anvil"
cp -r "$tmp/smithay/smallvil" "$tmp/smithay/anvil" "$dest/"
"$python" -I -c "import sys; sys.path.insert(0, '$here'); import pinned; print(pinned.setup('$dest'))"
