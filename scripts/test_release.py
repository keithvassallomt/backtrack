# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>
"""Tests for release.py's rewrites, against the real packaging files:
python3 -m unittest discover -s scripts."""

import unittest
import xml.etree.ElementTree as ElementTree

import release

COMMIT = "0123456789abcdef0123456789abcdef01234567"
HISTORY = [
    ("0.2.0", "2026-12-01", "### Added\n\n- Search across time."),
    ("0.1.0", "2026-11-01", "### Added\n\n- The first release."),
]


class Rewrites(unittest.TestCase):
    def test_the_friendlyhub_manifest_builds_the_tag_not_the_checkout(self):
        manifest = release.friendlyhub_manifest(release.MANIFEST.read_text(), "v0.2.0", COMMIT)
        self.assertNotIn("type: dir", manifest)
        self.assertIn("        tag: v0.2.0\n", manifest)
        self.assertIn(f"        commit: {COMMIT}\n", manifest)
        laid_over = "path: tools.backtrack.Backtrack.metainfo.xml\n        dest: packaging\n"
        self.assertIn(laid_over, manifest)
        # The generated sources stay beside it, by the same names.
        self.assertIn("      - cargo-sources.json\n", manifest)
        self.assertIn("  - python3-requirements.json\n", manifest)

    def test_the_metainfo_carries_the_history_newest_first(self):
        metainfo = release.metainfo_with_releases(release.METAINFO.read_text(), HISTORY)
        component = ElementTree.fromstring(metainfo.encode())
        versions = [r.get("version") for r in component.find("releases")]
        self.assertEqual(versions, ["0.2.0", "0.1.0"])
        with self.assertRaises(ValueError):
            release.metainfo_with_releases(metainfo, HISTORY)

    def test_the_pkgbuild_gains_the_tags_checksum(self):
        pkgbuild = release.pkgbuild_with_checksum(
            (release.ROOT / "packaging/aur/PKGBUILD").read_text(), "ab" * 32
        )
        self.assertIn(f"sha256sums=('{'ab' * 32}')", pkgbuild)
        self.assertNotIn("SKIP", pkgbuild)

    def test_each_series_gets_its_own_version_below_the_archives(self):
        changelog = (release.ROOT / "packaging/deb/changelog").read_text()
        rewritten = release.changelog_for_series(changelog, "resolute", "26.04")
        first = rewritten.splitlines()[0]
        self.assertRegex(first, r"^backtrack \([^)]+-1~ubuntu26\.04\.1\) resolute; urgency=")
        self.assertEqual(rewritten.splitlines()[1:], changelog.splitlines()[1:])

    def test_a_ppa_is_split_into_owner_and_name(self):
        parts = release.ppa_parts("ppa:keith/backtrack-testing")
        self.assertEqual(parts, ("keith", "backtrack-testing"))

    def test_the_tags_release_run_is_found_beside_its_ci_run(self):
        runs = [
            {"databaseId": 3, "headBranch": "v0.2.0-rc1", "workflowName": "CI"},
            {"databaseId": 2, "headBranch": "v0.2.0-rc1", "workflowName": "Release"},
            {"databaseId": 1, "headBranch": "v0.1.0", "workflowName": "Release"},
        ]
        self.assertEqual(release.release_run(runs, "v0.2.0-rc1"), 2)
        self.assertIsNone(release.release_run(runs[:1], "v0.2.0-rc1"))


if __name__ == "__main__":
    unittest.main()
