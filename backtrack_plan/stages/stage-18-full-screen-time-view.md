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
of T1, changes only the main window and stands on its own. T10 to T17 came out
of Keith's check of T3 on his laptop (2026-10-31) and go before T4; the last
three in the order Keith set: T16, T15, then T17. T18 to T20, which T15 led
to, follow T17; T21, which T17 led to, is built once T17's afternoon of
measurements is in, so that nothing installed meanwhile disturbs them.

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
- On a desktop whose wallpaper Backtrack does not read, the starfield is there
  from the first frame instead of after the attempt to read one.
- Home and End flew past outlines: only the cards where the stack would land
  were made, and those on the screen were let go as it set off. The cards
  around the stack are now kept too, and those along the way are made as it
  flies, after those where it lands. A long flight through hundreds of backups
  can still pass a few cards before they are made.
- Over Keith's wallpaper (S18-T18), only the two cards behind the front one
  showed, the second faint: frames had run late while he stepped back, so the
  stack drew three cards rather than nine, and it took them back only after two
  seconds of smooth motion, never at rest. His display refreshes at 120 Hz,
  where 60 frames a second counted as late. Frames are now measured against 60
  a second at most, and at rest the cards come back one every 120 ms.
- Then, after a step, some cards behind the front one showed as empty outlines,
  and some lost their top part. Two causes. Under GTK's 3D transforms each
  card's nodes went through an off-screen picture, some drawn partly or not at
  all; the cards face the eye, so they are now drawn scaled towards the
  vanishing point, with no 3D transform (`rewind::scale_at`). And the screen
  was left stale where GTK did not count it as changed (Hyprland, at a scale of
  1.6; a screenshot, or `GSK_DEBUG=full-redraw`, put it right): every Rewind
  frame now repaints the whole screen, its base colour a step different from
  the frame before, as the starfield's moving stars always made it do.
- With all of these in, Keith looked again on the VMs (`scripts/rewind-check`
  7 and 8): Rewind over GNOME's, Cinnamon's, Xfce's and Plasma's wallpaper,
  natively and in the Flatpak, the cards whole as it stepped and glided, and a
  changed wallpaper shown the next time it opened.

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

**Keys** (Keith, 2026-11-04): Up and Down, Page Up and Page Down, Home and
End keep S18-T3's meanings, stepping through time, whatever has the focus;
Left and Right move among the icons, Enter opens the folder selected, and
Alt+Up goes up a folder, as in the main window. In the list, a row is chosen
with the mouse or Tab. Enter on a file is S18-T6's.

