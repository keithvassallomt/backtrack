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

**Current stage:** 6 (complete) → next: Stage 7
**Last updated:** 2026-09-20

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
- [x] S03-T3 Full org.backtrack.Daemon1 interface + signals
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

- 2026-09-20 (Stage 6 — Definition of Done): the Alice story driven end to end
  against the demo repository, with the window's own actions fired over
  `org.gtk.Actions` so each claim is a measurement rather than an impression.
  - **Deleted folders reappear, and only they are flagged.** Stepping back from
    snapshot 30, `home` holds 2 entries with no badges at snapshots 16–18 and 3
    entries with exactly one "deleted after this" from snapshot 15 down — which
    is where the fixture deletes `old-client-folder`.
  - **Stepping is not the bottleneck.** Sixteen consecutive steps, each a fresh
    `folder_at` off the main loop, completed in ~50 ms in total.
  - **"Next change" lands on the change, not near it.** From the oldest
    snapshot with `report.odt` selected, `newer-change` jumped to 8 and then
    20 — exactly the two entries at which the fixture rewrites the file,
    skipping the 6 and 11 identical snapshots between.
  - **The preview comes out of the repository.** `PreviewFile` returned a
    descriptor for `report.odt` at snapshot 30 and 29 bytes of text were read
    from it, through the daemon's cache.
  - **Back Up Now completes and says so**, on a machine already `HEALTHY` —
    the case with no `StatusChanged` to hear, which is why S06-T10 exists.
    The dev-mode pause was set 58 seconds out and lifted itself on time.
  - **D-Bus activation works from cold**: the window started a stopped daemon
    and it answered.
  - Keyboard-only operation and both colour schemes confirmed by hand.
    Screenshots of the two schemes are outstanding and will be attached to the
    S06-T1 issue.
  - Full suite green: **517 tests**, 544 with the real-borg integration set.

- 2026-09-20 (Stage 6 — GTK timeline browser): the stage is mostly a lesson in
  how much of a GUI is not the widgets.
  - **The view models are plain data with no GTK in them** — sidebar grouping,
    calendar grid, density bars, every formatted string. They are the parts
    most likely to be wrong and the only parts a test can check without a
    display. Time is passed in rather than read from the clock, which makes
    "what does the sidebar look like on a Monday" a question a test can ask.
  - **Index reads run on their own thread.** The queries are fast; none is
    guaranteed to be, and a 200 ms stall on the main loop is a visible stutter
    in the one interaction the whole app is built around.
  - **The sidebar is a `GtkTreeListModel`, not a section model** (which the
    stage file suggested). The older groups have to collapse — "May 2026 (31)"
    is what keeps the sidebar readable over a year of backups — and sections
    cannot collapse.
  - **"Previous/next change to this file" is a split-button dropdown, not a
    long-press.** A long-press is undiscoverable and unreachable from the
    keyboard, and this is the feature that makes forty identical hourlies
    navigable.
  - **The calendar is a grid of buttons, not `GtkCalendar`.** A day with no
    backup behind it has to be insensitive; GtkCalendar will let you select it,
    and answering that click with nothing is worse than not offering it. The
    "stock widget wins" rule covers styling, not behaviour.
  - **A badge that lied, found by browsing rather than by a test.**
    `folder_at` measured "deleted after this" against `MAX(seq)`, including
    archives appended but not yet catalogued — which have no version rows. On a
    first-run backfill, which indexes newest-first over hours, that would have
    put an orange badge on every file in the user's home directory. Now
    measured against the newest *catalogued* archive, which is what
    `latest_catalogued_seq` already existed for.
  - **`JobFinished` was missing from the interface** (S06-T10). `StatusChanged`
    fires only when the health state moves, so a successful backup on an
    already-healthy machine announces nothing at all, and a client that started
    it could only poll. Stage 7's restores need the same signal.
  - **The demo fixture was dated to a fixed month** (S06-T9) and had stopped
    exercising the sidebar's Today/Yesterday/This week bands the moment that
    month passed. It is now relative to the run, with a deliberate two-day hole
    so the calendar's unclickable days and the strip's gap stubs have something
    to draw. Borg's `--timestamp` does not round-trip through `{time:%s}`, so
    the generator measures the offset with a throwaway archive rather than
    encoding a guess that would rot.
  - **The app enters a Tokio runtime it otherwise has no use for.** `oo7` turns
    on zbus's Tokio backend for the whole workspace, so zbus panics when called
    from outside a runtime; the window's own concurrency stays GLib's.
  - **Forcing a colour scheme cannot outrank a user stylesheet.** A
    `~/.config/gtk-4.0/gtk.css` that redefines the libadwaita palette loads
    above the theme's and wins — correct behaviour, and completely silent about
    itself. `BACKTRACK_THEME` now runs against an empty `XDG_CONFIG_HOME` so
    the scheme can actually decide the colours, and the application says so
    when something else will.

