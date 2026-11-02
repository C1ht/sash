# SPDX-License-Identifier: Apache-2.0
#
# M9 stage B: the DRM backend in a QEMU/KVM guest (virtio-gpu), scripted end to end.
# Prints PASS/FAIL per check, writes the guest display's screenshots (QEMU screendump)
# and sash-wm's log to OUTDIR. The guest must have been set up once with
# tools/vm/guest-setup.sh (see tools/vm/README.md).
#
# Usage: python3 -I tools/vm/scenario.py WORKDIR IMAGE ISO OUTDIR

import base64
import gzip
import math
import os
import re
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from vm import VM  # noqa: E402

WORKDIR, IMAGE, ISO, OUT = sys.argv[1:5]
os.makedirs(OUT, exist_ok=True)
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
RELEASE = "/mnt/sash/sash-wm-release"
DEBUG = "/mnt/sash/sash-wm-debug"
results = []


def check(name, ok, detail=""):
    results.append((name, ok))
    print(f"{'PASS' if ok else 'FAIL'}  {name}  {detail}", flush=True)


def shot(vm, name):
    path = os.path.join(OUT, f"{name}.png")
    vm.screendump(path)
    return path


def sh(vm, command, timeout=120):
    return vm.run(command, timeout=timeout)[0]


def start(vm, binary=RELEASE, env="", args="", windows=2):
    sh(vm, "systemctl stop sash-session 2>/dev/null; chvt 1; sleep 1")
    sh(vm, f"{env} /mnt/sash/start-session.sh {binary} {args}; sleep 3")
    for n in range(1, windows + 1):
        sh(vm, f"(/mnt/sash/foot.sh {n} &); sleep 1.5")
    time.sleep(1)


def pid(vm):
    return sh(vm, "systemctl show -p MainPID --value sash-session").strip()


def active(vm):
    return sh(vm, "systemctl is-active sash-session").strip()


def vt_mode(vm):
    return sh(vm, "kbdinfo -C /dev/tty2 getmode").strip()


def lines(vm):
    return int(sh(vm, "wc -l < /tmp/sash.log"))


def count(vm, pattern):
    """Lines of sash-wm's log (colour codes removed) that match the regex."""
    return int(sh(vm, f"sed 's/\\x1b\\[[0-9;]*m//g' /tmp/sash.log | grep -cE '{pattern}' || true"))


def fetch_log(vm, name):
    encoded = sh(vm, "gzip -c /tmp/sash.log | base64 -w0", timeout=300)
    text = gzip.decompress(base64.b64decode(encoded)).decode(errors="replace")
    path = os.path.join(OUT, f"{name}.log")
    with open(path, "w") as f:
        f.write(text)
    return path


def output_mode(vm):
    log = sh(vm, "sed 's/\\x1b\\[[0-9;]*m//g' /tmp/sash.log | grep -o 'mode=\"[0-9x@]*\"' | tail -1")
    found = re.search(r"(\d+)x(\d+)@(\d+)", log)
    if not found:
        print(sh(vm, "systemctl status sash-session --no-pager | tail -5; head -c 2000 /tmp/sash.log"))
        raise RuntimeError("sash-wm did not start")
    return int(found.group(1)), int(found.group(2)), int(found.group(3))


