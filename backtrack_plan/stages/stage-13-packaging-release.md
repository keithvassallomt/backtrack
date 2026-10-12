# Stage 13 — Packaging & Release

## Objective
Installable four ways: the Flatpak (portal-correct, offline-buildable, published
on FriendlyHub), a Fedora RPM (COPR, Fedora 44 and later), an Ubuntu deb (PPA,
Ubuntu 26.04 and later) and an Arch package (AUR). Every other distribution,
Debian and RHEL included, uses the Flatpak. One command, run by the human,
takes a release from tag to every channel (`just release`). Both packaging
paths were mandated from day one, and CI has been building them since Stage 0
grew them; this stage completes and hardens them.

## Prerequisites
All prior stages functional. Read [../reference/stack.md](../reference/stack.md)
§9 and the app-id/license facts in [../reference/brief.md](../reference/brief.md).

Order of work: S13-T7 first, since every file this stage creates is named for
the application ID. Then S13-T1 and S13-T2, then
[Stage 14](stage-14-flatpak-first-class.md) (the Flatpak as a first-class
build), then S13-T3; Stage 14's order of work says why. S13-T4 and S13-T8 can
go any time after S13-T7. S13-T5, S13-T9 and S13-T6 come last, in that order.

## Tasks

### S13-T1 — Flatpak manifest
`packaging/flatpak/tools.backtrack.Backtrack.yml`:
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
  - No `--own-name`: the daemon's bus name is under the application ID (S13-T7).

  NO `--device=all`, NO `--socket=system-bus`, NO
  `--talk-name=org.freedesktop.Flatpak`.
- Metainfo: `packaging/tools.backtrack.Backtrack.metainfo.xml`
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
  `just rpm` builds in mock/fedora container. COPR project built for Fedora 44
  and later (44, 45, rawhide), and its description says so: GTK 4.22 and
  libadwaita 1.9 are the floor, which Fedora 43 does not meet. Builds are
  submitted by `just release` (S13-T9).
- deb: `packaging/deb/` (debhelper, vendored build, since Launchpad's builders
  have no network) same three-way split; `just deb`. PPA for Ubuntu 26.04 and
  later, and its description says so; one upload per supported series, listed
  in `packaging/deb/series`. Uploads are signed and sent by `just release`
  (S13-T9) on Keith's machine, with his key, which never leaves it.
- Check that Ubuntu 26.04's archive meets the floor (GTK 4.22, libadwaita 1.9,
  a rustc new enough for the workspace). If it does not, the PPA starts at the
  first release that does, and its description says that instead.
- Post-install behaviour verified: units enabled via preset on first install
  (`systemctl --user preset`), nautilus/dolphin pick up plugins after FM restart
  (documented in package READMEs).
**Accept:** `just rpm && just deb` produce installable packages in containers;
fresh-VM install → wizard runs (documented smoke checklist for each distro).

### S13-T5 — Release pipeline
CI on tag `v*`: verify the tag equals the workspace version (`just
verify-version` against the tag; a mismatch fails loudly, meaning the human
forgot bump-version or vice versa), build every artifact, and attach them to a
GitHub release with the CHANGELOG section as its notes: the Flatpak bundle
(`.flatpak`), the RPMs and the source RPM, the debs for Ubuntu 26.04, a source
tarball with the crates vendored (what the PPA and the AUR build from), and
`SHA256SUMS`. A pre-release tag (`v0.1.0-rc1`) makes a GitHub pre-release.
CI publishes nowhere else, and holds no credential beyond its GitHub token:
COPR, the PPA, the AUR and FriendlyHub are `just release`'s (S13-T9), so every
credential lives on one machine. Backtrack goes on FriendlyHub, not Flathub
(Keith, 2026-10-12); what FriendlyHub takes is S14-T9.
**Accept:** dry-run tag `v0.1.0-rc1` on a branch produces the full artifact set.

### S13-T6 — Release runbook
`RELEASING.md`: 1) human decides version; 2) human runs
`just bump-version X.Y.Z`; 3) human moves CHANGELOG `[Unreleased]` → `[X.Y.Z] -
date`; 4) human commits and merges to main; 5) human runs `just release`
(S13-T9), which tags, waits for CI and publishes to every channel; 6) human
reads the summary it prints, and runs it again if a channel failed; 7)
post-release: human bumps nothing (next bump when next release is decided).
Also covers renewing the credentials `just release` checks (COPR's API token
expires) and what to do when a channel rejects a release. AI's role during
release: NOTHING except being asked to fix red CI; `just release`, like `just
bump-version`, is run by the human only. Restate the version-control contract
in bold.
**Accept:** runbook exists; a full rc dry-run followed it verbatim without
improvisation.

### S13-T7 — Application ID under backtrack.tools
Backtrack's domain is backtrack.tools (Keith, 2026-10-12), so its names move
under it, all at once, before any packaging file is named for the old ones.
`org.backtrack` was never a domain anyone held.
- Application ID `io.github.keithvassallomt.Backtrack` becomes
  `tools.backtrack.Backtrack`: the GApplication, the launcher, the icons, the
  metainfo, the Background portal request, the keyring's `app-id` attribute,
  the tray's icon name, and the docs.
- The daemon's bus name and interface, both `org.backtrack.Daemon1` today,
  become `tools.backtrack.Backtrack.Daemon1`, with object path
  `/tools/backtrack/Backtrack/Daemon1` and development bus name
  `tools.backtrack.Backtrack.Daemon1.Dev`. One name for bus and interface,
  under the application ID, which is what lets Flatpak export the daemon's
  activation file (S14-T1).
- Packaging files renamed to match (`packaging/dbus/`, the launcher); `units.rs`'s
  agreement check and the interface's introspection snapshot regenerated;
  `reference/stack.md` §2 and VERSIONING.md's location table follow.
