#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
#
# Runs as root inside the Debian 13 test guest (once): the runtime packages sash-wm's
# DRM backend and the test clients need (and sshd, for the remote-login check), and an
# unprivileged user for the session.
set -e
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends \
    libseat1 libinput10 libgbm1 libegl1 libegl-mesa0 libgles2 libgl1-mesa-dri \
    libxkbcommon0 foot fonts-dejavu-core kbd procps python3-minimal openssh-server \
    wl-clipboard adwaita-icon-theme
id sash >/dev/null 2>&1 || useradd -m -s /bin/bash sash
echo "guest ready"
