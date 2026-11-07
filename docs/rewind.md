# Rewind

Rewind is Backtrack's full-screen way back through a folder's history: the
folder as it is now in front, its backups receding behind it into the
distance, and a timeline down the right-hand edge. Going back in time flies
the stack towards you. It is for the moment you want something back and want
to see the folder as it was, rather than read a list of dates.

Backtrack's window stays the everyday tool. Rewind shows the same backups and
restores through the same steps; only the way of looking differs.

## Opening Rewind

| From | How | Where it opens |
|---|---|---|
| Backtrack's window | The Rewind button in the header bar, **Rewind…** in the main menu, or Ctrl+T | The folder, backup and file the window is showing |
| GNOME Files | Right-click a file: **Rewind This File…**. Right-click a folder, or the empty space of the folder being shown: **Rewind This Folder…** | That file's folder, with the file selected; or that folder |
| Dolphin | The same two items, in the **Backtrack** submenu | As in GNOME Files |
| The application launcher | Right-click Backtrack in GNOME's dock or Plasma's task manager: **Rewind** | Where Backtrack's window would open: your home folder, if that is what you back up |
| The tray icon | **Rewind…** in its menu, on desktops with a tray | As from the launcher |

From the file manager, the launcher and the tray, Rewind starts at the folder
as it is now, and leaving it takes you back to where you were. Backtrack's
window, if it is open, is left as it was.

The file managers' items need Backtrack's integration for them; see
[File-manager integration](file-manager-integration.md). In Dolphin 26.04 and
later you can give Rewind's items keyboard shortcuts of your own, in
Dolphin's **Configure Keyboard Shortcuts**.

## Moving through time

- **Up** and **Down** (or Page Up and Page Down, or the mouse wheel) step one
  backup older or newer. Held down, the stack glides.
- **Home** goes to the folder as it is now, **End** to the oldest backup.
- **The timeline** down the right-hand edge has a mark for each backup: the
  oldest at the top, now at the bottom, labelled by day and by month. Point at
  it to magnify the marks near the pointer, rest on one to see when it was,
  and click one to fly there. The two round arrows beside it step older and
  newer.

Only the backups in which the folder changed are listed, as in the window. A
backup kept only on this computer, a snapshot, and things in the Trash are
marked as the window's sidebar marks them.

## The window in front

The folder at the backup in front, as icons or, with the buttons in its title
bar, as the window's list. Pictures show as pictures: the desktop's own
thumbnails where it has them, and, for a picture that has changed since,
one made from the backup's copy. **changed** and **deleted** say how each
file differs from the one on your computer now.

Open a folder by double-clicking it, or selecting it and pressing Enter. Go
up with Alt+Up, or through the path at the top left. Left and Right move
among the icons. Ctrl+H shows or hides hidden files.

## Restoring

**Restore**, under the stack, puts back the selected file or folder as it was
in the backup in front; with nothing selected, the whole folder. Enter does
the same, except on a folder, which Enter opens. If something is in the way
on your computer, Backtrack asks what to do, as it does in the window, and
nothing changes until you answer.

Once a restore is done, Rewind closes, and Backtrack offers to undo it, in a
bar under the window's header bar that stays until you use it or restore
something else. When Rewind was opened from elsewhere, the offer comes as a
notification: its **Undo** button undoes the restore, and clicking the
notification itself opens Backtrack with the same bar.

**Cancel**, or Escape, leaves without changing anything.

## Behind the stack

Rewind shows your desktop's wallpaper behind the stack, dimmed. Where it
cannot find it, or cannot read it, it shows a field of stars instead, which
streams past as you move through time.