**Built** (2026-11-04, in progress):
- `ui::rewind::folder` builds a window of Rewind's, the one in front and the
  cards behind alike: the header bar with the folder's trail, the backup's
  date and the pair of buttons, then the folder as icons (a grid view, each
  cell the same height, the badge on the icon's foot) or as the main
  window's list (its columns). The choice holds for as long as Backtrack
  runs. The selected file is shown selected on every card.
- At rest, the window in front is live; as the stack sets off, its drawing
  is kept for the flight, scrolled and selected as it was, and the window is
  let go. A change to the selection, the view or hidden files makes the cards
  again, each drawn as it was until it is.
- Each backup is listed as the main window's file pane lists it, through
  `ui::files::Lister`, taken out of the pane for both: the Trash's entries,
  folders only partly backed up, the "not on your disk" answers, hidden
  files. The folder as it is now is read from the disk, with sizes and dates.
- Rewind moves in a state of its own, forked from the window's
  (`AppState::fork`): walking into a folder lists its backups and lands on
  them as the window would, while the window stays where it was. However
  Rewind is closed, the window then takes its folder and its selection, the
  window's own version of the file selected, or none if its backup has no
  such file.
- Thumbnails (`model::thumbnail`): the desktop's own, from its cache, where
  the thumbnail's `Thumb::MTime` is the backed-up version's, for the window
  in front and the cards behind; for an image whose version the cache does
  not hold, one made from its copy in the backup (through the daemon, as the
  preview fetches it), or on the disk for the folder as it is now, in the
  window in front only, as each comes into sight.

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
  status`, the tray), and `backtrack status` names the setting that would let
  it run. Once the hold clears, it says nothing of it.
- The scheduler logs a hold once, as the daemon logs the skip.

**Built** (2026-10-31): the daemon's status carries the hold (`held_up`), said
only while a backup is due and none is running. Checked on Keith's laptop on
battery: the window, the tray and `backtrack status` said why, and a backup ran
within two minutes of plugging in, after which the hold was no longer
mentioned. Keith then shortened the words, which a tray menu cut off: "Last
backup 1 hour ago · waiting for charger" in the window and "Last: 1h ago.
Waiting for charger." in the tray; `backtrack status` still names
`backup.on_battery`.

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

**Built** (2026-10-31): `host::wait_until_replaced`, which the tray and the
service already ran to notice an uninstall, now also tells an upgrade: the
file at the program's path is another file than the one running (a package
manager writes the new program beside the old and renames it over, and the
old stays on disk while it runs). It looks every 30 seconds. The tray then
lets go of its bus name and starts the new program in its own place. The
service waits until no job is running or waiting, then exits with `UPGRADED`
(76), a failure, which the unit's `Restart=on-failure` starts again as the new
program: it cannot start itself afresh, as systemd ends a `Type=dbus` service
once it lets go of its name. A tray that cannot read the service's status
(`zbus::Error::Variant`) says "Restarting after an update…". Smoke-tested on a
private session bus with copies of both programs, replaced as pacman does: the
service left with 76 thirty seconds later, and the tray showed its icon again
from the new program in the same process.

Inside a Flatpak, `/app` does not change while the instance runs, so the
update is learnt from Flatpak (`backtrack_core::upgrade`, behind the core
crate's `upgrade` feature): the installation's `active` link, beside the
running commit's directory that `/.flatpak-info` names, read every 30 seconds,
and the Flatpak portal's update monitor (`CreateUpdateMonitor`), which looks
twice an hour, for any installation the link cannot be read in. Flatpak hides
its installations from a sandbox even with `--filesystem=host`, so the
manifest grants Backtrack's own, read-only, per user and system-wide. The
daemon then exits with `UPGRADED` once idle, and its supervisor asks the
portal to start the newest `backtrackd --supervise` in a sandbox of its own
(`Spawn` with `FLATPAK_SPAWN_FLAGS_LATEST_VERSION`, in a scope of its own, not
the portal's) and leaves. A sandbox does not end with its first process: the
tray, the daemon's child, ran on in the old one holding its bus name, so the
tray starts its own new version through the portal too.

On the Plasma VM (Flatpak 1.18.4), built here into a repository and installed
from it per user, then updated while running: within 22 seconds of `flatpak
update`, the tray and the daemon each handed over to the new commit, and only
it ran afterwards. With the link hidden (`--nofilesystem` override) and the
portal polling every 15 seconds, the portal's word alone did the same within
two seconds of its look.

On Keith's laptop (`scripts/rewind-check 6`), reinstalling the AUR package
while both ran: the service noticed 33 seconds later, exited with 76, and
systemd started the new one five seconds after; the tray came back by itself.
With a backup running during the install, the service waited for it to finish
before restarting.

**Accept:** on Keith's laptop, installing a newer AUR build while the tray and
the service run: within a minute, with no command run, the tray shows the new
service's status and `systemctl --user status backtrackd` shows the service
started after the install; with a backup running during the install, the
service restarts after the backup ends; the same with the Flatpak on the
Plasma VM, or a note in this section of what the Flatpak cannot do.

### S18-T14 — On battery, a backup that has waited long enough runs anyway
*(found 2026-10-31 on Keith's laptop)* With backups on battery off (the
default), a laptop that stays on battery goes without a backup for as long as
it does: 21 hours on Keith's laptop. Backing up hourly on battery is not free
either: each backup there checks all 2.9 million files of the included
folders, about five minutes of Borg, then catalogues the archive and prunes.
- A new setting, `backup.on_battery_after` (hours, default 3): with backups on
  battery off, a scheduled backup runs on battery anyway once the last backup
  is that old. It can be turned off, for backups only on mains power.
- Preferences shows it with Back up on battery power, and only while that is
  off.
- While a backup is held up on battery (S18-T10), the status says when it will
  run anyway.

**Built** (2026-10-31): `backup.on_battery_after`, 3 by default and 0 for no
limit. Preflight lets a backup held up on battery run once the last backup is
that old (`preflight::battery_wait_ends`), and never before a first backup,
which is by far the largest and the one most worth leaving for mains power.
While one is held, the status's next backup is when the wait ends: the window
says "waiting for charger until 13:00", `backtrack status` "waiting for mains
power until … then backs up on battery anyway (backup.on_battery_after)", and
Preferences "Next backup: today at 13:00". The tray keeps Keith's short line.
Preferences and the wizard show "Wait for mains power" (Up to 1 hour, 3, 6, 12
hours, Up to 1 day, Always) under Back up on battery power while that is off.
Checked on Keith's laptop on battery (`scripts/rewind-check 5`), with the wait
set to an hour rather than kept at three, which changes only the length: the
row in Preferences shown and hidden with the switch, the hold and its end said
in the window, `backtrack status` and the tray, and a backup that ran on
battery, with no command, once the wait was over.

**Accept:** unit tests on the scheduler's decision: on battery with the
setting at 3, a backup held at 2 h 59 min since the last one and run at
3 h 1 min; held with the setting off; run on mains as before; a config
without the key reads as 3. Tests on the status saying when the held backup
will run. On Keith's laptop on battery, a backup runs three hours after the
last one with no command run.

### S18-T15 — Investigate: the wallpaper on Hyprland and other desktops
*(raised by Keith 2026-10-31)* Rewind shows its starfield on Hyprland because
Backtrack reads only GNOME's and Plasma's wallpaper (S18-T2), not because
there is none. On Hyprland and similar compositors, the wallpaper is drawn by
a separate program, and no Wayland protocol or portal tells an application
which picture it shows (the Wallpaper portal only sets one). On Keith's laptop
that program is the Caelestia shell (Quickshell), which records its current
wallpaper in `~/.local/state/caelestia/wallpaper/path.txt`; hyprpaper is
configured there but not running.
- Survey how each common wallpaper program can be asked for its current
  picture, per output: hyprpaper, swww, swaybg, wpaperd, mpvpaper, waypaper,
  Caelestia and the other Quickshell shells (Noctalia, DankMaterialShell).
- The same for the desktops S18-T2 left to the starfield: Sway, niri, COSMIC,
  Xfce, Cinnamon, MATE and Budgie.
- For each, what reading it takes inside the Flatpak.

**Found** (2026-10-31). Checked on Keith's laptop: the rest is from each
project's documentation or source, as surveyed that day.
- **Which program draws it, on Hyprland.** `hyprctl layers` (the `j/layers`
  request on Hyprland's socket) names the background layer of each monitor and
  its process: `caelestia-background`, from `qs -c caelestia`, on Keith's
  laptop. That says which record to read, so a record left behind by a program
  no longer used is not taken for the wallpaper. The socket is under
  `$XDG_RUNTIME_DIR/hypr`, which a Flatpak does not see; granting it would also
  grant control of Hyprland, so in the Flatpak the records alone decide.
- **Caelestia** (2.5.0, checked): `~/.local/state/caelestia/wallpaper/path.txt`
  holds the picture's absolute path, and `current` beside it links to it. One
  picture on every screen; its per-monitor settings hold only the bar's.
  Readable in the Flatpak under `HOST_XDG_STATE_HOME`, as is the picture.
- **hyprpaper** (0.8.4 installed, checked; not running there): its live state
  is `hyprctl hyprpaper listactive`, over a protocol of its own since 0.8; its
  config, `~/.config/hypr/hyprpaper.conf`, has per-monitor `wallpaper` blocks
  since 0.8, and Keith's still uses the older one-line syntax. The config
  misses changes made at run time.
- **awww** (swww until 0.12): `~/.cache/awww/<version>/<output>`, written on
  every change, per output, readable in the Flatpak; swww 0.11 used
  `~/.cache/swww/<output>`.
- **wpaperd**: one link per output, `~/.local/state/wpaperd/wallpapers/<output>`.
- **waypaper**: `~/.config/waypaper/config.ini` (or its state file), per output,
  but only what waypaper itself last set.
- **Noctalia** (5.x): `[wallpaper.default] path`, in its config
  (`~/.config/noctalia/*.toml`) and in `~/.local/state/noctalia/settings.toml`,
  which it writes when a picture is chosen and which wins; `color:#rrggbb` for
  a colour. Its per-monitor settings (`[wallpaper.monitor.<name>]`) name
  folders, not pictures. (Checked in its documentation while building S18-T18:
  the survey had them as pictures.) 4.x used `~/.cache/noctalia/wallpapers.json`.
- **DankMaterialShell** (2.x): `~/.local/state/dms/session.json`,
  `wallpaperPath`, per output when `perMonitorWallpaper` is set.
- **swaybg** and **mpvpaper**: no record; the picture is only on their command
  line, which a Flatpak cannot see. Sway's `output … bg` lines can be parsed
  from its config (with its `include`s and variables); **niri** draws no picture
  itself and leaves it to one of the above.
- **COSMIC**: `~/.local/state/cosmic/com.system76.CosmicBackground/v1/wallpapers`,
  per output, rewritten on every change.
- **Xfce**: `~/.config/xfce4/xfconf/xfce-perchannel-xml/xfce4-desktop.xml`,
  `/backdrop/screen0/monitor<connector>/workspace<n>/last-image`, per output
  and per workspace (the current workspace is not in the file).
- **Cinnamon** and **MATE**: GSettings like GNOME's
  (`org.cinnamon.desktop.background` `picture-uri`, `org.mate.background`
  `picture-filename`), read from dconf's files as S18-T2 reads GNOME's.
  **Budgie** uses GNOME's key, which S18-T2 already reads.
- Every record outside `$HOME` (a picture under `/usr`) is read in the Flatpak
  under `/run/host`, as S18-T2 does.

**Accept:** findings recorded in this section for each program and desktop
above: how its current wallpaper is read, per output, natively and in the
Flatpak, checked on Keith's laptop for Caelestia and hyprpaper; tasks added for
the ones Keith picks. Keith picked all three groups (2026-10-31): S18-T18,
T19 and T20.

### S18-T16 — Container storage is left out of backups by default
*(raised by Keith 2026-10-31)* Podman's storage, `~/.local/share/containers`,
was in Keith's backups: 364,000 files checked every hour, with a "permission
denied" warning for each that its containers own under other user IDs. Its
images and containers (`overlay*`) are rebuilt from images. Its volumes hold
containers' data, but a backup of their files misses what the containers own
(Nextcloud's `data` folder and `config.php`, on Keith's laptop) and copies the
rest while it is being written; `podman volume export` is how they are backed
up.
- The default exclusions gain Podman's storage (`**/.local/share/containers`)
  and rootless Docker's (`**/.local/share/docker`).
- Preferences → Backup shows the two as one row, **Container storage**, and
  the wizard's line on what is left out by default names it.
- As with S15-T8, the defaults seed a new setup; an existing one keeps its own
  list, which Preferences → Backup edits.

**Built** (2026-10-31): on Keith's laptop the storage held about 150,000
files of images and containers and 213,000 in volumes, of which 14 (among them
Nextcloud's `data` folder and `config.php`) could not be read. Checked with
borg 1.4.5 on a throwaway repository: both folders are excluded and not gone
into, and `~/containers`, `~/.config/containers` and
`~/.local/share/containers-notes.txt` are kept. Keith's own setup keeps its
list until he adds the row.

**Accept:** pattern tests: Podman's and rootless Docker's storage folders are
excluded, and folders that only resemble them are not; a default
configuration lists them, read in Preferences as one row.

### S18-T17 — Investigate: the service's memory use during a backup
*(found 2026-10-31 on Keith's laptop)* systemd reported the service's memory
peak at 3.8 GB over an hour holding one backup of 2.9 million files, and at
7.1 GB in an earlier run. The figure counts Borg, which runs in the service's
unit, as well as the service itself.
- Which process peaks, and in which step: Borg's create, the catalogue's
  reading of the archive, the prune, or the reconcile at start.
- Whether it grows from one backup to the next (a leak) or with the size of
  the archive (a peak), measured on a copy of Keith's catalogue and on a
  generated archive of millions of files.

**Found** (2026-10-31), sampled every two seconds from `/proc` and the
unit's cgroup through the 14:16 backup on Keith's laptop (2,563,206 files,
container storage left out since S18-T16):
- Borg's create held up to 792 MB (its files cache and chunk index for that
  many files), from 14:16:46 to 14:18:42; its prune, up to 302 MB.
- The catalogue's ingest of the archive took the daemon from 155 MB to 430 MB,
  from 14:18:42 to 14:19:21. `PathResolver` keeps every path it resolves for
  the length of an ingest, so that a folder is looked up once; it keeps every
  file too, though none is looked up twice, which at 2.5 million files is
  likely most of that rise.
- After the backup the daemon held 267 MB, where it had held 152 MB before:
  memory the allocator keeps, or a slow leak; one backup does not say which.
- The unit's peak, the figure systemd reports, was 1,884 MB, of which 1,229 MB
  was page cache: files Borg read, cached by the kernel and charged to the unit,
  which the kernel takes back when anything needs it. That is most of the
  3.8 GB and 7.1 GB seen earlier.

**Accept:** findings recorded in this section: where the memory goes, with
figures, and whether it grows over backups; tasks added for what Keith picks.

### S18-T18 — Rewind shows the wallpaper of Hyprland's shells and wallpaper programs
From S18-T15's findings. On a desktop other than GNOME or Plasma, Backtrack
reads the record kept by the program that draws the wallpaper: Caelestia,
awww (and swww before it), wpaperd, hyprpaper, waypaper, Noctalia and
DankMaterialShell.
- On Hyprland, natively, the background layer named on each monitor (the
  `j/layers` request on Hyprland's socket) says whose record to read, so a
  record left behind by a program no longer used is not taken for the
  wallpaper.
- Without that (in the Flatpak, or on another compositor such as niri), the
  most recently written record that names a picture which exists is read.
- The picture for the monitor Rewind opens on, where the record keeps one per
  monitor.
- `--wallpaper-report` says whose record was read, and why that one.

**Built** (2026-10-31): `model::wallpaper::records`, a parser for each
record, each checked against the project's own source or documentation
(Caelestia's on Keith's laptop), and the choice between them; the files and
Hyprland's socket are read in `wallpaper::others`. swaybg, which keeps no
record, is read from its command line when Hyprland's layer names it, outside
the Flatpak. On Keith's laptop the report reads Hyprland's background layer,
`caelestia-background`, and draws Caelestia's picture; with Hyprland's socket
hidden, as in the Flatpak, it finds four records (Caelestia's from that
morning, and awww's, hyprpaper's and waypaper's from July and early October)
and takes Caelestia's, the newest. In the Flatpak on the Plasma VM, told it was
on Hyprland, with a Caelestia record written by hand naming Fedora's JPEG XL
under `/usr`: the record read through the host's state folder, the picture
from `/run/host`, drawn at 3840 by 2160. Rewind no longer shows the starfield at once
on a desktop other than GNOME or Plasma: it looks for a record first, as it
asks GNOME and Plasma.

Checked by Keith on his laptop: Rewind over his Caelestia wallpaper, and after
he changed it, over the new one.

**Accept:** unit tests on each record's format, on fixtures shaped as each
project writes them, and on the choice between records with Hyprland's layers
and without; on Keith's laptop, natively, Rewind shows Caelestia's wallpaper,
and after `caelestia wallpaper` changes it, the new one; the Flatpak's reading
of the records checked on a VM, with a record in place.

### S18-T19 — Rewind shows the wallpaper of COSMIC, Xfce, Cinnamon and MATE
From S18-T15's findings, each a reader of the desktop's own settings.
- COSMIC: its state file, per output.
- Xfce: `xfce4-desktop.xml`, the monitor's `last-image` for its single
  workspace if one is set, else the first.
- Cinnamon and MATE: their GSettings keys, read from dconf's files and the
  compiled schemas as S18-T2 reads GNOME's, slideshows included.

**Found on the GNOME VM** (2026-10-31, Xfce 4.20 and Cinnamon 6.7.9 installed
beside GNOME), when Keith looked:
- Fedora 45's Cinnamon is 6.7, which keeps the wallpaper in
  `picture-uri-list` (one entry for each monitor by its connector, or one for
  all in its default 'mirror' mode) and leaves `picture-uri` as it was: a
  changed wallpaper did not show. The list is read first now
  (`gnome::from_list`), the older keys only while it is empty.
- Xfce writes nothing of the wallpaper, to its file or to xfconf, until it is
  changed, and showed the starfield. The system's settings folders are read
  next, where xfconf finds a distribution's defaults, then xfdesktop's own
  default, which its build names: Fedora's `images/default.jxl`, upstream's
  `xfce-x.svg`.

Checked by Keith on the GNOME VM with both fixed (`scripts/rewind-check 7`):
in GNOME, Xfce and Cinnamon, natively and in the Flatpak, Rewind showed the
desktop's wallpaper, and after it was changed, the new one. COSMIC and MATE
rest on their unit tests.

**Accept:** unit tests on fixtures of each desktop's files; on a VM with Xfce
and Cinnamon installed, natively and in the Flatpak, Rewind shows the
desktop's wallpaper, and after it is changed, the new one.

### S18-T20 — Rewind shows Sway's wallpaper
From S18-T15's findings: Sway keeps no record of it beyond its config.
- The config Sway reads (the first of `~/.sway/config`, `~/.config/sway/config`,
  i3's and `/etc/sway/config`), with its `include`s and `set $variables`, for
  `output <name|*> bg <file> <mode>`: the monitor's own line, else `output *`'s,
  the last one read winning.
- A change made at run time with `swaymsg` is not seen; the report says the
  picture came from the config.

**Built** (2026-10-31): `model::wallpaper::sway`, reading a config as Sway
does (comments, `\` continuations, `set` substituted in the lines after it and
only in a `set`'s value, `include`s where they stand, `output` lines and
blocks, `bg … solid_color`). Natively it reads the newest `swaybg`'s command
line first, as on Hyprland, which a change made with `swaymsg` is in; the
config is what the Flatpak, which cannot see other processes, reads.

**Found on the GNOME VM** (2026-11-04, Sway 1.12 from Fedora 45, with
`sway-config-upstream`): Fedora's `/etc/sway/config` names
`Sway_Wallpaper_Blue_1920x1080.png` for `output *` and includes
`/etc/sway/config.d/*`. `backtrack-gtk --wallpaper-report` read it natively
from swaybg's command line and in the Flatpak from `/run/host/etc/sway/config`;
with a user config that includes Fedora's and then sets the wallpaper through
an `include` of `~/.config/sway/config.d/*` and a `$wallpaper` variable, both
read the new picture. A change made with `swaymsg output * bg …` was read
natively and, as expected, not in the Flatpak, which still named the config's.
Two tests were added after mutants survived: a config that includes itself,
and a `bg` line commented out inside an `output { }` block; all 16 mutants of
the parser fail its tests. The check is `scripts/rewind-check 9`.

Checked by Keith on the GNOME VM (`scripts/rewind-check 9`): natively Rewind
showed Fedora's Sway wallpaper, and after the config was given one of its
own, that one; the Flatpak showed the config's too.

**Accept:** unit tests on configs with includes, variables, `~`, `*` and named
outputs; on a VM with Sway, natively and in the Flatpak, Rewind shows the
wallpaper its config names.

### S18-T21 — Cataloguing a backup holds less memory
From S18-T17: the catalogue's ingest took the daemon from 155 MB to 430 MB for
2.5 million files, much of it `PathResolver` keeping every file it resolved,
none of which is looked up again.
- The resolver keeps only folders; a file is looked up and inserted, not kept.
- A lookup no longer copies the name it looks up.

**Built** (2026-11-04): the resolver keeps each parent's folders in a map of
its own, by name, so a name is looked up as the `&str` it is; a path's last
part is kept only when the item is a folder. Files are looked up and
inserted as before, so the catalogue's queries per item are unchanged.

**Measured** (2026-11-04) with
`cargo run --release -p backtrack-core --example ingest_memory`: 2,500,000
files in 50,000 folders (2,550,250 items with the folders), ingested as a
first backup and again as the next, unchanged, into a catalogue on disk; the
peak resident memory of each ingest above what was resident before it:

| Ingest | Before | After |
|---|---|---|
| First backup (every item a new version) | +306 MB | +7 MB |
| Next backup (every item carried on) | +281 MB | +0 MB |
| Resident after the first | 88 MB | 12 MB |

The 88 MB the old resolver left resident after its ingest was memory the
allocator kept from the dropped cache, the same shape as the daemon holding
267 MB after a backup where it had held 152 MB before (S18-T17). The time per
ingest, 8 to 11 seconds, varied more between runs than between the two.

**Accept:** the catalogue an ingest writes is unchanged (the ingest tests pass
as they are); a test that the resolver keeps folders and not files; on a
generated listing of 2.5 million files in 50,000 folders, the ingest's peak
memory measured before and after, and recorded in this section.

## Definition of Done
All tasks done; the CHANGELOG's Unreleased section introduces Rewind; mockups 25 and 26 match what was built; the README and the file
manager documentation (`docs/file-manager-integration.md`) mention the new
menu items.
