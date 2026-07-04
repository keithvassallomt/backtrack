# Backtrack — Progress Tracker

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

**Current stage:** 0 (not started)
**Last updated:** (never — set on first commit)

## Stage 0 — Bootstrap ([stage file](stages/stage-00-bootstrap.md))
- [ ] S00-T1 Git repo, license, .gitignore, README skeleton
- [ ] S00-T2 Cargo workspace with four crates compiling
- [ ] S00-T3 Logging foundation (tracing, JSONL rotation) wired into all binaries
- [ ] S00-T4 Justfile: setup / build / check / test / run-daemon / run-app / demo-repo
- [ ] S00-T5 CHANGELOG.md + versioning policy files (version 0.1.0 set by human)
- [ ] S00-T6 just bump-version recipe (single-source version propagation)
- [ ] S00-T7 CI: fmt, clippy, tests, println-guard, license-header check

## Stage 1 — Core index ([stage file](stages/stage-01-core-index.md))
- [ ] S01-T1 Schema migrations + open/integrity-check on start
- [ ] S01-T2 Interval-encoded ingest from borg-list JSONL fixtures
- [ ] S01-T3 Timeline queries (folder@snapshot, file history, diff-vs-previous)
- [ ] S01-T4 FTS5 filename search incl. deleted-file lifespans
- [ ] S01-T5 Changed-since-archive query (feeds offline spool)
- [ ] S01-T6 Prune/expiry handling (close intervals, merge)
- [ ] S01-T7 demo-repo fixture generator

## Stage 2 — Borg adapter ([stage file](stages/stage-02-borg-adapter.md))
- [ ] S02-T1 BackupEngine trait + typed error taxonomy
- [ ] S02-T2 BorgCli: create/list/extract/prune/compact/check with --log-json parsing
- [ ] S02-T3 Keyring (Secret Service) passphrase provider
- [ ] S02-T4 Repo setup/import/key-export operations
- [ ] S02-T5 Integration tests against real borg (CI)

## Stage 3 — Daemon, D-Bus, CLI ([stage file](stages/stage-03-daemon-dbus-cli.md))
- [ ] S03-T1 backtrackd skeleton: config load, single-instance, D-Bus name
- [ ] S03-T2 Job model (queue, IDs, cancel/pause, progress events)
- [ ] S03-T3 Full org.backtrack.Daemon1 interface + signals
- [ ] S03-T4 systemd user units + D-Bus activation
- [ ] S03-T5 backtrack CLI mapping the interface (incl. status --json, doctor)

## Stage 4 — Backup pipeline ([stage file](stages/stage-04-backup-pipeline.md))
- [ ] S04-T1 Scheduler (timer, missed-run catch-up, pause/resume)
- [ ] S04-T2 Preflight: battery (UPower), metered (NetworkManager), pause state
- [ ] S04-T3 create → stream-index → prune per retention → scheduled compact
- [ ] S04-T4 Checkpoint/interrupted-backup handling (hidden from timeline)
- [ ] S04-T5 First-run backfill indexing (newest-first, background)

## Stage 5 — Offline protection ([stage file](stages/stage-05-offline-protection.md))
- [ ] S05-T1 Destination reachability probe + network-change wakeup
- [ ] S05-T2 Spool repo: capped, changed-files-only archives
- [ ] S05-T3 btrfs detection + subvolume snapshot mode
- [ ] S05-T4 Reconnect: immediate catch-up backup, spool expiry (~30 days)
- [ ] S05-T5 Status surfaces ("on this computer" archives in index)

## Stage 6 — GTK timeline browser ([stage file](stages/stage-06-gtk-timeline.md))
- [ ] S06-T1 App shell, main window layout, dark/light (mockups 1, 6)
- [ ] S06-T2 Snapshot sidebar with grouping + badges
- [ ] S06-T3 File pane bound to index (status badges incl. "deleted after this")
- [ ] S06-T4 Older/Newer stepping + Ctrl+←/→ (+ "next change to selected file")
- [ ] S06-T5 Calendar popover (mockup 7)
- [ ] S06-T6 Timeline density strip (indicator + snap-to-snapshot jump)
- [ ] S06-T7 Preview pane via PreviewFile fd, cancellable, cached
- [ ] S06-T8 Primary menu (mockup 14) with working Back Up Now / Pause

