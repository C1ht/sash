#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Investigation (after M10): does the nested window stop drawing with a client, and how?
# Per case: nested sash-wm (the measurement build) with Smithay's EGL logging, one
# client for D seconds; prints whether rendering failed and with what error, how many
# redraw events and frames came after it, and the GPU memory in use. Firefox goes
# through start_firefox (throwaway profile, no crash reporter, no network).
# Usage: tools/measure/probe-nested-egl.sh LOGDIR [D] [CASE...]
#   cases: foot kitty mpv ff ff-software (default: all; ff = Firefox, ff-software = Firefox
#   with GPU rendering off)
set -u
source "$(dirname "$0")/common.sh"
OUT=$1 D=${2:-20}
shift 2 2> /dev/null || shift
CASES=${*:-foot kitty mpv ff ff-software}
build_measure > /dev/null 2>&1
for case in $CASES; do
    LOG=$OUT/egl-$case.log
    RUST_LOG=sash_wm=debug,smithay::backend::egl=debug,smithay::backend::renderer=debug \
        start_sash "$LOG"
    case $case in
        foot) start_client foot -e sh -c "sleep 120" ;;
        kitty) start_client kitty --config NONE sh -c "sleep 120" ;;
        mpv) start_client mpv --no-config --really-quiet --loop=inf --vo=gpu \
                 --gpu-context=wayland "av://lavfi:testsrc2=size=640x360:rate=30" ;;
        ff) start_firefox about:blank ;;
        ff-software)
            FIREFOX_PREFS='user_pref("gfx.webrender.software", true);
user_pref("layers.acceleration.disabled", true);
user_pref("widget.dmabuf.force-enabled", false);' start_firefox about:blank ;;
    esac
    sleep 3
    mem0=$(nvidia-smi --query-gpu=memory.used --format=csv,noheader 2> /dev/null)
    sleep "$D"
    mem1=$(nvidia-smi --query-gpu=memory.used --format=csv,noheader 2> /dev/null)
    plain=$(sed 's/\x1b\[[0-9;]*m//g' "$LOG")
    failed=$(grep -m1 'rendering failed' <<< "$plain" | grep -oP 'err=\K.*?(?= counters=)')
    after=$(grep -n 'rendering failed' <<< "$plain" | head -1 | cut -d: -f1)
    redraws_after=0 frames_after=0
    if [ -n "$after" ]; then
        tail=$(tail -n +$((after + 1)) <<< "$plain")
        frames_after=$(grep -c 'frame_time_us' <<< "$tail")
        redraws_after=$(grep -c 'rendering failed' <<< "$tail")
    fi
    echo "$case: frames $(grep -c frame_time_us <<< "$plain"), failure: ${failed:-none}," \
         "frames after it: $frames_after, further failures: $redraws_after," \
         "GPU memory $mem0 -> $mem1"
    grep -E 'EGL|egl' <<< "$plain" | grep -viE 'self-test|formats=' | head -5 | cut -c1-220
    stop_all
done
