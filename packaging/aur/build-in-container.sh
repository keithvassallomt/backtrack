#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>
#
# Runs inside a fresh Arch container (`just aur-build`): the clean environment
# `pkgctl build` gives in a clean chroot. /build holds the PKGBUILD and the
# source tarball; the packages are left there.
set -euo pipefail
pacman -Syu --noconfirm --needed base-devel namcap cargo gtk4 libadwaita borg >/dev/null
# makepkg refuses to run as root. The builder works in a copy, so nothing on
# the mounted folder ends up owned by a user the host does not have; only the
# packages are copied back, as root, which is the host's user.
useradd -m builder
cp -r /build /work
chown -R builder /work
# -d: the run-time dependencies namcap asks for are not installed (see the
# PKGBUILD); everything the build needs is.
su builder -c 'cd /work && makepkg -d --noconfirm'
cp /work/*.pkg.tar.zst /build/
echo "== namcap"
namcap /build/PKGBUILD /build/*.pkg.tar.zst || true
echo "== installed"
pacman -U --noconfirm /build/backtrack-[0-9]*.pkg.tar.zst >/dev/null
backtrack --version
backtrack-gtk --version
pacman -Qlq backtrack | grep -vE '/$'
