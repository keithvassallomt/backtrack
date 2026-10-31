# Stage 18 — The Full-Screen Time View

## Objective
A full-screen way through the history, in the spirit of Time Machine's: the
folder as it is now in front, its earlier backups receding behind it into the
distance, a timeline down the side, and Restore and Cancel below. Moving back
in time flies the stack towards the viewer. It opens from Backtrack's window,
from the file manager's right-click menu, from the launcher and from the tray.

The main window stays the everyday tool; this is a second way in, made for
the moment someone wants a thing back and wants it to feel like going back
for it. "Time Machine" is Apple's trademark and appears nowhere in the
interface. The view is called **Rewind** (Keith, 2026-10-20): it says what
the view does, and reads naturally in a menu ("Rewind This Folder…"). Warp was
considered and set aside, as the name of a GNOME Circle application and of a
well-known terminal. These files also call it the full-screen view.

What makes it practical here: every folder at every backup is listed from the
local catalogue, without Borg, so flying through fifty backups costs fifty
catalogue queries, not fifty extractions.

## Prerequisites
Stage 15 (the backups listed for a folder: the stack and the timeline show
those, not every backup); S14-T2 (application actions, which the file managers
and the Flatpak open the view through). Stages 16 and 17 are not required: when
they are in, the Trash's entries and Snapper's snapshots appear in the view as
they do in the main window.

## Order of work
T1 first, since everything visible follows the chosen design. T2 does not
depend on it and can run beside it. Then T3 to T8 in order. T9, which came out
of T1, changes only the main window and stands on its own. T10 to T13 came out
of Keith's check of T3 on his laptop (2026-10-31) and go before T4.

## Tasks

### S18-T1 — Mockups of the full-screen view, to choose a design
Four design directions for the view, made to be iterated on with Keith before
anything is built. They differ in:
- **The stack:** straight back into the distance, as Time Machine's does; fanned
  to one side; a cascade of pages; and one further direction of its own.
