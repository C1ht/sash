#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# The fresh-clone step (owner, after M17 E1): clones the pushed main into a new directory
# and runs tools/check-all.sh there, so its counts come from the same runner output as
# check-all's own. The clone (and its build) is deleted afterwards.
# Usage: tools/fresh-clone.sh [repository URL or path]   (default: the origin remote)
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
URL=${1:-$(git -C "$ROOT" remote get-url origin)}
BASE=${SASH_FRESH_DIR:-$HOME/.cache/sash/fresh-clone}
mkdir -p "$BASE"
DIR=$(mktemp -d "$BASE/clone.XXXXXX")
trap 'rm -rf -- "$DIR"' EXIT
git clone -q --branch main "$URL" "$DIR/sash"
cloned=$(git -C "$DIR/sash" rev-parse HEAD)
echo "fresh clone of main at $cloned"
if [ "$cloned" != "$(git -C "$ROOT" rev-parse HEAD)" ]; then
    echo "note: the clone is not the local HEAD ($(git -C "$ROOT" rev-parse HEAD)); push first?"
fi
"$DIR/sash/tools/check-all.sh"
