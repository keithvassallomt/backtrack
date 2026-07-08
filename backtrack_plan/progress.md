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

**Current stage:** 1 (complete) → next: Stage 2
**Last updated:** 2026-07-07

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

- 2026-07-07 (S00-T2): Minimum supported platform pinned to GTK 4.14 / libadwaita
  1.5 (GNOME 46 / Ubuntu 24.04 LTS) via crate version features — required for
  libadwaita 0.9 to resolve `gtk::Accessible`. Revisit at Stage 13 packaging.
- 2026-07-07 (S00-T4): `just setup` (sudo dnf/apt installs) was NOT run on a
  clean container; build/check/test/run recipes verified locally. Clean-machine
  walkthrough still to be exercised.
- 2026-07-07 (S00-T7): Stage file body specifies fmt/clippy/tests, println-guard,
  verify-version, cargo-audit, integration tests — implemented and CI verified
  green on the branch; a scratch commit adding `println!` was confirmed to fail
  CI at the guard step. The progress.md label's "license-header check" is NOT in
  the stage acceptance and no source files carry SPDX headers yet; deferred
  pending a decision (raise with human). CI uses actions/checkout@v4 (Node 20
  deprecation warning — cosmetic).
- 2026-07-07 (S01-T1): Workspace `rusqlite` pin lowered 0.40 → 0.39. rusqlite
  0.40 pulls `libsqlite3-sys` 0.38.x, whose build script uses the unstable
  `cfg_select!` macro and fails to compile on Rust 1.94.1 (Fedora stable, the
  same toolchain CI installs from `fedora:latest`). 0.39 → `libsqlite3-sys`
  0.37 builds cleanly; `bundled` still compiles SQLite with FTS5 (verified: the
  `fts_names` virtual table is created by the v1 migration under test). Revisit
  when the toolchain or crate is fixed upstream.
- 2026-07-07 (S01-T2): Ingest perf (in-memory, dev hardware) — 200k-item first
  ingest ~2.9s debug / ~0.5s release (~70k items/sec debug, well over the
  20k/sec bar); a second identical 200k ingest (the per-path diff-lookup hot
  path) stays within the same budget. Both under the 15s CI ceiling.
- 2026-07-07 (S01-T2): Fixtures — the small fixture (`testdata/small-listing.jsonl`,
  a real 100-file/5-dir/1-symlink `borg list --json-lines` capture) is checked
  in and drives an end-to-end parse->ingest test. The "large (200k)" fixture is
  synthesised in-process by the perf test rather than kept as a gitignored file,
  so CI stays hermetic (no pre-test generation step); this deviates from the
  stage file's literal "gitignored, built by just demo-repo" wording but meets
  the acceptance ("large-fixture ingest under 15s in CI"). Change detection is
  kind+size+mtime+chunk_hash; Borg `list` supplies no chunk hash, so size+mtime
  decide, as designed. Ingest assumes chronological archive order (backfill is
  Stage 4). proptest-regressions/ is checked in per proptest convention.
- 2026-07-07 (S01-T3): `IndexReader` opens the DB with `query_only=ON` (rather
  than strict read-only) to sidestep WAL shared-memory access issues while still
  forbidding writes; it verifies schema_version == current and never migrates.
  `folder_at` on the 200k-file index runs under the 10ms budget (asserted in
  test). `archives_overview` returns per-archive summaries newest-first (repo
  flag included); day/week/month bucketing is left to the GUI as presentation.
  `next_change` uses interval boundaries: when the file exists at `from_seq` the
  answer is the adjacent archive past its interval end/start; when absent, the
  nearest (re)appearance/disappearance.
- 2026-07-07 (S01-T4): `search` uses the default FTS5 unicode61 tokenizer, so a
  name like `invoice-may.pdf` indexes as tokens `invoice`/`may`/`pdf`; queries
  are token-exact or, with a trailing `*`, token-prefix (the plan's
  "substring-ish"). User input is wrapped in a double-quoted FTS phrase (quotes
  doubled) so punctuation/operators in crafted filenames can't inject MATCH
  syntax. Ranking: not-exists-today first, then latest existence, then bm25.
  Charlie test (file living only in archives 10..40) confirmed for `contract`
  and `cont*`, incl. that `contract` does not match `container`.
- 2026-07-07 (S01-T5): `changed_since(seq, live_entries)` takes the walker's
  output as an injected iterator (no I/O in core), returning added+modified
  paths present on disk that differ from index@seq by size/mtime. Deletions
  (indexed-but-absent) are intentionally excluded — the spool can only archive
  files that still exist. The real walker (Stage 5) must truncate live mtime to
  microseconds to match Borg's stored resolution, else everything reads changed.
- 2026-07-07 (S01-T6): `remove_archives` makes the index identical to a
  from-scratch ingest of the survivors: it densely **renumbers** surviving seqs
  to 1..=k, clamps/remaps each version interval onto them (dropping versions that
  lived only in removed archives), and coalesces intervals that removal made
  adjacent with identical content. Renumbering rewrites version seqs — a full
  pass, acceptable since prune is infrequent (Stage 4); a seq-preserving variant
  can come later if it matters. Orphaned path/fts rows from fully-removed files
  are left in place (harmless: every query joins `versions`, so they are
  invisible, and re-ingest reuses them). The property test is the acceptance
  oracle — verified to bite via a mutation (disabling coalesce fails it).
- 2026-07-07 (S01-T7): Added an `xtask` workspace member (unpublished, at repo
  root so it is outside the crates/ println/license gates — it legitimately
  prints progress). `just demo-repo` shells to real borg to build a 30-snapshot
  history, ingests each via `borg list --json-lines`, and self-verifies the
  old-client-folder deleted-after signal. Runs in ~12s (budget 2 min), writing
  demo-repo/demo-src/index.db under ~/.local/share/backtrack-dev/. The fake home
  is mutated *incrementally* per day (not rebuilt) so unchanged files keep their
  mtime and intervals extend; emptied dirs are pruned so a deleted folder truly
  vanishes. Fast default test builds the same index borg-free (content-derived
  mtime) and asserts the deletion; the borg round-trip is gated behind the
  `integration` feature (CI's test-integration). `cargo test --workspace
  --features integration` confirmed to enable the feature on both core and xtask.
- 2026-07-07 (Stage 1 done): full quality gate green — 50 core unit/property
  tests + 3 xtask tests; clippy -D warnings clean; license headers present;
  verify-version OK. Perf recorded above (ingest, folder_at, demo-repo).
