# File-manager integration

Backtrack adds a few items to the file manager's right-click menu. Each one
opens Backtrack's own window at the right place and does nothing else. Deeper
integration is a trap: Nautilus removed the API that let extensions put widgets
in its window, and Déjà Dup dropped its own Nautilus plugin because it broke
with each GNOME release. See `backtrack_plan/reference/open-questions.md`, Q1.

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
