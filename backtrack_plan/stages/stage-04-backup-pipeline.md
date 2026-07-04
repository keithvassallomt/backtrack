# Stage 4 — Backup Pipeline

## Objective
The daemon backs up on schedule without supervision: hourly timer, sane preflight,
create → stream-index → prune → compact, interrupted-run recovery, and first-run
backfill of pre-existing repos.

## Prerequisites
Stages 0–3. Read [../reference/stack.md](../reference/stack.md) §5,
[../reference/open-questions.md](../reference/open-questions.md) Q3 (prune/compact
notes, files-cache behaviour).

## Tasks

### S04-T1 — Scheduler
Internal tokio timer driven by config `frequency` (hour default; also daily/weekly/
manual). Missed runs (suspend/laptop lid): on wake, if last success older than one
period → run within 2 min (jittered). Pause state (`Pause(until)`) persists across
daemon restarts. `BackupNow` bypasses pause once.
**Accept:** time-mocked tests: normal cadence, wake-after-suspend catch-up,
pause honored and self-expires.

### S04-T2 — Preflight
Before each run, in order: paused? → skip(log info). On battery (UPower D-Bus
`OnBattery`) and config says don't → skip quietly, retry on AC signal. Metered
(NetworkManager `Metered` enum) and destination is remote and config says don't →
skip. Destination reachable? → if not, hand off to offline path (Stage 5; until
then, log + set PROTECTED_LOCALLY-pending state). Local disk space for
index/cache growth (soft check).
**Accept:** each gate unit-tested against mocked UPower/NM properties.

### S04-T3 — The happy path
`create` with stable archive naming `bt-{hostname}-{iso8601}`; exclude patterns from
config (compiled to borg `--pattern` args; defaults per prototype: Trash, caches,
node_modules, VM images, *.iso). On success: stream `list_archive` into
`ingest_archive` (same job, phase "cataloguing"); then prune per retention policy
(borg prune flags from config; default hourly24/daily7/weekly4/monthly6) +
`remove_archives` in index; compact runs on its own schedule (daily 03:00-ish,
off-peak, skipped if a job is active).
**Accept:** integration test on demo repo: run twice → 2 archives, index rows
correct, prune honors a tightened policy (oldest hourly disappears from repo AND
index atomically).

### S04-T4 — Interruptions
Daemon killed mid-create: borg checkpoint archives (`.checkpoint`) must be
excluded from ingest and from the timeline; next run proceeds normally (files
cache makes it cheap). Ingest crash after create: archive exists but not indexed →
on startup, reconcile repo archive list vs index `archives` and ingest any missing
("1 backup not yet browsable" DEGRADED per health.md — full surfacing in Stage 10,
log-only now).
**Accept:** kill -9 during integration-test create → next run completes and
reconciliation ingests everything; no `.checkpoint` rows in index.

### S04-T5 — First-run backfill
On ImportRepo (existing repo with N archives): ingest newest archive first
(synchronously — the UI needs one browsable snapshot fast), then remaining
archives newest→oldest as a background Index job (idle-priority, cancellable,
resumes after restart via `archives.status='pending'` rows).
**Accept:** import a 30-archive demo repo → newest browsable in seconds;
backfill completes in background; restart mid-backfill resumes where it left off.

## Definition of Done
Leave the daemon running against demo-repo with a 1-minute dev frequency for
30+ minutes: correct number of archives, prune keeping policy-consistent set,
no memory growth (log RSS at start/end), progress.md updated, CHANGELOG entry
("Added: automatic hourly backups…").
