# Stage 13 — Packaging & Release

## Objective
Installable three ways — Flatpak (portal-correct, offline-buildable), Fedora RPM
(COPR), Ubuntu deb (PPA) — with a CI release pipeline and a human-controlled
release runbook. Both packaging paths were mandated from day one; CI has been
building them since Stage 0 grew them — this stage completes and hardens them.

## Prerequisites
All prior stages functional. Read [../reference/stack.md](../reference/stack.md)
§9 and the app-id/license facts in [../reference/brief.md](../reference/brief.md).
[Stage 14](stage-14-flatpak-first-class.md) (the Flatpak as a first-class build)
runs in the middle of this one: S13-T1 and S13-T2 first, then Stage 14, then
S13-T3. Its order of work says why.

## Tasks

### S13-T1 — Flatpak manifest
`packaging/flatpak/io.github.keithvassallomt.Backtrack.yml`:
- Runtime `org.gnome.Platform` / SDK `org.gnome.Sdk` (current stable at
  implementation time; pin exact version in the manifest).
- Modules: borgbackup (build from pip sources with its deps — msgpack etc. — as a
  python module with generated `python3-requirements.json`; pin borg 1.4.x),
  btrfs-progs (the command only, S14-T8), then backtrack (cargo, offline via T2
  sources).
- App + daemon in one Flatpak; daemon launched via the app's background portal
  autostart (S12-T4), `--command=backtrackd` for the autostart entry, through
  S14-T7's supervisor.
- **Portals over permissions** wherever a portal exists: FileChooser (Restore To…,
  key export), Background/Autostart, OpenURI. Static permissions justified in a
  comment per line. Corrected 2026-10-12: the first list read only
  `--filesystem=home`, which misses removable drives under `/run/media` and
  `/media`, folders chosen outside home, and every other Flatpak's data.
  - `--filesystem=host`: reading everything the user chose to back up, wherever
    it is, and restoring anywhere. Document why portal-per-file is impossible
    for a backup daemon.
  - `--filesystem=~/.var/app`: Flatpak hides it behind an empty tmpfs even with
    `host`, so without this, other Flatpaks' data (mail, game saves) would be
    left out of every backup with no warning.
  - `--share=network`, `--socket=ssh-auth`: ssh destinations, with the user's
    agent.
  - `--talk-name=org.gtk.vfs.*`, `--filesystem=xdg-run/gvfs`,
    `--filesystem=xdg-run/gvfsd`: network shares, and the wizard's drive list.
  - `--talk-name=org.freedesktop.Notifications`: the daemon's notifications and
    their click handling use this interface directly (Stage 10).
  - `--talk-name=org.freedesktop.secrets` (S14-T6) and
    `--talk-name=org.kde.StatusNotifierWatcher` (S14-T3).
  - `--system-talk-name=org.freedesktop.UPower` and
    `--system-talk-name=org.freedesktop.NetworkManager`: the battery and metered
    checks. Without them both read as unknown, and backups run anyway.
  - `--socket=wayland`, `--socket=fallback-x11`, `--share=ipc`.
  - No `--own-name`: the daemon's bus name is under the application ID (S14-T1).

  NO `--device=all`, NO `--socket=system-bus`, NO
  `--talk-name=org.freedesktop.Flatpak`.
- Metainfo: `packaging/io.github.keithvassallomt.Backtrack.metainfo.xml`
  (AppStream: screenshots = the mockups hosted in-repo, release notes fed from
  CHANGELOG at release time) + desktop file + icon set (symbolic + scalable —
  request icon assets from the human; placeholder acceptable until then).
**Accept:** `flatpak-builder --install` produces a working app: wizard → backup to
a `--filesystem`-visible destination → restore round-trip INSIDE the sandbox
(needs S14-T1, without which the window cannot start the daemon);
`flatpak run --command=sh` probe confirms no println-era logging leaks to journal.

### S13-T2 — Offline sources
`just flatpak-sources`: regenerate `cargo-sources.json` via
`flatpak-cargo-generator.py` (vendor the script in `packaging/flatpak/tools/`)
from Cargo.lock, and `python3-requirements.json` for borg via
`flatpak-pip-generator`. Node is **N/A** — this project contains no JS; do not add
node-sources (recorded decision). CI job builds the Flatpak fully offline
(`--sandbox --disallow-network` equivalent) to prove sources are complete.
**Accept:** clean CI runner builds Flatpak with network cut post-download; sources
regenerate reproducibly (recipe is idempotent — second run yields no diff).

### S13-T3 — Portal & sandbox test matrix
Documented checklist executed on GNOME and KDE hosts, Flatpak build:
FileChooser portal (Restore To…, recovery-key save), Notification portal
(health states), Background portal (daemon survives session login, appears in
GNOME background apps, and activating it there opens the main window: S12-T4's
acceptance, which only a Flatpak can meet), keyring access, SMB destination via gvfs, ssh destination,
Wayland + Xorg session each. Also: a drive plugged in after the daemon started
is seen and backed up to; another Flatpak's data under `~/.var/app` is in the
backup; and every Stage 14 task's in-sandbox check, repeated on both desktops.
Failures become issues; matrix result committed to
`packaging/flatpak/TEST-MATRIX.md` with date.
**Accept:** matrix fully green, committed, dated.

### S13-T4 — RPM + deb
- RPM: `packaging/rpm/backtrack.spec` — subpackages `backtrack` (bins, systemd
  user units + dbus service file, metainfo/desktop/icons), `backtrack-nautilus`
  (extension, Requires nautilus-python), `backtrack-dolphin` (servicemenu).
  `just rpm` builds in mock/fedora container. COPR project wired to CI (webhook
  or `copr-cli` on tag).
- deb: `packaging/deb/` (debhelper, dh-cargo or vendored build) same three-way
  split; `just deb`; PPA upload documented (human runs dput — key handling stays
  human).
- Post-install behaviour verified: units enabled via preset on first install
  (`systemctl --user preset`), nautilus/dolphin pick up plugins after FM restart
  (documented in package READMEs).
**Accept:** `just rpm && just deb` produce installable packages in containers;
fresh-VM install → wizard runs (documented smoke checklist for each distro).

### S13-T5 — Release pipeline
CI on tag `v*`: verify tag == workspace version (`just verify-version` vs tag —
mismatch fails loudly = human forgot bump-version or vice versa), build all three
artifacts, attach to GitHub release with CHANGELOG section extracted as notes,
trigger COPR. The Flatpak artifact is a bundle (`.flatpak`). Publishing moved to
S14-T9: Backtrack goes on FriendlyHub, not Flathub (Keith, 2026-10-12).
**Accept:** dry-run tag `v0.1.0-rc1` on a branch produces the full artifact set.

### S13-T6 — Release runbook
`RELEASING.md`: 1) human decides version; 2) human runs
`just bump-version X.Y.Z`; 3) human moves CHANGELOG `[Unreleased]` → `[X.Y.Z] -
date`; 4) human commits + tags `vX.Y.Z`; 5) CI does the rest; 6) post-release:
human bumps nothing (next bump when next release is decided); AI's role during
release: NOTHING except being asked to fix red CI. Restate the version-control
contract in bold.
**Accept:** runbook exists; a full rc dry-run followed it verbatim without
improvisation.

## Definition of Done
All three install paths proven on fresh systems; portal matrix green; release
dry-run complete; progress.md fully `[x]` through Stage 13; CHANGELOG
`[Unreleased]` clean and ready for the human's 0.1.0 release decision.
