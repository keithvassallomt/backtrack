#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>
#
# Runs inside a fresh Ubuntu container for `just release`: builds one Ubuntu
# series' source package from /build (its orig tarballs and debian/). With
# UPLOAD set, signs it as KEY through the host's gpg agent, whose restricted
# socket is mounted in (only the public key is imported here), and uploads it.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null
apt-get install -y -qq --no-install-recommends dpkg-dev devscripts debhelper dput gnupg >/dev/null
cd /build
tree="$(basename backtrack_*.orig.tar.gz .orig.tar.gz | tr _ -)"
mkdir "$tree"
tar -xzf backtrack_*.orig.tar.gz -C "$tree" --strip-components=1
mkdir "$tree/vendor"
tar -xJf backtrack_*.orig-vendor.tar.xz -C "$tree/vendor" --strip-components=1
cp -r debian "$tree/debian"
cd "$tree"
# Source only, with the orig tarballs included (-sa, which a version's first
# upload needs); Launchpad installs the build dependencies itself (-d).
dpkg-buildpackage -S -sa -d -us -uc
cd /build
if [ -n "${UPLOAD:-}" ]; then
    chmod 700 /root/.gnupg
    gpg --batch --quiet --import key.asc
    debsign -k"${KEY}" ./*_source.changes
    dput "${UPLOAD}" ./*_source.changes
fi
ls ./*.dsc ./*_source.changes
