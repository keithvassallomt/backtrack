# Stage 10 — Health Model & Failure UX

## Objective
Backtrack knows when the user's data is at risk and says so honestly — and every
failure it can name comes with a button that fixes it.
[../reference/health.md](../reference/health.md) is **normative for this entire
stage**: states, timings, copy, and the failure catalogue. Do not invent states
or reword copy.

## Prerequisites
Stages 3–5, 6, 9. Mockups [23](../mockups/23-health-banner.png),
[24](../mockups/24-passphrase-dialog.png).

## Tasks

### S10-T1 — State machine
Daemon-side health evaluator: inputs (last success time, last attempt+error,
pause state, offline mode, spool/disk margins, pending-index count) → one state
(HEALTHY / PROTECTED_LOCALLY / PAUSED / AT_RISK / BROKEN / DEGRADED) per the
health.md table. Escalation timers: AT_RISK at 24 h, re-notify 72 h then weekly;
BROKEN notify immediately, max 1/day. Persisted last-notified timestamps.
StatusChanged carries the state + reason code.
**Accept:** table-driven unit tests covering every row of the health.md state
table + timer tests with mock clock (incl. "failed 14:00, succeeded 15:00 → no
notification ever fired").

### S10-T2 — Notifications
GNotification (works in Flatpak via portal): title/body per health.md copy,
default action opens app at the resolution flow. Respect Preferences policy
mapping exactly: "Only when attention is needed" = AT_RISK + BROKEN only;
"After every backup" adds success notes; "Never" = never (health still visible
in-app).
**Accept:** policy matrix test via a mock notification sink; clicking (activation
param) routes to the right flow.

### S10-T3 — Banner
Main-window banner (`AdwBanner`) per mockup 23: yellow AT_RISK ("No successful
backups for 3 days" + Fix…), red BROKEN (copy per catalogue row + action button),
info-tone PROTECTED_LOCALLY/PAUSED status line (not a warning banner — subtle,
per health.md principle 4). DEGRADED: badge in Preferences only until 7 days.
**Accept:** forcing each state via a dev D-Bus method (`BACKTRACK_DEV` only)
renders the right banner; screenshots vs mockup 23.

### S10-T4 — Resolution flows
One flow per catalogue row, launched from banner button / notification / Fix…:
- **Passphrase missing/wrong** → dialog per mockup 24 (copy verbatim), verifies
  against repo, re-stores in keyring, resumes; "Lost the passphrase?" link →
  recovery-key import path.
- **Auth failed** → destination re-auth (remount/ssh credential prompt).
- **Destination full** → dialog offering Free Up Space (compact job), retention
  edit (opens prefs), Change destination.
- **Local disk full** → storage overview dialog (spool cap, stash size, cache) with
  one-click reductions.
- **Repo corrupt** → guided: run check (progress) → offer `--repair` with the
  plain-language warning from health.md → unrecoverable: fresh-start flow that
  RENAMES the old repo aside (never deletes) and reruns setup.
**Accept:** each flow integration-tested where the failure is inducible (wrong
passphrase, read-only destination, tiny quota dir, corrupted test repo);
repo-corrupt flow proven to never delete (fs assertion).

### S10-T5 — Scheduled verification
Monthly `borg check` (repository level; archives sampled) as a low-priority job;
index `integrity_check` on daemon start (already Stage 1) now surfaces DEGRADED
"Catalogue rebuilding…" + auto-rebuild from repo listings.
**Accept:** mock-clock triggers monthly check; induced index corruption →
auto-rebuild completes and state returns HEALTHY.

### S10-T6 — Doctor
Extend `backtrack doctor`: include health state history (ring buffer of last 50
transitions), last error per subsystem, escalation timer state. Still zero
secrets (grep test stays).
**Accept:** bundle from a BROKEN state names the failing catalogue row.

## Definition of Done
Kill-switch drill: with the app running, (1) delete keyring entry, (2) make
destination unwritable, (3) fill spool quota — each produces the documented state,
banner, notification, and a working resolution back to HEALTHY. progress.md +
CHANGELOG updated.