- **The background:** the desktop's own wallpaper, dimmed (S18-T2); Backtrack's
  own (a starfield, after Time Machine's original); a plain gradient.
- **The timeline:** ticks down the right-hand edge, or a strip along the
  bottom like the main window's.
- **The front window:** a grid of icons and thumbnails, or the main window's
  list.

Each direction is an HTML prototype Keith opens in a browser, animated with
the arrow keys, so the motion is judged and not imagined; light and dark. After
each round, the chosen direction is refined until Keith settles it.

**Accept:** Keith has chosen a direction; stills of it, light and
dark, are committed as mockups 25 and 26 in `backtrack_plan/mockups/`, and the
later tasks here are updated to cite them.

**Chosen** (Keith, 2026-10-27, over two rounds): the stack straight back into
the distance; the desktop's wallpaper, dimmed, with a starfield where it
cannot be read; the timeline down the right-hand edge, magnifying near the
pointer; the front window as icons, switchable to a list. The prototype the
choice was made in is `backtrack_plan/mockups/rewind/rewind.html`, and the
later tasks take their measures from it (the depth of the stack, the curve and
duration of a step, the magnification).

### S18-T2 — The desktop's wallpaper as the view's background
Researched on the GNOME and Plasma VMs (Fedora 45, Flatpak 1.18.4,
2026-10-20); what follows is what was found to work.

**GNOME.** The wallpaper is `org.gnome.desktop.background` `picture-uri`, or
`picture-uri-dark` when the colour scheme is dark, with `picture-options` for
the fit.
- Native: read through GSettings.
- Flatpak: GSettings inside the sandbox reads the sandbox's own settings and
  returns the runtime's default (`adwaita-l.jxl`), not the user's. The Settings
  portal does not expose `org.gnome.desktop.background`, and the GNOME 50
  runtime has no dconf GIO module, so the old dconf permissions recipe does
  nothing either. What works with the permissions the manifest already has
  (`--filesystem=host`): reading dconf's own layers in dconf's order. First
  the user's database, `~/.config/dconf/user` (a GVDB file); then the system
  databases the host's `/etc/dconf/profile/user` names, under
  `/run/host/etc/dconf/db/`; then the key's default in the host's compiled
  schemas, `/run/host/usr/share/glib-2.0/schemas/gschemas.compiled`, through
  `gio::SettingsSchemaSource::from_directory`. On the VM the wallpaper came
  from the last of these: Fedora sets it in a vendor schema override, and the
  user's database does not hold the key. Whether `/run/host/etc` carries
  `dconf/db` with `host` alone is checked here.
- A timed slideshow (`.xml`) resolves to the picture for the current time.

**Plasma.** `org.kde.PlasmaShell.wallpaper(screen)` on the session bus returns
the screen's wallpaper plugin and its settings, and answers inside the
Flatpak once the manifest gains `--talk-name=org.kde.plasmashell`.
- `org.kde.image`: `Image` names a file, or a wallpaper package (a folder with
  `contents/images/<W>x<H>.<ext>` and `contents/images_dark/`), resolved to the
  image closest to the screen's size, the dark one when the colour scheme is
  dark. An empty `Image` means the default, which the active look-and-feel
  package names (`[Wallpaper] Image=` in its `contents/defaults`; Fedora's is
  `Fedora`). The resolution is checked against what the desktop shows.
- `org.kde.color`: that colour. A slideshow or any other plugin: the fallback.

**Both.** Inside the Flatpak, the host's `/usr` is at `/run/host/usr`, which
`--filesystem=host` already reaches; a path under `/usr` is read from there.
Fedora's wallpapers are JPEG XL: they are decoded with glycin, whose JXL loader
the GNOME runtime ships (native packages depend on the distribution's glycin
loaders), off the main thread, scaled once to the screen and cached by path
and modification time. Any other desktop (Hyprland, Sway and the rest), and any
failure along the way, gets Backtrack's own background: the starfield of the
S18-T1 prototype, which is always dark, whatever the style (only the windows
follow the style). Over the wallpaper, the view darkens it as mockups 25 and
26 show.

**Found while building it** (2026-10-27):
- GTK decodes through glycin itself: `gdk::Texture` read Fedora 45's 5120 by
  5120 JPEG XL natively and inside the Flatpak (runtime 51) on the GNOME VM,
  so Backtrack takes no glycin dependency of its own. glycin's loaders run in
  a sandbox the Flatpak portal makes only for a real application: in a bare
  `flatpak run` of the SDK the portal refuses, and JPEG XL fails there alone.
- `/run/host/etc/dconf/db` is there with `--filesystem=host` alone. Fedora's
  profile is `user`, then `local`, `site` and `distro`; `HOST_XDG_CONFIG_HOME`
  is unset in the Flatpak, so the user's database is `~/.config/dconf/user`.
- Plasma's default wallpaper follows `kdeglobals` as KConfig reads it:
  `~/.config`, then `~/.config/kdedefaults`, `/etc/xdg` and Fedora's
  `/usr/share/kde-settings/kde-profile/default/xdg`, which names Fedora's
  look-and-feel. The Plasma VM's own `kdeglobals` names Breeze Dark, and its
  desktop shows the Next package's dark picture, as resolved here (compared
  with a screenshot: 0.25% different, against 32% for the light picture).
  `Color` comes as a struct of one ARGB number. The first screen's wallpaper
  is the one used: Plasma numbers its screens its own way.
- GTK reports a scale of 3.56 for a rotated 5K monitor on Hyprland at scale 2,
  the panel's width over the rotated width, so the screen's size is taken at
  a scale of two at most.
- `backtrack-gtk --wallpaper-report` (hidden from `--help`) finds and draws
  the wallpaper as Rewind will, and says where each setting came from.

**Accept:** unit tests on fixtures: a dconf user database with and without the
key, a system database, a vendor default from compiled schemas, the light and
dark choice, an XML slideshow, and a Plasma package resolved by size and by
dark. On the GNOME and Plasma VMs, native and Flatpak, the report finds and
draws the desktop's own wallpaper, and after it is changed in the desktop's
settings it finds the new one. On Keith's laptop (Hyprland), the report falls
back to the starfield. (The view itself arrives in S18-T3, whose check sees
the wallpaper in it.)

### S18-T3 — The full-screen view and its stack
A full-screen window on the monitor it was opened from, drawing the stack
straight back into the distance, as mockups 25 and 26 show: each backup behind
is further back and a little higher, darkened with its distance, and up to
nine are drawn.
- The backup in front is a live widget (S18-T4); the ones behind are drawn
  from their folders' drawings, made once and kept, so only one real file view
  exists at a time. GTK's GPU renderer and its 3D transforms carry the depth and
  perspective.