vm = VM(WORKDIR, IMAGE, ISO)
vm.start()
try:
    vm.login()
    sh(vm, "mkdir -p /mnt/sash; mountpoint -q /mnt/sash || mount -o ro /dev/sr0 /mnt/sash")
    print("guest:", sh(vm, "uname -r; cat /etc/debian_version").replace("\n", " "))

    # 1. Starts on the console (no --backend: it chooses DRM itself) and shows two
    # windows.
    start(vm)
    W, H, HZ = output_mode(vm)
    auto = count(vm, 'backend=Drm chosen_by="environment"')
    check("starts in a logind session on VT 2, DRM chosen from the environment",
          active(vm) == "active" and vt_mode(vm) == "graphics" and auto == 1,
          f"{W}x{H}@{HZ}, VT 2 {vt_mode(vm)}, auto-chosen: {auto}")
    print("renderer:", sh(vm, f"grep -oE '/usr/lib/[^ ]*(dri|gallium|llvm)[^ ]*' /proc/{pid(vm)}/maps | sort -u | tr '\\n' ' '"))
    shot(vm, "01-two-windows")

    # 2. Keyboard into the focused window (foot 2).
    vm.type_text("echo keyboard ok")
    vm.keys("ret")
    time.sleep(0.5)
    shot(vm, "02-keyboard")

    # 3. Pointer (absolute, tablet): click into foot 1, type there.
    vm.pointer_abs(W // 4, H // 2, W, H)
    time.sleep(0.2)
    vm.button("left", True)
    vm.button("left", False)
    time.sleep(0.3)
    vm.type_text("echo tablet click ok")
    vm.keys("ret")
    # 4. Relative pointer (virtio mouse): move right into foot 2, click, type.
    for _ in range(10):
        vm.pointer_rel(W // 20, 0)
        time.sleep(0.02)
    vm.button("left", True)
    vm.button("left", False)
    time.sleep(0.3)
    vm.type_text("echo relative ok")
    vm.keys("ret")
    time.sleep(0.5)
    shot(vm, "03-pointer")
    check("keyboard, tablet and relative pointer input reach the windows (see 02, 03)", True,
          "check the screenshots")

    # 5. Super is the modifier: Super+F floats foot 2, Super+Y snaps it to the top-left
    # quarter, Super+H moves focus.
    vm.keys("meta_l", "f")
    vm.keys("meta_l", "y")
    time.sleep(0.5)
    shot(vm, "04-super-float-quarter")

    # 6. A scripted Super+drag of the floating window for 5 s; frame statistics.

    before = lines(vm)
    vm.key_down("meta_l")
    x0, y0 = W // 4, H // 4
    vm.pointer_abs(x0, y0, W, H)
    vm.button("left", True)
    t0 = time.monotonic()
    motions = 0
    while time.monotonic() - t0 < 5:
        t = time.monotonic() - t0
        vm.pointer_abs(x0 + int(150 * math.sin(2 * math.pi * t / 2)),
                       y0 + int(100 * math.sin(2 * math.pi * t)), W, H)
        motions += 1
        time.sleep(0.004)
    vm.button("left", False)
    vm.key_down("meta_l", False)
    after = lines(vm)
    log = fetch_log(vm, "drag")
    stats = subprocess.run(["python3", "-I", os.path.join(ROOT, "tools/measure/analyze.py"), log,
                            str(before), str(after)], capture_output=True, text=True).stdout
    print(f"drag: {motions} motions\n{stats}")
    check("frames during a drag", "frames:" in stats)
    # virtio-gpu completes flips at once: the cap must hold rendering at the refresh rate.
    fps = re.search(r"= ([0-9.]+) fps", stats)
    capped = count(vm, "rendering is capped at the refresh rate")
    check("unpaced flips: rendering capped at the refresh rate",
          fps is not None and float(fps.group(1)) <= HZ * 1.02 and capped == 1,
          f"{fps.group(1) if fps else '?'} fps at {HZ} Hz, cap logged: {capped}")

    # 7. Idle CPU and memory with three windows.
    sh(vm, "(/mnt/sash/foot.sh 3 &); sleep 2")
    p = pid(vm)
    idle = sh(vm, f"""a=$(awk '{{print $14+$15}}' /proc/{p}/stat); sleep 30; b=$(awk '{{print $14+$15}}' /proc/{p}/stat);
    echo "ticks=$((b-a)) hz=$(getconf CLK_TCK)"; grep -E 'VmRSS|RssAnon' /proc/{p}/status | tr -s ' '""", timeout=120)
    print("idle 30 s, 3 windows:", idle.replace("\n", "; "))

    # 8. VT switch away and back: same windows.
    vm.keys("ctrl", "alt", "f1")
    time.sleep(1.5)
    shot(vm, "05-vt1-console")
    away = sh(vm, "fgconsole").strip()
    vm.keys("ctrl", "alt", "f2")
    time.sleep(2)
    shot(vm, "06-vt2-back")
    log = sh(vm, "grep -cE 'PauseSession|ActivateSession' /tmp/sash.log")
    check("Ctrl+Alt+F1 leaves, Ctrl+Alt+F2 comes back with the same windows (see 05, 06)",
          away == "1" and sh(vm, "fgconsole").strip() == "2" and active(vm) == "active",
          f"VT after F1: {away}, session events logged: {log}")

    # 9. Super+Shift+E quits and the console comes back.
    vm.keys("meta_l", "shift", "e")
    time.sleep(2)
    shot(vm, "07-after-quit")
    check("Super+Shift+E quits; VT 2 is back in text mode", active(vm) != "active"
          and vt_mode(vm) == "text", f"unit {active(vm)}, VT 2 {vt_mode(vm)}")

    # 10. Keyboard layout from XKB_DEFAULT_LAYOUT=de: the key labelled Y on a US keyboard
    # types z.
    start(vm, env="XKB_DEFAULT_LAYOUT=de", windows=1)
    vm.type_text("echo ")
    vm.keys("y")
    vm.keys("z")
    vm.keys("ret")
    time.sleep(0.5)
    shot(vm, "08-xkb-de")
    check("XKB_DEFAULT_LAYOUT=de: US keys y z type z y (see 08)", True, "check the screenshot")

    # 11. kill -TERM: clean exit, console back.
    p = pid(vm)
    sh(vm, f"kill -TERM {p}; sleep 2")
    check("kill -TERM restores the console", active(vm) != "active" and vt_mode(vm) == "text",
          f"unit {active(vm)}, VT 2 {vt_mode(vm)}")
    shot(vm, "09-after-term")

    # 12. A crash: SIGABRT (what a panic=abort or an assertion in C code does) and
    # SIGKILL. No clean-up runs in sash-wm; logind must give the console back.
    # (kill -SEGV is not a crash for a Rust program: its stack-overflow handler resets
    # the default action and returns, since there was no faulting instruction.)
    for sig in ("ABRT", "KILL"):
        start(vm, windows=1)
        p = pid(vm)
        sh(vm, f"kill -{sig} {p}; sleep 3")
        check(f"a crash (SIG{sig}) restores the console", active(vm) != "active"
              and vt_mode(vm) == "text", f"unit {active(vm)}, VT 2 {vt_mode(vm)}")
        shot(vm, f"10-after-{sig.lower()}")
    start(vm, windows=1)
    check("starts again after the crash (device not held)", active(vm) == "active")

    # 13. --exit-after (debug build).
    start(vm, binary=DEBUG, args="--exit-after 5", windows=1)
    time.sleep(6)
    log = sh(vm, "grep -c 'exit-after reached' /tmp/sash.log")
    check("--exit-after 5 quits after 5 s and restores the console",
          active(vm) != "active" and vt_mode(vm) == "text" and log.strip() == "1",
          f"unit {active(vm)}, VT 2 {vt_mode(vm)}")

    # 14. Started on a VT that is not in front: sash-wm waits until it is.
    start(vm, env="CHVT_DELAY=3", windows=1)
    waited = sh(vm, "grep -c 'waiting for the session to become active' /tmp/sash.log").strip()
    check("started before its VT is in front, it waits and then runs",
          active(vm) == "active" and waited == "1", f"waited: {waited}")

    # 15. Unplug and plug back: the connector is forced off (sysfs), and the kernel sends
    # a real hotplug uevent (debugfs trigger_hotplug, as a cable pull would); then the
    # same with the connector back. sash-wm must see the udev event, stop rendering while
    # disconnected, and do a full modeset and redraw on reconnect.
    start(vm, windows=2)
    connector = sh(vm, "ls -d /sys/class/drm/card0-*/ | head -1").strip().rstrip("/")
    name = connector.rsplit("card0-", 1)[1]
    trigger = sh(vm, f"mountpoint -q /sys/kernel/debug || mount -t debugfs none /sys/kernel/debug; "
                     f"ls /sys/kernel/debug/dri/*/{name}/trigger_hotplug 2>/dev/null | head -1").strip()
    print("connector:", connector, "hotplug trigger:", trigger or "none")
    hotplug = f"echo 1 > {trigger}" if trigger else "udevadm trigger --action=change /sys/class/drm/card0"
    shot(vm, "11-before-unplug")
    events0 = count(vm, "udev event")
    sh(vm, f"echo off > {connector}/status; {hotplug}; sleep 2")
    events1 = count(vm, "udev event")
    gone = count(vm, "display disconnected")
    frames0 = count(vm, "frame frame_time")
    for _ in range(20):
        vm.pointer_rel(10, 5)
        time.sleep(0.05)
    time.sleep(1)
    frames1 = count(vm, "frame frame_time")
    alive = active(vm)
    sh(vm, f"echo detect > {connector}/status; {hotplug}; sleep 2")
    back = count(vm, "display connected again")
    for _ in range(20):
        vm.pointer_rel(-10, -5)
        time.sleep(0.05)
    time.sleep(1)
    frames2 = count(vm, "frame frame_time")
    shot(vm, "12-after-replug")
    check("unplug: a udev event arrives and rendering stops; sash-wm keeps running",
          events1 > events0 and gone == 1 and frames1 == frames0 and alive == "active",
          f"udev events {events0} -> {events1}, disconnect logged {gone}, "
          f"frames while unplugged {frames1 - frames0}")
    check("replug: full modeset, frames again, same windows (see 11, 12)",
          back == 1 and frames2 > frames1 and active(vm) == "active",
          f"reconnect logged {back}, frames after {frames2 - frames1}")
    sh(vm, "systemctl stop sash-session; chvt 1; sleep 1")

    # 16. tools/stage-c.sh on a console of the guest (the owner's stage C tool): escape
    # instructions first, Enter starts sash-wm with --exit-after, a client starts, the
    # log is written, the console comes back.
    sh(vm, "rm -rf /home/sash/.cache/sash; systemctl reset-failed stage-c 2>/dev/null; "
           "systemd-run --unit=stage-c --collect -p PAMName=login -p User=sash "
           "-p TTYPath=/dev/tty2 -p StandardInput=tty-force -p StandardOutput=tty "
           "-p StandardError=tty --setenv=STAGE_C_BIN=" + DEBUG + " --setenv=RUST_LOG=sash_wm=debug "
           "/bin/bash /mnt/sash/stage-c.sh 8 vmtest -- foot; sleep 2; chvt 2; sleep 1")
    shot(vm, "13-stage-c-instructions")
    vm.keys("ret")
    time.sleep(4)
    shot(vm, "14-stage-c-running")
    time.sleep(12)
    shot(vm, "15-stage-c-done")
    log = sh(vm, "ls /home/sash/.cache/sash/stage-c/*-vmtest.log").strip()
    content = sh(vm, f"sed 's/\\x1b\\[[0-9;]*m//g' {log} | grep -E '^== |exit-after reached|starting the client' ")
    print("stage-c log:", log, "\n" + content)
    check("tools/stage-c.sh: instructions, run, client, fsynced log, console back (see 13-15)",
          "exit-after reached" in content and "exit status 0" in content
          and "starting the client" in content and vt_mode(vm) == "text"
          and sh(vm, "ls /home/sash/.cache/sash/stage-c/sash-wm.pid 2>/dev/null").strip() == "",
          f"log {log}")
    fetch_log(vm, "final")
    sh(vm, "systemctl stop sash-session; chvt 1")
finally:
    vm.stop()
    print(f"\n{sum(ok for _, ok in results)} of {len(results)} checks passed")