## Stage 7 — Restore engine ([stage file](stages/stage-07-restore-engine.md))
- [ ] S07-T1 Staging→compare→rename pipeline in core
- [ ] S07-T2 Skip-identical + conflict detection (newer/older cues)
- [ ] S07-T3 Single-file conflict dialog (mockup 4)
- [ ] S07-T4 Folder summary + review checklist (mockups 5, 8)
- [ ] S07-T5 replaced/ stash with 30-day expiry + "Recently Replaced Files" view
- [ ] S07-T6 Undo toast wired to stash
- [ ] S07-T7 Restore To… (choose destination, zero-conflict path)

## Stage 8 — Search & compare ([stage file](stages/stage-08-search-compare.md))
- [ ] S08-T1 SearchFiles D-Bus method over FTS5
- [ ] S08-T2 Search UI grouped by file, deleted-first ranking (mockup 20)
- [ ] S08-T3 View-in-Timeline + Restore-Latest actions
- [ ] S08-T4 Compare view: text diff (mockup 9), images side-by-side

## Stage 9 — Wizard & preferences ([stage file](stages/stage-09-wizard-preferences.md))
- [ ] S09-T1 Wizard flow incl. existing-repo import (mockups 10–13)
- [ ] S09-T2 Recovery-key export gate (cannot continue without save/print)
- [ ] S09-T3 First-backup kickoff + expectation copy
- [ ] S09-T4 Preferences: General/Backup/Storage/Security/Advanced (mockups 15–19)
- [ ] S09-T5 Run-wizard-again path (non-destructive)

## Stage 10 — Health ([stage file](stages/stage-10-health.md))
- [ ] S10-T1 Health state machine + escalation timers per health.md
- [ ] S10-T2 Notifications respecting user policy
- [ ] S10-T3 Main-window banner states (mockup 23)
- [ ] S10-T4 Resolution flows: passphrase (mockup 24), reauth, disk-full, repair
- [ ] S10-T5 Monthly borg check schedule + index integrity check
- [ ] S10-T6 backtrack doctor diagnostic bundle

## Stage 11 — Disaster recovery ([stage file](stages/stage-11-disaster-recovery.md))
- [ ] S11-T1 RestoreEverything job: per-top-folder, resumable
- [ ] S11-T2 DR entry dialog (mockup 21) off the import path
- [ ] S11-T3 Progress window, pause/cancel, honest ETA (mockup 22)
- [ ] S11-T4 Post-restore: enable schedule only after completion

## Stage 12 — Integrations & tray ([stage file](stages/stage-12-integrations-tray.md))
- [ ] S12-T1 Nautilus python extension (mockup 2)
- [ ] S12-T2 Dolphin service menu (mockup 3, menu part only)
- [ ] S12-T3 StatusNotifierItem tray for non-GNOME (status, actions)
- [ ] S12-T4 Background portal presence (GNOME quick-settings launch path)
- [ ] S12-T5 App detects missing plugins → hints distro package (prefs General)

## Stage 13 — Packaging & release ([stage file](stages/stage-13-packaging-release.md))
- [ ] S13-T1 Flatpak manifest (GNOME runtime, portals, bundled borg)
- [ ] S13-T2 cargo-sources.json generation recipe (offline flatpak build)
- [ ] S13-T3 Portal permissions audit + test matrix
- [ ] S13-T4 RPM spec + COPR; deb + PPA (app, nautilus, dolphin packages)
- [ ] S13-T5 CI release artifacts on tag; Flathub submission checklist
- [ ] S13-T6 Release runbook (human bumps version → just bump-version → tag)

## Blocked

(nothing)

## Notes / decisions made during implementation

(append dated entries here; never delete)