- 2026-07-08 (Stage 2 — Borg adapter): landed the `engine` + `secret` modules in
  `backtrack-core` and a new `backtrack-testkit` crate. Key decisions:
  - **Dispatch:** `BackupEngine` is `#[async_trait]`, held as `Arc<dyn BackupEngine>`
    (not generics) — the daemon swaps Borg1/Borg2/Mock at runtime; the per-call box
    is noise next to a subprocess spawn. `BorgCli` is the v1 impl.
  - **JobStream:** concrete struct = `Stream<JobEvent>` over a `tokio::mpsc` channel;
    owns the borg child + a `tokio_util` `CancellationToken`. `cancel()` and `Drop`
    both trip the token; the stderr-reader task SIGTERM/kills the child. Terminal
    outcome is delivered **in-band** as the final `JobEvent::Finished(Result<..>)`,
    because `create()` returns `Ok(JobStream)` long before borg finishes. The reader
    observes cancellation even while blocked on a full channel (send wrapped in the
    cancel `select!`), so a cancelled-and-not-polled consumer never wedges the child.
  - **`MockEngine`/`MockSecretStore`** live in `backtrack-testkit` (publish=false),
    consumed by later stages as a **dev-dependency only** — no test scaffolding in
    the production library. `MockEngine` builds streams via the public
    `JobStream::from_events`.
  - **Error taxonomy → health.md:** `EngineError::health_failure()` returns
    `Option<HealthFailure>`; `HealthFailure` is exactly the 7 engine-relevant
    failure-catalogue rows. `RepoUnreachable` (→ PROTECTED_LOCALLY state, not a
    failure), `LockedByOther` (transient) and `BorgFailed` (uncategorised) map to
    `None`. A unit test proves every catalogue row is covered.
  - **borg flags chosen:** `create --json --list --filter AME --compression <c>
    [--one-file-system] [--exclude ..] <repo>::<name> <sources..>`; `list
    --json-lines <repo>::<archive>` (NO explicit `--format` — json-lines already
    carries type/mode/size/mtime, parsed by Stage 1's `BorgItem::from_json_line`);
    `extract --list` (into `dest` via `current_dir`) and `extract --stdout` for
    single-file preview; `prune --list --keep-{hourly,daily,weekly,monthly}=N`;
    `compact`; `check [--repository-only|--archives-only]`; `init
    --encryption=repokey-blake2`; `info --json`; `key export`.
  - **Env on every job/data command:** `BORG_PASSPHRASE` (from `SecretStore`, child
    env only — never on disk), `BORG_RELOCATED_REPO_ACCESS_IS_OK=no`,
    `BORG_EXIT_CODES=modern`, `LC_ALL=C.UTF-8`, `LANG=C.UTF-8`, `--log-json`. Version
    probe (`borg --version`) enforces `>= 1.2` → else `BorgMissing`.
  - **Classification** (`classify.rs`): exit code + captured error-level `log_message`
    lines → `EngineError`, precedence PassphraseWrong → AuthFailed → RepoUnreachable
    → DestinationFull → LockedByOther → RepoCorrupt → BorgFailed. Broad English
    fragments ("is incorrect", "does not exist", "manifest") must **co-occur** with a
    domain token on the same line so unrelated borg messages aren't misclassified;
    stable `msgid`s (`Repository.DoesNotExist`, `LockTimeout`,
    `Repository.CheckNeeded`) are matched directly.
  - **SecretStore:** `oo7` (Secret Service) for real use; a JSON file store gated on
    `BACKTRACK_DEV=1` keeps CI headless. Missing entry → `PassphraseMissing` (never a
    prompt). `oo7 0.6` compiled unmodified.
  - **tokio `io-util`** added explicitly to the workspace features — the adapter uses
    `tokio::io` directly (was only enabled transitively via `oo7→zbus`).
  - **Verified against real borg 1.4.4** (local + CI Fedora container): init → 2
    archives → list → extract_stdout byte-match → prune/compact/check; wrong
    passphrase → `PassphraseWrong`, unreachable path → `RepoUnreachable`; borg stores
    member paths with the leading `/` stripped (`tmp/…/hello.txt`). Integration suite
    green two consecutive runs. Deferred to Stage 4: `BORG_EXIT_CODES=modern` makes a
    borg *warning* exit classify as a job failure — revisit when the backup pipeline
    handles warnings (e.g. a file vanishing mid-create).

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
- 2026-08-06 (S03-T1): `config.toml` lives in the data directory, not
  `XDG_CONFIG_HOME`, per stack.md §3 — one directory to back up, inspect, or
  wipe. Loading is asymmetric on purpose: unknown keys warn (a stale key after a
  downgrade must not stop backups), a wrong-typed value fails (silently ignoring
  a setting the user believes is in force is worse than refusing to start). The
  schema mirrors the Preferences pages one section per page, so S09-T4's
  "every key has exactly one control" check has something to compare against.
  Single-instance is D-Bus name ownership with `DoNotQueue`, not a PID file:
  nothing stale survives a crash, and without `DoNotQueue` a second instance
  queues and silently inherits the name when the first exits, leaving two
  daemons that both believe they own the index. **Under `BACKTRACK_DEV` the bus
  name gains a `.Dev` suffix** — dev mode already redirects the data directory,
  and a dev daemon answering the real GUI would report an empty timeline for a
  fully protected machine. Interface and object path are unchanged, so
  introspection is identical in both modes; S03-T4's activation file will need
  templating because of it.
- 2026-08-06 (S03-T2): Repository admission is a reader/writer rule mirroring
  Borg's own locking (create/prune/compact/check exclusive; extract/list
  shared), so colliding work queues instead of failing on a lock the user has
  never heard of. The queue is *scanned*, not peeked — a restore may pass a
  queued backup, because someone restoring a file during a long index backfill
  should not wait for both, and the backup it passes is retried next tick.
  Cancellation resolves to `Done`, never `Failed`: a cancelled job is not a
  failure and must not raise a health banner. Two consequences — a job that
  finished in the gap between the request and the teardown reports *completed*
  (the archive exists; saying otherwise is a lie the user would act on), and an
  engine error raised while tearing down reports as the cancellation it followed
  from. Pause is implemented only for guided DR (per-folder, resumable);
  everything else is refused with a typed error. `SIGSTOP` is never used: a
  stopped Borg keeps its repository lock and sockets open indefinitely, so every
  other job would block on a holder that will never run again. A stream that
  ends without a terminal event is recorded as **failed** — an unverified backup
  reported as complete is the one failure mode this product cannot have.
