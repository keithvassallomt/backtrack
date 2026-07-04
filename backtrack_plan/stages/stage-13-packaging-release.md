# Stage 13 — Packaging & Release

## Objective
Installable three ways — Flatpak (portal-correct, offline-buildable), Fedora RPM
(COPR), Ubuntu deb (PPA) — with a CI release pipeline and a human-controlled
release runbook. Both packaging paths were mandated from day one; CI has been
building them since Stage 0 grew them — this stage completes and hardens them.

## Prerequisites
All prior stages functional. Read [../reference/stack.md](../reference/stack.md)
§9 and the app-id/license facts in [../reference/brief.md](../reference/brief.md).

## Tasks

### S13-T1 — Flatpak manifest
`packaging/flatpak/io.github.keithvassallomt.Backtrack.yml`:
- Runtime `org.gnome.Platform` / SDK `org.gnome.Sdk` (current stable at
  implementation time; pin exact version in the manifest).
- Modules: borgbackup (build from pip sources with its deps — msgpack etc. — as a
  python module with generated `python3-requirements.json`; pin borg 1.4.x), then
  backtrack (cargo, offline via T2 sources).
- App + daemon in one Flatpak; daemon launched via the app's background portal
  autostart (S12-T4), `--command=backtrackd` for the autostart entry.
- **Portals over permissions** wherever a portal exists: FileChooser (Restore To…,
  key export), Background/Autostart, Notification, OpenURI. Static permissions
  kept minimal and justified in a comment per line:
  `--filesystem=home` (the product's job is reading the home dir to back it up;
  document why portal-per-file is impossible for a backup daemon),
  `--filesystem=xdg-run/gvfs` + `--talk-name=org.gtk.vfs.*` (network shares),
  `--share=network` (ssh destinations), `--talk-name=org.freedesktop.secrets`
  (keyring), session bus own-name `org.backtrack.Daemon1`.
  NO device=all, NO system-bus, NO x11-fallback beyond `--socket=wayland
  --socket=fallback-x11`.
- Metainfo: `packaging/io.github.keithvassallomt.Backtrack.metainfo.xml`
  (AppStream: screenshots = the mockups hosted in-repo, release notes fed from
  CHANGELOG at release time) + desktop file + icon set (symbolic + scalable —
  request icon assets from the human; placeholder acceptable until then).
**Accept:** `flatpak-builder --install` produces a working app: wizard → backup to
a `--filesystem`-visible destination → restore round-trip INSIDE the sandbox;
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
GNOME background apps), keyring access, SMB destination via gvfs, ssh destination,
Wayland + Xorg session each. Failures become issues; matrix result committed to
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
trigger COPR. Flathub: prepare submission checklist doc
(`packaging/flatpak/FLATHUB.md`: repo requirements, review gotchas — but actual
submission is a human step, listed in the runbook).
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
