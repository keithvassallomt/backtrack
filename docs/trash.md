# Getting things back from the Trash

Backtrack backs up on a schedule, every hour unless you have chosen otherwise.
Anything that exists only between two backups is in neither of them: a file
you write at 12:15 and delete at 12:45 was not there at 12:00, and was gone
again by 13:00. Older backups are also thinned out as they age (hourly ones are
kept for a day, then one a day for a week, and fewer after that), so a file
that lived from 12:15 to 14:45 is in the 13:00 and 14:00 backups only until
those two are pruned.

Deleting from a file manager usually means moving to the Trash, and the Trash
keeps the file. So while it is still there, Backtrack shows it in the timeline
as well, at the time you deleted it.

## What you see

**An entry in the timeline.** Each gap between two backups (and the time since
the newest) in which something went to the Trash gets one entry, timed at the
last thing deleted in it. It sits among the backups in the sidebar, the strip
and the calendar, with the badge **in the Trash**.

**The folder as it was.** Opening the entry shows the folder as the backup
before it had it, with everything moved to the Trash since put back in its
place and badged **in the Trash**. If you had A and B at 12:00, wrote C, and
moved C to the Trash at 12:45, the entry shows A, B and C, because that is
what the folder held at 12:44. A and B are shown as the 12:00 backup has them,
which is the nearest record there is; each row's own modification time says
how old it is.

**Only what is worth showing.** Something counts when no backup has it as it
was: a file no backup has, or one that changed after the last backup before
you deleted it, or a folder with such a file inside it. A file moved to the
Trash exactly as a backup has it makes no entry, because the backup already
gives it back.

**A preview.** Selecting something from the Trash previews the Trash's copy.
Inside a folder that went to the Trash whole, everything you see is the
Trash's copy.

An entry is not a backup. It is on this computer only, nothing of it is on
your backup destination, and it is gone as soon as the Trash is emptied. The
badge's tooltip says so.

## Restoring

**Restore** takes it out of the Trash and puts it back where it was, as the
file manager's own Restore would. If something else now has its name, you are
asked, as for any restore: replace what is there (kept for 30 days in
Recently Replaced Files), keep both, or leave it.

**Undo**, on the message that follows, moves it back into the Trash, with the
time you first deleted it, so its entry comes back where it was. If something
else has gone into the Trash under the same name in the meantime, it goes back
beside it as `name.2.ext`, the way the desktop names a second item of the same
name. Undo never deletes anything that came from the Trash.

**A whole folder** restored from an entry brings back both halves in one
summary: what the backup before the entry has, and what went to the Trash.

**Restore To** moves it into the folder you choose, copying first when that
folder is on another drive. It, too, leaves the Trash.

If you leave part of something in the Trash (a file you chose not to replace,
or one that was already the same on your computer), that part stays in the
Trash, and its entry stays for as long as what is left holds something no
backup has.

None of this needs your backup destination: what is in the Trash is already on
this computer, so it restores with the destination unplugged.

## Searching

Search finds what is in the Trash by name, with the same rules as everything
else it searches. A result in the Trash says so, and when it was moved there:
"Moved to the Trash 23 Oct at 12:45 · in no backup" for something no backup
ever had, or "Existed: 26 Jun – 23 Oct · moved to the Trash at 12:45" for a
file the backups have older versions of.

**View in Timeline** goes to its Trash entry, and **Restore Latest Version**
restores the Trash's copy, whenever that copy is the newest one. If the file
was made again and backed up after an older copy went to the Trash, the
backup is newer: the result says an older copy is in the Trash, and Restore
Latest Version restores the backup's.

## What the Trash cannot bring back

- **Anything not in the Trash.** A file deleted with `rm`, with Shift+Delete,
  or by emptying the Trash never reaches it, or no longer is in it, and
  Backtrack has nothing to show.
- **The earlier of two copies trashed from one place.** If the same file is
  moved to the Trash twice between two backups, the entry shows the later
  copy. Both are still in the Trash, and your file manager shows them.
- **Another drive's Trash.** Each drive keeps its own Trash for things deleted
  from it. Backtrack reads that Trash only on a drive holding one of your
  backed-up folders.
- **Excluded files.** Something from a place you excluded from your backups is
  not shown, just as it is not backed up.
- **Files inside a trashed folder, by searching.** Search finds a folder in
  the Trash by its own name. A file inside it that no backup has is not found
  by its name: search for the folder, or open the folder's entry and look
  inside.

## How it works

Backtrack reads the Trash the way GNOME and KDE both keep it (the
freedesktop.org Trash specification): your own Trash in
`~/.local/share/Trash`, and the per-drive Trash folders (`.Trash/<your user
id>` and `.Trash-<your user id>` at the top of a drive) of the drives holding
your backed-up folders. In each, `files` holds what was deleted, and `info`
holds a note for each item saying where it was and when it was deleted.
Inside the Flatpak, it reads the same folders.

The background service watches those folders, so an entry appears, changes or
goes within a second or so of the Trash changing, and again whenever a backup
or a prune changes what counts. It reads the Trash and never writes to it,
except to move something out of it when you restore it and back in when you
undo that. The Trash itself is never backed up (`**/.local/share/Trash` is one
of the default exclusions), so what Backtrack shows from it lasts exactly as
long as the Trash keeps it.
