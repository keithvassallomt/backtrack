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

### S10-T7 — Local snapshot badges
Added after the Definition of Done, at Keith's direction: "on this computer"
did not say what matters about a local snapshot. Before the destination has
caught up, the timeline badge reads "local backup only", in red, and its
tooltip says the snapshot exists only on this computer. Once the destination
has caught up, the badge reads "local snapshot", in yellow, and its tooltip
says the destination now has the snapshot's data, that the intermediate
versions here are only on this computer, and the date the snapshot is
removed. The yellow badge is shown only when the snapshot holds a file
version the next backed-up snapshot does not; otherwise there is no badge.
The removal date comes from the same rule the daemon removes snapshots by.
**Accept:** unit tests cover each phase and the no-difference case against a
real catalogue, including a catalogue query that stays an index lookup; the
badges and tooltips are checked in the window.

### S10-T8 — An unwritable destination is not a failed sign-in
Found in the Flatpak hand checks (Keith, 2026-10-12): a local folder that Borg
may not write to reads "Backtrack can't sign in to …", and its fix asks for a
sign-in. Borg's classifier takes every "Permission denied" as a refused
sign-in, which is right for ssh's "Permission denied (publickey)" and wrong for
the operating system's `[Errno 13] Permission denied: '/path'`.
- health.md's catalogue gains the row: destination not writable, `BROKEN`, its
  own banner and notification copy, and a resolution that names the folder and
  says how to give it back its permissions, or to choose another place.
- The classifier tells the two apart: ssh's refusal stays `AuthFailed`; a
  filesystem `PermissionError` on the repository is the new error.
- The Fix page, the notification and `backtrack status` use the new row, and
  the D-Bus reason is its own.

**Accept:** unit tests classify both kinds of "Permission denied" from Borg's
real output; with a local destination made read-only, the notification, banner
and Fix page say it cannot be written to, and making it writable again returns
to `HEALTHY`; an SSH server that refuses the key still reads "can't sign in".

### S10-T9 — A locked keyring is not a wrong passphrase
Found in the Flatpak hand checks (2026-10-12): with KWallet locked, the
Secret Service hands back an empty secret for Backtrack's item instead of an
`IsLocked` error, `oo7` passes it on, and Borg, given an empty passphrase,
says it is wrong. The banner then reads "The saved passphrase no longer
matches the backup.", and every retry is another failed wallet access, which
KWallet reports as "repeated failed attempts". health.md's row for a locked
keyring is "Backtrack needs your backup passphrase again.".
- The keyring store checks the item's `Locked` property before reading it,
  and treats a locked item, and an empty secret from any service, as no
  passphrase (`PassphraseMissing`). A Backtrack passphrase is never empty.
- Scheduled work keeps looking, through the item's `Locked` property alone,
  so backups carry on by themselves once the keyring is unlocked, and a
  locked item is never asked for its secret.
- The Fix flow's re-store works on a locked keyring: the Secret Service's own
  unlock prompt appears, and the passphrase is saved once it is unlocked.

**Accept:** a unit test with a store that answers empty gets
`PassphraseMissing`; on the Plasma VM with the wallet locked, the banner and
notification ask for the passphrase again, KWallet shows no "repeated failed
attempts" warning over an hour, and the Fix flow saves the passphrase and
returns to `HEALTHY`; on the GNOME VM, a locked login keyring gives the same.

## Definition of Done
Kill-switch drill: with the app running, (1) delete keyring entry, (2) make
destination unwritable, (3) fill spool quota — each produces the documented state,
banner, notification, and a working resolution back to HEALTHY. progress.md +
CHANGELOG updated.
