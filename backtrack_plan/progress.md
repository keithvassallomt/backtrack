# Backtrack — Progress Tracker

## For Keith to do

Steps only Keith can take, kept current as work turns them up. Tick each when
done; Claude removes ticked items.

- [ ] Fedora account and two COPR projects: `backtrack`, and
  `backtrack-testing` for release candidates (S13-T4, S13-T9).
- [ ] Launchpad account with the PPA signing key registered, and two PPAs:
  `backtrack` and `backtrack-testing` (S13-T4, S13-T9). Launchpad may ask for
  the Ubuntu Code of Conduct to be signed first.
- [ ] AUR account with an SSH key (S13-T8, S13-T9).
- [ ] Icons for `tools.backtrack.Backtrack`: a scalable one and a symbolic one
  (S13-T1; placeholders until then).
- [ ] One hand-check session on the Flatpak in the VMs, once Stage 14's fixes
  are in: the wizard clicked through (S13-T1), Backtrack's entry in GNOME's
  Background Apps opening the window (S12-T4), and the S13-T3 matrix. Claude
  writes the walkthrough script first.

> **Contract:** Update this file in the SAME COMMIT as the work it describes.
> `[ ]` upcoming · `[/]` in progress · `[x]` done (acceptance criteria verified) ·
> `[!]` blocked (explain under **Blocked** below).
> Marking `[x]` without the stage file's acceptance criteria passing is a breach of
> contract. Version numbers are human-only — see README.md.
>
> **GitHub board:** this file is the source of truth; the [Backtrack Project
> board](https://github.com/users/keithvassallomt/projects/2) mirrors it. After
> changing any checkbox, run `just sync-board-apply` to update the board. New
> tasks: add them here + to the stage file, then `just provision-board-apply`.
> See [../CLAUDE.md](../CLAUDE.md) for the full workflow.

**Current stage:** 12 (in progress)
**Last updated:** 2026-10-12

## Stage 0 — Bootstrap ([stage file](stages/stage-00-bootstrap.md))
- [x] S00-T1 Git repo, license, .gitignore, README skeleton
- [x] S00-T2 Cargo workspace with four crates compiling
- [x] S00-T3 Logging foundation (tracing, JSONL rotation) wired into all binaries
- [x] S00-T4 Justfile: setup / build / check / test / run-daemon / run-app / demo-repo
- [x] S00-T5 CHANGELOG.md + versioning policy files (version 0.1.0 set by human)
- [x] S00-T6 just bump-version recipe (single-source version propagation)
- [x] S00-T7 CI: fmt, clippy, tests, println-guard, license-header check
- [x] S00-T8 License headers (SPDX) + check-license-headers CI gate

## Stage 1 — Core index ([stage file](stages/stage-01-core-index.md))
- [x] S01-T1 Schema migrations + open/integrity-check on start
- [x] S01-T2 Interval-encoded ingest from borg-list JSONL fixtures
- [x] S01-T3 Timeline queries (folder@snapshot, file history, diff-vs-previous)
- [x] S01-T4 FTS5 filename search incl. deleted-file lifespans
- [x] S01-T5 Changed-since-archive query (feeds offline spool)
- [x] S01-T6 Prune/expiry handling (close intervals, merge)
- [x] S01-T7 demo-repo fixture generator

## Stage 2 — Borg adapter ([stage file](stages/stage-02-borg-adapter.md))
- [x] S02-T1 BackupEngine trait + typed error taxonomy
- [x] S02-T2 BorgCli: create/list/extract/prune/compact/check with --log-json parsing
- [x] S02-T3 Keyring (Secret Service) passphrase provider
- [x] S02-T4 Repo setup/import/key-export operations
- [x] S02-T5 Integration tests against real borg (CI)

## Stage 3 — Daemon, D-Bus, CLI ([stage file](stages/stage-03-daemon-dbus-cli.md))
- [x] S03-T1 backtrackd skeleton: config load, single-instance, D-Bus name
- [x] S03-T2 Job model (queue, IDs, cancel/pause, progress events)
- [x] S03-T3 Full tools.backtrack.Backtrack.Daemon1 interface + signals
- [x] S03-T4 systemd user units + D-Bus activation
- [x] S03-T5 backtrack CLI mapping the interface (incl. status --json, doctor)

## Stage 4 — Backup pipeline ([stage file](stages/stage-04-backup-pipeline.md))
- [x] S04-T1 Scheduler (timer, missed-run catch-up, pause/resume)
- [x] S04-T2 Preflight: battery (UPower), metered (NetworkManager), pause state
- [x] S04-T3 create → stream-index → prune per retention → scheduled compact
- [x] S04-T4 Checkpoint/interrupted-backup handling (hidden from timeline)
- [x] S04-T5 First-run backfill indexing (newest-first, background)

## Stage 5 — Offline protection ([stage file](stages/stage-05-offline-protection.md))
- [x] S05-T1 Destination reachability probe + network-change wakeup
- [x] S05-T2 Spool repo: capped, changed-files-only archives
- [x] S05-T3 btrfs detection + subvolume snapshot mode
- [x] S05-T4 Reconnect: immediate catch-up backup, spool expiry (~30 days)
- [x] S05-T5 Status surfaces ("on this computer" archives in index)
- [x] S05-T6 Borg warning exits (1, 100–127) are not backup failures

## Stage 6 — GTK timeline browser ([stage file](stages/stage-06-gtk-timeline.md))
- [x] S06-T1 App shell, main window layout, dark/light (mockups 1, 6)
- [x] S06-T2 Snapshot sidebar with grouping + badges
- [x] S06-T3 File pane bound to index (status badges incl. "deleted after this")
- [x] S06-T4 Older/Newer stepping + Ctrl+←/→ (+ "next change to selected file")
- [x] S06-T5 Calendar popover (mockup 7)
- [x] S06-T6 Timeline density strip (indicator + snap-to-snapshot jump)
- [x] S06-T7 Preview pane via PreviewFile fd, cancellable, cached
- [x] S06-T8 Primary menu (mockup 14) with working Back Up Now / Pause
- [x] S06-T9 Demo fixture dated relative to now (so the sidebar's bands appear)
- [x] S06-T10 JobFinished signal, replacing the GUI's completion polling

## Stage 7 — Restore engine ([stage file](stages/stage-07-restore-engine.md))
- [x] S07-T1 Staging→compare→rename pipeline in core
- [x] S07-T2 Skip-identical + conflict detection (newer/older cues)
- [x] S07-T3 Single-file conflict dialog (mockup 4)
- [x] S07-T4 Folder summary + review checklist (mockups 5, 8)
- [x] S07-T5 replaced/ stash with 30-day expiry + "Recently Replaced Files" view
- [x] S07-T6 Undo toast wired to stash
- [x] S07-T7 Restore To… (choose destination, zero-conflict path)
- [x] S07-T8 Demo fixture stores real paths, so its history can be restored

## Stage 8 — Search & compare ([stage file](stages/stage-08-search-compare.md))
- [x] S08-T1 SearchFiles D-Bus method over FTS5
- [x] S08-T2 Search UI grouped by file, deleted-first ranking (mockup 20)
- [x] S08-T3 View-in-Timeline + Restore-Latest actions
- [x] S08-T4 Compare view: text diff (mockup 9), images side-by-side
- [x] S08-T5 Real image bytes in the demo fixtures (preview + compare need them)
- [x] S08-T6 “Not on your disk” as a fact about the disk, not the catalogue

## Stage 9 — Wizard & preferences ([stage file](stages/stage-09-wizard-preferences.md))
- [x] S09-T1 Wizard flow incl. existing-repo import (mockups 10–13)
- [x] S09-T2 Recovery-key export gate (cannot continue without save/print)
- [x] S09-T3 First-backup kickoff + expectation copy
- [x] S09-T4 Preferences: General/Backup/Storage/Security/Advanced (mockups 15–19)
- [x] S09-T5 Run-wizard-again path (non-destructive)

## Stage 10 — Health ([stage file](stages/stage-10-health.md))
- [x] S10-T1 Health state machine + escalation timers per health.md
- [x] S10-T2 Notifications respecting user policy
- [x] S10-T3 Main-window banner states (mockup 23)
- [x] S10-T4 Resolution flows: passphrase (mockup 24), reauth, disk-full, repair
- [x] S10-T5 Monthly borg check schedule + index integrity check
- [x] S10-T6 backtrack doctor diagnostic bundle
- [x] S10-T7 Local snapshot badges say whether the destination has them

## Stage 11 — Disaster recovery ([stage file](stages/stage-11-disaster-recovery.md))
- [x] S11-T1 RestoreEverything job: per-top-folder, resumable
- [x] S11-T2 DR entry dialog (mockup 21) off the import path
- [x] S11-T3 Progress window, pause/cancel, honest ETA (mockup 22)
- [x] S11-T4 Post-restore: enable schedule only after completion

## Stage 12 — Integrations & tray ([stage file](stages/stage-12-integrations-tray.md))
- [x] S12-T1 Nautilus python extension (mockup 2)
- [x] S12-T2 Dolphin service menu (mockup 3, menu part only)
- [x] S12-T3 StatusNotifierItem tray for non-GNOME (status, actions)
- [/] S12-T4 Background portal presence (GNOME quick-settings launch path)
- [x] S12-T5 App detects missing plugins → hints distro package (prefs General)
- [x] S12-T6 Development VMs: one-command setup and push for GNOME and Plasma

## Stage 13 — Packaging & release ([stage file](stages/stage-13-packaging-release.md))
- [/] S13-T1 Flatpak manifest (GNOME runtime, portals, bundled borg)
- [/] S13-T2 cargo-sources.json generation recipe (offline flatpak build)
- [ ] S13-T3 Portal permissions audit + test matrix
- [ ] S13-T4 RPM spec + COPR (Fedora 44+); deb + PPA (Ubuntu 26.04+)
- [ ] S13-T5 CI release artifacts on tag
- [ ] S13-T6 Release runbook (human bumps version → just bump-version → just release)
- [x] S13-T7 Application ID tools.backtrack.Backtrack, daemon names under it
- [ ] S13-T8 AUR packages (app, nautilus, dolphin)
- [ ] S13-T9 just release: from tag to every channel in one command

## Stage 14 — Flatpak as a first-class build ([stage file](stages/stage-14-flatpak-first-class.md))
- [x] S14-T1 Daemon activation inside the Flatpak
- [ ] S14-T2 Window opens through the session bus (notification clicks, tray)
- [ ] S14-T3 Tray icon started by the daemon inside the sandbox
- [ ] S14-T4 File-manager integration for Flatpak installs (per-user copies)
- [ ] S14-T5 Read the host's os-release (Fix page, doctor)
- [ ] S14-T6 Secret Service keyring in every build
- [ ] S14-T7 Restart the daemon after a failure, as the unit does
- [ ] S14-T8 Btrfs snapshots inside the Flatpak (bundled btrfs-progs)
- [ ] S14-T9 Publish on FriendlyHub

## Blocked


Nothing is blocked.
