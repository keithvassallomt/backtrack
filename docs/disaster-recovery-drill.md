# Disaster recovery drill

The Stage 11 drill: a laptop that is gone, its backups, and a new computer to
bring everything back to. It proves the whole path a person takes, from the
Welcome page to the first backup of the restored computer, including an
interruption in the middle.

`scripts/dr-drill` builds the fixture and does the fiddly parts. Everything it
makes lives under `/var/tmp/backtrack-drill`, outside `/home` on purpose: a
scratch home inside the real one would sit inside the old laptop's archive
paths, which no real computer does.

## What it needs

- BorgBackup, and a debug build: `cargo build --workspace`.
- The development units installed (`just install-units`). The drill points the
  development daemon at the scratch home with a systemd drop-in rather than
  running a second daemon beside it, because D-Bus activation goes through that
  unit: a daemon killed mid-drill is brought back into the drill, never into the
  real development data. `dr-drill reset` removes the drop-in.
- Free space in `/var/tmp` of about three times the drill's size (the old home,
  the backups, and the restored home). The default is 6 GB; 12 GB gives more
  time to pause and interrupt.
- No Backtrack window open: the drill's window has to be the new computer's.

## The fixture

`scripts/dr-drill setup [GB]` builds:

- **The old laptop's home**, `/var/tmp/backtrack-drill/old/home/keith`:
  Desktop, Documents (1,500 small notes and a few documents), Downloads, Music,
  Pictures (40 photographs and one added later), a Videos file, hidden settings
  (`.bashrc`, `.config`, `.ssh`, `.local/share`), and the old laptop's own
  Backtrack settings, which a recovery must never restore.
- **Its backups**, `/var/tmp/backtrack-drill/old-laptop-backups`, encrypted
  with the passphrase `drill passphrase`: two archives named as Backtrack names
  them, taken yesterday at 08:00 and 22:00 by the drill computer's clock, with
  the paths a real laptop would have (`home/keith/...`).
- **The new computer's home**, `/var/tmp/backtrack-drill/new/home/keith`. It is
  not empty: it has a `.bashrc` of its own, which differs from the old one and
  is what the summary at the end asks about, and `Desktop/welcome.txt`, which
  the old laptop never had and which nothing may touch.

## Walkthrough

1. `scripts/dr-drill setup 12`, then `scripts/dr-drill daemon`, then
   `scripts/dr-drill app`. The Welcome page opens: the new computer has nothing
   set up.
2. **Import.** Choose *Already have backups? Import…*, then *Choose Folder…*,
   and pick `/var/tmp/backtrack-drill/old-laptop-backups`. Enter
   `drill passphrase` and press *Open Backups*.
3. **The offer.** The page reads "Welcome back. Restore this computer?", with
   "Backups found for keith@oldlaptop — Latest: Yesterday, 22:00", the size,
   and "2 snapshots". *Restore everything* is chosen and marked Recommended.
   The dropdown lists *Yesterday, 22:00* and *Yesterday, 08:00*.
4. **Start Restore.** The wizard closes and the progress window opens:
   "Restoring your files…", the bar, a line like "41% · 5 of 12 GB · less than
   a minute left", the file being restored, and the folders in the order they
   run, smallest first: Desktop, Documents, Downloads, Music, Videos, Pictures,
   Settings and other files. A tick marks each folder that is back.
5. **Pause, then Resume.** The bar holds still and the line says "paused".
   The file Borg was writing when Pause was pressed is finished first, so the
   bar does not go backwards. Resume carries on from the same place.
6. **Close the window and reopen it.** The restore carries on without it.
   `scripts/dr-drill app` comes back to the progress window, not to the
   timeline.
7. **Kill the daemon.** `scripts/dr-drill kill`. systemd restarts it after five
   seconds, and it carries on: whole files already fetched are kept, and the
   daemon's log says "carrying on from where the fetch stopped" with how many
   were kept and how many are still to fetch.
8. **The end.** A notification, "Your files are back". The window says how many
   files were restored and that one file already on this computer is different
   in the backup. *Choose Which to Keep…* opens the folder-restore summary for
   that one file; either answer is fine.
9. **The first backup.** Within a minute of the restore finishing, the new
   computer backs itself up, into the same repository, as what the old laptop
   backed up: its home folder.
10. **Check.** `scripts/dr-drill check` compares the new home with the old
    laptop's, file by file (contents and modification times), and prints the
    archives and the timings from the daemon's log. Expected: nothing missing;
    only on the new computer, `Desktop/welcome.txt` and the folders Borg and
    GVFS keep their own state in; different, `.bashrc` (unless the summary was
    answered with Replace). The archives list gains one from this computer.
11. `scripts/dr-drill again` starts a fresh new computer against the same
    backups, for the variations below. `scripts/dr-drill reset` puts the
    development daemon back and deletes the drill.

### Variations

- **Restore selected folders…** opens a checklist of the folders, each with its
  size, all ticked, from the newest backup; its own *Folders as of* dropdown
  picks another. (The dropdown on the first page belongs to *Restore
  everything* alone, and greys out when another choice is made.) Untick
  Videos and pick *Yesterday, 08:00*: the restore leaves Videos out, and
  `check` reports it missing, `Pictures/2025/holiday/IMG_2100.jpg` missing
  (it was only in the 22:00 backup), and `Desktop/todo.txt` different.
- **Just browse — restore things later** opens the timeline over the old
  laptop's backups and restores nothing. Nothing is backed up either, because
  nothing has been chosen to back up.
- **Cancel** during the restore asks whether to keep what is restored so far or
  discard it. Keep leaves the finished folders in place; Discard takes away what
  the restore added and nobody has changed since. Either way `.bashrc` and
  `Desktop/welcome.txt` are untouched, and the restore is forgotten.

## Timings

### Through the window

Keith's drill on 2026-10-05, 12 GB, following the walkthrough above on the
development machine (a debug build, a local repository on an SSD):

| Step | Time |
|---|---|
| Import, until the restore was offered | 1 s |
| Restore, start to finish | 56 s, including a 19 s pause and a killed daemon |
| Pause, pressed during the 3 GB video | took effect 10 s later, once the video was whole; it was kept |
| Window closed and reopened during the restore | reattached |
| Daemon killed during Pictures | back 5 s later; kept 16 photographs, fetched 28 |
| First backup after the restore | started at once, 27 s |

The drill found one thing: *Choose Which to Keep…* waited 24 s for its
dialog, queued behind that first backup although it never touches the
repository. Fixed the same day; see the decision log.

### Driven over D-Bus

The same drill without the window, so that each step could be timed to the
second:

| Step | 3 GB | 12 GB, killed at 50% | 12 GB, paused at 56% |
|---|---|---|---|
| Building the fixture | 20 s | 75 s | (reused) |
| Import, until the newest backup is browsable | 1.2 s | 1.2 s | 1.2 s |
| Restore, start to finish | 11 s | 41 s, including 5 s of restart | 47 s, including 8 s paused |
| Resumed after the interruption | | kept 7 photographs, fetched 37 | kept the photograph being written |
| First backup after the restore | started at once, 8 s | started at once, 23 s | started at once |

The restore ran at about 300 MB/s. The estimate in the log tracked it: 22
seconds left at 5.4 GB of 12, with about 23 to go.
