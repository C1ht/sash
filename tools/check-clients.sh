#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Static check of tools/ for the client rules (CLAUDE.md, "Process and privacy rules"):
# - a browser (anything that reaches the network or reports crashes) is started only
#   by start_firefox in tools/measure/common.sh (throwaway profile, no crash reporter,
#   telemetry, updates or network);
# - offscreen captures (SIGUSR1 to sash-wm) only through the guarded helpers, which
#   skip the capture while a client that shows device names runs;
# - no process is signalled by name.
# Usage: tools/check-clients.sh   (exit status 1 on a violation)
set -u
cd "$(dirname "$0")/.."
fail=0
report() { echo "check-clients: $*"; fail=1; }

# Code lines only (comments and the browser list itself are left out).
code_lines() {
    grep -nE "$1" "${@:2}" 2> /dev/null | grep -vE '^[^:]+:[0-9]+:\s*#' || true
}

browsers='\b(firefox(-esr)?|chromium|google-chrome|brave|epiphany)\b'
while IFS= read -r hit; do
    file=${hit%%:*}
    line=${hit#*:}; line=${line#*:}
    case $file in
        tools/measure/common.sh)
            # Allowed: the browser list, the refusal message and start_firefox itself.
            [[ $line =~ BROWSERS=|use\ start_firefox|firefox\ --no-remote ]] && continue ;;
        tools/check-clients.sh) continue ;;
    esac
    [[ $line =~ start_firefox ]] && continue
    # The option name --firefox (tools/stage-c.sh) is not a start.
    stripped=${line//--firefox/}
    [[ $stripped =~ $browsers ]] || continue
    report "$file: a browser started outside start_firefox: $line"
done < <(code_lines "$browsers" $(git ls-files 'tools/*.sh' 'tools/*.py'))

while IFS= read -r hit; do
    file=${hit%%:*}
    case $file in
        tools/check-clients.sh) continue ;;
        tools/measure/common.sh)
            grep -q 'SASH_NO_CAPTURE_PIDS' "$file" || report "$file: capture without the guard" ;;
        *.py)
            grep -q 'capture_allowed()' "$file" || report "$file: SIGUSR1 without capture_allowed()" ;;
        *)
            report "$file: a capture outside the guarded helpers: ${hit#*:}" ;;
    esac
done < <(code_lines 'USR1' $(git ls-files 'tools/*.sh' 'tools/*.py'))

while IFS= read -r hit; do
    report "signals a process by name: $hit"
done < <(code_lines '\b(pkill|killall)\b' $(git ls-files 'tools/*.sh' 'tools/*.py') \
    | grep -v '^tools/check-clients.sh:')

if [ "$fail" = 0 ]; then
    echo "ok: browsers only through start_firefox, captures guarded, no kill by name"
fi
exit "$fail"