- Moving to another backup flies the stack, never landing between two
  backups. From rest it eases in and out (`ADW_EASE_IN_OUT_CUBIC`): slow, fast,
  then slow, over 800 ms for one backup and somewhat longer for a longer
  flight, as in the prototype. A move begun while the stack is still moving
  keeps the speed it had and eases out, and a held key runs at most two
  backups ahead of the stack, so holding a key glides rather than stopping at
  each backup. Over the starfield, the stars stream with the motion.
- Keys: Up and Down (and Page Up and Page Down) step, Home goes to now and End
  to the oldest, Escape cancels, Enter restores.
- With animations switched off in the desktop's settings, a crossfade instead
  of the flight.
- When frames run late, fewer backups are drawn behind, rather than the
  motion stuttering. A virtual machine rendering in software is not the
  measure: Keith judges the motion on real hardware.

**Built** (2026-10-27): Rewind opens with Ctrl+T in Backtrack's window, for
now (S18-T7 adds the button, the menu item and the other ways in). Each
backup's folder is a real widget, laid out once and its drawing kept with its
shadow, two at a time, the front one first; the stack is those drawings. The
front card is one too until S18-T4 makes it a live file view. The folder as it
is now is read from the disk. GNOME turns GTK's animations off when it renders
in software, as on the GNOME VM, so Rewind crossfades there; on the Plasma VM
it flies. `scripts/rewind-check` is Keith's check on his laptop.

**Found on Keith's laptop** (2026-10-31), and fixed:
- The cards were first rendered into textures off-screen, and the first such
  render in a process held the main loop for about three seconds on the
  laptop's Intel GPU, whatever its size, so the stack opened as empty
  outlines. Each card's drawing is now kept as GTK's render nodes and drawn
  with the window's own frames, which pay no such cost.
- Hyprland makes a window full screen only after it is first shown, so Rewind
  first laid out its stack for a small window. It now takes the monitor's size
  from the start.
- On a desktop with no wallpaper to read, the starfield is there from the first
  frame instead of after the attempt to read one.
- Home and End flew past outlines: only the cards where the stack would land
  were made, and those on the screen were let go as it set off. The cards
  around the stack are now kept too, and those along the way are made as it
  flies, after those where it lands. A long flight through hundreds of backups
  can still pass a few cards before they are made.

**Accept:** on Keith's laptop, stepping through fifty backups of a real folder
looks smooth to Keith, over the starfield; on both VMs, native and Flatpak, the
view opens full screen over the desktop's own wallpaper (S18-T2), steps by
every key above, and Escape returns to where it was opened from; with
animations off, crossfades.

### S18-T4 — The full-screen view's front window
The folder at the backup in front, as a file view: listed from the catalogue as
the main window's file pane is, and sharing the window's state (folder,
selection), so leaving the view lands where it was.
- Icons by default, as mockups 25 and 26 show, switched to the main window's
  list by a pair of buttons in the front window's header bar. In the icon
  view every cell is the same height: the badge is pinned to the foot of the
  icon, and the name has room for two lines and never more.
