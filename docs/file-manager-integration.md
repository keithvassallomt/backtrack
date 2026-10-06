# File-manager integration

Backtrack adds a few items to the file manager's right-click menu. Each one
opens Backtrack's own window at the right place and does nothing else. Deeper
integration is a trap: Nautilus removed the API that let extensions put widgets
in its window, and Déjà Dup dropped its own Nautilus plugin because it broke
with each GNOME release. See `backtrack_plan/reference/open-questions.md`, Q1.

## Installing

Preferences → General lists both integrations. One that is installed shows
*Installed* and a switch that takes its items away without uninstalling it;
one that is not shows **Install…**, which opens this section.

- **GNOME Files (Nautilus)** needs nautilus-python (`nautilus-python` on
  Fedora, `python3-nautilus` on Ubuntu, `python-nautilus` on Arch) and
  Backtrack's Nautilus package. Nautilus loads extensions when it starts, so
  quit it afterwards: `nautilus -q`.
- **Dolphin** needs Backtrack's Dolphin package only, and shows the items from
  the next right-click.

Backtrack's own packages, and their names for each distribution, come with
the packaging (Stage 13). From a checkout, `just install-nautilus-dev` and
`just install-dolphin-dev` install the development copies described below.

Preferences finds an integration by its file in the user's data directory
(`~/.local/share`) or a system one (`/usr/share`):
`nautilus-python/extensions/backtrack.py` and
`kio/servicemenus/backtrack.desktop`. It looks again each time the
Preferences window comes back into focus, so an integration installed while it
is open shows when you return to it.

## Nautilus

`integrations/nautilus/backtrack.py`, a nautilus-python extension (API 4.x).

| Right-click on | Items |
|---|---|
| One file | Restore Previous Version… |
| One folder | Restore Previous Version…, Browse Backups of This Folder… |
| The empty space of the folder being shown | Browse Backups of This Folder… |
| More than one item | nothing |

*Restore Previous Version…* runs `backtrack-gtk --path <its folder> --select
<it>`; *Browse Backups of This Folder…* runs `backtrack-gtk --path <folder>`.
Both go through the window's single-instance launch, so a second right-click
re-points the window already open.

The items appear only where they would find something:

- **Inside a backed-up folder.** Nautilus asks for menu items on every
  right-click and waits, so the extension never calls the daemon. Instead, the
  daemon writes the backed-up folders to `roots.json` in its data folder, on
  every start and every configuration change:

  ```json
  { "version": 1, "nautilus": true, "roots": ["/home/keith"] }
  ```

  The extension reads it again only when it changes. `nautilus` is the switch
  in Preferences → General → File manager integration; off, the extension
  stays installed and offers nothing.
- **On the same filesystem as that folder.** Backups stay on the filesystem
  each backed-up folder starts on, so a USB stick, a network share or another
  btrfs subvolume mounted inside one is not in them.
- **On this computer.** Network locations, the trash, Recent and search
  results' own folder are not files on this computer's disks.

### When a change shows

Nautilus builds the menu when the selection changes, not at each
right-click, and keeps it until the selection changes again. A change to the
backed-up folders or to the Preferences switch therefore shows from the next
click in Nautilus: right-clicking a file that has stayed selected since
before the change shows the menu as it was.

