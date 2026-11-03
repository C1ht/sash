#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Clipboard and drag and drop by hand, scripted (M10): nested sash-wm (the measurement
# build) with foot, pavucontrol and Firefox on a local test page. Every client gets a
# throwaway config; Firefox gets a new empty profile whose proxy points nowhere, so it
# reaches no network. Offscreen captures go to CAPDIR: look at every one and delete any
# that shows personal content (pavucontrol's Playback tab lists the audio streams).
# Then idle CPU and memory with the three windows.
# Usage: tools/measure/run-clipboard.sh LOG CAPDIR
set -u
source "$(dirname "$0")/common.sh"
LOG=$1
export SASH_CAPTURE_DIR
SASH_CAPTURE_DIR=$(realpath "$2")
WORK=$(mktemp -d)
PROFILE="$WORK/firefox-profile"
mkdir -p "$PROFILE" "$WORK/files"
echo "a sample file for the drag test" > "$WORK/files/sample.txt"
cat > "$PROFILE/user.js" <<'EOF'
user_pref("network.proxy.type", 1);
user_pref("network.proxy.http", "127.0.0.1");
user_pref("network.proxy.http_port", 9);
user_pref("network.proxy.ssl", "127.0.0.1");
user_pref("network.proxy.ssl_port", 9);
user_pref("network.proxy.allow_hijacking_localhost", true);
user_pref("network.proxy.no_proxies_on", "");
user_pref("network.captive-portal-service.enabled", false);
user_pref("network.connectivity-service.enabled", false);
user_pref("app.update.enabled", false);
user_pref("app.normandy.enabled", false);
user_pref("extensions.update.enabled", false);
user_pref("toolkit.telemetry.enabled", false);
user_pref("datareporting.healthreport.uploadEnabled", false);
user_pref("datareporting.policy.dataSubmissionEnabled", false);
user_pref("browser.safebrowsing.malware.enabled", false);
user_pref("browser.safebrowsing.phishing.enabled", false);
user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.aboutwelcome.enabled", false);
user_pref("browser.startup.homepage_override.mstone", "ignore");
user_pref("browser.newtabpage.enabled", false);
user_pref("trailhead.firstrun.didSeeAboutWelcome", true);
EOF
cat > "$WORK/page.html" <<EOF
<!doctype html><meta charset="utf-8"><title>sash test page</title>
<style>body{margin:0;font:20px sans-serif}
a#f{display:block;height:40vh;background:#cde;font-size:48px;padding:20px}
textarea{width:90%;height:3em;font-size:24px}button{font-size:24px}</style>
<a id="f" href="file://$WORK/files/sample.txt">sample.txt (drag me)</a>
<textarea id="t">sash-m10-copy-test</textarea>
<button id="b">copy image</button>
<canvas id="c" width="64" height="64"></canvas>
<script>
const t = document.getElementById('t'); t.focus(); t.select();
const c = document.getElementById('c').getContext('2d');
c.fillStyle = '#d33'; c.fillRect(0, 0, 64, 64);
document.getElementById('b').onclick = () => document.getElementById('c').toBlob(
  b => navigator.clipboard.write([new ClipboardItem({'image/png': b})]));
</script>
EOF
build_measure > /dev/null 2>&1
RUST_LOG=sash_wm=debug start_sash "$LOG"
# pavucontrol alone first, with no capture (it lists the machine's audio devices): it
# gets keyboard focus and a selection offer, keeps running, and is closed by its PID.
start_client pavucontrol
PAVU=${PIDS[-1]}
sleep 3
printf 'offered to pavucontrol' | WAYLAND_DISPLAY=$SASH_SOCKET wl-copy > /dev/null 2>&1
sleep 2
if kill -0 "$PAVU" 2> /dev/null; then echo "pavucontrol: running after a selection offer"; \
    else echo "pavucontrol: NOT running"; fi
kill "$PAVU"; wait "$PAVU" 2> /dev/null
unset 'PIDS[-1]' # stopped: never signalled again (its PID could be reused)
sleep 1
start_client foot -e sh -c "printf '\\n  foot\\n'; exec bash --norc"
sleep 1.5
# No crash reporter (it could send a report past the profile's proxy); Firefox's own
# output goes to FFLOG for diagnosis.
FFLOG=${FFLOG:-/dev/null}
WAYLAND_DISPLAY=$SASH_SOCKET XDG_CONFIG_HOME=$(mktemp -d) MOZ_ENABLE_WAYLAND=1 \
    MOZ_CRASHREPORTER_DISABLE=1 firefox --no-remote --profile "$PROFILE" \
    "file://$WORK/page.html" > "$FFLOG" 2>&1 &
PIDS+=("$!")
sleep 12
read -r SASH_W SASH_H < <(sed 's/\x1b\[[0-9;]*m//g' "$LOG" \
    | grep -oP ' resize w=\K\d+ h=\d+' | tail -1 | tr -d 'h=')
echo "pid=$SASH_PID socket=$SASH_SOCKET output=${SASH_W}x$SASH_H"
python3 -I "$ROOT/tools/measure/drive_clipboard.py" "$LOG" "$SASH_INPUT" "$SASH_SOCKET" \
    "$SASH_W" "$SASH_H" "$SASH_PID" "$WORK" | while read -r line; do
    echo "$line"
    set -- $line
    if [ "$1" = dnd ]; then
        python3 -I "$ROOT/tools/measure/analyze.py" "$LOG" "$2" "$3"
    fi
done
# Idle with the three windows: 10 s.
sleep 3
T=$(getconf CLK_TCK)
a=$(awk '{print $14+$15}' "/proc/$SASH_PID/stat")
t0=$(date +%s.%N)
sleep 10
b=$(awk '{print $14+$15}' "/proc/$SASH_PID/stat")
t1=$(date +%s.%N)
python3 -I -c "print(f'idle CPU: {100*($b-$a)/$T/($t1-$t0):.3f}% of one core over {$t1-$t0:.1f}s')"
grep -E 'VmRSS|RssAnon|RssFile' "/proc/$SASH_PID/status"
grep -E '^Pss:' "/proc/$SASH_PID/smaps_rollup"
stop_all
rm -rf "$WORK"
