# Stage 5 — Offline Protection

## Objective
When the destination is unreachable, Backtrack keeps protecting changed files
locally (btrfs snapshots where available, else a capped spool repo) and catches up
on reconnect — exactly per
[../reference/offline-strategy.md](../reference/offline-strategy.md), which is
normative for this stage. The wizard copy already promises this behaviour
(mockup 12): "keeps protecting your changes on this computer and catches up when
it reconnects."

## Prerequisites
Stages 0–4. Read offline-strategy.md fully.

## Tasks

### S05-T1 — Reachability + wakeup
Destination probe (cheap, <2 s timeout): local path → exists+writable; ssh → TCP
+ borg `repo_info` with short timeout; mounted share → mountpoint present + stat.
Subscribe to NetworkManager connectivity signals → on change, re-probe within
30 s (debounced). State transitions logged and pushed via StatusChanged
(HEALTHY ↔ PROTECTED_LOCALLY).
**Accept:** simulated flap tests (mock prober): no thrash (min 60 s between
transitions); reconnect triggers catch-up scheduling (T4).

### S05-T2 — Spool repo path
On offline tick (non-btrfs): compute changed set via `changed_since(last_network_seq)`
(Stage 1 T5) → if empty, skip quietly. Else `borg create` into
`~/.local/share/backtrack/spool/` repo (created lazily, same passphrase via
SecretStore, `repo='spool'` in index) with an explicit file list. Enforce cap
(config `spool_cap_gb`, default 10): before create, if projected size busts cap →
drop oldest spool archives (repo + index) first; if a single hourly delta exceeds
cap → take it anyway once, then hold with a DEGRADED log (never silently stop).
**Accept:** integration: cut destination (rename dir), touch 3 files, offline tick
→ spool archive holds exactly 3 files; index shows it flagged spool; cap test with
tiny cap evicts oldest first.

### S05-T3 — btrfs mode
Detection at startup + on config change: home dir on btrfs && daemon can create
snapshots (try a probe snapshot in `~/.local/share/backtrack/snapshots/`, note:
requires the subvolume containing $HOME; if $HOME isn't a subvolume boundary,
snapshot the containing subvolume and record the relative prefix). Offline tick →
read-only snapshot named `bt-local-{iso8601}`; register in index as
`repo='fs-snapshot'` by walking the snapshot with the same ingest path (fast: only
metadata). Expiry: delete snapshots past retention (24 h of hourlies or after
catch-up + 30 days, whichever first) — snapshots are cheap but not free.
Fallback: any btrfs failure → log warn once, use spool path.
**Accept:** integration test behind a loopback btrfs image (skip-if-unavailable in
CI with a warning): snapshot created, indexed, browsable via `folder_at`, expired
correctly. Restore-from-snapshot = plain file copy (verify one file round-trips).

### S05-T4 — Reconnect catch-up
On reconnect: immediately schedule a normal network backup (respecting preflight);
on its success mark spool/fs-snapshot archives `expirable_at = now + 30 days`;
expiry job removes them (repo delete/subvol delete + `remove_archives`). If Borg 2
ever lands, `borg transfer` replaces expiry — leave a `// BORG2:` marker comment.
**Accept:** end-to-end: offline (2 spool archives) → reconnect → catch-up archive
in primary repo, spool marked expirable; time-travel test (mock clock) expires them.

### S05-T5 — Surfacing
GetStatus includes: offline?, mode (spool/fs-snapshot), local snapshot count +
bytes, expirable count. Index rows already carry `repo` — Stage 6 renders the
"on this computer" badge from it; Stage 9's Storage prefs shows usage (mockup 17
"Currently using: 1.2 GB · 14 snapshots on this computer").
**Accept:** `backtrack status --json` shows the offline block with correct numbers
during the T2 integration test.

## Definition of Done
Full offline→online cycle green in CI (spool path) and locally (btrfs path);
status copy matches offline-strategy.md ("never an error, never a nag" — assert
no ERROR-level logs in the happy offline cycle); progress.md updated; CHANGELOG
entry added.