- 2026-08-06 (S03-T3): The interface is pinned by an introspection snapshot
  rendered from the interface itself (no bus needed, so it runs in CI).
  Deviations from stack.md §2, both deliberate: **`ResumeJob` added** (a job that
  can be paused and never resumed is a bug, not an API — S11 needs it), and
  **`GetConfigKey` added** alongside `GetConfig`. Configuration crosses the bus
  as TOML rather than D-Bus variants: half the settings are lists, which a
  stringly-typed dictionary cannot express without inventing an escaping
  convention both ends must implement identically, and TOML is already the
  on-disk format. `PreviewFile` returns a descriptor onto the daemon's private
  cache (1 GB, LRU, keyed by `(archive, path)` — an archive is immutable, so a
  hit can never be stale), which is what lets a sandboxed GUI read a file it
  could not open itself. Writing the end-to-end test found **three real bugs**:
  (1) `borg create` was missing `--progress`, so borg emitted no
  `archive_progress` at all and every progress bar in the application would have
  sat still for the whole backup — added to create/extract/prune/compact/check;
  (2) `repo_info` counted archives from `borg info --json`, which carries no
  archive list, so it reported zero for every repository however full — now
  reads `borg list --json`, which carries both the count and the repository id;
  (3) a job failing with an *unclassified* error cleared the blocking-failure
  flag, dismissing a `BROKEN` banner the user still needed to act on — a failure
  can now raise the flag but never lower it, since only a successful backup
  proves the problem is gone. Health is seeded from the catalogue at startup, or
  the daemon would forget every backup it had ever taken each time it restarted.
- 2026-08-07 (S03-T4): The shipped units live in `packaging/` and are the real,
  packaged files; `just install-units` rewrites them on the way into
  `~/.config/systemd/user/` rather than keeping a second dev copy, so what is
  tested is what will be packaged. Dev substitutions: the binary path, the
  `.Dev` bus name, and `Environment=BACKTRACK_DEV=1`. Sandboxing is deliberately
  light — `ProtectHome`/`ProtectSystem`/`ReadOnlyPaths` would each break a daemon
  whose whole job is reading everything the user can read and restoring wherever
  they choose; a test pins their absence so adding one has to be deliberate.
  `Restart=on-failure`, not `always`: losing the single-instance race exits 0 and
  must not be relaunched. Activation correctness depends on four separate files
  agreeing (claimed name, `BusName=`, `Name=`, `SystemdService=`) and nothing in
  the build checks that, so `units.rs` asserts it against `core::dbus`.
  **Bug found by the acceptance test:** the daemon claimed its bus name before
  seeding health from the catalogue, and claiming the name *is* the readiness
  announcement — systemd marks the unit started and the activating client's
  queued call arrives immediately. The first `GetStatus` after a cold start
  therefore reported "never backed up" for a machine with 30 archives, while a
  second call reported the truth. Startup now finishes all state assembly before
  claiming the name; verified over 8 consecutive cold starts.
