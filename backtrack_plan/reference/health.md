# Backtrack — Health Model & Failure UX

*Backup tools live or die on "did it actually back up?" This doc defines what Backtrack
monitors, when it speaks up, and how every known failure is presented and resolved.*

## Principles

1. **Silence means safe.** No news, badges, or notifications while everything works.
2. **Attention is earned by risk, not by events.** A failed backup at 14:00 that
   succeeds at 15:00 never deserved a notification.
3. **Every alert names the fix.** No error surfaces without a button that starts the
   resolution.
4. **Never cry wolf.** Offline-with-safety-net is *not* a warning state — it's the
   product working as designed.

## Health states

The daemon computes one overall state, exposed via `GetStatus()` and `StatusChanged`:

| State | Meaning | UI |
|---|---|---|
| `HEALTHY` | Last backup succeeded within 2× frequency | Nothing. Green check in Storage prefs. |
| `PROTECTED_LOCALLY` | Destination unreachable; offline safety net active | Subtle status line in main window; **no** notification |
| `PAUSED` | User paused (self-expiring) | Status line with resume time |
| `AT_RISK` | No successful backup (network *or* local) for > 24h | Yellow banner in app + one notification |
| `BROKEN` | Backups cannot run without user action | Red banner + notification, repeated max 1×/day |
| `DEGRADED` | Backups run but something needs eventual action (spool near cap, repo nearly full, catalogue rebuild advised) | Badge in Preferences only; mentioned in app banner after 7 days |

Escalation timing (defaults, config-visible in the future, not in v1 UI):
- `AT_RISK` after 24h without any successful protection; notification repeats at 72h, then weekly.
- `BROKEN` notifies immediately on first detection, then max once daily.

## Failure catalogue

Every error the daemon can hit maps to one of these, each with defined copy and a
resolution action. (Typed errors in core → D-Bus error names → this table.)

| Failure | Detection | State | User-facing copy (banner) | Resolution flow |
|---|---|---|---|---|
| Passphrase missing (keyring reset/locked) | Secret Service lookup fails | `BROKEN` | "Backtrack needs your backup passphrase again." | Dialog: enter passphrase → re-store in keyring; link to recovery-key help |
| Wrong passphrase (repo key changed) | borg exit code / error msg | `BROKEN` | "The saved passphrase no longer matches the backup." | Same dialog + "restore from recovery key" path |
| Destination credentials expired (SMB/SSH auth) | mount/ssh failure distinct from unreachable | `BROKEN` | "Backtrack can't sign in to nas.local." | Re-auth dialog for that destination |
| Destination full | borg ENOSPC / preflight | `BROKEN` | "The backup drive is full." | Offer: run Free Up Space (compact), shorten retention, or change destination |
| Local disk full (spool/staging) | preflight checks | `DEGRADED`→`BROKEN` | "Not enough space on this computer to keep protecting changes." | Open storage overview; lower spool cap; clear stash |
| Repo corruption | `borg check` (scheduled monthly, and after repeated failures) | `BROKEN` | "The backup needs repair." | Guided flow: re-run check → `borg check --repair` with plain-language warning → if unrecoverable, guided fresh-repo start that *preserves the old repo* for manual salvage |
| Index corruption | SQLite integrity check on daemon start | `DEGRADED` | (Preferences badge only) "Catalogue rebuilding…" | Automatic: rebuild from repo in background; browsing degrades gracefully to indexed-so-far |
| Borg missing / wrong version | startup probe | `BROKEN` | "Backtrack's backup engine is missing." | Distro-specific install hint; Flatpak build can't hit this |
| Backup interrupted (sleep/shutdown) | checkpoint archive detected | `HEALTHY` | None — next run resumes; checkpoints are hidden from the timeline | Automatic |
| Snapshot taken but indexing failed | ingest error | `DEGRADED` | Badge: "1 backup not yet browsable" | Auto-retry with backoff; manual Rebuild as fallback |

## Where health appears

1. **Main window status line** (under the header bar, only when not `HEALTHY`):
   e.g. "⚠ No backups for 3 days — Fix…" / "ⓘ Backup drive not reachable — protecting
   changes on this computer." Clicking opens the resolution flow directly.
2. **Notifications** per the state table — and respecting the user's General preference
   ("Only when attention is needed" default maps exactly to `AT_RISK` + `BROKEN`).
3. **Preferences → Storage**: last-backup row doubles as health detail (last success,
   last attempt, last error with "Show Details").
4. **`backtrack status` / `doctor`** for terminal users and bug reports.

## What is deliberately *not* monitored

- Verification of every archive on every run (monthly `borg check` only — full verify is
  hours on big repos; user can trigger manually in Advanced).
- Restore-test drills, SMART/disk health, destination free-space forecasting: out of
  scope for v1; the first two are candidates for later, the last is the NAS's job.

## Mockups

- `../mockups/23-health-banner.png` — main-window `AT_RISK` banner state.
- `../mockups/24-passphrase-dialog.png` — passphrase-recovery dialog (the dialog is
  normative; ignore the invented background window in that render).
