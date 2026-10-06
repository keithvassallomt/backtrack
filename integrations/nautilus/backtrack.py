# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

"""Backtrack's items in the Nautilus right-click menu.

Two items, and nothing else:

- "Restore Previous Version…" on a selected file or folder, which opens
  Backtrack's window on the folder it is in with it selected.
- "Browse Backups of This Folder…" on a selected folder, or on the empty
  space of the folder being shown, which opens the window on that folder.

The extension is a launcher and does no more than that. Nautilus's extension
API has changed under every plugin that tried to do more, so this one only
adds menu items and leaves the rest to the window, through the launch
contract the window documents (`backtrack-gtk --path DIR --select FILE`).

The items appear only where they would find something: inside the folders
Backtrack backs up, on this computer's own disks. Nautilus asks for items on
every right-click and waits for the answer, so the extension never calls the
daemon. The daemon publishes the backed-up folders to `roots.json` in
Backtrack's data folder whenever they change, and this reads that file.
"""

import json
import os

from gi.repository import Gio, GObject, Nautilus

# The window to launch, and whether this copy belongs to a development
# checkout. `just install-nautilus-dev` rewrites these two lines in the copy
# it installs: the debug build, and the development daemon's data.
APP = "backtrack-gtk"
DEVELOPMENT = False

RESTORE = "Restore Previous Version…"
BROWSE = "Browse Backups of This Folder…"
# The clock beside both items in the mockups. Some Nautilus releases draw
# icons on extension items and some do not; where they do not, this is unused.
ICON = "document-open-recent-symbolic"

# The roots file format this extension understands. Anything else is treated
# as no file at all.
VERSION = 1


def roots_file():
    """Where the daemon publishes the backup roots.

    The same rule as the daemon's own data folder: `$XDG_DATA_HOME/backtrack`,
    else `~/.local/share/backtrack`, with `backtrack-dev` in development.
    """
    base = os.environ.get("XDG_DATA_HOME") or os.path.join(
        os.path.expanduser("~"), ".local", "share"
    )
    leaf = "backtrack-dev" if DEVELOPMENT else "backtrack"
    return os.path.join(base, leaf, "roots.json")


class Published:
    """The roots file as last read, read again only when it changes."""

    def __init__(self, path):
        self.path = path
        self.stamp = None
        self.roots = ()

    def current(self):
        """The backup roots to offer the menu items in, as a tuple of paths.

        Empty when the file is missing, unreadable, from a format this does
        not know, or says the Nautilus items are switched off.
        """
        try:
            info = os.stat(self.path)
        except OSError:
            self.stamp, self.roots = None, ()
            return self.roots
        stamp = (info.st_ino, info.st_mtime_ns, info.st_size)
        if stamp != self.stamp:
            self.stamp = stamp
            self.roots = read(self.path)
        return self.roots


def read(path):
    """The roots `path` publishes, or nothing."""
    try:
        with open(path, encoding="utf-8") as file:
            published = json.load(file)
    except (OSError, ValueError):
        return ()
    if not isinstance(published, dict) or published.get("version") != VERSION:
        return ()
    if published.get("nautilus") is not True:
        return ()
    roots = published.get("roots")
    if not isinstance(roots, list):
        return ()
    return tuple(
        root.rstrip("/") or "/" for root in roots if isinstance(root, str) and os.path.isabs(root)
    )


def root_of(path, roots):
    """The backup root `path` is inside, or is, or None."""
    for root in roots:
        if path == root or path.startswith(root.rstrip("/") + "/"):
            return root
    return None


def backed_up(path, roots):
    """Whether Backtrack's backups would hold `path`.

    It has to be inside a backup root, and on the same filesystem as that
    root: backups stay on the filesystem each root starts on, so a network
    share or a USB stick mounted inside a backed-up folder is not in them.
    """
    root = root_of(path, roots)
    if root is None:
        return False
    try:
        return os.lstat(path).st_dev == os.stat(root).st_dev
    except OSError:
        return False


def local_path(info):
    """The path of a file Nautilus is showing, or None when it is not a file
    on this computer (a network location, the trash, search results)."""
    if info.get_uri_scheme() != "file":
        return None
    return info.get_location().get_path()


def launch(*arguments):
    """Open Backtrack's window with `arguments`.

    The window is a single application: a second launch re-points the window
    already open rather than adding another.
    """
    launcher = Gio.SubprocessLauncher.new(Gio.SubprocessFlags.NONE)
    if DEVELOPMENT:
        launcher.setenv("BACKTRACK_DEV", "1", True)
    launcher.spawnv([APP, *arguments])


def restore(path):
    """Open the folder `path` is in, with `path` selected."""
    launch("--path", os.path.dirname(path), "--select", path)


def browse(folder):
    """Open `folder`."""
    launch("--path", folder)


def item(name, label, action, path):
    """A menu item that runs `action(path)` when chosen."""
    entry = Nautilus.MenuItem(name=f"Backtrack::{name}", label=label, icon=ICON)
    entry.connect("activate", lambda _item: action(path))
    return entry


class BacktrackMenu(GObject.GObject, Nautilus.MenuProvider):
    """Adds Backtrack's items to the menus of backed-up files and folders."""

    def __init__(self):
        super().__init__()
        self.published = Published(roots_file())

    def get_file_items(self, files):
        if len(files) != 1:
            return []
        path = local_path(files[0])
        if path is None or not backed_up(path, self.published.current()):
            return []
        items = [item("Restore", RESTORE, restore, path)]
        if files[0].is_directory():
            items.append(item("Browse", BROWSE, browse, path))
        return items

    def get_background_items(self, current_folder):
        path = local_path(current_folder)
        if path is None or not backed_up(path, self.published.current()):
            return []
        return [item("BrowseHere", BROWSE, browse, path)]
