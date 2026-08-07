<!--
  Changelog rules (keep this block):
  - Format follows Keep a Changelog (https://keepachangelog.com/en/1.1.0/).
  - Categories, in this order: Added, Changed, Deprecated, Removed, Fixed, Security.
  - Entries are user-facing sentences describing the change, not commit subjects.
  - AI/contributors add entries under [Unreleased] ONLY. A human moves them into a
    versioned section at release time via `just bump-version` (see VERSIONING.md).
-->

# Changelog

All notable changes to Backtrack are documented here. This project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) and the
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format.

## [Unreleased]

### Added
- Backtrack keeps protecting your files when the backup destination isn't
  reachable. Away from your backup drive, on a train, or off the network, the
  files you change are kept safely on this computer instead, hourly, and appear
  in the timeline as ordinary snapshots you can browse and restore from. This is
  not an error state and Backtrack will not nag you about it.
- Local protection is deliberately small: only the files that have actually
  changed are kept, so an offline hour normally costs megabytes rather than a
  copy of everything. It is encrypted with the same passphrase as your backups,
  stays inside a storage limit you can set, and drops its oldest snapshots first
  if it reaches that limit.
- On filesystems that support it, Backtrack uses instant copy-on-write snapshots
  instead — cheaper still, and a restore is a straight file copy. It checks
  whether that will genuinely work on your machine and quietly uses the other
  method if not.
- Reconnecting to the backup destination starts a backup straight away rather
  than waiting for the next scheduled time, so the gap where your latest work
  exists only on this computer closes as soon as it can. What was kept locally
  is then held for another 30 days — it still has the in-between versions from
  while you were away — and cleared afterwards.
- `backtrack status` now reports whether the destination is reachable, how
  changes are being protected meanwhile, and how much is being held on this
  computer.
- Automatic backups on a schedule: hourly by default, with daily, weekly, and
  manual-only alternatives. A machine that was asleep or switched off catches up
  shortly after it wakes rather than waiting for the next slot, and a pause set
  from the menu is honoured for its full duration even if the computer restarts
  in the meantime. "Back Up Now" always runs, pause or no pause.
- Scheduled backups are skipped, quietly and without counting as a failure, when
  the computer is on battery or the connection is metered — unless you have said
  otherwise in Preferences — and start as soon as that changes rather than
  waiting for the next scheduled time. A backup destination that is not plugged
  in or not mounted is reported rather than treated as an error.
- Every backup is catalogued as it is taken, so new snapshots are browsable and
  searchable the moment the backup finishes. Old snapshots are removed according
  to the retention policy, from the catalogue and the repository together, and
  repository space is reclaimed on a daily cadence.
- Backups that were interrupted are recovered automatically: a snapshot that
  reached the backup destination but was not catalogued is picked up the next
  time Backtrack starts, and Borg's partial "checkpoint" archives never appear in
  the timeline.
- Pointing Backtrack at a backup repository that already holds history makes the
  most recent snapshot browsable straight away, with the rest of the history
  filling in behind it, newest first. The catalogue picks up where it left off if
  the computer is restarted part way through.
- Project bootstrap: Cargo workspace (core library plus daemon, GTK app, and CLI
  binaries), structured logging with JSONL rotation, developer task runner,
  versioning policy, and continuous integration.

### Fixed
- A file that is deleted or moved while a backup is reading it no longer fails
  the whole backup. The backup completes with everything that still existed, as
  it always did underneath, and is now reported that way instead of as a failure
  needing attention.
- File modification times shown in Backtrack are now correct rather than offset
  by your timezone.

## [0.1.0] - TBD

Initial development version.

[Unreleased]: https://github.com/keithvassallomt/backtrack/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/keithvassallomt/backtrack/releases/tag/v0.1.0
