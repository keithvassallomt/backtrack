# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>
"""Tests for release.py's rewrites, against the real packaging files:
python3 -m unittest discover -s scripts."""

import contextlib
import io
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ElementTree
from datetime import UTC, datetime
from pathlib import Path
from unittest import mock

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
            {"databaseId": 3, "headBranch": "v0.2.0-rc1", "headSha": "b", "workflowName": "CI"},
            {
                "databaseId": 2,
                "headBranch": "v0.2.0-rc1",
                "headSha": "b",
                "workflowName": "Release",
            },
            {"databaseId": 1, "headBranch": "v0.1.0", "headSha": "a", "workflowName": "Release"},
        ]
        self.assertEqual(release.release_run(runs, "v0.2.0-rc1", "b"), 2)
        self.assertIsNone(release.release_run(runs[:1], "v0.2.0-rc1", "b"))

    def test_a_retagged_release_waits_for_its_own_run(self):
        # The tag deleted and pushed again at a fix: the earlier, failed run is
        # still listed under the same tag until the new one appears.
        earlier = {
            "databaseId": 1,
            "headBranch": "v0.2.0-rc1",
            "headSha": "a",
            "workflowName": "Release",
        }
        self.assertIsNone(release.release_run([earlier], "v0.2.0-rc1", "b"))


def job(name, status, conclusion="", started="2026-10-18T09:12:41Z", completed="", steps=()):
    """A job as `gh run view --json jobs` gives it."""
    return {
        "name": name,
        "status": status,
        "conclusion": conclusion,
        "startedAt": started,
        "completedAt": completed or "0001-01-01T00:00:00Z",
        "url": "https://github.com/o/r/actions/runs/1/job/2",
        "steps": list(steps),
    }


class Progress(unittest.TestCase):
    def test_each_job_is_said_as_it_starts_and_as_it_ends(self):
        said = {}
        first = [job("Source tarballs", "in_progress"), job("RPMs (Fedora 44)", "queued")]
        news = release.job_news(first, said, UTC)
        self.assertEqual(news, [("Source tarballs", "09:12:41  Source tarballs: started")])
        self.assertEqual(release.job_news(first, said, UTC), [])
        second = [
            job("Source tarballs", "completed", "success", completed="2026-10-18T09:24:10Z"),
            job("RPMs (Fedora 44)", "in_progress", started="2026-10-18T09:24:12Z"),
        ]
        news = release.job_news(second, said, UTC)
        self.assertEqual(
            news,
            [
                ("Source tarballs", "09:24:10  Source tarballs: passed in 11m 29s"),
                ("RPMs (Fedora 44)", "09:24:12  RPMs (Fedora 44): started"),
            ],
        )

    def test_a_failed_job_names_the_step_and_links_the_log(self):
        failed = job(
            "RPMs (Fedora 44)",
            "completed",
            "failure",
            completed="2026-10-18T09:13:37Z",
            steps=[
                {"name": "Run actions/checkout@v7", "conclusion": "success"},
                {"name": "Run just dist", "conclusion": "failure"},
            ],
        )
        news = release.job_news([failed], {}, UTC)
        self.assertEqual(
            news,
            [
                (
                    "RPMs (Fedora 44)",
                    "09:13:37  RPMs (Fedora 44): failure at “Run just dist” after 56s",
                ),
                ("RPMs (Fedora 44)", "    https://github.com/o/r/actions/runs/1/job/2"),
            ],
        )

    def test_a_skipped_job_has_no_duration(self):
        # GitHub stamps a skipped job as finished a second before it started.
        skipped = job(
            "GitHub release",
            "completed",
            "skipped",
            started="2026-10-18T08:35:50Z",
            completed="2026-10-18T08:35:49Z",
        )
        news = release.job_news([skipped], {}, UTC)
        self.assertEqual(news, [("GitHub release", "08:35:49  GitHub release: skipped")])

    def test_a_quiet_stretch_names_the_jobs_still_running(self):
        jobs = [
            job("Flatpak bundle", "in_progress"),
            job("Source tarballs", "completed", "success", completed="2026-10-18T09:13:37Z"),
        ]
        now = datetime(2026, 10, 18, 9, 30, 0, tzinfo=UTC)
        line = release.still_running(jobs, now)
        self.assertEqual(line, "still running: Flatpak bundle (17m 19s)")
        self.assertIsNone(release.still_running(jobs[1:], now))


