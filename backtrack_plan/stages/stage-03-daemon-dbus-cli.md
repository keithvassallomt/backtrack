# Stage 3 — Daemon, D-Bus API, CLI

## Objective
`backtrackd` runs as a systemd user service exposing `org.backtrack.Daemon1` on the
session bus, with a job model and signals; `backtrack` CLI drives it. After this
stage, `backtrack backup-now` triggers a real backup of the demo repo end to end.

## Prerequisites
Stages 0–2. Read [../reference/stack.md](../reference/stack.md) §2 (the API table
is normative — implement exactly these methods/signals, names included).

## Tasks

### S03-T1 — Daemon skeleton
Config load from `config.toml` (serde; documented defaults; unknown keys warn not
fail). Single-instance via D-Bus name ownership (`org.backtrack.Daemon1`).
Clean shutdown on SIGTERM (finish or checkpoint current job). Own the single
`IndexWriter`. `BACKTRACK_DEV=1` path separation from Stage 0 honored everywhere.
**Accept:** two daemon starts → second exits 0 with "already running" log;
kill -TERM during idle exits < 1 s.

### S03-T2 — Job model
Job registry: u64 IDs, kinds (Backup, Restore, RestoreEverything, Index, Prune,
Compact, Check), states (Queued, Running, Paused, Cancelling, Done, Failed{err}).
One backup-class job at a time (queue); restores can run alongside indexing but
not alongside backup (repo lock — serialize on a repo-guard). `CancelJob`,
`PauseJob` (pause = SIGSTOP is forbidden; implement as graceful checkpoint for
DR jobs, reject for others with a typed error). Progress events fan out as D-Bus
signals with ≤4 Hz rate limiting.
**Accept:** unit tests on the state machine incl. illegal transitions; cancel during
a MockEngine backup lands in Done-with-Cancelled outcome.

### S03-T3 — D-Bus interface
zbus service implementing every method/signal from stack.md §2: BackupNow, Pause,
Resume, GetStatus, RestoreFiles, PreviewFile (returns pipe fd; daemon extracts via
`extract_stdout`, caches by (archive, path, hash) under a 1 GB LRU cache dir),
CompareFile, SearchFiles, RestoreEverything, Prune, Verify, Compact, CancelJob,
PauseJob, SetConfig/GetConfig, SetupRepo, ImportRepo; signals BackupProgress,
RestoreProgress, IndexingProgress, StatusChanged. Errors cross as named D-Bus
errors mirroring the taxonomy (`org.backtrack.Error.PassphraseMissing` etc.).
Methods that are premature (SearchFiles before Stage 8 UI, RestoreEverything
before Stage 11 UX) still get functional implementations now — the core supports
them; UIs come later.
**Accept:** `busctl introspect` matches the documented interface exactly (snapshot
test); a zbus-client test drives BackupNow against demo repo and receives progress
signals then a StatusChanged(HEALTHY).

### S03-T4 — systemd + activation
`packaging/systemd/backtrackd.service` (user unit, `Type=dbus`,
`BusName=org.backtrack.Daemon1`) + D-Bus service activation file. `just
install-units` (dev-mode install to `~/.config/systemd/user/`). Wizard/GUI later
rely on activation — the daemon must come up on first method call.
**Accept:** with units installed, `busctl --user call ... GetStatus` cold-starts the
daemon; `systemctl --user status backtrackd` healthy.

### S03-T5 — CLI
`backtrack` (clap): `status [--json]`, `backup-now`, `pause <duration>|resume`,
`restore <archive> <path>... [--dest DIR] [--policy ...]`, `search <query>`,
`prune|verify|compact`, `config get|set`, `doctor` (collects: versions, config
redacted of secrets, health state, last 200 log lines, repo/index quick stats →
tarball path printed). Human output pretty, `--json` stable for scripts.
**Accept:** golden-file tests for `status --json`; `doctor` bundle contains no
passphrase material (test greps).

## Definition of Done
End-to-end demo: `just demo-repo && just run-daemon &` then `backtrack backup-now`
creates archive #31, index updates (visible via `backtrack search report`), CI
green, progress.md updated.
