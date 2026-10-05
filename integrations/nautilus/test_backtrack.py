# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

"""Tests for the Nautilus extension, run without Nautilus.

The extension imports `gi.repository.Nautilus`, which exists only inside a
running Nautilus, so these tests put stand-ins for the three GObject modules
it uses in `sys.modules` before importing it. Everything the extension decides
is decided in plain Python, and that is what is tested here.

Run with `python3 -m unittest discover integrations/nautilus`.
"""

import json
import os
import sys
import tempfile
import types
import unittest
from unittest import mock


class MenuItem:
    def __init__(self, **properties):
        self.properties = properties
        self.handlers = []

    def connect(self, signal, handler):
        self.handlers.append((signal, handler))

    def activate(self):
        for signal, handler in self.handlers:
            if signal == "activate":
                handler(self)


def stand_ins():
    nautilus = types.SimpleNamespace(MenuItem=MenuItem, MenuProvider=type("MenuProvider", (), {}))
    gobject = types.SimpleNamespace(GObject=type("GObject", (), {}))
    gio = types.SimpleNamespace()
    repository = types.ModuleType("gi.repository")
    repository.Nautilus = nautilus
    repository.GObject = gobject
    repository.Gio = gio
    gi = types.ModuleType("gi")
    gi.repository = repository
    return {"gi": gi, "gi.repository": repository}


with mock.patch.dict(sys.modules, stand_ins()):
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import backtrack


class FileInfo:
    """What Nautilus hands the extension for one file."""

    def __init__(self, path, scheme="file", directory=False):
        self.path = path
        self.scheme = scheme
        self.directory = directory

    def get_uri_scheme(self):
        return self.scheme

    def get_location(self):
        return types.SimpleNamespace(get_path=lambda: self.path)

    def is_directory(self):
        return self.directory


class Fixture(unittest.TestCase):
    """A backed-up folder on disk, and a roots file that names it."""

    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.home = os.path.join(self.scratch.name, "home")
        os.makedirs(os.path.join(self.home, "Projects", "designs"))
        self.report = os.path.join(self.home, "Projects", "report.odt")
        with open(self.report, "w") as file:
            file.write("report")
        self.roots_file = os.path.join(self.scratch.name, "data", "roots.json")
        self.publish({"version": 1, "nautilus": True, "roots": [self.home]})

        environment = mock.patch.dict(
            os.environ, {"XDG_DATA_HOME": os.path.join(self.scratch.name, "xdg")}
        )
        environment.start()
        self.addCleanup(environment.stop)

        self.launched = []
        launch = mock.patch.object(
            backtrack, "launch", lambda *arguments: self.launched.append(list(arguments))
        )
        launch.start()
        self.addCleanup(launch.stop)

        self.menu = backtrack.BacktrackMenu()
        self.menu.published = backtrack.Published(self.roots_file)

    def publish(self, content):
        os.makedirs(os.path.dirname(self.roots_file), exist_ok=True)
        # Written aside and renamed, as the daemon does, so the file is a new
        # one each time and the extension notices.
        with open(self.roots_file + ".tmp", "w") as file:
            file.write(content if isinstance(content, str) else json.dumps(content))
        os.replace(self.roots_file + ".tmp", self.roots_file)

    def labels(self, items):
        return [entry.properties["label"] for entry in items]


class TheItems(Fixture):
    def test_a_file_offers_its_previous_versions(self):
        items = self.menu.get_file_items([FileInfo(self.report)])
        self.assertEqual(self.labels(items), ["Restore Previous Version…"])

        items[0].activate()
        self.assertEqual(
            self.launched,
            [["--path", os.path.dirname(self.report), "--select", self.report]],
        )

    def test_a_folder_offers_its_previous_versions_and_its_backups(self):
        designs = os.path.join(self.home, "Projects", "designs")
        items = self.menu.get_file_items([FileInfo(designs, directory=True)])
        self.assertEqual(
            self.labels(items),
            ["Restore Previous Version…", "Browse Backups of This Folder…"],
        )

        items[1].activate()
        self.assertEqual(self.launched, [["--path", designs]])

    def test_the_folder_being_shown_offers_its_backups(self):
        projects = os.path.join(self.home, "Projects")
        items = self.menu.get_background_items(FileInfo(projects, directory=True))
        self.assertEqual(self.labels(items), ["Browse Backups of This Folder…"])

        items[0].activate()
        self.assertEqual(self.launched, [["--path", projects]])

    def test_a_backup_root_itself_is_offered(self):
        items = self.menu.get_background_items(FileInfo(self.home, directory=True))
        self.assertEqual(self.labels(items), ["Browse Backups of This Folder…"])

    def test_the_item_names_are_the_extensions_own(self):
        items = self.menu.get_file_items([FileInfo(self.report)])
        self.assertTrue(items[0].properties["name"].startswith("Backtrack::"))

    def test_more_than_one_file_offers_nothing(self):
        other = os.path.join(self.home, "Projects", "designs")
        items = self.menu.get_file_items([FileInfo(self.report), FileInfo(other)])
        self.assertEqual(items, [])


