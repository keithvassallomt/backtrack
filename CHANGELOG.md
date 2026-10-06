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
- A welcome wizard for a new computer. Choose what to back up (your personal
  folders, measured as you choose, or folders you pick yourself), where it goes
  (an external drive Backtrack finds for you, a network folder, or an SSH
  server), and a passphrase. Backtrack checks there is room, creates the backup
  there, and asks you to save or print its recovery key before it will go any
  further. The first backup then starts, and you can close the window while it
  runs: a notification says when it has finished.
- Already have backups? The wizard opens existing Backtrack or Borg backups from
  a folder or an SSH server and takes you straight to browsing them, starting
  with the newest while the older ones are read in behind it. A drive that
  already holds backups is offered to you rather than written over.
- Preferences, with every setting Backtrack has, on five pages: General,
  Backup, Storage, Security and Advanced. Changes take effect as you make them;
  there is nothing to apply or save.
- Choose the folders to back up and the things to leave out, the schedule, how
  long backups are kept, and how much room local snapshots may take while the
  backup destination is away. The Storage page shows how much space the
  backups take, before and after deduplication, and when the last one ran.
- Change your passphrase, and save or print the recovery key again, from
  Preferences → Security. The printed key says plainly that it works together
  with your passphrase, not instead of it.
- Check your backups for damage, free up the space deleted backups were using,
  rebuild the catalogue, and open the logs, from Preferences → Advanced.
  Reset All Settings starts the setup again and never deletes a backup.
- Run the welcome wizard again from Preferences to change how Backtrack is set
  up. Only what you change is changed. Moving to a new destination asks first,
  starts the new one fresh, and leaves the backups you already have where they
  are, ready to open again with Import.
- Backtrack now tells you when your backups need you, and only then. A yellow
  banner appears after a day with no successful backup, a red one when backups
  have stopped until something is done, and each says what is wrong and has a
  Fix… button that starts putting it right. Being away from your backup drive
  is not a warning: the window says so quietly while your changes are kept on
  this computer.
- Notifications for the same problems, following your choice in Preferences:
  only when attention is needed (the default), after every backup as well, or
  never. A backup that fails once and succeeds an hour later is never
  announced. A day without a backup is announced once, again at three days,
  then weekly; backups that have stopped are announced at once, then at most
  once a day. Clicking a notification opens its fix, and a notification about
  a problem that has since been fixed is taken down.
- A fix for each problem Backtrack can name: entering the passphrase again, or
  putting back your saved recovery key when the backups' key has changed;
  signing in to a network share again; making room on a full backup drive;
  seeing where Backtrack's space on this computer has gone and giving some of
  it back; checking, repairing and, if nothing else works, starting afresh
  from damaged backups, which are always kept rather than deleted; and
  installing the backup engine if it is missing.
- Backtrack checks your backups for damage once a month in the background, and
  sooner if backups keep failing for no known reason.
- `backtrack doctor` now includes how the backups' health has changed, the last
  error from each part of Backtrack, and which notifications have gone out.
- Restore a whole computer. After importing your backups onto a new computer
  from the Welcome page, Backtrack offers to bring everything back: your home
  folder as it was at any backup, only the folders you choose, or nothing for
  now. It works when the new computer's home folder is somewhere else or under
  another name.
- The restore runs a folder at a time, smallest first, in a window that shows
  how much is back, how long is left, the file being restored, and which
  folders are done. The window can be closed: the restore carries on, and
  opening Backtrack comes back to it.
- A whole-computer restore can be paused, and survives Backtrack or the
  computer being restarted: it carries on from where it was, keeping the files
  it had already fetched. Pausing finishes the file it is on first.
- Files already on the new computer are never overwritten without asking. Any
  that differ from the backup are set aside and asked about all at once, with
  the same summary as restoring a folder, once everything else is back.
- Cancelling a whole-computer restore keeps what has been restored so far, or
  takes away exactly what the restore added and nobody has changed since.
- Backups start again once a whole-computer restore has finished, never during
  it, and the first one runs straight away. A computer set up by importing
  from the Welcome page backs up what the old one did.
