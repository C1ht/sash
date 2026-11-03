# SPDX-License-Identifier: Apache-2.0
#
# Makes a replay fixture (sash-wm/src/tests/fixtures/replay/) from a debug bundle:
# events before CUT seconds, only the kinds the replay uses, downsampled. Kept: layout
# snapshots and output size, window commits and destroys, buttons, modifier keys (no
# other keys: nothing typed), and pointer positions: the last one before each button,
# key or window event, plus the first and every 25th one during a drag (the first:
# a snapped window pulled out of its zone is measured from it). Nothing else (no device
# names, frames, counters or screenshots). Writes OUT/events.jsonl only.
#
# Usage: python3 -I tools/replay-fixture.py BUNDLE OUT CUT_SECONDS
import json
import os
import sys

MODIFIERS = {"Shift_L", "Shift_R", "Control_L", "Control_R", "Alt_L", "Alt_R",
             "Super_L", "Super_R"}
EVERY = 25


def main():
    bundle, out, cut = sys.argv[1], sys.argv[2], float(sys.argv[3])
    cut_us = int(cut * 1e6)
    events = []
    with open(os.path.join(bundle, "events.jsonl")) as f:
        for line in f:
            event = json.loads(line)
            if event["t_us"] < cut_us:
                events.append((event, line))
    kept = []
    last_position = None  # (event, line) not yet kept
    in_drag = False
    drag_motions = 0
    for event, line in events:
        kind = event["kind"]
        if kind == "grab_start":
            in_drag, drag_motions = True, 0
            continue
        if kind == "grab_end":
            in_drag = False
            continue
        if kind == "pointer_position":
            if in_drag:
                drag_motions += 1
                if drag_motions == 1 or drag_motions % EVERY == 0:
                    kept.append(line)
                    last_position = None
                    continue
            last_position = line
            continue
        keep = (kind in ("layout", "commit", "destroy", "pointer_button")
                or (kind == "key" and event.get("key") in MODIFIERS))
        if not keep:
            continue
        if kind != "layout" and last_position is not None:
            kept.append(last_position)
            last_position = None
        kept.append(line)
    for line in kept:
        event = json.loads(line)
        assert event["kind"] != "key" or event["key"] in MODIFIERS, "a typed key"
        assert "name" not in event, "a device name"
    os.makedirs(out, exist_ok=True)
    path = os.path.join(out, "events.jsonl")
    with open(path, "w") as f:
        f.writelines(kept)
    print(f"{path}: {len(kept)} of {len(events)} events before {cut} s, "
          f"{os.path.getsize(path)} bytes")


main()