class OutsideTheBackups(Fixture):
    def test_a_folder_outside_every_root_offers_nothing(self):
        elsewhere = os.path.join(self.scratch.name, "elsewhere")
        os.makedirs(elsewhere)
        self.assertEqual(self.menu.get_background_items(FileInfo(elsewhere)), [])

    def test_a_name_that_only_starts_like_a_root_is_outside_it(self):
        neighbour = self.home + "2"
        os.makedirs(neighbour)
        self.assertEqual(self.menu.get_background_items(FileInfo(neighbour)), [])

    def test_a_network_location_offers_nothing(self):
        share = FileInfo(None, scheme="smb")
        self.assertEqual(self.menu.get_background_items(share), [])
        self.assertEqual(self.menu.get_file_items([share]), [])

    def test_the_trash_offers_nothing(self):
        trashed = FileInfo(None, scheme="trash")
        self.assertEqual(self.menu.get_file_items([trashed]), [])

    def test_a_disk_mounted_inside_a_root_offers_nothing(self):
        stick = os.path.join(self.home, "stick")
        os.makedirs(stick)
        real = os.lstat

        def mounted(path, *arguments, **keywords):
            info = real(path, *arguments, **keywords)
            if path == stick or path.startswith(stick + "/"):
                fields = list(info)
                fields[2] = info.st_dev + 1
                return os.stat_result(fields)
            return info

        with mock.patch.object(backtrack.os, "lstat", mounted):
            self.assertEqual(self.menu.get_background_items(FileInfo(stick)), [])
            self.assertNotEqual(self.menu.get_background_items(FileInfo(self.home)), [])

    def test_a_file_that_has_gone_offers_nothing(self):
        gone = os.path.join(self.home, "gone.txt")
        self.assertEqual(self.menu.get_file_items([FileInfo(gone)]), [])


class TheRootsFile(Fixture):
    def test_switched_off_in_preferences_offers_nothing(self):
        self.publish({"version": 1, "nautilus": False, "roots": [self.home]})
        self.assertEqual(self.menu.get_file_items([FileInfo(self.report)]), [])

    def test_a_change_is_picked_up_without_restarting_nautilus(self):
        self.assertNotEqual(self.menu.get_file_items([FileInfo(self.report)]), [])
        self.publish({"version": 1, "nautilus": True, "roots": []})
        self.assertEqual(self.menu.get_file_items([FileInfo(self.report)]), [])
        self.publish({"version": 1, "nautilus": True, "roots": [self.home + "/"]})
        self.assertNotEqual(self.menu.get_file_items([FileInfo(self.report)]), [])

    def test_no_file_offers_nothing(self):
        os.remove(self.roots_file)
        self.assertEqual(self.menu.get_file_items([FileInfo(self.report)]), [])

    def test_an_unreadable_file_offers_nothing(self):
        for broken in ["", "{", "[]", '{"version": 1, "nautilus": true, "roots": "/"}']:
            with self.subTest(content=broken):
                self.publish(broken)
                self.assertEqual(self.menu.get_file_items([FileInfo(self.report)]), [])

    def test_a_format_from_another_version_offers_nothing(self):
        self.publish({"version": 2, "nautilus": True, "roots": [self.home]})
        self.assertEqual(self.menu.get_file_items([FileInfo(self.report)]), [])

    def test_relative_roots_are_ignored(self):
        self.publish({"version": 1, "nautilus": True, "roots": ["home", 7, self.home]})
        self.assertEqual(self.menu.published.current(), (self.home,))


class WhereTheFileIs(unittest.TestCase):
    def test_it_is_in_the_data_folder_beside_the_configuration(self):
        with mock.patch.dict(os.environ, {"XDG_DATA_HOME": "/x/data"}):
            self.assertEqual(backtrack.roots_file(), "/x/data/backtrack/roots.json")

    def test_an_empty_xdg_data_home_falls_back_to_the_home_folder(self):
        with mock.patch.dict(os.environ, {"XDG_DATA_HOME": "", "HOME": "/home/k"}):
            self.assertEqual(backtrack.roots_file(), "/home/k/.local/share/backtrack/roots.json")

    def test_development_reads_the_development_daemons_file(self):
        with (
            mock.patch.dict(os.environ, {"XDG_DATA_HOME": "/x/data"}),
            mock.patch.object(backtrack, "DEVELOPMENT", True),
        ):
            self.assertEqual(backtrack.roots_file(), "/x/data/backtrack-dev/roots.json")


if __name__ == "__main__":
    unittest.main()