class Tags(unittest.TestCase):
    def test_each_job_shares_its_channels_colour(self):
        tags = [release.job_tag(n) for n in ("RPMs (Fedora 44)", "Debs (Ubuntu 26.04)")]
        self.assertEqual(tags, ["CI rpm", "CI deb"])
        self.assertEqual(release.COLOURS["CI rpm"], release.COLOURS["COPR"])
        self.assertEqual(release.COLOURS["CI deb"], release.COLOURS["PPA"])
        self.assertEqual(release.COLOURS["CI flatpak"], release.COLOURS["FriendlyHub"])
        self.assertEqual(release.job_tag("A job added later"), "CI")

    def test_a_line_is_coloured_on_a_terminal_and_plain_in_the_log(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "release.log"
            r = release.Run("0.2.0", False, Path(directory), {}, log, step="PPA", colour=True)
            shown = io.StringIO()
            with contextlib.redirect_stdout(shown):
                r.say("uploaded")
            # Every message starts in the same column, whichever its tag.
            label = "[PPA]".ljust(len("[FriendlyHub]"))
            self.assertEqual(shown.getvalue(), f"\033[1;38;5;208m{label}\033[0m uploaded\n")
            self.assertEqual(log.read_text(), f"{label} uploaded\n")

    def test_a_commands_output_is_said_under_the_steps_tag(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "release.log"
            r = release.Run("0.2.0", False, Path(directory), {}, log, step="COPR", colour=False)
            label = "[COPR]".ljust(len("[FriendlyHub]"))
            shown = io.StringIO()
            with contextlib.redirect_stdout(shown):
                r.run(
                    sys.executable, "-c", "print('Build 1: running'); print('Build 1: succeeded')"
                )
            lines = shown.getvalue().splitlines()
            self.assertEqual(
                lines[1:], [f"{label} Build 1: running", f"{label} Build 1: succeeded"]
            )
            self.assertEqual(log.read_text().splitlines(), lines)


class Rerun(unittest.TestCase):
    def test_what_a_rerun_reads_is_never_a_cached_answer(self):
        with mock.patch("urllib.request.urlopen") as urlopen:
            urlopen.return_value.__enter__.return_value.read.return_value = b"{}"
            release.fetch("https://api.launchpad.net/1.0/~o/+archive/ubuntu/p")
        (request,), _ = urlopen.call_args
        self.assertEqual(request.get_header("Cache-control"), "no-cache")
        self.assertEqual(request.get_header("Pragma"), "no-cache")

    def test_a_candidates_aur_build_is_not_made_twice_from_the_same_files(self):
        with tempfile.TemporaryDirectory() as directory:
            aur = Path(directory)
            self.assertFalse(release.aur_built_from(aur, "tree-a"))
            (aur / "backtrack-0.2.0rc1-1-x86_64.pkg.tar.zst").touch()
            self.assertFalse(release.aur_built_from(aur, "tree-a"))
            (aur / release.BUILT_FROM).write_text("tree-a")
            self.assertTrue(release.aur_built_from(aur, "tree-a"))
            # A change to any file, the version among them, is another tree.
            self.assertFalse(release.aur_built_from(aur, "tree-b"))
            (aur / "backtrack-0.2.0rc1-1-x86_64.pkg.tar.zst").unlink()
            (aur / "backtrack-nautilus-0.2.0rc1-1-any.pkg.tar.zst").touch()
            self.assertFalse(release.aur_built_from(aur, "tree-a"))

    def test_a_version_built_on_copr_is_not_built_again(self):
        # As COPR's API lists them: the second of these was a rerun's.
        builds = [
            {"id": 2, "state": "running", "source_package": {"version": "0.1.0~rc1-1"}},
            {"id": 1, "state": "succeeded", "source_package": {"version": "0.1.0~rc1-1"}},
        ]
        self.assertTrue(release.copr_built(builds, "0.1.0~rc1"))
        self.assertFalse(release.copr_built(builds[:1], "0.1.0~rc1"))
        self.assertFalse(release.copr_built(builds, "0.1.0~rc2"))
        # copr-cli's own list has no version, which is how the first
        # candidate's second run came to build it again.
        self.assertFalse(release.copr_built([{"id": 1, "state": "succeeded"}], "0.1.0~rc1"))


def agent_options(default_ttl="", max_ttl="", ignore=""):
    """`gpgconf --list-options gpg-agent`'s lines for the cache, as set."""
    return (
        f"default-cache-ttl:24:0:expire cached PINs after N seconds:3:3:N:600::{default_ttl}\n"
        f"max-cache-ttl:24:2:set maximum PIN cache lifetime to N seconds:3:3:N:7200::{max_ttl}\n"
        f"ignore-cache-for-signing:8:0:do not use the PIN cache when signing:0:0::::{ignore}\n"
    )


class Passphrase(unittest.TestCase):
    def test_four_hours_holds_it_to_the_ppa_step(self):
        self.assertIsNone(release.passphrase_held(agent_options("14400", "14400")))

    def test_the_agents_own_defaults_forget_it_sooner(self):
        # Ten minutes since last used, and two hours in all.
        problem = release.passphrase_held(agent_options())
        self.assertIn("default-cache-ttl 14400 and max-cache-ttl 14400", problem)
        problem = release.passphrase_held(agent_options("14400"))
        self.assertIn("set max-cache-ttl 14400 in", problem)

    def test_an_option_left_unset_is_read_as_its_default(self):
        # A gpg-agent whose own defaults were long enough would need no setting.
        options = agent_options().replace(":600::", ":14400::").replace(":7200::", ":14400::")
        self.assertIsNone(release.passphrase_held(options))

    def test_a_cache_ignored_for_signing_is_no_cache(self):
        problem = release.passphrase_held(agent_options("14400", "14400", ignore="1"))
        self.assertIn("ignore-cache-for-signing", problem)


if __name__ == "__main__":
    unittest.main()