- 2026-08-07 (Stage 4 — Definition of Done): the daemon left running against the
  demo repo at a 1-minute development frequency for **31 minutes**. Every
  criterion met, measured rather than asserted:
  - **32 backups taken, 32 catalogued**, zero warnings and zero errors in the log.
  - **Repository and catalogue agree exactly** — 32 archives each, none pending.
  - **Retention is policy-consistent**: `borg prune --dry-run` with the configured
    policy afterwards reports nothing further to remove, so the surviving set is
    precisely what the policy specifies rather than merely a plausible number.
  - **No memory growth.** RSS 24,832 kB at start → 26,676 kB at end. The growth
    is decelerating rather than linear (+476 kB over the first 4 backups, +220 kB
    over the last 13), which is allocator warm-up, not a leak. File descriptors
    steady at 16 throughout, so no subprocess or pipe is being leaked either.
  - `backtrack status` and `--json` both correct against the live daemon.
  Two real defects were found and fixed while this ran, both by re-reading the
  pipeline rather than by a test: **(1)** a failing prune failed the whole backup,
  and since a failed job records no successful backup, a machine backed up
  perfectly every hour would have drifted to `AT_RISK` purely because
  housekeeping kept failing — retention failures are now warnings, while failures
  the user must act on (full or damaged repository) still fail the job; **(2)** a
  listing that broke off reported the rollback ("the listing was incomplete", a
  paraphrase of the question) instead of why Borg stopped talking. Also tidied:
  `ArchiveSummary` now carries `catalogued`, which Stage 6's sidebar needs to
  badge a snapshot "cataloguing…" rather than showing it as empty, and
  reconciliation is skipped outright when the destination is not reachable, so an
  unplugged drive does not write an error to the log on every start.
- 2026-08-07 (Stage 5 — Definition of Done): a live daemon driven through a full
  offline→online cycle against the demo repository, at a 20-second development
  cadence. Every claim measured rather than asserted:
  - **Online → offline → online, and the state reads correctly at each step.**
    `HEALTHY` (destination reachable, nothing held locally) → destination
    removed and one file edited → `PROTECTED_LOCALLY`, **1 local snapshot,
    45.8 kB, 0 expirable** → destination restored → `HEALTHY`, **1 local
    snapshot, 1 expirable**. The catch-up backup ran on the reconnect itself
    rather than waiting for the next tick, and dating the local snapshot for
    expiry followed from it.
  - **The local snapshot held exactly the one file that changed** —
    `changes protected on this computer archive="bt-local-…" files=1` — while
    the primary archives catalogued the full 8-item tree.
  - **The catalogue agrees:** 31 primary archives, 1 spool, 1 marked expirable.
  - **Never an error, never a nag: 71 log lines in the cycle window, all INFO.
    Zero ERROR, zero WARN.** The scan reports the number of lines it read, so
    the claim cannot be satisfied by reading nothing — which the first version
    of it did.
  - The status copy matches offline-strategy.md: "The backup destination isn't
    reachable — changes are being kept on this computer", and
    `On this PC  1 snapshot · 45.8 kB`.
  - Schema v1 → v2 migration verified on the real development index.
  - Full suite green: **450 tests**, including the real-borg integration set.

