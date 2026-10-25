# SPDX-License-Identifier: Apache-2.0
#
# Sends synthetic input commands to sash-wm (debug build) and prints the replies.
# Usage: python3 -I tools/measure/synth.py SOCKET_PATH "cmd1" "cmd2" ...
import socket, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(sys.argv[1])
f = s.makefile("rw")
for cmd in sys.argv[2:]:
    f.write(cmd + "\n"); f.flush()
    print(cmd, "->", f.readline().strip())
