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
- Backtrack now has a window. Pick a folder and move through time beside it:
  the folder stays where it is while the backups change around it, so finding
  an older version of something is a matter of looking rather than searching.
  Everything you browse is read from the catalogue on this computer, so it is
  instant and it works with the backup destination switched off, unplugged or
  on the other side of the world.
- Backups are listed grouped the way you would think of them — today by the
  hour, then yesterday, this week, last week, and older months you can open
  when you want them. Backups kept on this computer while the destination was
  away are marked as such, and one still being catalogued says so rather than
  pretending to be empty.
- Files that were deleted later reappear as you step back, marked "deleted
  after this"; files you have changed since are marked "changed since then".
  Both are words as well as colour, so they are still readable in a screenshot
  or with colour-blindness.
- Step one backup at a time with the Older and Newer buttons or Ctrl+Left and
  Ctrl+Right, and use the arrow beside them to skip straight to the next time
  the selected file actually changed — which saves stepping through forty
  hourly backups in which nothing happened to it.
- A calendar for longer jumps, with the days that have backups shaded and the
  days that do not left plain and unclickable, and a strip along the bottom
  showing the whole history at a glance: where backups are dense, where they
  are thin, and where a week is missing. The strip can be dragged, and it
  always lands on a backup that exists.
- A preview pane showing the selected file as it was at that point in time,
  with text and images shown in place. What is already known about a file
  appears the instant you select it; the contents follow, with a spinner and a
  way to stop if the backup destination is slow.
- A menu with Back Up Now and a pause that expires by itself — for an hour,
  until tomorrow, or until you resume — along with keyboard shortcuts, help
  and about. There is no way to switch backups off from here, deliberately.
  A line along the bottom of the window says how backups are doing.
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
- Restoring, with the awkward parts answered rather than passed on to you. A
  file that has not changed is left alone instead of being offered as a
  decision. A file that has says which of the two versions is newer — yours or
  the backup's — because that is the thing you actually need to know and it is
  the one thing this kind of dialog usually omits.
- Restoring a folder is one screen, not a storm of pop-ups. It says how many
  files are identical, how many will be replaced and how many of those are
  newer on your disk, what will be added, and — in words, because it is the
  first thing anyone fears — that files which exist only on your computer are
  kept and nothing is deleted. The numbers are not an estimate: the whole
  restore is worked out before anything is touched, so Cancel costs nothing.
- A Review list behind that screen, if you want to go file by file. Every
  clash with both versions' dates and sizes, a tick per file, and a count on
  the button so you can see what you are about to do. Anything that changed
  kind — a file where you now have a folder — starts unticked and stays that
  way unless you say otherwise.
- Replacing a file is never destructive. The version that was there is kept
  for 30 days, and "Recently Replaced Files" in the menu lists them grouped by
  the restore that displaced them, with a button to put any of them back. The
  toast's Undo puts a whole restore back; this is for when you notice on
  Friday what went wrong on Tuesday.
- Restore To… puts the files in a folder you choose instead of over the
  originals, in a new folder named for the backup they came from. Nothing on
  your computer is touched and there is nothing to decide, so it is the way to
  look at an old version before committing to it.
- Search across every backup at once, from the toolbar or Ctrl+F. Type part of
  a name and Backtrack looks through its whole history, including the backups
  where the file no longer exists, which is the case an ordinary file search
  cannot help you with at all.
- What you have lost comes first. Results are ordered by what is missing from
  your computer rather than by what is missing from the last backup, because a
  file you deleted an hour ago is still in the last backup and is exactly the
  one you are looking for.
- Each result says where the file lived and when it existed: "Existed: 26 Jun
  to 1 Jul, 8 versions, 214 KB". That line is usually how you recognise the
  thing you are after, long after you have forgotten which folder it was in.
  Anything no longer on your computer is marked, and only those offer to put
  themselves back, because a file that is still there does not need restoring
  over itself.
- View in Timeline takes a result to the last backup that still had it, with
  the file picked out, so you land looking at the thing you clicked rather
  than at the folder it used to be in.
- Compare with Today shows the backed-up version beside the one on your
  computer before you decide anything. Text files get the changes marked,
  green for what has been added since the backup and red for what has gone,
  with a count of how many separate places differ. Images are shown side by
  side, because a list of changed pixels is not something anyone can read.
  Anything else gets its dates and sizes and a straight answer about whether
  the two are the same. Restore This Version hands over to the usual restore,
  safety copy and all; Keep Current Version simply closes.
- Files now show whether they are still on your computer, at every point in
  the timeline. Previously the newest backup could only ever say nothing,
  which is the moment you are most likely to be looking at when something has
  just gone missing.
- Project bootstrap: Cargo workspace (core library plus daemon, GTK app, and CLI
  binaries), structured logging with JSONL rotation, developer task runner,
  versioning policy, and continuous integration.

### Changed
- Backtrack now requires GNOME 50 or newer (GTK 4.22, libadwaita 1.9).
- The backup service now announces when a job has finished, so an application
  that started one learns the outcome as it happens instead of asking
  repeatedly whether it is done yet.

### Fixed
- A file could be shown as "deleted after this" while it still existed, in the
  window between a backup finishing and its file list being read. On a first
  run, which reads that list for every backup in turn, this could have been the
  first thing shown about an entire folder.
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
