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

### Fixed
- Backups that need no passphrase now open from Already have backups?
  without one: Vorta's unencrypted backups, and Pika Backup's made with
  Continue Unencrypted. The wizard used to stop at "Something went wrong".
- When Backtrack carries on backing up into backups another program made,
  such as Vorta or Pika Backup, thinning out older backups no longer removes
  some of that program's. Backtrack now thins out only its own.
- Backups whose key is kept in a file of its own, as Vorta's key-file
  encryption keeps it, now open from the wizard once you choose that file.
  The wizard used to stop at "Something went wrong".
- Rewind opened from the file manager, the launcher or the tray now shows as
  part of Backtrack, with its name and icon, in GNOME's window switcher and
  overview. It used to appear there as a separate application with neither.

## [0.1.0] - 2026-11-07

### Added
- Automatic, encrypted, deduplicated backups with Borg, to an external drive,
  a network folder or an SSH server: hourly by default, or daily, weekly or by
  hand. A computer that was asleep catches up when it wakes, and backups wait
  while on battery or a metered connection unless you say otherwise. A welcome
  wizard sets it all up, with a recovery key to save or print, or opens
  backups you already have.
- Protection away from the backup destination: the files you change are kept
  on this computer, small and encrypted, as snapshots you can browse like
  backups, and sent on as soon as the destination is back.
- A window for going back in time: the folder stays where it is while you
  step through its backups, grouped by today, yesterday, this week and
  earlier months, listing only those in which the folder changed. Files are
  marked "deleted" or "changed"; a calendar, a strip of the whole history and
  a jump to a file's next change move further; a preview shows the file as it
  was. Everything is read from a catalogue on this computer, so it is instant
  and works with the destination unplugged.
- Rewind, a full-screen view of a folder's history: the folder as it is now in
  front, its backups receding behind it over your desktop's wallpaper, and a
  timeline down the side. Open it from the window (Ctrl+T), the file manager,
  the launcher or the tray.
- Restores that ask only what matters: which of two versions is newer, one
  summary for a folder with a file-by-file review if you want it, and nothing
  ever deleted. Undo stays offered after a restore, and every file a restore
  replaces is kept for 30 days in Recently Replaced Files. Restore To… puts
  files in a folder of your choice instead.
- Search across every backup (Ctrl+F), with what is missing from your
  computer first and where and when each file existed, and Compare with Today
  for text, images and other files.
- What you move to the Trash appears in the timeline until the Trash is
  emptied, so a file made and deleted between two backups can still be got
  back. On btrfs, Snapper's snapshots appear too, and Preferences can set
  Snapper up on Fedora, openSUSE and Arch Linux.
- Restoring a whole computer onto a new one: everything, or the folders you
  choose, a folder at a time, paused and resumed at will, never overwriting a
  file on the new computer without asking.
- Backtrack says when your backups need you, and only then: a banner and a
  notification after a day without a backup or when backups have stopped,
  each with a Fix… for the problem. A monthly check for damage runs in the
  background.
- Preferences with every setting on five pages, applied as you change them,
  including the passphrase, the recovery key, checking and repairing the
  backups, and running the wizard again.
- Right-click items in GNOME Files and Dolphin (Restore Previous Version…,
  Browse Backups of This Folder…, Rewind This File…, Rewind This Folder…), a
  tray icon on desktops with a tray, and the `backtrack` command for status,
  backups, search and restores from a terminal.
- Accessible throughout: a screen reader can follow the window and Rewind,
  everything works from the keyboard, and high contrast is honoured.
- Packages for Fedora (COPR), Ubuntu (PPA) and Arch Linux (AUR), and a Flatpak
  on FriendlyHub, for x86_64 and 64-bit ARM.
- Guides to
  [Rewind](https://github.com/keithvassallomt/backtrack/blob/main/docs/rewind.md),
  [the Trash](https://github.com/keithvassallomt/backtrack/blob/main/docs/trash.md),
  [Snapper](https://github.com/keithvassallomt/backtrack/blob/main/docs/snapper.md)
  and the
  [file manager items](https://github.com/keithvassallomt/backtrack/blob/main/docs/file-manager-integration.md).

[Unreleased]: https://github.com/keithvassallomt/backtrack/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/keithvassallomt/backtrack/releases/tag/v0.1.0