- 2026-08-07 (Stage 5 — Offline protection): the design question the stage
  turned on was not "how do we archive locally" but "how do we know what
  changed", and that is where the surprises were.
  - **Two timestamp defects, both latent since Stage 1, both fatal to the
    spool.** Listings were read from `borg list --json-lines`, whose `mtime` is
    a naive local-time string with no offset — so the catalogue held every
    file's modification time shifted by the machine's UTC offset, which is also
    what Stage 6's file pane would have displayed. And Borg *rounds* nanoseconds
    to microseconds where the walk truncated. Change detection is an equality
    test, so either defect alone made every file on the machine compare as
    modified. Listings now use `--format` with `{mtime:%s.%f}`, exactly as
    archive timestamps already used `{time:%s}` and for the same reason.
  - **Exact timestamp comparison is not achievable, and the number is measured.**
    Borg can only report microseconds through a value it renders via a float, so
    the stored value and the one read from the filesystem do not always round the
    same way. On a 3,000-file tree nothing had touched: an exact comparison
    reported **234 files as modified**, a two-microsecond tolerance reported
    none. No `--format` key exposes the raw nanoseconds. `MTIME_TOLERANCE_MICROS`
    is that finding, and the test that produced both numbers is kept.
  - **Exclusion patterns had to be reimplemented locally**, because the spool
    must know the delta before it can ask Borg anything, and a walk that ignored
    exclusions would report every file under `~/.cache` as new — those paths are
    excluded from backups, so the catalogue has never heard of them, so they can
    only compare as new. Semantics were measured against borg 1.4.5 rather than
    read off the manual: `*` does cross `/` in the default style and does not
    under `sh:`, patterns match the archive-relative path, and a matched
    directory prunes its subtree. `re:` is reported rather than guessed at.
  - **A delta archive needs a delta ingest.** `ingest_delta` folds in what the
    archive lists and then carries every interval that ended at the previous
    archive across this one, so a local snapshot browses as the whole tree.
    Ingesting it as a full listing would show every unchanged file as deleted at
    that snapshot — somebody browsing an hour spent on a train would find their
    documents missing. Deletions during an offline window are not captured,
    which is the trade offline-strategy.md names.
  - **btrfs mode will not run on a stock desktop, and that is a product finding
    rather than a bug.** Measured with btrfs-progs 7.1: `$HOME` is not a
    subvolume boundary (inode 257, inside a root-owned `@home`); an unprivileged
    process cannot snapshot a root-owned subvolume; and `btrfs subvolume delete`
    is refused without `user_subvol_rm_allowed`. Backtrack's daemon is a *user*
    service, so it detects btrfs, tries, fails, says so once, and the spool
    carries the load. The probe therefore creates **and removes** a real
    snapshot: one that can be created but not removed is worse than none, since
    hourly snapshots nothing can expire would fill the disk with no way out from
    inside the application. Removal clears the read-only property and unlinks
    the tree, which works with the permissions a user service has. Tested for
    real against a subvolume the test user owns — the only kind an unprivileged
    process can snapshot, which is the same limitation that decides the
    fallback.
  - **Two archives in one second collided.** `bt-local-<iso>` has one-second
    resolution and Borg refuses a duplicate name outright (exit 30), failing the
    whole run — reachable by pressing Back Up Now twice while away, or by the
    development interval override. A colliding snapshot is now dated to the next
    free second. **The primary path has the same latent flaw** (`bt-{host}-{iso}`)
    and is untouched here: no test reaches it, backups are minutes apart, and
    changing primary archive naming has a wider blast radius. Worth fixing when
    something else takes that code.
  - **`JobKind::Offline` exists for one reason:** only a backup that reached the
    *real* destination may start the local snapshots' expiry clock. Marking them
    discardable because another local snapshot succeeded would throw away the
    only copy of the versions they hold, and telling the two apart from
    bookkeeping rather than from the job itself is exactly how that goes wrong.
  - **Deviations, both deliberate.** The cap setting is
    `storage.offline.space_limit_gb`, which was already in the schema and mirrors
    the Preferences page, rather than the stage file's `spool_cap_gb`. And the
    delta is computed against the newest *catalogued* archive of any kind rather
    than the last network one: the union across spool archives is the same set of
    at-risk files, and each hour's archive is smaller for it.
  - The volatile parts of the data directory are now excluded from every backup
    — spool, snapshots, cache, staging, replaced, logs, and the live `index.db`.
    Not the whole directory: `config.toml` and `state.toml` are small, rarely
    changed, and exactly what somebody restoring a machine wants back.
  - **Three defects found by running the daemon rather than by the suite**, which
    is what the definition-of-done run is for:
    1. **The catch-up never ran.** A backup overdue by more than one interval is
       deferred by a jittered delay; the loop slept it and asked again, and
       because nothing had advanced the attempt clock it was still overdue, so it
       deferred again — forever. A laptop closed for two hours would never have
       backed up again, which is exactly the case the catch-up exists for. The
       tests pinned that a catch-up is *returned*; nothing pinned that it ever
       resolves into a run.
    2. **Every archive was empty.** The first version of the data-directory
       exclusion also excluded any backup source living inside it, which the
       development fixture's does. The backups "succeeded" holding nothing, with
       no warning anywhere.
    3. **Two backups in one second collided.** Borg refuses a duplicate archive
       name outright (exit 30). This had been latent since Stage 4 and was made
       reachable by this stage's reconnect catch-up, which can land in the same
       second as a scheduled tick — it appeared as an ERROR in the log for
       something nobody did wrong. Both primary and local names now move to the
       next free second.
  - **The first "no ERROR logs" check was vacuous** and said so only when
    challenged: it globbed `*.jsonl` and the rotated log is
    `backtrackd.jsonl.<date>`, so it read zero lines and reported zero errors.
    The scan now reports how many lines it read, and the definition-of-done
    figure below comes from a scan that is not vacuous.

