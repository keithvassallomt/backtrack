#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>
#
# Runs inside a fresh Fedora container (`just rpm`): builds the RPMs and the
# source RPM COPR takes, lints them, and installs the main package.
set -euo pipefail
dnf install -y -q rpm-build dnf-plugins-core rpmlint >/dev/null
dnf builddep -y -q /root/rpmbuild/SPECS/backtrack.spec >/dev/null
rpmbuild -ba /root/rpmbuild/SPECS/backtrack.spec
echo "== rpmlint"
rpmlint /root/rpmbuild/SPECS/backtrack.spec /root/rpmbuild/RPMS/*/*.rpm /root/rpmbuild/SRPMS/*.rpm || true
echo "== installed"
dnf install -y -q /root/rpmbuild/RPMS/x86_64/backtrack-[0-9]*.rpm >/dev/null
backtrack --version
backtrack-gtk --version
rpm -ql backtrack
