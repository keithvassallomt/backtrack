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

## [0.1.0] - TBD

Initial development version.

[Unreleased]: https://github.com/keithvassallomt/backtrack/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/keithvassallomt/backtrack/releases/tag/v0.1.0