- 2026-08-07 (S04-T5): `ImportRepo` returns once the **newest** snapshot is
  browsable and leaves the rest to the background job, newest to oldest. Two
  different kinds of work on purpose: the first is synchronous because the caller
  is a wizard about to show a timeline, and a wizard that says "you're set up"
  over an empty timeline reads as a failure; the second is a job because a year
  of history is minutes of reading and nothing should wait on it, including that
  method's reply. Resume needs no new machinery — the outstanding work is a query
  (`archives.status = 'pending'`, newest first), so a fresh daemon picks up
  exactly where the last one stopped, and startup reconciliation is the same code
  path.
  - **Bug found by a hung test, and it was a genuine deadlock.**
    `uncatalogued_count` took the catalogue mutex directly on a runtime thread.
    An ingest in flight holds that mutex in its blocking half while waiting for
    the next batch of items, and the only thing that can send one is the async
    half — which cannot run while the runtime thread is parked on the mutex. It
    is now `async` and goes through `spawn_blocking`, where nothing depends on
    the wait. The same shape would have deadlocked a multi-thread runtime under
    load, not only the single-threaded test one.
  - **Backfill priority:** a running backfill makes the scheduler report `busy`,
    so a scheduled backup is *skipped* (not queued) and runs on the next tick
    once the catalogue job ends, woken immediately by the job-completion waker.
    Borg's own locking means `create` genuinely cannot run beside `list`, so the
    delay is real but bounded and self-correcting. Real idle priority (nice/
    ionice on the subprocess) would need `unsafe` `pre_exec` and is not worth it.
  - **Verified live, and this is the strongest evidence Stage 4 has:** starting
    from an unconfigured machine, `ImportRepo` against the 30-archive demo repo
    returned with all 30 archives known and snapshot 30 browsable, 29 pending;
    the background backfill finished in about 6 seconds. The resulting catalogue
    is **structurally identical to the forward-built fixture** — the same 16
    version rows with the same interval boundaries (`report.odt` splitting at 8
    and 20, `notes.txt` at 25, `old-client-folder` ending at 15). Filling the
    catalogue backwards produces exactly what filling it forwards does.
  - **Finding for Stage 6:** Borg archives the source path as a single item and
    records no entries for the directories above it, so `folder_at("")` — the
    catalogue's tree root — is legitimately empty for any source that is not `/`.
    The timeline must open at the backed-up root, not at `/`. Confirmed against a
    real archive listing.
- 2026-08-07 (S04-T4): Reconciliation is one job, `start_catalogue`, used for
  three problems that are really the same one: a backup that reached the
  repository but was never catalogued (killed daemon, power cut), an adopted
  repository with nothing catalogued, and a prune that changed what exists. It
  runs on every start as a `JobKind::Index` job — a *shared* repository lock, so
  a restore and the next hourly backup proceed alongside it, and neither the bus
  name nor the first backup waits for it.
  - **Bug found while writing it, and it was the serious kind.** A listing that
    broke off mid-stream was committed as a complete catalogue and the archive
    marked browsable — a snapshot holding half its files, with the user's
    documents missing from the timeline and nothing anywhere saying so. Fixed by
    making the ingest fallible: the batch channel carries `Result`, a
    `ListingIncomplete` rolls the whole transaction back, and the archive stays
    `pending` to be re-read. A test pins that the retry produces no duplicates.
  - **Second bug, found by running it.** The scheduler and the reconcile job were
    both started *before* the bus name was claimed, so an instance about to lose
    the single-instance race could begin a backup and start rewriting the
    catalogue first. Borg's locking would have prevented corruption; the user
    would have seen spurious failures from a daemon that was never meant to be
    running. Startup is now explicitly two phases — assemble every fact that
    shapes an answer *before* the claim (the S03-T4 invariant, unchanged), start
    everything that *acts on* the repository *after* it.
  - **Checkpoint archives**: filtered at `parse_archive_line`, the single funnel
    every caller goes through, matching `<name>.checkpoint` and
    `<name>.checkpoint.N`. Discovered while testing that **Borg 1.4 already hides
    them from `list`** — and that `borg create` refuses a `.checkpoint` name
    outright (reporting it as already existing) while `borg rename` accepts one.
    The filter stays as defence in depth: that behaviour is version-dependent,
    and a checkpoint reaching the timeline would offer a restore from a backup
    that never finished. The rename trick is what makes the integration test
    cheap — producing a real checkpoint honestly needs gigabytes of
    incompressible data and a race with Borg's checkpoint timer, which does not
    belong in a test suite.
  - Verified against real Borg: an archive removed from the catalogue and a
    second never catalogued are both recovered by reconciliation, with the
    contents queryable afterwards; a checkpoint-named archive appears in neither
    the engine's listing nor the catalogue. Verified live: the stray archive left
    by an S04-T1 test run (seq 31, sitting between 30 and 32) was catalogued on
    the next start, and the demo repo and catalogue now both report 32 with
    nothing pending.
