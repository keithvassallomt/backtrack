#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>
"""A version's section of CHANGELOG.md, as release notes.

    release_notes.py VERSION              the section's entries, as Markdown,
                                          each on one line
    release_notes.py --appstream VERSION  the same, as an AppStream <release>

The release workflow (S13-T5) puts the first on the GitHub release, and the
FriendlyHub files (S14-T9) put the second in the metainfo, so what a user
reads about a release comes from one place. A version whose section is still
dated TBD, or missing, is refused: there is nothing yet to release. A
pre-release (0.2.0-rc1) without a section of its own, which Keep a Changelog
does not give one, takes the [Unreleased] entries, dated today.
"""

import re
import sys
from datetime import date as today
from pathlib import Path
from xml.sax.saxutils import escape

CHANGELOG = Path(__file__).resolve().parent.parent / "CHANGELOG.md"
HEADING = re.compile(r"^## \[(?P<version>[^\]]+)\](?: - (?P<date>\S+))?\s*$")
DATE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
# Keep a Changelog's footer, the link each heading's version stands for
# ("[0.1.0]: https://…"): after the last section, and part of none.
LINK_REFERENCE = re.compile(r"^\[[^\]]+\]:\s*\S+")
# A Markdown link, [text](target).
LINK = re.compile(r"\[([^\]]+)\]\(([^)\s]+)\)")


class NotReleased(Exception):
    pass


def section(text, version):
    """The date and body of `version`'s section."""
    lines = text.splitlines()
    found = next(
        (
            (start, heading)
            for start, heading in enumerate(map(HEADING.match, lines))
            if heading and heading.group("version") == version
        ),
        None,
    )
    if found is None:
        raise NotReleased(f"CHANGELOG.md has no section for {version}")
    start, heading = found
    date = heading.group("date") or ""
    if not DATE.match(date):
        undated = date or "no date"
        raise NotReleased(f"CHANGELOG.md's section for {version} is not dated ({undated})")
    return date, body_from(lines[start + 1 :])


def body_from(lines):
    """A section's body: its lines up to the next version's heading, or the
    links at the foot of the file."""
    body = []
    for line in lines:
        if HEADING.match(line) or LINK_REFERENCE.match(line):
            break
        body.append(line)
    return "\n".join(body).strip()


def notes(text, version):
    """The date and body a release of `version` carries: its own dated
    section, or for a pre-release without one, [Unreleased] as of today."""
    try:
        return section(text, version)
    except NotReleased:
        if "-" not in version:
            raise
    lines = text.splitlines()
    start = next((i for i, line in enumerate(lines) if line.strip() == "## [Unreleased]"), None)
    if start is None:
        raise NotReleased(f"CHANGELOG.md has no section for {version}, and no [Unreleased]")
    return today.today().isoformat(), body_from(lines[start + 1 :])


def released(text, up_to):
    """Every dated version from `up_to` back, newest first, as (version, date,
    body): the history an AppStream listing carries."""
    versions = [
        heading.group("version")
        for heading in map(HEADING.match, text.splitlines())
        if heading and DATE.match(heading.group("date") or "")
    ]
    if up_to not in versions:
        section(text, up_to)  # raises, saying why
    return [(v, *section(text, v)) for v in versions[versions.index(up_to) :]]


def entries(body):
    """The body's categories, each with its entries, continuation lines joined."""
    categories = []
    for line in body.splitlines():
        if line.startswith("### "):
            categories.append((line[4:].strip(), []))
        elif line.startswith("- ") and categories:
            categories[-1][1].append(line[2:].strip())
        elif line.strip() and categories and categories[-1][1]:
            categories[-1][1][-1] += " " + line.strip()
    return [(name, items) for name, items in categories if items]


def markdown(body):
    """The body as a GitHub release shows it: each entry on one line. GitHub
    breaks a release's notes, as a comment, wherever the text breaks, so an
    entry wrapped in CHANGELOG.md would read as a narrow column."""
    out = []
    for name, items in entries(body):
        out += [f"### {name}", "", *(f"- {item}" for item in items), ""]
    return "\n".join(out).strip()


def inline(text):
    """Markdown emphasis and code, in the markup AppStream allows; a link,
    which it does not allow, as its text."""
    text = escape(LINK.sub(r"\1", text))
    text = re.sub(r"`([^`]+)`", r"<code>\1</code>", text)
    text = re.sub(r"\*\*([^*]+)\*\*", r"<em>\1</em>", text)
    return re.sub(r"\*([^*]+)\*", r"<em>\1</em>", text)


def appstream(version, date, body):
    """`version`'s <release> element."""
    out = [f'<release version="{escape(version)}" date="{date}">', "  <description>"]
    for name, items in entries(body):
        out.append(f"    <p>{escape(name)}</p>")
        out.append("    <ul>")
        out.extend(f"      <li>{inline(item)}</li>" for item in items)
        out.append("    </ul>")
    out += ["  </description>", "</release>"]
    return "\n".join(out)


def main(argv):
    as_appstream = "--appstream" in argv
    rest = [arg for arg in argv if arg != "--appstream"]
    if len(rest) != 1:
        sys.exit(__doc__)
    version = rest[0].removeprefix("v")
    try:
        date, body = notes(CHANGELOG.read_text(), version)
    except NotReleased as error:
        sys.exit(f"release_notes: {error}")
    print(appstream(version, date, body) if as_appstream else markdown(body))


if __name__ == "__main__":
    main(sys.argv[1:])
