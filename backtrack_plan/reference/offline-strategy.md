# Backtrack — Offline Backup Strategy

*What happens when the backup destination (NAS, SMB/NFS share, SSH server) is unreachable.*

## The problem

Backtrack backs up hourly to a network location. Laptops leave home. When the
destination is unreachable we still want hourly protection — but:

- Borg's "incremental" behaviour is **per-repository**: deduplication works against
  the chunks already stored *in that repo*. A brand-new local repo starts empty, so
  the first backup into it is a **full copy** of everything — slow, and potentially
  bigger than the laptop's free disk.
- Borg has **no offline queue**: you cannot write chunks "for" the network repo
  while it's away and push them later. Repository writes are transactional against
  the live repo, and hand-copying chunks between repos is not supported.
- Borg 2 introduces `borg transfer` (move archives between repos), which will
  eventually allow consolidation — but Borg 2 is still beta and not a v1 foundation.

## The reframe that makes it cheap

While the network location is unreachable, **everything unchanged since the last
successful network backup is already safe** — it's on the NAS. The only data at
risk is **files modified during the offline window**. That is typically megabytes
per hour, not the full 100+ GB backup set.

So the offline job is *not* "a backup of everything." It is:

> **Back up only files changed since the last network archive, into a small local
> "spool" repository.**

## The design

```
online:   hourly borg create ──────────────► network repo (primary)
                                                   │ last success: 09:00
network drops ─┐
offline:  hourly borg create of ONLY files
          modified since 09:00 ────────────► local spool repo
          (megabytes per hour, capped size)   (~/.local/share/backtrack/spool)
network returns ─┐
          1. immediately run a normal network backup   ← current state is now safe
          2. keep spool archives ~30 days              ← they cover the versions
             (intermediate versions from the gap)         made while offline
          3. prune / expire the spool
```

### Why the spool stays small

- **The changed-file list is nearly free to compute.** Backtrack's local index
  already records the exact state of every file at the last network archive.
  Compare mtimes/sizes against it (cheap first cut: `mtime > last-archive-time`)
  and feed Borg an explicit file list.
- Unchanged files never enter the spool — they're already protected remotely.
- The spool repo is **size-capped** (e.g. 5–10 GB), pruned aggressively, and
  removed entirely after its archives expire.
- Same passphrase as the primary repo — the user never sees a second secret.

### Reconnection sequence

1. Destination becomes reachable (network-change signal or periodic probe).
2. Run a **normal network backup immediately** — the current state of every file
   is now safely off-machine. From this moment nothing is at risk.
3. Spool archives are kept ~30 days: they hold the *intermediate* versions
   created during the offline window (the 10:00, 11:00, 12:00 edits of a file
   that changed several times while away).
4. After expiry, prune the spool. (Future: with Borg 2's `borg transfer`,
   consolidate spool archives into the primary repo instead of expiring them.)

### UI rules

- Snapshots taken offline appear in the timeline with a small **"on this
  computer"** badge — they're real, browsable, restorable.
- The restore engine reads from whichever repo holds the requested version;
  the user never chooses a repo.
- Status copy while offline: *"Backup drive not reachable — protecting your
  changes on this computer until it returns."* Never an error, never a nag.
- Browsing history always works offline anyway (it reads the local index,
  not the repo).

## The honest trade-off

Offline-window versions live **only on the laptop** until reconnection (and, for
intermediate versions, until Borg 2 `transfer` lands). If the laptop dies *during*
the offline window, those versions die with it — but the last network backup is
intact. This is the same trade Time Machine makes and nobody minds:

> **Precedent:** when a Mac is away from its backup disk, Time Machine takes
> *local APFS snapshots* on the internal drive (~hourly, kept ~24h) and never
> transfers them to the backup disk — reconnecting just triggers a normal
> incremental. It's widely considered one of Time Machine's best features.

## Optional upgrade: filesystem snapshots where available

On **btrfs** systems (default on Fedora and openSUSE), the local safety net can be
real filesystem snapshots instead of a spool repo: copy-on-write, effectively free,
auto-expiring — a byte-for-byte match of Apple's approach.

| Filesystem | Offline safety net |
|---|---|
| btrfs | hourly CoW snapshots (near-zero cost), auto-expire |
| ext4 / xfs / others | Borg spool repo of changed files (design above) |

Same UI, same badges, same restore flow either way.

## Consequences elsewhere

- **Onboarding (wizard step 3)** must not say backups "pause" when the location is
  unreachable — they don't. Correct copy: *"If this location isn't reachable,
  Backtrack keeps protecting your changes on this computer and catches up when it
  reconnects."* (Mockup 12 updated accordingly.)
- **Disk-space monitoring**: the spool cap counts toward Backtrack's disk usage;
  surface it in the app's storage overview.
- **Security**: the spool repo is encrypted with the same key; it lives in the
  user's home and dies with a device wipe, matching user expectations.
