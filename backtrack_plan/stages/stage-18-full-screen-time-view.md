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
interface. The view's name is settled with the mockups (S18-T1); until then
these files call it the full-screen view.

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
depend on it and can run beside it. Then T3 to T8 in order.

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
each round, the chosen direction is refined until Keith settles it. The view's
name is decided in the same rounds.

**Accept:** Keith has chosen a direction and a name; stills of it, light and
dark, are committed as mockups 25 and 26 in `backtrack_plan/mockups/`, and the
later tasks here are updated to cite them.

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
failure along the way, gets Backtrack's own background from S18-T1.

**Accept:** unit tests on fixtures: a dconf user database with and without the
key, a system database, a vendor default from compiled schemas, the light and
dark choice, an XML slideshow, and a Plasma package resolved by size and by
dark. On the GNOME and Plasma VMs, native and Flatpak, the view shows the
desktop's own wallpaper, and after it is changed in the desktop's settings the
next opening shows the new one. On Keith's laptop (Hyprland), Backtrack's own
background.

### S18-T3 — The full-screen view and its stack
A full-screen window on the monitor it was opened from, drawing the stack the
chosen design (S18-T1) describes.
- The backup in front is a live widget (S18-T4); the ones behind are drawn
  from cached images of their folders, so only one real file view exists at a
  time. GTK's GPU renderer and its 3D transforms carry the depth and
  perspective.
- Moving to another backup flies the stack, with a spring animation, never
  landing between two backups. Keys: Up and Down (and Page Up and Page Down)
  step, Home and End go to the ends, Escape cancels, Enter restores.
- With animations switched off in the desktop's settings, a crossfade instead
  of the flight.
- When frames run late, fewer backups are drawn behind, rather than the
  motion stuttering. A virtual machine rendering in software is not the
  measure: Keith judges the motion on real hardware.

**Accept:** on Keith's laptop, stepping through fifty backups of a real folder
looks smooth to Keith; on both VMs, the view opens full screen, steps by every
key above, and Escape returns to where it was opened from; with animations off,
crossfades.

### S18-T4 — The full-screen view's front window
The folder at the backup in front, as a file view: listed from the catalogue as
the main window's file pane is, and sharing the window's state (folder,
selection), so leaving the view lands where it was.
- Walking into a folder and back up, as in the main window.
- Icons by file type. Real thumbnails where the backed-up version is the one
  on disk now (the desktop's thumbnail cache already has those), and fetched
  lazily, for the front window only, where it is not; never for the backups
  behind.
- The badges the main window shows ("deleted after this", "changed since
  then", and Stages 16 and 17's when they are in).

**Accept:** unit tests on which files get a thumbnail and from where; on a VM,
walking into a folder and back in the view, with thumbnails for images
unchanged since the backup and icons for the others, and leaving the view at
the same folder and selection.

### S18-T5 — The full-screen view's timeline
The listed backups (Stage 15) as the chosen design's timeline, with its
labels ("Today (Now)", the months) and the step arrows.
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

## Definition of Done
All tasks done; the CHANGELOG's Unreleased section introduces the view by its
chosen name; mockups 25 and 26 match what was built; the README and the file
manager documentation (`docs/file-manager-integration.md`) mention the new
menu items.
