# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>
"""Tests for release_notes.py: python3 -m unittest discover -s scripts."""

import unittest
import xml.etree.ElementTree as ElementTree

import release_notes

CHANGELOG = """# Changelog

## [Unreleased]

### Added

- Something not released yet.

## [0.2.0] - 2026-12-01

### Added

- Search finds files deleted since, in **every** backup.
- Restore a folder `as it was`, entries that wrap
  onto a second line included.

### Fixed

- A banner that said <nothing>.

## [0.1.0] - 2026-11-01

### Added

- The first release.

## [0.0.9] - TBD
"""


class ReleaseNotes(unittest.TestCase):
    def test_a_section_ends_at_the_next_version(self):
        date, body = release_notes.section(CHANGELOG, "0.2.0")
        self.assertEqual(date, "2026-12-01")
        self.assertIn("A banner that said <nothing>.", body)
        self.assertNotIn("The first release.", body)
        self.assertNotIn("not released yet", body)

    def test_an_undated_or_missing_version_is_refused(self):
        with self.assertRaises(release_notes.NotReleased):
            release_notes.section(CHANGELOG, "0.0.9")
        with self.assertRaises(release_notes.NotReleased):
            release_notes.section(CHANGELOG, "9.9.9")

    def test_a_pre_release_without_a_section_takes_the_unreleased_entries(self):
        date, body = release_notes.notes(CHANGELOG, "0.3.0-rc1")
        self.assertEqual(body, "### Added\n\n- Something not released yet.")
        self.assertRegex(date, r"^\d{4}-\d{2}-\d{2}$")
        # A release still needs its own dated section.
        with self.assertRaises(release_notes.NotReleased):
            release_notes.notes(CHANGELOG, "0.3.0")

    def test_the_history_runs_from_a_version_back(self):
        history = release_notes.released(CHANGELOG, "0.2.0")
        self.assertEqual(
            [(v, d) for v, d, _ in history], [("0.2.0", "2026-12-01"), ("0.1.0", "2026-11-01")]
        )
        self.assertEqual([v for v, _, _ in release_notes.released(CHANGELOG, "0.1.0")], ["0.1.0"])
        with self.assertRaises(release_notes.NotReleased):
            release_notes.released(CHANGELOG, "0.0.9")

    def test_the_appstream_release_is_valid_markup(self):
        date, body = release_notes.section(CHANGELOG, "0.2.0")
        element = ElementTree.fromstring(release_notes.appstream("0.2.0", date, body))
        self.assertEqual(element.get("version"), "0.2.0")
        self.assertEqual(element.get("date"), "2026-12-01")
        items = [ElementTree.tostring(li, encoding="unicode") for li in element.iter("li")]
        self.assertEqual(len(items), 3)
        self.assertIn("<em>every</em>", items[0])
        self.assertIn("<code>as it was</code>, entries that wrap onto a second line", items[1])
        self.assertIn("&lt;nothing&gt;", items[2])
        self.assertEqual([p.text for p in element.iter("p")], ["Added", "Fixed"])


if __name__ == "__main__":
    unittest.main()