- Nothing has been released, so there is no migration code. Development
  machines and the VMs lose the passphrase stored under the old `app-id`; the
  recipes that store the demo passphrase use the new one, and running them
  again puts it back.

**Accept:** `just check` green; a search for `io.github.keithvassallomt`,
`org.backtrack` and `/org/backtrack` finds nothing outside `docs/superpowers/`
(Stage 2's historical working documents); on
both VMs after a push, the window starts the daemon through the bus, the tray
shows its state, a remembered passphrase is found, and the launcher shows
Backtrack under the new ID.

### S13-T8 — AUR packages
`packaging/aur/PKGBUILD`, split the same three ways as the RPM and the deb:
`pkgbase=backtrack`, `pkgname=(backtrack backtrack-nautilus backtrack-dolphin)`.
Split rather than optional dependencies because the window decides whether a
plugin is installed by looking for its file (S12-T5), and a Nautilus extension
shipped without nautilus-python would report an integration that does nothing.
- `backtrack` depends on `gtk4`, `libadwaita` and `borg`, makedepends on
  `cargo`, and builds from the release's source tarball (`cargo build --frozen
  --release`). It installs the binaries, user units, D-Bus activation files,
  launcher, icons, metainfo and the tray's autostart entry.
- `backtrack-nautilus` depends on `python-nautilus`; `backtrack-dolphin` on
  `dolphin`.
- No install scriptlet enables units: the daemon enables its own when "Run in
  background" is on (`background::apply`).
- `pkgver` is a version location: `just bump-version` updates it, `just
  verify-version` checks it, and VERSIONING.md gains the row. `just release`
  (S13-T9) fills in the checksums in the AUR's own repository; the copy here
  carries none, so a release leaves nothing to commit.
- The name `backtrack` is free on the AUR and in Arch's repositories (checked
  2026-10-12).

**Accept:** `pkgctl build` builds all three packages in a clean chroot;
`namcap` warnings are fixed or explained in the PKGBUILD; installed on Keith's
Arch machine, the wizard runs and a backup completes; `.SRCINFO` regenerates
with no diff.

### S13-T9 — `just release`
One command, run by the human once `just bump-version` and the CHANGELOG move
are merged to main, that takes a release from tag to every channel. It never
changes a version: it reads the one `just bump-version` set, and refuses if
anything disagrees with it.
1. **Preflight, before anything is published.** On `main` (a pre-release may
   run from a branch), clean, level with `origin`; `just verify-version` and
   `just check` pass; CHANGELOG has a dated section for this version and an
   empty `[Unreleased]`; every credential works: `gh auth status`, `copr-cli
   whoami` (with how long the COPR token has left), the PPA signing key (a test
   signature) and `ssh aur@aur.archlinux.org` (the AUR's greeting). Any
   failure stops the run and says what to fix.
2. **Tag and wait.** Creates and pushes the annotated tag `vX.Y.Z`, waits for
   S13-T5's workflow, and checks the GitHub release has every artifact and
   `SHA256SUMS`.
3. **COPR.** Submits the release's source RPM for Fedora 44 and later, and waits
   for the builds.
4. **PPA.** Builds a source package per series in `packaging/deb/series`, in an
   Ubuntu container, signs it on the host with Keith's key, uploads each with
   `dput`, and reports Launchpad's build state.
5. **AUR.** Fills in the PKGBUILD's checksums from the release tarball,
   regenerates `.SRCINFO`, builds in a clean chroot, then commits and pushes to
   the AUR's repository.
6. **FriendlyHub.** Writes what FriendlyHub takes (S14-T9) into
   `~/Downloads/backtrack-X.Y.Z-friendlyhub/`, or the directory given (`just
   release DIR`): the manifest, pinned to the tag's commit; the AppStream
   metainfo with this version's release entry, written from its CHANGELOG
   section, whose entries are already user-facing sentences; and the generated
   sources, `cargo-sources.json` and `python3-requirements.json` (no
   `node-sources.json`: Backtrack has no JavaScript, S13-T2). Keith submits
   them. `just friendlyhub-files [DIR]` writes the same set for a version
   already tagged.
7. **Summary.** Each channel, its link, and its state.

- **Resumable.** Each publishing step checks whether this version is already
  there (the release, the COPR build, the PPA upload, the AUR's `pkgver`) and
  skips it if so; the FriendlyHub files are simply written again. After a
  failure, fixing the cause and running `just release` again carries on from
  where it stopped. Every run writes a log under `dist/release/`.
- **Pre-releases** (`X.Y.Z-rc1`) go to a testing COPR project and a testing
  PPA, and stop short of the AUR, printing what they would have pushed. The
  FriendlyHub files are written as for a release; whether a release candidate
  goes to FriendlyHub is Keith's call when he submits.
- **`just release-dry`** runs the preflight and every build and generation step
  locally, FriendlyHub's files included, and publishes nothing.
- Human-only, like `just bump-version` (S13-T6).

**Accept:** `just release-dry` completes on Keith's machine. With the version
bumped by Keith to `0.1.0-rc1`, `just release` publishes it to GitHub, the
testing COPR and the testing PPA, every build succeeding, and leaves the
FriendlyHub set in `~/Downloads` (S14-T9's checks pass on it); running it again
straight after skips every publishing step; a run with one credential removed
stops at the preflight, having published nothing.

## Definition of Done
All four install paths proven on fresh systems; portal matrix green; `just
release` has taken a release candidate to every channel that takes
pre-releases; progress.md fully `[x]` through Stages 13 and 14; CHANGELOG
`[Unreleased]` clean and ready for the human's 0.1.0 release decision.
