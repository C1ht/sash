# SPDX-License-Identifier: Apache-2.0
#
# M9 stage B: the DRM backend in a QEMU/KVM guest (virtio-gpu), scripted end to end.
# Prints PASS/FAIL per check, writes the guest display's screenshots (QEMU screendump)
# and sash-wm's log to OUTDIR. Everything is in ~/.cache/sash/vm: build the ISO with
# tools/vm/build-iso.sh and set the guest up with tools/vm/prepare.py first (see
# tools/vm/README.md). The image is checked against its SHA512 before every run.
#
# Usage: python3 -I tools/vm/scenario.py [OUTDIR]   (default ~/.cache/sash/vm/out)

import base64
import gzip
import hashlib
import math
import os
import re
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import vm as vmlib  # noqa: E402
from vm import VM  # noqa: E402

WORKDIR = os.path.join(vmlib.CACHE, "guest")
IMAGE = vmlib.image_path()
ISO = os.path.join(vmlib.CACHE, "sash.iso")
OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(vmlib.CACHE, "out")
if not os.path.exists(os.path.join(WORKDIR, "ready")):
    sys.exit("the guest is not set up; run tools/vm/prepare.py first")
os.makedirs(OUT, exist_ok=True)
SSH_PORT = 22422
SSH_KEY = os.path.join(vmlib.CACHE, "ssh", "id_ed25519")
REMOTE_LOGIN = "DRM needs a local console session; this looks like a remote login"
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
    return int(sh(vm, f"sed 's/\\x1b\\[[0-9;]*m//g' /tmp/sash.log | grep -cE -e '{pattern}' || true"))


def fetch_log(vm, name):
    """Copies the guest's log over the serial console, checked with md5 (retried)."""
    for _ in range(3):
        sh(vm, "cp /tmp/sash.log /tmp/sash.log.copy")
        expected = sh(vm, "md5sum < /tmp/sash.log.copy | cut -d' ' -f1").strip()
        encoded = sh(vm, "gzip -c /tmp/sash.log.copy | base64 -w0", timeout=300)
        try:
            data = gzip.decompress(base64.b64decode(encoded))
        except (ValueError, OSError, EOFError):
            continue
        if hashlib.md5(data).hexdigest() == expected:
            break
    else:
        raise RuntimeError("the log could not be copied intact over the serial console")
    text = data.decode(errors="replace")
    path = os.path.join(OUT, f"{name}.log")
    with open(path, "w") as f:
        f.write(text)
    return path


def output_mode(vm):
    log = sh(vm, "sed 's/\\x1b\\[[0-9;]*m//g' /tmp/sash.log | grep -o 'mode=\"[0-9x@.]*\"' | tail -1")
    found = re.search(r"(\d+)x(\d+)@([0-9.]+)", log)
    if not found:
        print(sh(vm, "systemctl status sash-session --no-pager | tail -5; head -c 2000 /tmp/sash.log"))
        raise RuntimeError("sash-wm did not start")
    return int(found.group(1)), int(found.group(2)), float(found.group(3))


def ssh(command):
    """Runs a command in the guest as \"sash\" over SSH (a remote login)."""
    result = subprocess.run(
        ["ssh", "-p", str(SSH_PORT), "-i", SSH_KEY, "-o", "BatchMode=yes",
         "-o", "StrictHostKeyChecking=no",
         "-o", f"UserKnownHostsFile={os.path.join(vmlib.CACHE, 'ssh', 'known_hosts')}",
         "sash@127.0.0.1", command], capture_output=True, text=True, timeout=60)
    return result.stdout + result.stderr


def session_env(text):
    """The XDG_ session variables in `env`-style output."""
    found = dict(line.split("=", 1) for line in text.splitlines()
                 if line.startswith("XDG_") and "=" in line)
    return {k: found.get(k, "(unset)") for k in ("XDG_SESSION_ID", "XDG_SEAT", "XDG_VTNR",
                                                 "XDG_SESSION_TYPE", "XDG_SESSION_CLASS")}


