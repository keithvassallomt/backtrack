# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>
"""docs/snapper.md and backtrack-snapper-setup give the same commands.

The guide says what the script does, step by step, for anyone who wants to
know or to do it by hand (S17-T5). Each was checked on a machine of its
distribution, so the two drifting apart would leave one of them untested:
every command in the guide must be one the script runs there, and every
command the script runs must be in the guide.
"""

import re
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GUIDE = ROOT / "docs" / "snapper.md"
SCRIPT = ROOT / "packaging" / "snapper" / "backtrack-snapper-setup"

# The distributions, as the script names them and as the guide's headings do.
DISTROS = {
    "fedora": "Fedora",
    "opensuse": "openSUSE",
    "arch": "Arch Linux",
    "ubuntu": "Ubuntu",
}

# Commands the guide offers for looking, which the script has no need of.
ONLY_IN_THE_GUIDE = {
    "stat -f -c %T /home",
    "stat -c %i /home",
    "systemctl list-timers 'snapper-*'",
    "systemctl list-timers snapper-timeline.timer",
    "df -h /home",
    "snapper -c home delete 42",
    "ls /etc/snapper/configs",
    "backtrack-snapper-setup",
}


def guide_commands():
    """The guide's commands, as (distribution or None for all, command).

    A block is one distribution's when it is under that distribution's
    heading (`### Fedora`), or follows a line naming it (`On Fedora:`), which
    names the next block only.
    """
    commands = []
    under = None
    named = None
    in_block = False
    block = None
    for line in GUIDE.read_text().splitlines():
        if in_block:
            if line.startswith("```"):
                in_block = False
            elif line.strip():
                commands.append((block, line.strip()))
            continue
        if line.startswith("```sh"):
            in_block = True
            block = named or under
            named = None
            continue
        heading = re.match(r"^#+ (.*)$", line)
        if heading:
            under = _distro(heading.group(1))
            named = None
            continue
        on = re.match(r"^On (.*):$", line)
        if on:
            named = _distro(on.group(1))
    return commands


def _distro(name):
    return next((key for key, shown in DISTROS.items() if shown == name), None)


def documented():
    """The distributions the guide has an install section for."""
    text = GUIDE.read_text()
    return [key for key, name in DISTROS.items() if f"\n### {name}\n" in text]


def script_commands(distro):
    """What the script runs on `distro`, setting up then undoing."""
    out = subprocess.run(
        ["bash", str(SCRIPT), "--print", distro],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return [line for line in out.splitlines() if line.strip()]


class GuideAndScript(unittest.TestCase):
    def test_every_command_in_the_guide_is_one_the_script_runs(self):
        for distro in documented():
            runs = set(script_commands(distro))
            for which, command in guide_commands():
                if which not in (None, distro) or command in ONLY_IN_THE_GUIDE:
                    continue
                with self.subTest(distro=distro, command=command):
                    self.assertIn(command, runs)

    def test_every_command_the_script_runs_is_in_the_guide(self):
        for distro in documented():
            given = {c for which, c in guide_commands() if which in (None, distro)}
            for command in script_commands(distro):
                with self.subTest(distro=distro, command=command):
                    self.assertIn(command, given)

    def test_the_guide_covers_fedora(self):
        self.assertIn("fedora", documented())

    def test_a_distribution_the_script_does_not_know_is_refused(self):
        run = subprocess.run(
            ["bash", str(SCRIPT), "--print", "debian"],
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(run.returncode, 0)
        self.assertEqual(run.stdout, "")


if __name__ == "__main__":
    unittest.main()
