#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Every local check before a tag (owner, after M15): fmt; clippy and tests in the three
# configurations (debug, release, the debug-record feature); cargo deny; the crate
# boundaries (tools/check-deps.sh, owner M17); the release
# check (with the glibc floor of the trixie build); check-clients; similarity against the
# pinned references. Ends with a summary: passed, skipped and failed per step. Exit 1 if
# any step failed. Not included (they need the VM or the trixie container):
# tools/vm/build-iso.sh, tools/vm/scenario.py, tools/text-goldens.sh.
#
# Similarity needs the pinned reference and copydetect (tools/similarity/requirements.txt):
#   python3 -m venv ~/.cache/sash/similarity/venv
#   ~/.cache/sash/similarity/venv/bin/pip install -r tools/similarity/requirements.txt
#   tools/similarity/reference.sh ~/.cache/sash/similarity/ref ~/.cache/sash/similarity/venv/bin/python
# Without them the step is skipped (and says so).
set -uo pipefail
cd "$(dirname "$0")/.."
SIM=${SASH_SIMILARITY_DIR:-$HOME/.cache/sash/similarity}
LOG=$(mktemp -d)
trap 'rm -rf -- "$LOG"' EXIT
summary=()

# step NAME COMMAND...: runs it, keeps its output, records passed or failed.
step() {
    local name=$1
    shift
    echo "== $name"
    if "$@" > "$LOG/out" 2>&1; then
        summary+=("passed   $name"$'\n'"$(detail)")
    else
        summary+=("FAILED   $name"$'\n'"$(detail)")
        tail -30 "$LOG/out"
    fi
}

# Counts from cargo test output: "(N passed, M ignored, K skipped)".
detail() {
    local passed ignored skipped
    passed=$(grep -oE '[0-9]+ passed' "$LOG/out" | awk '{s+=$1} END {print s+0}')
    ignored=$(grep -oE '[0-9]+ ignored' "$LOG/out" | awk '{s+=$1} END {print s+0}')
    skipped=$(grep -oE '[0-9]+ skipped' "$LOG/out" | awk '{s+=$1} END {print s+0}')
    if [ "$passed$ignored$skipped" != "000" ]; then
        echo "           $passed passed, $ignored ignored, $skipped skipped"
        grep -E '^  skipped: ' "$LOG/out" | sed 's/^/           /' | sort -u
    fi
    grep -E '^SKIPPED' "$LOG/out" | sed 's/^/           /'
    grep -E '^glibc floor' "$LOG/out" | sed 's/^/           /'
}

step "fmt" cargo fmt --all --check
for config in "" "--release" "--features sash-wm/debug-record"; do
    label=${config:-debug}
    step "clippy $label" cargo clippy --locked --workspace $config --all-targets -- -D warnings
    step "test $label" cargo test --locked --workspace $config
done
step "cargo deny" cargo deny check
step "crate boundaries" tools/check-deps.sh
step "release check" tools/check-release.sh
step "check-clients" tools/check-clients.sh
if [ -x "$SIM/venv/bin/python" ] && [ -d "$SIM/ref/smallvil" ]; then
    echo "== similarity"
    if "$SIM/venv/bin/python" -I tools/similarity/summary.py sash-wm/src "$SIM/ref" 25 30 6 \
        > "$LOG/sim" 2>&1; then
        high=$(awk '$1 ~ /\.rs$/ {for (i = 2; i <= NF; i++) if ($i ~ /%$/) { v = $i; sub("%", "", v); if (v + 0 > 40) { print $1 " " $i; break } }}' "$LOG/sim")
        summary+=("passed   similarity (pinned reference verified; above 40%: ${high:-none})")
        head -2 "$LOG/sim"
    else
        summary+=("FAILED   similarity")
        tail -5 "$LOG/sim"
    fi
else
    summary+=("SKIPPED  similarity: no pinned reference in $SIM (see the header of this script)")
fi

echo
echo "== summary"
failed=0
for line in "${summary[@]}"; do
    printf '%s\n' "$line" | grep -v '^$'
    case $line in FAILED*) failed=1 ;; esac
done
exit $failed