- Right-click in GNOME Files (Nautilus): "Restore Previous Version…" on a
  file or folder opens Backtrack on it, and "Browse Backups of This Folder…"
  opens Backtrack on the folder. The items appear only inside the folders
  Backtrack backs up, never on network locations or disks mounted inside
  them, and the switch in Preferences takes them away without restarting
  Files. Needs nautilus-python.
- Right-click in Dolphin: the same two items, in a Backtrack submenu. Dolphin
  cannot tell which folders are backed up, so they appear on every file and
  folder on this computer. The switch in Preferences hides them from the next
  right-click.
- Preferences → General says whether the Nautilus and Dolphin integrations
  are installed, and offers Install… with instructions when one is not.
- A tray icon on desktops that have a system tray (Plasma, Xfce, Cinnamon and
  others; not GNOME). It says when the last backup was, asks for attention
  only when backups are at risk or stopped, and offers Back Up Now, Pause
  Backups, Resume Backups and Open Backtrack. It starts at login.
- A folder with nothing to show now says why: it is not one of the folders
  Backtrack backs up (with a button to add it), it will be in the next
  backup, or it was not in the latest one and may be excluded or on another
  disk.
- Project bootstrap: Cargo workspace (core library plus daemon, GTK app, and CLI
  binaries), structured logging with JSONL rotation, developer task runner,
  versioning policy, and continuous integration.

### Changed
- Backtrack now requires GNOME 50 or newer (GTK 4.22, libadwaita 1.9).
- The backup service now announces when a job has finished, so an application
  that started one learns the outcome as it happens instead of asking
  repeatedly whether it is done yet.

- "Run in background" now does what it says. On, Backtrack starts when you log
  in; off, it stops once its window is closed and nothing is running.
- With "Remember passphrase" off, the passphrase is kept only while Backtrack
  is running, and is not stored in the system keyring.
- The timeline now updates as backups finish and as imported history is read
  in, rather than showing what was there when the window was opened.
- A backup destination that is there but will not accept backups, such as a
  drive that has become read-only, now stops backups with a banner and a
  notification saying so. It was treated as unreachable, and covered quietly
  by the protection on this computer.
- A backup that cannot sign in to an SSH server is now reported as a sign-in
  problem rather than as the server being unreachable.
- A damaged catalogue no longer stops Backtrack starting. It is set aside and
  read again from your backups, and what has been read so far can be browsed
  in the meantime.
- Snapshots kept on this computer now say whether your backup destination has
  them. Until it catches up they are marked "local backup only", in red.
  Afterwards, one that still holds versions of files the destination never
  got is marked "local snapshot", in yellow, and says when it will be
  removed; one holding nothing of its own is no longer marked at all.

### Fixed
- Pausing or resuming backups is announced to other programs straight away,
  rather than up to a minute later.
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
- The upload speed limit now applies to backups to an SSH server. Previously
  it was saved but had no effect.
- Automatic retention now always keeps the recommended set of backups. Custom
  numbers set earlier were being used instead, even with Automatic switched on.
- A file whose name contains an ampersand no longer shows as a blank line in
  Recently Replaced Files, the restore summary and its review list, the
  conflict dialog, or the messages that say what was restored.
- Verify Repository Health reported damaged backups as healthy.
- When the storage limit for protection on this computer was too small to keep
  your latest changes, Backtrack still counted them as protected. It now says
  they are not, and how to make room.
- A daily schedule was reported as not backed up recently in the hours before
  each daily backup was due.
- Backups imported from another computer no longer make a new computer look
  overdue before its own first backup has run.
- When a change is too big for the space set aside on this computer,
  Backtrack no longer deletes the local snapshots it already holds to make
  room that would still not be enough. It keeps them, and says it cannot
  protect the new change until the limit is raised or the backup drive is
  back.
- Protection on this computer that has gone over its space limit is now
  reported for as long as it is over, rather than forgotten at the next
  backup with nothing to save.
- Restoring a folder no longer asks for free space for a second copy of it.
  The restored files are moved into place from a working copy on the same
  disk, which takes no extra room, but a large folder was refused on any disk
  with less than twice its size free.

## [0.1.0] - TBD

Initial development version.

[Unreleased]: https://github.com/keithvassallomt/backtrack/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/keithvassallomt/backtrack/releases/tag/v0.1.0