Nautilus does let an extension say its items have changed (the menu
provider's `items-updated` signal), but nautilus-python 4.2 cannot use it:
Nautilus connects to the C object nautilus-python wraps each Python
extension in, and a signal emitted from Python goes out on the Python
object, which nothing listens to. Checked against a running Nautilus 50.3
with a handler count on the Python object (none).

### Installing it for development

```sh
just install-nautilus-dev     # needs nautilus-python
nautilus -q                   # Nautilus loads extensions when it starts
```

The recipe installs the shipped file to
`~/.local/share/nautilus-python/extensions/backtrack.py` with two lines
rewritten: `APP` points at this checkout's debug build, and `DEVELOPMENT`
makes it read the development daemon's `roots.json` and launch the window
with `BACKTRACK_DEV=1`. `just uninstall-nautilus-dev` removes it.

Packages install it unchanged to `/usr/share/nautilus-python/extensions/`
(Stage 13). nautilus-python is `nautilus-python` on Fedora,
`python3-nautilus` on Ubuntu and `python-nautilus` on Arch.

### Tests

`just check-integrations`, which `just check` and CI run: ruff, and the unit
tests in `integrations/nautilus/test_backtrack.py`. Those run without
Nautilus, with stand-ins for the GObject modules.

### Manual checklist

`scripts/fm-check` sets up the cases the unit tests cannot reach: the real
Nautilus, a disk mounted inside the backed-up folder (an unprivileged FUSE
mount, no sudo), and a backed-up file in the trash. It works against the
development daemon's demo fixture, whose one backed-up folder is
`~/.local/share/backtrack-dev/demo-src`.

1. `scripts/fm-check setup`, then `scripts/fm-check nautilus`. Nautilus quits
   and reopens on the fixture's `home/Documents`, which holds `report.odt`,
   `invoice-may.pdf` and `notes.txt`.
2. **A file.** Right-click `report.odt`. *Restore Previous Version…* is in the
   menu; compare where with mockup 2. Choose it: Backtrack opens on
   `Documents` with `report.odt` selected.
3. **The folder being shown.** Right-click the empty space beside the files.
   *Browse Backups of This Folder…* is in the menu. Choose it: Backtrack's
   window, still open, comes forward. It is already on `Documents`, so it
   changes nothing and `report.odt` stays selected; a different folder clears
   the selection (step 4).
4. **A folder.** Go up to the fixture's `home` and right-click `Projects`. Both items are
   there. *Browse Backups of This Folder…* opens `Projects`; *Restore Previous
   Version…* opens `home` with `Projects` selected.
5. **A disk mounted inside the backed-up folder.** In the fixture's `home`, open
   `usb-stick`. Right-click `holiday.jpg`, then the empty space: no Backtrack
   items either time.
6. **Outside the backups.** Go to your own home folder and right-click a
   file, then the empty space: no Backtrack items.
7. **The trash.** Open Trash and right-click `backtrack-fm-check-draft.txt`:
   no Backtrack items, although it came from the backed-up `Documents`.
8. **The switch.** In Backtrack, Preferences → General, turn off *GNOME Files
   (Nautilus)*. Back in Nautilus, click `notes.txt`, then right-click
   `report.odt`: no Backtrack items, without restarting Nautilus. Turn it on
   again, click `notes.txt`, and right-click `report.odt`: the item is back.
   The click in between matters; see "When a change shows" above.
9. `scripts/fm-check reset` unmounts the stick, empties the draft from the
   trash, uninstalls the extension and quits Nautilus.

Note while doing it whether the clock icon from mockup 2 shows beside the
items. The extension asks for one, and whether it is drawn is up to the
Nautilus release.

### Checked

Keith's run on 2026-10-05, on Hyprland with Nautilus 50.3.1 and
nautilus-python 4.2.0, against the development daemon: every step above
passed. Nautilus 50 draws no icon beside extension items, so mockup 2's clock
does not appear. The run found that a change to the switch shows from the
next click rather than at once, which is now step 8 and "When a change
shows".

## Dolphin

Two KDE service menus (KDE Frameworks 6), which put the same two items in a
**Backtrack** submenu:

| File | Item | Offered on |
|---|---|---|
| `integrations/dolphin/backtrack.desktop` | Restore Previous Version… | one file or folder |
| `integrations/dolphin/backtrack-folder.desktop` | Browse Backups of This Folder… | one folder |

They are two files because KIO matches file types per file, not per action,
and Browse is for folders only. KIO merges submenus of the same name, and
sorts a submenu's items by action name, so the action names put Restore first
as mockup 3 does.

*Restore Previous Version…* runs `backtrack-gtk --select <it>`, which opens
the window on its folder with it selected; *Browse Backups of This Folder…*
runs `backtrack-gtk --path <folder>`. Dolphin offers a folder's menus on the
empty space of that folder too, so both items appear there: Restore opens the
folder above with this one selected.

Neither appears on more than one item, outside `file://` (network locations
and the trash), or anywhere KIO does not offer service menus.

### The switch

A service menu cannot read a setting, but Dolphin keeps its own list of
service-menu actions to leave out: `~/.config/kservicemenurc`, which its
Settings → Context Menu page writes and KIO reads each time it builds a menu.
The daemon writes Backtrack's two actions there as hidden when the
Preferences switch is turned off, and removes them when it is turned back on:

```ini
[Show]
backtrackPreviousVersion=false
backtrackThisFolder=false
```

It writes only when the switch changes, and at startup only when the switch
is off (for a configuration edited by hand while the daemon was stopped). A
switch left on never writes, so an item somebody hid from Dolphin's own
settings stays hidden. The change shows from the next right-click.

### Everywhere else, the window explains

A service menu is a fixed file: KIO decides whether to show it from the file
type, the location's protocol and the number of items, and nothing else. It
cannot read `roots.json`, so the Dolphin items appear on every local file and
folder, backed up or not. The window makes up for it: a folder with nothing
to show says why, and only says what it knows.

| The folder | The window says |
|---|---|
| Outside every backed-up folder | *This folder is not backed up*, with **Add to Backups…** |
| In a backed-up folder that was added since the latest backup | *Not backed up yet*: it will be in the next one |
| In a backed-up folder, but not in the latest backup | *Not in the latest backup*: it may be newer than that backup, excluded, or on another disk |
| Empty at the backup being viewed, or not there yet at an older one | *Not in this backup* |

The window cannot tell an exclusion or a mounted disk from a folder created
since the last backup, because only the daemon applies those rules, so it
names all three rather than promising the next backup. **Add to Backups…**
adds the folder to *What to back up* and opens Preferences on the Backup
page, where the folder's × takes it out again. These states belong to the
window, so they show however it was opened: `just run-app --path /etc` on
any desktop shows the first.

### Installing it for development

```sh
just install-dolphin-dev
```

The recipe installs both files to `~/.local/share/kio/servicemenus/` with the
command rewritten to this checkout's debug build and `BACKTRACK_DEV=1`, and
marks them executable: KIO runs a service menu from the home folder only if
its file is executable. Dolphin reads service menus each time it builds a
menu, so there is nothing to restart. `just uninstall-dolphin-dev` removes
them.

Packages install them unchanged to `/usr/share/kio/servicemenus/` (Stage 13),
where the executable bit is not needed.

The files use `Type=Application`, not the `Type=Service` of older service
menus, so that `desktop-file-validate` accepts them: KIO ignores `Type`, and
`NoDisplay=true` keeps them out of application launchers. `just
check-integrations` validates them, and CI runs it.

### Manual checklist

On the Plasma development VM (see `docs/development-vms.md`), in a terminal
on its screen, in its checkout:

1. `scripts/fm-check setup`, then `scripts/fm-check dolphin`. Dolphin opens on
   the fixture's `home/Documents`.
2. **A file.** Right-click `report.odt`. The menu has a **Backtrack** submenu
   holding *Restore Previous Version…*; compare with mockup 3. Choose it:
   Backtrack opens on `Documents` with `report.odt` selected.
3. **The folder being shown.** Right-click the empty space beside the files.
   *Backtrack* holds both items, Restore first. Choose *Browse Backups of This
   Folder…*: the window comes forward, still on `Documents`.
4. **A folder.** Go up to the fixture's `home` and right-click `Pictures`.
   Both items are there. *Browse…* opens `Pictures`; *Restore…* opens `home`
   with `Pictures` selected.
5. **Outside the backups.** Go to your own home folder, right-click
   `Downloads` and choose *Browse Backups of This Folder…*. The window says
   *This folder is not backed up*, with **Add to Backups…**.
6. **Adding it.** Choose **Add to Backups…**. Preferences opens on Backup with
   `Downloads` under *What to back up*, and the window now says *Not backed up
   yet*. Take `Downloads` out again with its ×.
7. **A disk mounted inside the backed-up folder.** In the fixture's `home`,
   open `usb-stick`, right-click `holiday.jpg` and choose *Restore Previous
   Version…*. The window opens on `usb-stick` and says *Not in the latest
   backup*.
8. **The trash.** Open Trash and right-click `backtrack-fm-check-draft.txt`:
   no Backtrack submenu.
9. **Two items.** Select `report.odt` and `notes.txt` together and
   right-click: no Backtrack submenu.
10. **The switch.** In Backtrack, Preferences → General shows Dolphin as
    *Installed*. Turn its switch off, then right-click `report.odt`: no
    Backtrack submenu. Turn it on again: the submenu is back at the next
    right-click.
11. `scripts/fm-check reset` unmounts the stick, empties the draft from the
    trash and uninstalls the menu.

Note while doing it which icon the submenu has. KIO takes it from the first
of the two files it reads, so it is the clock of *Restore Previous Version…*
or the folder of *Browse Backups of This Folder…*.