- 2026-08-07 (S04-T3): A backup is now one job with three phases — `archiving`,
  `cataloguing`, `pruning` — composed over a new public `JobStream::channel`,
  rather than three jobs. Three jobs would mean three progress bars, a cancel
  that could leave half a backup behind, and a "finished" the moment the archive
  existed but before it was browsable. **The archive row is written before its
  file list is read**, so a crash during cataloguing leaves a `pending` row
  reconciliation can find; recording it only after cataloguing would lose the
  archive silently. A cataloguing failure is a *warning*, not a failed backup:
  the files are in the repository and the user is protected, and a banner saying
  otherwise would be false and would train people to ignore banners.
  - **The index gained an order-independent ingest.** `ingest_pending(seq, …)`
    compares each item against its neighbours on *both* sides (the interval
    ending at `seq-1` and the one starting at `seq+1`) and extends, rejoins, or
    opens a version accordingly. Chronological ingest only ever finds a left
    neighbour — the original Stage 1 behaviour, unchanged — while backfill only
    finds a right one and a crash-hole finds both. A proptest is the oracle:
    reserve every archive, catalogue them in an arbitrary order, and the result
    must equal a plain chronological ingest. `sync_archives` reconciles the
    catalogue against the repository, inserting missing archives in chronological
    position and **splitting any version interval that would span an
    uncatalogued archive** — an interval spanning an archive nobody has read is
    the catalogue asserting something it does not know. Ingesting that archive
    later rejoins the halves if the content did match.
  - **Timestamps come from `borg list --format '{id}\t{time:%s}\t…'`, not
    `--json`.** Borg's JSON reports archive times as a naive ISO string in the
    machine's *local* time with no offset recorded, so reading it back costs a
    timezone database and still guesses across a DST boundary; `{time:%s}` asks
    Borg's own Python for the epoch it already holds. Verified: archive
    `backtrack-1786085752` reports exactly 1786085752. The name is the last
    tab-separated field because an imported repository's archive names are
    whatever somebody typed.
  - **Archive naming** is `bt-{hostname}-{YYYYMMDDThhmmssZ}`, basic ISO 8601
    rather than extended: the extended form's colons are the one character Borg
    gives meaning to in `repo::archive`. The trailing `Z` makes the instant
    unambiguous when a repository is read on a machine in another timezone.
    `CreateSpec` gained `created_at` so the catalogue dates an archive from the
    instant its name was built, not from whenever the ingest finished — on a
    long first backup those are hours apart.
  - **Cataloguing streams**: the listing crosses from Borg's pipe into SQLite in
    4096-item batches over a bounded channel, with the blocking ingest on a
    blocking thread. At 500k files the difference from buffering is hundreds of
    megabytes.
  - **Prune reconciles rather than parses.** After `borg prune` the catalogue is
    re-synced from the repository's own archive list, so it is correct even if
    Borg removed something we did not predict, and any earlier disagreement is
    repaired for free.
  - **Deviation, deliberate:** scheduled `compact` runs daily and only when no
    job holds the repository, but *not* at "03:00-ish". Knowing when 03:00 is
    locally costs a timezone database; the property behind the requirement —
    never compact while a backup is competing for the same repository — is what
    is implemented. Revisit at Stage 6, which needs local dates for the sidebar
    and will bring the means. A daemon that has never compacted starts its clock
    rather than compacting a freshly-adopted repository over a slow link.
  - **This closes the Stage 3 Definition-of-Done gap.** Verified live on the demo
    repo: repository and catalogue both report 32 archives (they were 34 and 30),
    prune removed archives from both together, and `backtrack search report`
    finds files from the newly-created archive. One archive sits `pending` — an
    archive created by an S04-T1 test run before the pipeline existed, which
    `sync_archives` discovered and slotted into chronological position. That is
    exactly the S04-T4/T5 case, waiting for its listing.
