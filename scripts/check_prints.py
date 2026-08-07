#!/usr/bin/env python3
"""Guard against printing where logging is wanted.

The daemon and the library must speak through `tracing`: their output goes to a
rotating JSONL log that `backtrack doctor` collects, and anything written
straight to stdout or stderr is lost to whoever is trying to work out what
happened. `backtrack-cli` is exempt — a command-line tool's output *is* its
product, and its errors belong on the user's terminal rather than in a log file
they have not opened.

Test code is exempt everywhere. A test that skips because the machine cannot do
what it needs — no btrfs, no permission enforcement when running as root — has
to say so, and it has no logging subscriber to say it through.

This replaces an inline `grep ... | grep -v tests?` in the CI workflow, which
excluded any line whose *text* happened to contain "test". That is not the same
question as "is this test code", and the difference was not academic: it passed
only while no test-module print existed that lacked the word, and it flagged
`backtrack-cli`'s legitimate error output from the day that was written.
"""

import pathlib
import re
import sys

# `print!`/`eprint!` are the same sin as their `ln` cousins and were missed by
# the pattern this replaces.
FORBIDDEN = re.compile(r"\b(e?print!|e?println!|dbg!)")
CFG_TEST = re.compile(r"#\[cfg\((all\()?test\b")
FILE_CFG_TEST = re.compile(r"#!\[cfg\((all\()?test\b")

# Crates whose job is to write to a terminal.
EXEMPT_CRATES = {"backtrack-cli"}


def offending_lines(text: str) -> list[tuple[int, str]]:
    """Lines using a print macro outside `#[cfg(test)]` code."""
    lines = text.splitlines()

    # A file-level `#![cfg(test)]` / `#![cfg(all(test, ...))]` makes the whole
    # file test code.
    for line in lines[:40]:
        if FILE_CFG_TEST.search(line):
            return []

    found: list[tuple[int, str]] = []
    # Depth of the innermost `#[cfg(test)]` item we are inside, or None.
    test_depth: int | None = None
    depth = 0
    pending_cfg_test = False

    for number, line in enumerate(lines, start=1):
        stripped = line.strip()

        if CFG_TEST.search(stripped):
            pending_cfg_test = True

        if test_depth is None and FORBIDDEN.search(line) and not stripped.startswith("//"):
            found.append((number, stripped))

        opens = line.count("{")
        closes = line.count("}")
        if pending_cfg_test and opens:
            # The attribute's item has started; everything to its matching close
            # brace is test code.
            test_depth = depth
            pending_cfg_test = False
        depth += opens - closes
        if test_depth is not None and depth <= test_depth:
            test_depth = None

    return found


def main() -> int:
    root = pathlib.Path(__file__).resolve().parent.parent
    problems: list[str] = []
    checked = 0

    for path in sorted((root / "crates").rglob("*.rs")):
        if any(part in EXEMPT_CRATES for part in path.parts):
            continue
        checked += 1
        for number, line in offending_lines(path.read_text(encoding="utf-8")):
            problems.append(f"{path.relative_to(root)}:{number}: {line}")

    if checked == 0:
        print("check-prints: found no source files to check — this is a bug", file=sys.stderr)
        return 1

    if problems:
        print("Printing where tracing is wanted:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        print(
            "\nUse tracing (info!/warn!/debug!) instead. Test code and "
            "backtrack-cli are exempt.",
            file=sys.stderr,
        )
        return 1

    print(f"check-prints: {checked} files clean.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
