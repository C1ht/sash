#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
#
# Runs as root inside the test guest: starts foot as the session user on sash-wm's
# Wayland socket, titled and labelled with N. Usage: foot.sh N
exec runuser -u sash -- sh -c "cd /home/sash && XDG_RUNTIME_DIR=/run/user/$(id -u sash) \
    WAYLAND_DISPLAY=wayland-1 exec foot -e sh -c 'printf \"\\n  foot $1\\n\"; exec bash'" \
    > "/tmp/foot$1.log" 2>&1