- Walking into a folder and back up, as in the main window.
- Icons by file type. Real thumbnails where the backed-up version is the one
  on disk now (the desktop's thumbnail cache already has those), and fetched
  lazily, for the front window only, where it is not; never for the backups
  behind.
- The main window's badges, in the brief words S18-T9 gives them: "deleted"
  and "changed" on files, "trashed" and "Snapper" (Stages 16 and 17), each
  with the full meaning in its tooltip.

**Accept:** unit tests on which files get a thumbnail and from where; on a VM,
walking into a folder and back in the view, with thumbnails for images
unchanged since the backup and icons for the others, and leaving the view at
the same folder and selection.

### S18-T5 — The full-screen view's timeline
The listed backups (Stage 15) as a timeline down the right-hand edge, as
mockups 25 and 26 show: a tick for each backup, the oldest at the top and now
at the bottom, with its labels ("Today (Now)", "Yesterday", the months) and
the step arrows beside it.
- Near the pointer, the ticks widen and the labels grow, as a dock magnifies:
  fully at the pointer and falling off over a few ticks either side. The ticks
  keep their places, so what is clicked is what was drawn there.
- The step arrows (older and newer) change colour under the pointer, as a
  button does (Keith, 2026-10-27; the prototype has no hover colour).
- Clicking a backup flies the stack to it; hovering names it.
- Local snapshots, the Trash's entries and Snapper's snapshots are marked as
  the sidebar marks them.

**Accept:** unit tests on where labels go across a year of uneven backups; on a
VM, clicking and hovering, and the timeline following the stack as it steps.

### S18-T6 — Restore and Cancel in the full-screen view
- Restore restores the selection, or the folder when nothing is selected,
  through the restore pipeline, with the conflict dialog over the view.
- After a restore, the view returns to where it was opened from, and Undo is
  offered there: as a toast in Backtrack's window, or as a notification when
  the view was opened from elsewhere.
- Cancel, and Escape, return without changing anything.

**Accept:** on a VM, a restore from the view with a conflict, then Undo, once
opened from Backtrack's window and once from the file manager.

### S18-T7 — Ways into the full-screen view
- **Backtrack's window:** a header bar button, a primary menu item and a
  keyboard shortcut, opening at the folder and backup being viewed.
- **Nautilus:** a right-click item on a file (the view at its folder, the file
  selected) and on the folder background, beside the existing two.
- **Dolphin:** the same, in the service menu.
- **The launcher:** a desktop action in the `.desktop` file, which GNOME's dock
  and Plasma's task manager offer on right-click, opening at the home folder.
- **The tray menu**, on desktops with a tray.
- An application action, so the file managers and the Flatpak open the view
  through the session bus (as S14-T2 does for the window), and a command-line
  option that maps onto it.

Not possible, as of 2026-10-20: a toolbar button in Nautilus, whose extension
API has had no access to its window since Nautilus 43, or in Dolphin, whose
service menus appear only in context menus. A Dolphin keyboard shortcut for the
service menu item is added if Dolphin now supports shortcuts for service
menus: KDE's developers aimed for 26.04 at the earliest, and what shipped is
checked here.

**Accept:** each way in tried on both VMs, native and Flatpak, opening at the
right folder, with the right file selected where one was clicked.

### S18-T8 — The full-screen view's accessibility
- The stack and the timeline have accessible roles and names; stepping
  announces the backup now in front, as the main window's position label does.
- Everything the view does can be done from the keyboard alone.
- High contrast is honoured; the motion follows S18-T3's crossfade rule.

**Accept:** with Orca on the GNOME VM, stepping reads out each backup and the
timeline reads as a list of them; the whole of S18-T6's restore done with the
keyboard alone.

### S18-T9 — Brief badges in the main window
*(from S18-T1: Keith, 2026-10-27)* The badges in the main window say one word
or two, as Rewind's do, and leave the rest to their tooltips: "deleted"
(was "deleted after this") and "changed" (was "changed since then") in the
file pane; "Snapper" (was "Snapper snapshot") and "local" (was "local
snapshot") in the sidebar; "trashed" (was "in the Trash") in the sidebar, the
file pane and search.
"local backup only" and "not on your disk" keep their words. A changed folder's
tooltip says that it is something inside it. What a screen reader says for a
sidebar row keeps the full words, which are clearer spoken than the brief ones.
The guides and the CHANGELOG use the new words.

**Accept:** unit tests on each word, class and tooltip, and on which badge a
file pane row gets when more than one would hold; `just check` passes.

### S18-T10 — A backup held up by the battery or the network says so
*(found 2026-10-31 on Keith's laptop)* With backups on battery switched off
(the default) and the laptop on battery, every scheduled backup is skipped,
and nothing says why: the window and `backtrack status` say "Your files are
backed up" and "Next backup due now" while the last backup grows older (21
hours on Keith's laptop). The daemon knows the reason and keeps it only to log
it once. Meanwhile the scheduler logs "a scheduled backup was missed; catching
up shortly" at every retry: 118 times in four hours.
- A scheduled backup held up by the battery, or by a metered connection, says
  so wherever the next backup is shown (the window's status line, `backtrack
  status`, the tray), naming the setting in Preferences that would let it
  run. Once the hold clears, it says nothing of it.
- The scheduler logs a hold once, as the daemon logs the skip.

**Accept:** unit tests on what the status says while a backup is held up and
after the hold clears; a test that several ticks on battery log the hold once;
on Keith's laptop on battery, the window and `backtrack status` say why.

### S18-T11 — Hidden files are hidden, as file managers hide them
*(found 2026-10-31 on Keith's laptop)* Files and folders whose names begin
with a dot are shown everywhere: in the window's file pane, and in Rewind,
whose card for the folder as it is now lists every dotfile in a home folder.
- They are hidden by default in the file pane and in Rewind's cards. Show
  Hidden Files, in the primary menu, shows them: Ctrl+H, as in Nautilus and
  GTK's file chooser. Show All Backups moves to Ctrl+Shift+H (Keith,
  2026-10-31).
- Where it can be read, Backtrack starts from the desktop's own choice: GTK's
  file chooser setting `org.gtk.gtk4.Settings.FileChooser` `show-hidden`,
  which Nautilus shares, natively through GSettings and inside the Flatpak
  from dconf's files (S18-T2's reader). Ctrl+H changes it for Backtrack.

**Accept:** unit tests on which entries are hidden, and on the desktop's choice
read from fixtures (set, unset, and in the Flatpak's way); on the GNOME VM,
natively and in the Flatpak, a home folder's dotfiles hidden in the window and
in Rewind, Ctrl+H showing them, Ctrl+Shift+H toggling Show All Backups, and
`gsettings set org.gtk.gtk4.Settings.FileChooser show-hidden true` followed at
the next start.

### S18-T12 — The window and Rewind open quickly on a large catalogue
*(found 2026-10-31 on Keith's laptop)* Keith's catalogue holds 3.3 million
files (his whole home folder is backed up), in an 853 MB database with a
546 MB write-ahead log. Timed on a copy: opening it takes 4 s, because every
open runs SQLite's `quick_check`, which reads the whole file; which backups
differ for his home folder takes 2.8 s; and his home folder at one backup takes
165 ms, which Rewind asks once for each of its 31 cards (5 s). The window shows
nothing of his home folder for about 7 s. Documents, by contrast, answers in a
millisecond.
- The window does not check the whole catalogue each time it opens it; the
  daemon still does, when it opens it to write.
- Which backups differ for a folder, and a folder at a backup, are answered
  quickly however much lies below the folder.
- The write-ahead log is kept to a size once written back.

**Built** (2026-10-31): schema v5 keeps `ended`, for each folder and each
archive at which something below it ended, the earliest first archive among
what ended there; the writer rebuilds it in each transaction that changes the
versions (a third of a second on Keith's catalogue), and both questions read
it. The window's reader no longer runs `quick_check`. The log is limited to
64 MiB and written back after each change. On a raw copy of Keith's catalogue
and its 546 MB log: the daemon's first open, with the migration, 4.4 s; the
log then 0 MB; the window's first view of his home folder 1.2 ms (about 7 s
before); Rewind's 31 listings of it 5.7 ms (5.1 s before). The answers were
checked equal to the old queries' on that copy, for six folders at every
backup.

**Accept:** on a copy of Keith's catalogue, the window has his home folder's
backups and its listing within one second of opening the catalogue, and
Rewind all its cards' listings within one more; tests that bound each query's
cost on a large generated catalogue; the write-ahead log back under its limit
after a backup.

### S18-T13 — An upgrade restarts the running tray and service
*(found 2026-10-31 on Keith's laptop)* After installing a new version, the
tray icon already running kept the old one. It could not read the new
service's status (S18-T10 added to it), and said "The Backtrack service is not
running" until it was restarted by hand. The service likewise keeps running
the old version until it is restarted.
- The tray notices when its program is replaced on disk and starts the new one
  in its place.
- The service does the same once it is idle, never during a backup, a restore
  or a check.
- A tray that cannot read the running service's status says the two are of
  different versions, rather than that the service is not running.

**Accept:** on Keith's laptop, installing a newer AUR build while the tray and
the service run: within a minute, with no command run, the tray shows the new
service's status and `systemctl --user status backtrackd` shows the service
started after the install; with a backup running during the install, the
service restarts after the backup ends; the same with the Flatpak on the
Plasma VM, or a note in this section of what the Flatpak cannot do.

## Definition of Done
All tasks done; the CHANGELOG's Unreleased section introduces Rewind; mockups 25 and 26 match what was built; the README and the file
manager documentation (`docs/file-manager-integration.md`) mention the new
menu items.
