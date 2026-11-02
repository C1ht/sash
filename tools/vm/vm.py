# SPDX-License-Identifier: Apache-2.0
#
# Drives a QEMU/KVM guest for the DRM backend tests (M9 stage B): a root shell on the
# serial console, and QMP for keyboard and pointer input and screenshots of the guest's
# own display (screendump). Nothing is shown on the host's display (-display none).
#
# Used by tools/vm/scenario.py; see tools/vm/README.md.

import json
import os
import re
import shutil
import socket
import subprocess
import time


class VM:
    # The root password of the throwaway test guest (set at its first boot through a
    # systemd credential; the guest never leaves this machine and has no account of
    # the owner).
    PASSWORD = "sash"

    def __init__(self, workdir, image, iso, memory="4G", cpus=4, gpu="virtio-gpu-pci",
                 cpu="kvm64", firmware="uefi"):
        self.workdir = workdir
        # Socket paths must be short (108 bytes): they live in $XDG_RUNTIME_DIR.
        sockets = os.path.join(os.environ["XDG_RUNTIME_DIR"], "sash-vm")
        os.makedirs(sockets, mode=0o700, exist_ok=True)
        self.serial_path = os.path.join(sockets, "serial.sock")
        self.qmp_path = os.path.join(sockets, "qmp.sock")
        overlay = os.path.join(workdir, "guest.qcow2")
        if not os.path.exists(overlay):
            subprocess.run(["qemu-img", "create", "-q", "-f", "qcow2", "-b", os.path.abspath(image),
                            "-F", "qcow2", overlay, "8G"], check=True)
        self.args = [
            "qemu-system-x86_64", "-enable-kvm", "-cpu", cpu, "-m", memory,
            "-smp", str(cpus), "-nodefaults", "-display", "none", "-vga", "none",
            "-device", gpu,
            "-device", "virtio-keyboard-pci", "-device", "virtio-tablet-pci",
            "-device", "virtio-mouse-pci",
            "-drive", f"file={overlay},if=virtio,format=qcow2",
            "-drive", f"file={iso},media=cdrom,readonly=on",
            "-netdev", "user,id=net0", "-device", "virtio-net-pci,netdev=net0",
            "-chardev", f"socket,id=ser0,path={self.serial_path},server=on,wait=off",
            "-serial", "chardev:ser0",
            "-qmp", f"unix:{self.qmp_path},server=on,wait=off",
        ]
        # systemd credentials for the first boot: no first-boot questions, and the
        # guest's throwaway root password.
        for credential in (f"passwd.plaintext-password.root={self.PASSWORD}",
                           "firstboot.timezone=UTC", "firstboot.locale=C.UTF-8",
                           "firstboot.keymap=us"):
            self.args += ["-smbios", f"type=11,value=io.systemd.credential:{credential}"]
        if firmware == "uefi":
            # OVMF from edk2-ovmf; the variable store is a per-guest copy.
            vars_copy = os.path.join(workdir, "OVMF_VARS.4m.fd")
            if not os.path.exists(vars_copy):
                shutil.copyfile("/usr/share/edk2/x64/OVMF_VARS.4m.fd", vars_copy)
            self.args += [
                "-drive", "if=pflash,format=raw,readonly=on,file=/usr/share/edk2/x64/OVMF_CODE.4m.fd",
                "-drive", f"if=pflash,format=raw,file={vars_copy}",
            ]
        self.process = None
        self.serial = None
        self.qmp = None
        self.buffer = b""

    # Lifecycle -------------------------------------------------------------------

    def start(self):
        """Starts QEMU (detached; its PID goes to qemu.pid) and attaches to it."""
        self.process = subprocess.Popen(self.args, stdout=subprocess.DEVNULL,
                                        stderr=open(os.path.join(self.workdir, "qemu.log"), "w"),
                                        start_new_session=True)
        with open(os.path.join(self.workdir, "qemu.pid"), "w") as f:
            f.write(str(self.process.pid))
        for _ in range(100):
            if os.path.exists(self.serial_path) and os.path.exists(self.qmp_path):
                break
            time.sleep(0.1)
        self.attach()

    def attach(self):
        """Connects to the serial console and QMP of a running guest."""
        self.serial = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.serial.connect(self.serial_path)
        self.serial.settimeout(0.2)
        self.qmp = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.qmp.connect(self.qmp_path)
        self.qmp_file = self.qmp.makefile("rw")
        self.qmp_file.readline()  # greeting
        self.command("qmp_capabilities")

    def stop(self):
        """Ends QEMU through QMP (the guest is not shut down cleanly)."""
        try:
            self.command("quit")
        except (OSError, RuntimeError):
            pass
        if self.process:
            self.process.wait(timeout=30)

    # Serial console ----------------------------------------------------------------

    def read_until(self, pattern, timeout=60):
        deadline = time.monotonic() + timeout
        regex = re.compile(pattern.encode() if isinstance(pattern, str) else pattern)
        while time.monotonic() < deadline:
            match = regex.search(self.buffer)
            if match:
                out = self.buffer[: match.end()]
                self.buffer = self.buffer[match.end():]
                return out.decode(errors="replace")
            try:
                data = self.serial.recv(65536)
                if data:
                    self.buffer += data
            except socket.timeout:
                pass
        raise TimeoutError(f"waiting for {pattern!r}; last output: {self.buffer[-500:]!r}")

    def write(self, text):
        self.serial.sendall(text.encode())

    def login(self):
        """Logs in as root on the serial console."""
        self.write("\r")
        self.read_until(r"login: ", timeout=300)
        self.write("root\r")
        self.read_until(r"Password: ", timeout=30)
        self.write(self.PASSWORD + "\r")
        self.read_until(r"# ", timeout=30)
        # No job control: background jobs print no notices into later output.
        self.run("set +m; stty -echo cols 200; export PS1='# '; export TERM=dumb")

    def run(self, command, timeout=120):
        """Runs a shell command as root on the serial console; returns (output, status)."""
        marker = f"__done_{time.monotonic_ns()}__"
        self.write(f"{command}; echo {marker} $?\n")
        out = self.read_until(marker + r" (\d+)\r?\n", timeout=timeout)
        body, _, tail = out.rpartition(marker)
        status = int(tail.split()[0])
        body = body.replace("\r", "").strip()
        # A prompt left over from an earlier command.
        while body.startswith("#"):
            body = body[1:].lstrip(" ")
        return body.strip(), status

    # QMP -----------------------------------------------------------------------------

    def command(self, name, **arguments):
        message = {"execute": name}
        if arguments:
            message["arguments"] = arguments
        self.qmp_file.write(json.dumps(message) + "\n")
        self.qmp_file.flush()
        while True:
            reply = json.loads(self.qmp_file.readline())
            if "return" in reply or "error" in reply:
                if "error" in reply:
                    raise RuntimeError(f"QMP {name}: {reply['error']}")
                return reply["return"]

    def screendump(self, path):
        """The guest's own display, as a PNG."""
        self.command("screendump", filename=os.path.abspath(path), format="png")

    def keys(self, *qcodes, hold=0.05):
        """Presses the keys in order, then releases them in reverse (QEMU qcodes)."""
        events = [{"type": "key", "data": {"down": True, "key": {"type": "qcode", "data": k}}}
                  for k in qcodes]
        self.command("input-send-event", events=events)
        time.sleep(hold)
        events = [{"type": "key", "data": {"down": False, "key": {"type": "qcode", "data": k}}}
                  for k in reversed(qcodes)]
        self.command("input-send-event", events=events)
        time.sleep(0.05)

    def key_down(self, qcode, down=True):
        self.command("input-send-event", events=[
            {"type": "key", "data": {"down": down, "key": {"type": "qcode", "data": qcode}}}])

    def type_text(self, text):
        names = {" ": "spc", "\n": "ret", "-": "minus", ".": "dot", "/": "slash"}
        for ch in text:
            self.keys(names.get(ch, ch))

    def pointer_abs(self, x, y, width, height):
        """Moves the tablet pointer to (x, y) in a width x height output."""
        scale = lambda v, size: int(v * 0x7fff / max(size - 1, 1))
        self.command("input-send-event", events=[
            {"type": "abs", "data": {"axis": "x", "value": scale(x, width)}},
            {"type": "abs", "data": {"axis": "y", "value": scale(y, height)}}])

    def pointer_rel(self, dx, dy):
        self.command("input-send-event", events=[
            {"type": "rel", "data": {"axis": "x", "value": dx}},
            {"type": "rel", "data": {"axis": "y", "value": dy}}])

    def button(self, name, down):
        self.command("input-send-event", events=[
            {"type": "btn", "data": {"down": down, "button": name}}])
