#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>
#
# Runs inside a fresh Ubuntu container (`just deb`): unpacks the orig
# tarballs as dpkg would, builds the source package the PPA takes and the
# binary packages, lints them, and installs the main package.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null
apt-get install -y -qq --no-install-recommends build-essential devscripts equivs lintian >/dev/null
cd /build
tree="$(basename backtrack_*.orig.tar.gz .orig.tar.gz | tr _ -)"
mkdir "$tree"
tar -xzf backtrack_*.orig.tar.gz -C "$tree" --strip-components=1
mkdir "$tree/vendor"
tar -xJf backtrack_*.orig-vendor.tar.xz -C "$tree/vendor" --strip-components=1
cp -r /debian "$tree/debian"
cd "$tree"
mk-build-deps -i -r -t "apt-get -y -qq --no-install-recommends" debian/control >/dev/null
rm -f ../*-build-deps_*
dpkg-buildpackage -us -uc
echo "== lintian"
lintian --no-tag-display-limit ../*.changes || true
echo "== installed"
apt-get install -y -qq ../backtrack_*_"$(dpkg --print-architecture)".deb >/dev/null
backtrack --version
backtrack-gtk --version
dpkg -L backtrack | grep -vE '/$|/\.$'