vm = VM(WORKDIR, IMAGE, ISO, ssh_port=SSH_PORT)
vm.start()
try:
    vm.login()
    # Kernel messages on the serial console would mix into command output.
    sh(vm, "dmesg -n 1")
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
    # What logind set for the local console session sash-wm runs in.
    local = session_env(sh(vm, f"tr '\\0' '\\n' < /proc/{pid(vm)}/environ"))
    print("console session (VT 2, PAM login, like a display manager):", local)
    candidates = sh(vm, "sed 's/\\x1b\\[[0-9;]*m//g' /tmp/sash.log | grep -E 'mode candidate|mode candidates|output connector' | cut -c28-300")
    print(candidates)
    check("mode chosen by the rule and validated with a test commit before use",
          "passed its test commit" in candidates, "see the log lines above")
    shot(vm, "01-two-windows")

    # 1b. Session detection: what logind sets for a serial-console root login and for an
    # SSH login, and that sash-wm started over SSH refuses DRM (it must not touch the
    # display) with the remote-login message.
    serial = session_env(sh(vm, "env"))
    serial_props = sh(vm, "loginctl show-session \"$XDG_SESSION_ID\" -p Type -p Seat -p VTNr -p TTY -p Remote -p Service")
    print("serial console root login:", serial, serial_props.replace("\n", " "))
    remote = ssh("env; loginctl show-session \"$XDG_SESSION_ID\" -p Type -p Seat -p VTNr "
                 "-p TTY -p Remote -p Service; /mnt/sash/sash-wm-release; echo status=$?")
    print("SSH login:", session_env(remote),
          " ".join(l for l in remote.splitlines() if re.match(r"^(Type|Seat|VTNr|TTY|Remote|Service)=", l)))
    refused = REMOTE_LOGIN in remote and "status=1" in remote
    check("over SSH (no seat, no VT) sash-wm refuses DRM with the remote-login message; the "
          "console session has all three variables",
          refused and all(local[k] != "(unset)" for k in ("XDG_SESSION_ID", "XDG_SEAT", "XDG_VTNR"))
          and active(vm) == "active",
          f"SSH output ends: {remote.strip().splitlines()[-2:]}")

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

    # 8b. Renderer recovery (M11a): a VT switch away for longer than the flip watchdog
    # (2 s) is no failure: no watchdog while the session is switched away.
    vm.keys("ctrl", "alt", "f1")
    time.sleep(4)
    vm.keys("ctrl", "alt", "f2")
    time.sleep(2)
    check("renderer recovery: a 4 s VT switch away does not trip the 2 s flip watchdog",
          count(vm, "no page flip") == 0 and count(vm, "renderer failure") == 0
          and active(vm) == "active",
          f"watchdog lines {count(vm, 'no page flip')}, failures {count(vm, 'renderer failure')}")

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

    # 13b. --mode (debug build): an offered mode is used (still validated); a mode that is
    # not offered is refused before anything is committed (VT 2 stays in text mode).
    start(vm, binary=DEBUG, args="--mode 1280x800@75", windows=0)
    w, h, hz = output_mode(vm)
    requested = count(vm, "--mode: only the requested mode is tried")
    passed = count(vm, "mode candidate passed its test commit")
    check("--mode 1280x800@75 (debug build): that mode, validated",
          (w, h, round(hz)) == (1280, 800, 75) and requested == 1 and passed == 1
          and active(vm) == "active", f"{w}x{h}@{hz}")
    start(vm, binary=DEBUG, args="--mode 1234x567@60", windows=0)
    refused = count(vm, "is not offered; offered:")
    committed = count(vm, "candidate passed its test commit|frame frame_time")
    check("--mode with a mode that is not offered: refused, nothing committed",
          refused == 1 and committed == 0 and active(vm) != "active" and vt_mode(vm) == "text",
          f"unit {active(vm)}, VT 2 {vt_mode(vm)}")

    # 13c/13d. Fault injection (debug build). The kernel's DRM debug output (drm.debug:
    # core 0x01 logs every ioctl with the calling program's name; atomic 0x10) shows
    # which DRM ioctls sash-wm itself made. Counting all commits would prove nothing:
    # fbcon commits on its own (virtio's console cursor blink is a damage commit).
    SETTERS = ("DRM_IOCTL_MODE_ATOMIC", "DRM_IOCTL_MODE_SETCRTC", "DRM_IOCTL_MODE_PAGE_FLIP",
               "DRM_IOCTL_MODE_SETPLANE", "DRM_IOCTL_MODE_CURSOR", "DRM_IOCTL_MODE_DIRTYFB",
               "DRM_IOCTL_MODE_SETPROPERTY", "DRM_IOCTL_MODE_OBJ_SETPROPERTY")

    def traced_start(env):
        """Starts the debug build with `env` under DRM ioctl tracing; returns sash-wm's
        ioctls (name -> count) and whether the CRTC/plane state is unchanged."""
        sh(vm, "systemctl stop sash-session 2>/dev/null; chvt 1; sleep 1")
        sh(vm, "echo 0x11 > /sys/module/drm/parameters/debug; dmesg -C")
        before = sh(vm, "grep -E 'fb=|mode:|active=' /sys/kernel/debug/dri/0/state")
        sh(vm, f"{env} /mnt/sash/start-session.sh {DEBUG}; sleep 4")
        trace = sh(vm, "dmesg | grep -F 'comm=\"sash-wm-debug\"' | grep -oE 'DRM_IOCTL_[A-Z0-9_]+' "
                       "| sort | uniq -c")
        sh(vm, "echo 0 > /sys/module/drm/parameters/debug")
        after = sh(vm, "grep -E 'fb=|mode:|active=' /sys/kernel/debug/dri/0/state")
        ioctls = {name: int(n) for n, name in (l.split() for l in trace.splitlines() if l.strip())}
        return ioctls, before == after

    # 13c. The first two candidates' test commits fail: the fallback chain is logged and
    # the third candidate is used. Its test commit and the modeset are real ioctls from
    # sash-wm (the positive control for 13d's tracing).
    ioctls_c, _ = traced_start("SASH_DEBUG_REJECT_MODES=1920x1200@60,1680x1050@60")
    lines_c = sh(vm, "sed 's/\\x1b\\[[0-9;]*m//g' /tmp/sash.log | "
                     "grep -E 'fault injection|mode candidate|output connector' | cut -c28-400")
    print("--- log, first candidates rejected:\n" + lines_c)
    print("sash-wm's DRM ioctls:", ioctls_c)
    w, h, hz = output_mode(vm)
    check("fault injection: the first candidates rejected, the fallback chain logged, the "
          "next mode used", count(vm, "mode candidate rejected") == 2
          and count(vm, "candidate passed its test commit") == 1 and (w, h) == (1440, 900)
          and ioctls_c.get("DRM_IOCTL_MODE_ATOMIC", 0) > 0 and active(vm) == "active",
          f"{w}x{h}@{hz}, atomic ioctls {ioctls_c.get('DRM_IOCTL_MODE_ATOMIC', 0)}")
    sh(vm, "systemctl stop sash-session; chvt 1; sleep 1")

    # 13d. Every candidate rejected: sash-wm exits with a message, without one
    # state-changing DRM ioctl, and the CRTC/plane state is the same as before.
    ioctls_d, same = traced_start("SASH_DEBUG_REJECT_ALL_MODES=1")
    shot(vm, "16-all-rejected")
    lines_d = sh(vm, "sed 's/\\x1b\\[[0-9;]*m//g' /tmp/sash.log | "
                     "grep -E 'fault injection|mode candidate|passed|Error|untouched' | cut -c28-400")
    print("--- log, every candidate rejected:\n" + lines_d)
    print("sash-wm's DRM ioctls:", ioctls_d, "; CRTC/plane state unchanged:", same)
    setters = {k: v for k, v in ioctls_d.items() if k in SETTERS}
    check("fault injection: every candidate rejected, exit with a message, no commit or "
          "other state-changing ioctl, the console untouched (see 16)",
          "the display was left untouched" in lines_d
          and count(vm, "mode candidate rejected") == 5
          and count(vm, "candidate passed its test commit|frame frame_time") == 0
          and ioctls_d and not setters and same
          and active(vm) != "active" and vt_mode(vm) == "text",
          f"state-changing ioctls: {setters or 'none'}")
    sh(vm, "systemctl stop sash-session; chvt 1; sleep 1")

    # 14. Started on a VT that is not in front: sash-wm waits until it is.
    start(vm, env="CHVT_DELAY=3", windows=1)
    waited = sh(vm, "grep -c 'waiting for the session to become active' /tmp/sash.log").strip()
    check("started before its VT is in front, it waits and then runs",
          active(vm) == "active" and waited == "1", f"waited: {waited}")

    # 15b. Replug when the previous mode is gone: started at 3840x2160@60 (--mode, debug
    # build), which virtio-gpu no longer lists after the forced off/detect (so this runs
    # before 15: the mode list stays changed). The selection rule picks the mode (tested
    # again; on virtio its new preferred 1280x800), and the output and layout follow the
    # new size through the output-resize path: the floated foot 2, at the right half of
    # the 3840 px output, is re-clamped onto the smaller one. The connector is forced off
    # (sysfs) and the kernel sends a change uevent: debugfs trigger_hotplug where the
    # kernel has it, else udevadm trigger.
    connector = sh(vm, "ls -d /sys/class/drm/card0-*/ | head -1").strip().rstrip("/")
    name = connector.rsplit("card0-", 1)[1]
    trigger = sh(vm, f"mountpoint -q /sys/kernel/debug || mount -t debugfs none /sys/kernel/debug; "
                     f"ls /sys/kernel/debug/dri/*/{name}/trigger_hotplug 2>/dev/null | head -1").strip()
    print("connector:", connector, "hotplug trigger:", trigger or "none")
    hotplug = f"echo 1 > {trigger}" if trigger else "udevadm trigger --action=change /sys/class/drm/card0"
    start(vm, binary=DEBUG, args="--mode 3840x2160@60", windows=2)
    vm.keys("meta_l", "f")  # foot 2 floats at its tile, x = 1920
    time.sleep(0.5)
    shot(vm, "11b-before-unplug-3840x2160")
    sh(vm, f"echo off > {connector}/status; {hotplug}; sleep 2")
    sh(vm, f"echo detect > {connector}/status; {hotplug}; sleep 3")
    switched = count(vm, "no longer offers the mode in use; mode candidates")
    tested = count(vm, "candidate passed its test commit")
    w, h, hz = output_mode(vm)
    chosen = sh(vm, "sed 's/\\x1b\\[[0-9;]*m//g' /tmp/sash.log | grep 'passed its test commit' | tail -1 | grep -o 'mode=\"[0-9x]*'")
    shot(vm, "12b-after-replug-new-mode")
    check("replug without the previous mode: the rule's mode, tested, output and windows "
          "follow the new size (see 11b, 12b)",
          switched == 1 and tested == 2 and (w, h) != (3840, 2160)
          and chosen.endswith(f"{w}x{h}") and active(vm) == "active",
          f"3840x2160 -> {w}x{h}@{hz}, mode tests passed {tested}")
    sh(vm, "systemctl stop sash-session; chvt 1; sleep 1")

    # 15. Unplug and plug back: the connector is forced off (sysfs), and the kernel sends
    # a real hotplug uevent (debugfs trigger_hotplug, as a cable pull would); then the
    # same with the connector back. sash-wm must see the udev event, stop rendering while
    # disconnected, and do a full modeset and redraw on reconnect.
    start(vm, windows=2)
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
    check("unplug: no flip watchdog while the connector is off (M11a)",
          count(vm, "no page flip") == 0, f"watchdog lines {count(vm, 'no page flip')}")
    check("unplug: a udev event arrives and rendering stops; sash-wm keeps running",
          events1 > events0 and gone == 1 and frames1 == frames0 and alive == "active",
          f"udev events {events0} -> {events1}, disconnect logged {gone}, "
          f"frames while unplugged {frames1 - frames0}")
    check("replug: full modeset, frames again, same windows (see 11, 12)",
          back == 1 and frames2 > frames1 and active(vm) == "active",
          f"reconnect logged {back}, frames after {frames2 - frames1}")
    sh(vm, "systemctl stop sash-session; chvt 1; sleep 1")

    # 17. Debug recording (debug build with the debug-record feature, docs/DEBUG.md):
    # the banner; pointer motion without windows; Super+F11 writes a bundle; a window
    # opens (a layout change: an offscreen screenshot); "hunter2" is typed into it;
    # Super+F11 again. Bundles are private, keep no typed text, and their screenshots
    # come from the offscreen path.
    RECORD_BIN = "/mnt/sash/sash-wm-debug-record"
    sh(vm, "rm -rf /home/sash/rec")
    start(vm, binary=RECORD_BIN, env="SASH_DEBUG_RECORD=/home/sash/rec", windows=0)
    banner = count(vm, "DEBUG RECORDING IS ON")
    for i in range(40):  # tablet: absolute motion
        vm.pointer_abs(200 + 10 * i, 200 + 5 * i, W, H)
        time.sleep(0.025)
    for i in range(40):  # mouse: relative motion
        vm.pointer_rel(-8, 4)
        time.sleep(0.025)
    time.sleep(1.5)
    vm.keys("meta_l", "f11")
    time.sleep(2)
    sh(vm, "(/mnt/sash/foot.sh 1 &); sleep 2")
    vm.type_text("hunter2")
    time.sleep(1.2)
    vm.keys("meta_l", "f11")
    time.sleep(2.5)
    bundles = sh(vm, "ls -d /home/sash/rec/bundle-* 2>/dev/null").split()
    print("bundles:", bundles)
    hotkey_lines = count(vm, "debug hotkey")
    written_lines = count(vm, "debug bundle written")
    perms = sh(vm, "stat -c '%a %n' /home/sash/rec /home/sash/rec/bundle-* "
                   "/home/sash/rec/bundle-*/* /home/sash/rec/bundle-*/screenshots/* | sort -u")
    print(perms)
    first_counters = sh(vm, f"grep counters_1s {bundles[0]}/events.jsonl" if bundles else "true")
    print("counters, first bundle:\n" + first_counters)
    leaked = sh(vm, "grep -rl hunter2 /home/sash/rec; grep -rhoE '\"key\":\"[hunter2]\"' /home/sash/rec | sort -u")
    kinds = sh(vm, f"grep -oE '\"kind\":\"[a-z_0-9]+\"' {bundles[-1]}/events.jsonl | sort | uniq -c"
               if bundles else "true")
    print("event kinds, second bundle:\n" + kinds)
    shots = sh(vm, f"ls {bundles[-1]}/screenshots" if bundles else "true").split()
    print("screenshots, second bundle:", shots)
    if shots:
        # The default: outlines only (the window shows "hunter2", the picture must not).
        encoded = sh(vm, f"base64 -w0 {bundles[-1]}/screenshots/{shots[-1]}", timeout=300)
        with open(os.path.join(OUT, "17-outline-" + shots[-1]), "wb") as f:
            f.write(base64.b64decode(encoded))
    modes_ok = all(line.split()[0] in ("700", "600") for line in perms.splitlines() if line.strip())
    pointer_only = [l for l in first_counters.splitlines()
                    if '"pointer_events_in":0' not in l and '"frames_rendered":0' in l]
    # Note: the screenshots show window contents, so text a client echoes on screen is
    # in them; "no typed text" is about the recorded data (events, state, log).
    check("debug recording: banner, bundles on Super+F11, private files, no typed text in "
          "events/state/log, outline screenshots (see 17-outline-*)",
          banner == 1 and len(bundles) == 2 and modes_ok and leaked.strip() == ""
          and '"kind":"pointer_in"' in kinds and '"kind":"screenshot"' in kinds and bool(shots),
          f"bundles {len(bundles)}, screenshots {len(shots)}, leaked {leaked.strip() or 'nothing'}")
    print(f"seconds with pointer events in and no frame rendered: {len(pointer_only)}")
    check("debug recording: each Super+F11 is logged with where its bundle went (DRM)",
          hotkey_lines == 2 and written_lines == 2,
          f"hotkey lines {hotkey_lines}, bundle-written lines {written_lines}")
    # The cursor path (plane or composited) and the per-second cursor counters.
    paths = sh(vm, f"grep -h cursor_path {' '.join(b + '/events.jsonl' for b in bundles)}" if bundles else "true")
    print("cursor path events:\n" + paths)
    sums = {}
    for line in first_counters.splitlines():
        for name in ("cursor_plane_frames", "cursor_composited_frames", "pointer_frame_requests"):
            m = re.search(rf'"{name}":(\d+)', line)
            if m:
                sums[name] = sums.get(name, 0) + int(m.group(1))
    print("cursor counters, first bundle:", sums)
    check("debug recording: cursor_path event and cursor counters on DRM",
          '"kind":"cursor_path"' in paths and len(sums) == 3
          and sums["cursor_plane_frames"] + sums["cursor_composited_frames"] > 0
          and sums["pointer_frame_requests"] > 0,
          f"{paths.strip().splitlines()[:1]} {sums}")
    sh(vm, "systemctl stop sash-session; chvt 1; sleep 1")

    # 17b. Resume timing points (M9e): the recording build in session mode, one window,
    # a VT switch away and back; the session's events split the resume into its parts.
    sh(vm, "rm -rf /home/sash/rec17b")
    start(vm, binary=RECORD_BIN,
          env="SASH_DEBUG_RECORD=/home/sash/rec17b SASH_DEBUG_RECORD_SESSION=1", windows=1)
    vm.keys("ctrl", "alt", "f1")
    time.sleep(1.5)
    vm.keys("ctrl", "alt", "f2")
    time.sleep(2)
    vm.pointer_rel(5, 5)
    time.sleep(1)
    steps = sh(vm, "grep -h resume_step /home/sash/rec17b/session-*/events.jsonl")
    print("resume timing points:\n" + steps)
    wanted = ["resume start", "activate(true)", "DRM state reset", "libinput restored",
              "first frame start", "first frame drawn", "first frame committed (modeset)",
              "first flip", "first input event"]
    seen = [w for w in wanted if f'"step":"{w}"' in steps]
    check("resume timing points: every step of a VT switch back is recorded (M9e)",
          seen == wanted, f"{len(seen)} of {len(wanted)}: missing {sorted(set(wanted) - set(seen))}")
    sh(vm, "systemctl stop sash-session; chvt 1; sleep 1")

    # 17d. Renderer recovery (M11a, debug build with fault injection): a lost context at
    # frame 30 rebuilds the renderer and frames go on; every swap failing gives up after
    # the retries and one rebuild, and the clean exit gives the console back; flips that
    # stop trip the 2 s watchdog, and recovery ends the same way.
    start(vm, binary=DEBUG, env="SASH_DEBUG_CONTEXT_LOST=30", windows=1)
    for i in range(60):
        vm.pointer_abs(100 + 5 * i, 100 + 3 * i, W, H)
        time.sleep(0.03)
    time.sleep(1.5)
    frames_before = count(vm, "frame frame_time")
    for i in range(30):
        vm.pointer_abs(400 - 5 * i, 300, W, H)
        time.sleep(0.03)
    time.sleep(1)
    frames_after = count(vm, "frame frame_time")
    rebuilt, recovered = count(vm, "renderer rebuilt"), count(vm, "renderer recovered")
    check("renderer recovery: a lost context rebuilds the renderer once and frames go on",
          rebuilt == 1 and recovered >= 1 and frames_after > frames_before
          and active(vm) == "active",
          f"rebuilt {rebuilt}, recovered {recovered}, frames {frames_before} -> {frames_after}")
    sh(vm, "systemctl stop sash-session; chvt 1; sleep 1")

    start(vm, binary=DEBUG, env="SASH_DEBUG_FAIL_SWAPS=1000", windows=0)
    for i in range(20):
        vm.pointer_abs(200 + 10 * i, 200, W, H)
        time.sleep(0.05)
    time.sleep(8)
    gave_up = count(vm, "could not be recovered")
    retries, rebuilt = count(vm, "renderer retry"), count(vm, "renderer rebuilt")
    check("renderer recovery: every swap failing: retries, one rebuild, then a clean exit "
          "with the console back",
          gave_up >= 1 and rebuilt == 1 and retries >= 2 and active(vm) != "active"
          and vt_mode(vm) == "text",
          f"retries {retries}, rebuilt {rebuilt}, gave up {gave_up}, unit {active(vm)}, "
          f"VT 2 {vt_mode(vm)}")
    sh(vm, "systemctl stop sash-session 2>/dev/null; chvt 1; sleep 1")

    start(vm, binary=DEBUG, env="SASH_DEBUG_STOP_FLIPS=20", windows=0)
    for i in range(60):
        vm.pointer_abs(200 + 5 * i, 200, W, H)
        time.sleep(0.05)
    time.sleep(12)
    watchdog = count(vm, "no page flip for 2 s")
    gave_up = count(vm, "could not be recovered")
    check("renderer recovery: flips that stop trip the watchdog; recovery gives up "
          "cleanly when they never come back",
          watchdog >= 1 and gave_up >= 1 and active(vm) != "active" and vt_mode(vm) == "text",
          f"watchdog {watchdog}, gave up {gave_up}, unit {active(vm)}, VT 2 {vt_mode(vm)}")
    sh(vm, "systemctl stop sash-session 2>/dev/null; chvt 1; sleep 1")

    # 17c. Clipboard (M10): copy in one client, paste in another, on the DRM backend.
    # wl-copy and wl-paste (wl-clipboard, apt in the guest; owner, M10) each map a small
    # window to get keyboard focus, since sash-wm offers no data-control protocol.
    if sh(vm, "command -v wl-copy || true").strip() == "":
        sh(vm, "DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends "
               "wl-clipboard >/dev/null 2>&1; true", timeout=600)
    start(vm, windows=1)
    as_user = ("runuser -u sash -- env XDG_RUNTIME_DIR=/run/user/$(id -u sash) "
               "WAYLAND_DISPLAY=wayland-1")
    sh(vm, f"cd /tmp && printf 'sash-vm-clipboard-check' | {as_user} timeout 10 wl-copy; sleep 1")
    pasted = sh(vm, f"cd /tmp && {as_user} timeout 10 wl-paste -n 2>&1; true")
    sh(vm, f"cd /tmp && printf 'second copy' | {as_user} timeout 10 wl-copy; sleep 1")
    pasted2 = sh(vm, f"cd /tmp && {as_user} timeout 10 wl-paste -n 2>&1; true")
    check("clipboard: wl-copy then wl-paste between two clients on DRM; a second copy "
          "replaces the first",
          pasted.strip() == "sash-vm-clipboard-check" and pasted2.strip() == "second copy"
          and active(vm) == "active",
          f"pasted {pasted.strip()!r}, then {pasted2.strip()!r}")
    # wl-copy, serving the selection in the background, exits with the session.
    sh(vm, "systemctl stop sash-session; chvt 1; sleep 1")

    # 18. A session recorded on the DRM backend for replay (tools/replay.sh): windows
    # spawned by Super+Return with the pointer in different places (rule A), a float, a
    # Super+drag, a snap, a close; Super+F11 writes the bundle. Nothing is typed. The
    # bundle's events.jsonl and state.json are copied out (the replay fixture).
    sh(vm, "rm -rf /home/sash/rec18")
    start(vm, binary=RECORD_BIN, env="SASH_DEBUG_RECORD=/home/sash/rec18", windows=0)
    w18, h18, _ = output_mode(vm)
    # The third spawn has the pointer over window 1 while window 2 has focus: rule A
    # splits window 1 (the old rule split window 2).
    for at in [(w18 // 4, h18 // 2), (w18 // 4, h18 // 4), (w18 // 4, 3 * h18 // 4)]:
        vm.pointer_abs(at[0], at[1], w18, h18)
        time.sleep(0.3)
        vm.keys("meta_l", "ret")
        time.sleep(2)
    vm.keys("meta_l", "f")  # float the newest window
    time.sleep(0.8)
    vm.key_down("meta_l")  # Super+drag it
    vm.pointer_abs(3 * w18 // 4, h18 // 2, w18, h18)
    vm.button("left", True)
    for i in range(20):
        vm.pointer_abs(3 * w18 // 4 - 10 * i, h18 // 2 - 5 * i, w18, h18)
        time.sleep(0.03)
    vm.button("left", False)
    vm.key_down("meta_l", False)
    time.sleep(0.8)
    vm.keys("meta_l", "left")  # snap it to the left half
    time.sleep(0.8)
    vm.pointer_abs(w18 // 8, h18 // 8, w18, h18)
    vm.button("left", True)  # click focuses
    vm.button("left", False)
    time.sleep(0.5)
    vm.keys("meta_l", "q")  # close the focused window
    time.sleep(1.5)
    vm.keys("meta_l", "f11")
    time.sleep(2)
    b18 = sh(vm, "ls -d /home/sash/rec18/bundle-* | tail -1").strip()
    fixture = os.path.join(OUT, "18-replay-bundle")
    os.makedirs(fixture, exist_ok=True)
    for name in ("events.jsonl", "state.json"):
        encoded = sh(vm, f"gzip -c {b18}/{name} | base64 -w0", timeout=300)
        with open(os.path.join(fixture, name), "wb") as f:
            f.write(gzip.decompress(base64.b64decode(encoded)))
    typed = sh(vm, f"grep -c printable {b18}/events.jsonl || true").strip()
    kinds18 = sh(vm, f"grep -oE '\"kind\":\"[a-z_]+\"' {b18}/events.jsonl | sort | uniq -c")
    print("bundle 18:", b18, "\n" + kinds18)
    check("a DRM session recorded for replay (copied to 18-replay-bundle)",
          b18 != "" and typed == "0" and '"kind":"layout"' in kinds18
          and '"kind":"pointer_button"' in kinds18,
          f"printable keys: {typed}")
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