- 2026-08-07 (S04-T2): Three rules govern every gate. **A skip is a decision,
  not a failure** — nothing here raises a health banner and nothing here advances
  the attempt clock, so a backup deferred on battery runs when the charger goes
  in rather than at the top of the next hour. **Uncertainty never blocks a
  backup**: battery and metered are three-valued (`Option<bool>`), every test is
  `== Some(true)` and never `!= Some(false)`, so a desktop with no UPower or a
  container with no system bus backs up normally instead of quietly stopping
  because a service the user has never heard of is missing. **`BackupNow` passes
  every gate** — these are conditions on the *schedule*, and somebody pressing the
  button on battery has already decided. Properties are read with a plain
  `Properties.Get` (3 s timeout) rather than a caching zbus proxy: preflight runs
  once per backup, so caching saves nothing and a stale cache would report "on
  mains" for a laptop unplugged two minutes ago. NetworkManager's GUESS_YES/
  GUESS_NO are acted on (protecting an allowance is the point, and being wrong
  costs one deferred backup) while UNKNOWN stays unknown. The metered gate only
  applies when the destination is across the network, decided by Borg's remote
  syntax *and* by `statfs` magic on the path — a mounted NAS share is
  indistinguishable from a local directory by name alone, and `/mnt/nas` on a
  phone tether is exactly the case the setting exists for. Disk space is the
  stage's "soft check", two-tiered per health.md: under 1 GB free is `DEGRADED`
  (wired to the health model's `needs_attention`, previously hard-coded false),
  under 100 MB defers the run, since an index that runs out of disk mid
  transaction is worse than a backup an hour late. UPower and NetworkManager
  `PropertiesChanged` wake the scheduler, which is what turns "skipped: on
  battery" into a backup that starts when the charger goes in; failing to
  subscribe costs latency, not correctness. Skips are logged only when the reason
  changes — a laptop on battery all afternoon would otherwise bury the log.
  **Bug found by live checking:** `interval_for` propagated an absent
  `BACKTRACK_DEV_INTERVAL_SECS` out of the whole function with `?`, so under
  `BACKTRACK_DEV` *without* the override the interval read as `None` — the
  encoding for manual-only. Every development daemon silently stopped backing up
  on schedule, with nothing in the log to say so. The override rule is now a pure
  function with all four branches tested. Verified live against this machine's
  real UPower and NetworkManager (`on_battery=Some(false)`, `metered=Some(false)`
  from `u 4`/GUESS_NO) and against a bogus destination, which skipped with the
  right message and left no attempt recorded.
- 2026-08-07 (S04-T1): An internal tokio timer rather than a systemd timer, for
  one reason that settles it: a systemd timer can *start* a backup but cannot
  decline to run one, and every gate that matters (pause, battery, metered,
  reachability, a job already running) reads state only the daemon holds. The
  decision is a pure function over injected times, so "the laptop slept through
  six runs" is a test argument rather than a wait. **The cadence counts from the
  last *attempt*, not the last success** — counting from successes would turn a
  destination unplugged for a week into a backup attempt every tick for a week.
  A run late by more than one whole period is treated as a wake-from-suspend and
  deferred by a jittered delay of up to 2 minutes: firing the instant a lid opens
  races the network coming up, and the jitter stops every machine on a site from
  hitting one NAS at the same second after a power cut. The ordinary cadence is
  *not* jittered, or every hourly backup would carry up to two minutes of pointless
  latency. Pause and the attempt clock persist in a new `state.toml`, deliberately
  separate from `config.toml`: configuration is the user's stated intent and
  theirs to edit, this is bookkeeping, and "reset my settings" must not also mean
  "forget that backups are paused". Loading it is forgiving where config loading
  is strict — a corrupt config could silently disable a setting the user believes
  is in force, while a corrupt state file costs at worst a forgotten pause, which
  errs towards backing up. `BackupNow` bypasses a pause rather than refusing (the
  user pressing the button has said what they want) without lifting it. Added
  `Frequency::Weekly`, which the stage file listed and the schema lacked.
  `GetStatus`'s next-backup time now derives from the same input the scheduler
  decides on, since a promised time nothing will honour is worse than none.
  `BACKTRACK_DEV_INTERVAL_SECS` shortens the cadence for development only.
  Verified live: 20-second dev interval, runs at +0/+20/+40s with no drift, the
  wake-on-job-finish path exercised, three real archives in the demo repo, and
  the attempt clock on disk.
- 2026-08-07 (S03-T5): `Status` and `SearchResult` moved from the daemon into
  `core::dbus`, so the payloads have exactly one definition rather than a copy
  per client that can drift silently into a runtime unmarshalling error. Core
  gained `zvariant` (the type system only, not a D-Bus implementation).
  `--json` is treated as an interface with the same seriousness as the D-Bus
  one — pinned by a golden test, because somebody will put it in a monitoring
  script and never look at it again. On-the-wire `0`-means-never becomes JSON
  `null`: a script comparing `last_backup < cutoff` would read 0 as 1970 and
  conclude a healthy machine was decades overdue. The human rendering is
  explicitly *not* pinned and may be rewritten whenever it reads better.
  `doctor` redaction matches the *shape* of a secret assignment (a secret-ish
  word followed by `=` or `:`) rather than a list of field names, so it covers
  TOML, JSON, JSONL log fields and env dumps with one rule and keeps working
  when a new field name appears; the keyring is never read. Verified by planting
  a passphrase in `BORG_PASSPHRASE`, running the real command, extracting the
  real bundle and grepping every file — with a sanity check that the grep was
  not vacuous. **Bug found while checking live output:** an unconfigured machine
  computes `HEALTHY` (correctly — nothing has failed), so `status` printed "Your
  files are backed up" directly above "no destination is set up". The CLI now
  leads with configuredness and overrides the health headline entirely.
