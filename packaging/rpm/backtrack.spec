# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>
#
# Backtrack for Fedora 44 and later, built on COPR (S13-T4). Split three ways
# like the deb and the AUR package: the window, daemon, tray and CLI, then
# each file manager's integration. Built offline from the release's vendored
# crates (Source1), as Launchpad requires of the deb, so one source set
# serves both. `just rpm` builds it in a Fedora container.
#
# Version is a version location: `just bump-version` sets it, and adds the
# changelog entry. What rpmlint still says, and why it stands: no manual pages
# (every binary answers --help, and the documentation is the README), and no
# documentation in the two integrations, which hold one file each.

Name:           backtrack
Version:        0.1.0~rc3
Release:        1%{?dist}
Summary:        Browse your files as they were: backups you can step back through
License:        GPL-3.0-or-later
URL:            https://backtrack.tools
# A pre-release's Version spells rc with a tilde (0.1.0~rc1), so that it sorts
# before the release; its tag, tarballs and their directory keep the hyphen.
Source0:        https://github.com/keithvassallomt/backtrack/archive/v%{version_no_tilde}/backtrack-%{version_no_tilde}.tar.gz
Source1:        https://github.com/keithvassallomt/backtrack/releases/download/v%{version_no_tilde}/backtrack-%{version_no_tilde}-vendor.tar.xz

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  gcc
BuildRequires:  pkgconfig(gtk4) >= 4.22
BuildRequires:  pkgconfig(libadwaita-1) >= 1.9
BuildRequires:  desktop-file-utils
BuildRequires:  appstream
BuildRequires:  systemd-rpm-macros

Requires:       borgbackup >= 1.2
Requires:       hicolor-icon-theme
Recommends:     (%{name}-nautilus if nautilus)
Recommends:     (%{name}-dolphin if dolphin)
# SMB shares as destinations: the wizard mounts them through GVfs, which a
# Plasma desktop often lacks, and Borg reaches them through GVfs's FUSE folder.
Recommends:     gvfs-smb
Recommends:     gvfs-fuse

%description
Backtrack keeps your files backed up and lets you move back through time to
bring back what you lost. Pick a folder, step back to this morning or last
week, and restore a file without needing to know how backups work. It is
built on Borg Backup.

%package nautilus
Summary:        Restore Previous Version in GNOME Files, for Backtrack
BuildArch:      noarch
Requires:       %{name} = %{version}-%{release}
Requires:       nautilus-python

%description nautilus
Adds Restore Previous Version and Browse Backups of This Folder to GNOME
Files, opening Backtrack at the file or folder.

%package dolphin
Summary:        Restore Previous Version in Dolphin, for Backtrack
BuildArch:      noarch
Requires:       %{name} = %{version}-%{release}
Requires:       dolphin

%description dolphin
Adds Restore Previous Version and Browse Backups of This Folder to Dolphin,
opening Backtrack at the file or folder.

%prep
%autosetup -n backtrack-%{version_no_tilde} -a 1
mkdir -p .cargo
cat > .cargo/config.toml <<'TOML'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
TOML

%build
# The three packages in one build, so their shared crates are compiled with
# the same features (see `just install-units`).
cargo build --release --frozen --offline -p backtrackd -p backtrack-gtk -p backtrack-cli

%install
install -Dm755 -t %{buildroot}%{_bindir} \
    target/release/backtrackd target/release/backtrack-gtk \
    target/release/backtrack-tray target/release/backtrack
install -Dm644 -t %{buildroot}%{_userunitdir} packaging/systemd/backtrackd.service
install -Dm644 -t %{buildroot}%{_datadir}/dbus-1/services \
    packaging/dbus/tools.backtrack.Backtrack.Daemon1.service \
    packaging/dbus/tools.backtrack.Backtrack.service
desktop-file-install --dir=%{buildroot}%{_datadir}/applications \
    packaging/desktop/tools.backtrack.Backtrack.desktop
install -Dm644 -t %{buildroot}%{_metainfodir} packaging/tools.backtrack.Backtrack.metainfo.xml
install -Dm644 -t %{buildroot}%{_datadir}/icons/hicolor/scalable/apps \
    packaging/icons/hicolor/scalable/apps/tools.backtrack.Backtrack.svg
install -Dm644 -t %{buildroot}%{_datadir}/icons/hicolor/symbolic/apps \
    packaging/icons/hicolor/symbolic/apps/tools.backtrack.Backtrack-symbolic.svg
install -Dm644 -t %{buildroot}%{_sysconfdir}/xdg/autostart packaging/autostart/backtrack-tray.desktop
install -Dm644 -t %{buildroot}%{_datadir}/nautilus-python/extensions integrations/nautilus/backtrack.py
install -Dm644 -t %{buildroot}%{_datadir}/kio/servicemenus \
    integrations/dolphin/backtrack.desktop integrations/dolphin/backtrack-folder.desktop

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/tools.backtrack.Backtrack.desktop
appstreamcli validate --no-net %{buildroot}%{_metainfodir}/tools.backtrack.Backtrack.metainfo.xml

# The unit is not enabled here: the daemon enables its own when "Run in
# background" is on (background::apply). These keep systemd's view current,
# and disable the unit when the package is removed.
%post
%systemd_user_post backtrackd.service

%preun
%systemd_user_preun backtrackd.service

%files
%license LICENSE
%doc README.md
%{_bindir}/backtrackd
%{_bindir}/backtrack-gtk
%{_bindir}/backtrack-tray
%{_bindir}/backtrack
%{_userunitdir}/backtrackd.service
%{_datadir}/dbus-1/services/tools.backtrack.Backtrack.Daemon1.service
%{_datadir}/dbus-1/services/tools.backtrack.Backtrack.service
%{_datadir}/applications/tools.backtrack.Backtrack.desktop
%{_metainfodir}/tools.backtrack.Backtrack.metainfo.xml
%{_datadir}/icons/hicolor/scalable/apps/tools.backtrack.Backtrack.svg
%{_datadir}/icons/hicolor/symbolic/apps/tools.backtrack.Backtrack-symbolic.svg
%config(noreplace) %{_sysconfdir}/xdg/autostart/backtrack-tray.desktop

%files nautilus
%{_datadir}/nautilus-python/extensions/backtrack.py

%files dolphin
%{_datadir}/kio/servicemenus/backtrack.desktop
%{_datadir}/kio/servicemenus/backtrack-folder.desktop

%changelog
* Sun Oct 18 2026 Keith Vassallo <keith@vassallo.cloud> - 0.1.0~rc3-1
- New upstream release.

* Sun Oct 18 2026 Keith Vassallo <keith@vassallo.cloud> - 0.1.0~rc2-1
- New upstream release.

* Sun Oct 18 2026 Keith Vassallo <keith@vassallo.cloud> - 0.1.0~rc1-1
- New upstream release.

* Mon Oct 12 2026 Keith Vassallo <keith@vassallo.cloud> - 0.1.0-1
- Initial release.
