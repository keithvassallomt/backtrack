# Stage 16 — The Trash in the Timeline

## Objective
While something deleted from a backed-up folder is still in the Trash, the
timeline shows it where it was, at the time it was deleted, and restores it.

Two kinds of file are otherwise missing from the history:
- **A file that lived between two backups.** Created at 12:15 and moved to the
  Trash at 12:45, it was on disk for neither the 12:00 backup nor the 13:00 one.
  Time Machine has the same gap (Apple: hourly backups and hourly local
  snapshots, nothing in between).
- **A file thinned out of the history.** Hourly backups are kept for a day
  (`keep_hourly: 24`), and after that Borg keeps the last backup of each day. A
  file that lived from 12:15 to 14:45 is in the 13:00 and 14:00 backups, and in
  none once those are pruned.

Both are often still in the Trash, which Backtrack excludes from its backups
(`**/.local/share/Trash`) and has not looked at until now.

## The rule
**What is read.** The freedesktop.org Trash, which GNOME and KDE both
implement: the home Trash, and the per-drive Trash (`$topdir/.Trash/$uid` and
`$topdir/.Trash-$uid`) of every drive holding a backed-up folder. Each item is
`files/<name>` with `info/<name>.trashinfo` beside it, which records the
original `Path=` (percent-encoded; absolute in the home Trash, relative to the
drive's top in a per-drive one) and `DeletionDate=` (local time, no zone).
Inside the Flatpak, `$XDG_DATA_HOME` is the sandbox's own; the home Trash is
the host's, found through `HOST_XDG_DATA_HOME` and otherwise at
`~/.local/share/Trash`. The manifest's `--filesystem=host` already reaches it.

**Which items count.** An item counts if its original path is in a backed-up
folder, is not excluded, and it holds something no backup has: a file whose
size or modification time differ from the version at its original path in the
last backup before its deletion, or which that backup does not have; a folder
with such a file inside it. The backups are those that exist now, so a file
pruned out of the history comes back into it from the Trash.

**Where entries go.** One entry for each gap between consecutive backups, and
for the gap after the newest one, that holds a counting item. It is placed at
the latest deletion in that gap, and with Stage 15's listing, it is listed for
a folder when a counting item was at or below that folder, timed at the latest
deletion there.

**What an entry shows.** The folder as it was in the backup before the gap,
with every item moved to the Trash during the gap put back in its place and
marked "in the Trash". A trashed item takes the place of the backup's entry at
the same path. Inside a trashed folder, everything is the Trash's copy, which
is exactly what the folder held when it was deleted. Before the first backup,
the trashed items stand alone.

Decided with Keith, 2026-10-20: the entry shows the whole folder, not only the
trashed items. With A and B backed up at 12:00 and C created and trashed by
12:45, the folder at 12:44 held A, B and C; showing C alone would look as
though A and B had been deleted. A and B show their 12:00 versions, as the
nearest record there is, and each row carries its own modification time.

**What it is not.** An entry is not a backup. It is on this computer only, and
it goes when the Trash is emptied, so its badge and tooltip say so, as the
"local backup only" badge does (S10-T7).

**Accepted limits.**
- Nothing deleted with `rm`, with Shift+Delete, or from an emptied Trash can
  be shown.
- If the same path is moved to the Trash twice within one gap, the entry shows
  the later copy. The file manager's Trash still holds both.
- A per-drive Trash on a drive with no backed-up folder is not read.

## Prerequisites
Stage 15 (the listed backups for a folder, which the Trash's entries join),
Stage 7 (the restore pipeline), Stage 8 (search).

## Order of work
T1 and T2 first: everything else reads what they provide. T3, then T4 and T5
in either order.

## Tasks

### S16-T1 — Read the Trash
A `trash` module in `backtrack-core` reads both kinds of Trash by the rule in
this stage file: it decodes `Path=`, reads `DeletionDate=` as local time,
resolves per-drive paths against the drive's top, finds the host's home Trash
inside the Flatpak, and keeps only the items in backed-up folders that the
exclusion patterns do not exclude. A second function measures each item
against the index and says whether it counts. A malformed `.trashinfo`, or one
without its file, is skipped with a debug log, never treated as an error.

**Accept:** unit tests on fixture Trash folders: percent-encoded names;
absolute and per-drive relative paths; `DeletionDate` read in a zone other
than UTC; items outside the backed-up folders, and excluded ones, left out;
malformed or orphaned entries skipped; the counting rule: identical to the last
backup before the deletion does not count, changed or absent does, a folder
counts through a changed file inside it, and an item stops matching a pruned
backup and counts.

### S16-T2 — The daemon watches the Trash and serves it
The daemon keeps the Trash's items current with an inotify watch on each
Trash's `info` folder: a handful of folders, well inside any watch limit.
- New on `tools.backtrack.Backtrack.Daemon1`: a method listing the items with
  their original path, deletion time, kind, size, modification time and
  whether they count, and a signal when that list changes. The published
  interface test is updated with them.
- `PreviewFile` and the compare view's reads accept a trashed item, and hand
  back its bytes from the Trash.

**Accept:** a daemon test trashes a file the way the specification lays out,
sees the signal within a second and the item listed with the right original
path and time, empties the Trash and sees it gone; the preview of a trashed
file returns its bytes; inside the Flatpak on the GNOME VM, a file moved to the
Trash from Nautilus is listed.

### S16-T3 — Trash entries in the timeline
The sidebar, the stepping, the density strip and the calendar show the Trash's
entries among the backups, placed and listed by the rule in this stage file,
and the file pane, the preview and the compare view show an entry's folder as
the rule describes.
- The entry's row and each trashed item carry an "in the Trash" badge, with a
  tooltip saying that it is not a backup, is on this computer only, and goes
  when the Trash is emptied.
- Entries arrive and leave with S16-T2's signal, without reopening the window.

**Accept:** unit tests on placing items into gaps, on the per-folder time, and
on the folder an entry shows, including inside a trashed folder and before the
first backup; on the GNOME and Plasma VMs, with A and B backed up, C created
and moved to the Trash: an entry at C's deletion time shows A, B and C, with C
badged and previewable; emptying the Trash removes the entry while the window
is open.

### S16-T4 — Restore from the Trash
Restoring a trashed item takes it out of the Trash, as the file manager's own
Restore does, and Undo puts it back in.

The restore pipeline (stage, compare, rename; S07-T1) gains the Trash as a
source. The Trash is already on the same drive as the item's original place
(that is what a per-drive Trash is for), so the item needs no staging copy: it
is renamed from the Trash into place, and its `.trashinfo` removed. Before
that, it goes through the same steps as a backup's file: skipped when
identical, offered through the conflict dialog when something else is in its
place, and the file it replaces stashed. Restore To (S07-T7) moves it as well,
copying first when the destination is on another drive. A folder restored from
a Trash entry restores the backup's files and the Trash's items in one summary.

The move log (`restore::execute`) records a move out of the Trash as a kind of
its own, carrying the item's name in the Trash and its `.trashinfo` as it was.
Undo, walking the log backwards, renames the file back into the Trash and
writes that `.trashinfo` again, original deletion time included, so the
timeline's entry returns where it was. If another item has taken the name in
the meantime, the file goes back under a free name, as the specification
allows. Undoing a restore never deletes a file that came from the Trash.

Decided with Keith, 2026-10-20: a restore moves the item out of the Trash
rather than copying it, and Undo returns it there.

**Accept:** core tests: a trashed file restored by rename with its
`.trashinfo` gone; a conflict with a file now at its path, replaced and kept
both; Undo putting the file back in the Trash with its original `.trashinfo`,
including when the name has been taken; Restore To across drives; a mixed
folder restore undone in one step. On the GNOME and Plasma VMs, restoring C
from its entry puts C back in the folder and takes it out of the file manager's
Trash; Undo returns it to the Trash with its original deletion time, and the
timeline's entry comes back.

### S16-T5 — Search finds what is in the Trash
`SearchFiles` includes the Trash's counting items. A result says that the file
is in the Trash and when it was moved there; View in Timeline goes to its
entry, and Restore Latest restores the Trash's copy when it is newer than
every backup's.

**Accept:** unit tests on the result's wording and on which copy Restore
Latest picks; on the GNOME VM, searching for C finds it, and both actions work.

### S16-T6 — The daemon cannot deadlock on its own locks
Found during S16-T5's check on the GNOME VM, 2026-10-23: the daemon stopped
answering. Every worker thread was waiting on a lock. `schedule_input` took
the persisted state's lock and then the pause's; `health_inputs` took the
pause's and then the persisted state's. Each read its values inside one
struct literal, where a lock guard made for one field lives until the whole
literal is built, so the two held one lock each while waiting for the other.
A job ending sets both off at once: the window asks for the status, which
reads the schedule, while the daemon reassesses its health. `status_at` holds
`last_backup` the same way while it takes `destination_reachable`, which
`health_inputs` takes in the other order. The Trash's watcher, woken on the
deadlocked thread, never ran again, which is why the Trash's listing went
stale first.
- No lock is held while another is taken: each value is read under its own
  lock in a statement of its own.
- The same is checked across the daemon, not only in these three.

**Accept:** a test that reads the schedule and the health on two threads at
once, many times over, finishes (as written before the fix, it deadlocks);
the audit's findings are in the commit; on the GNOME VM, the S16-T5 check
runs through without the daemon stopping.

### S16-T7 — Two previews of the same file at once both succeed
Found with S16-T6: selecting a search result asks for its preview twice in a
moment, and the daemon extracted both into the same `.partial` file. The
first renamed it into place; the second's rename found nothing, and failed
with "No such file or directory", reported as the local disk being full.
- Each extraction has a temporary file of its own.
- A failure on this computer's disk is reported as a full disk only when the
  disk is full.

**Accept:** a test that previews the same file twice at once gets the file's
contents both times; a test that an error other than a full disk is not
reported as one.

### S16-T8 — Search can be closed from its button
Keith, 2026-10-23: there is no button to leave search. The header's search
button already toggles it, but looks the same while searching, so nothing
says it closes it. It becomes a toggle button, pressed while search is open,
as in GNOME's own applications.

**Accept:** on the GNOME VM, the button shows pressed while searching, and
clicking it leaves search; Escape and Ctrl+F still do.

## Definition of Done
All tasks done; the CHANGELOG's Unreleased section says what changed for the
user; the user documentation says what the Trash can and cannot bring back,
including the accepted limits above.
