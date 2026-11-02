# SPDX-License-Identifier: Apache-2.0
#
# Prepares the stage B test VM in ~/.cache/sash/vm (vm.CACHE): downloads the pinned
# Debian 13 image (or checks the one there against its SHA512), and sets up the guest
# once: runtime packages, the user "sash", and a throwaway SSH key for the remote-login
# check. The ISO with the binaries comes from tools/vm/build-iso.sh (same directory).
#
# Usage: python3 -I tools/vm/prepare.py [--fresh]   (--fresh: set the guest up again)

import os
import shutil
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import vm as vmlib  # noqa: E402

fresh = "--fresh" in sys.argv[1:]
image = vmlib.fetch_image()
iso = os.path.join(vmlib.CACHE, "sash.iso")
if not os.path.exists(iso):
    sys.exit(f"{iso} is missing; run tools/vm/build-iso.sh {vmlib.CACHE} first")

guest = os.path.join(vmlib.CACHE, "guest")
if fresh and os.path.isdir(guest):
    shutil.rmtree(guest)
os.makedirs(guest, exist_ok=True)
if os.path.exists(os.path.join(guest, "ready")):
    print(f"guest ready: {guest} (--fresh sets it up again)")
    sys.exit(0)

# A throwaway key for SSH into the guest; it opens nothing but the test guest.
ssh_dir = os.path.join(vmlib.CACHE, "ssh")
os.makedirs(ssh_dir, mode=0o700, exist_ok=True)
key = os.path.join(ssh_dir, "id_ed25519")
if not os.path.exists(key):
    subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "sash-test-vm",
                    "-f", key], check=True)
with open(key + ".pub") as f:
    public_key = f.read().strip()

vm = vmlib.VM(guest, image, iso)
vm.start()
try:
    vm.login()
    vm.run("mkdir -p /mnt/sash; mountpoint -q /mnt/sash || mount -o ro /dev/sr0 /mnt/sash")
    out, status = vm.run("sh /mnt/sash/guest-setup.sh 2>&1 | tail -2", timeout=1200)
    print(out)
    if "guest ready" not in out:
        raise RuntimeError("guest setup failed")
    vm.run("install -d -m 700 -o sash -g sash /home/sash/.ssh && "
           f"echo '{public_key}' > /home/sash/.ssh/authorized_keys && "
           "chown sash:sash /home/sash/.ssh/authorized_keys && "
           "chmod 600 /home/sash/.ssh/authorized_keys && sync")
    open(os.path.join(guest, "ready"), "w").close()
    print(f"guest ready: {guest}")
finally:
    vm.stop()
