# How Backtrack reads the Trash

The developer's half of [Get back a file you deleted between two
backups](trash.md): where Backtrack finds the Trash, what makes a timeline
entry, and when it changes.

## Where it reads

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

## What makes an entry

**An entry in the timeline.** Each gap between two backups (and the time since
the newest) in which something went to the Trash gets one entry, timed at the
last thing deleted in it. It sits among the backups in the sidebar, the strip
and the calendar, with the badge **trashed**.

**The folder as it was.** Opening the entry shows the folder as the backup
before it had it, with everything moved to the Trash since put back in its
place and badged **trashed**. If you had A and B at 12:00, wrote C, and
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
