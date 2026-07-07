#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>
"""Backtrack GitHub Projects board tooling.

Single source of truth for *task state* is backtrack_plan/progress.md. This
script mirrors that state onto a GitHub Projects (v2) board so the two never
drift by hand:

  provision  one-time (idempotent) setup: project, Status field, per-stage
             milestones, one issue per S##-T# task, all added to the board.
  sync       reconcile the board FROM progress.md: issue open/closed state and
             the Status column (Todo / In Progress / Done / Blocked).

Nothing mutates unless you pass --execute. Default is a dry run that prints
exactly what would happen.

Checkbox -> Status mapping (progress.md is authoritative):
  [ ] Todo   [/] In Progress   [x] Done   [!] Blocked
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path
from dataclasses import dataclass

REPO = "keithvassallomt/backtrack"
OWNER = "keithvassallomt"
PROJECT_TITLE = "Backtrack"

ROOT = Path(__file__).resolve().parent.parent
PLAN = ROOT / "backtrack_plan"
PROGRESS = PLAN / "progress.md"
STAGES_DIR = PLAN / "stages"

# checkbox char -> (Status option name, GitHub option colour, closed?)
STATUS = {
    " ": ("Todo", "GRAY", False),
    "/": ("In Progress", "YELLOW", False),
    "x": ("Done", "GREEN", True),
    "!": ("Blocked", "RED", False),
}

TASK_LINE = re.compile(r"^- \[(.)\] (S(\d+)-T\d+)\s+(.*)$")
TASK_HEADER = re.compile(r"^### (S\d+-T\d+) — (.+)$")
ISSUE_TITLE_ID = re.compile(r"^(S\d+-T\d+)\b")


@dataclass
class Task:
    tid: str            # S00-T3
    stage: int          # 0
    short_title: str    # from progress.md
    state: str          # ' ', '/', 'x', '!'
    body: str           # full section from the stage file


# ── data ────────────────────────────────────────────────────────────────────

def stage_files() -> dict[int, Path]:
    out = {}
    for p in sorted(STAGES_DIR.glob("stage-*.md")):
        m = re.match(r"stage-(\d+)-", p.name)
        if m:
            out[int(m.group(1))] = p
    return out


def stage_titles(files: dict[int, Path]) -> dict[int, str]:
    titles = {}
    for n, p in files.items():
        first = p.read_text().splitlines()[0]
        titles[n] = first.lstrip("# ").strip()  # "Stage 0 — Bootstrap"
    return titles


def parse_stage_bodies(path: Path) -> dict[str, str]:
    """Map S##-T# -> full markdown section (header excluded)."""
    text = path.read_text()
    bodies: dict[str, str] = {}
    current, buf = None, []
    for line in text.splitlines():
        h = TASK_HEADER.match(line)
        if h:
            if current:
                bodies[current] = "\n".join(buf).strip()
            current, buf = h.group(1), []
            continue
        if line.startswith("## ") and current:
            bodies[current] = "\n".join(buf).strip()
            current, buf = None, []
            continue
        if current is not None:
            buf.append(line)
    if current:
        bodies[current] = "\n".join(buf).strip()
    return bodies


def load_tasks() -> list[Task]:
    files = stage_files()
    bodies = {n: parse_stage_bodies(p) for n, p in files.items()}
    tasks: list[Task] = []
    for line in PROGRESS.read_text().splitlines():
        m = TASK_LINE.match(line)
        if not m:
            continue
        state, tid, stage_s, short = m.group(1), m.group(2), m.group(3), m.group(4)
        if state not in STATUS:
            fail(f"{tid}: unknown checkbox state '[{state}]' in progress.md")
        stage = int(stage_s)
        body = bodies.get(stage, {}).get(tid, "")
        if not body:
            warn(f"{tid}: no matching section found in stage file")
        tasks.append(Task(tid, stage, short.strip(), state, body))
    return tasks


# ── gh helpers ──────────────────────────────────────────────────────────────

DRY = True


def warn(msg: str) -> None:
    print(f"  ! {msg}", file=sys.stderr)


def fail(msg: str) -> None:
    print(f"ERROR: {msg}", file=sys.stderr)
    sys.exit(1)


def gh(args: list[str], mutating: bool, capture: bool = True) -> str | None:
    """Run a gh command. In dry mode, mutating calls are printed, not run."""
    if mutating and DRY:
        print(f"  [dry] gh {' '.join(args)}")
        return None
    try:
        res = subprocess.run(
            ["gh", *args], check=True,
            capture_output=capture, text=True,
        )
    except subprocess.CalledProcessError as e:
        fail(f"gh {' '.join(args)} failed:\n{e.stderr}")
    return res.stdout if capture else None


def gh_json(args: list[str], mutating: bool = False):
    out = gh(args, mutating=mutating)
    return json.loads(out) if out else None


def graphql(query: str, mutating: bool, **variables) -> dict | None:
    args = ["api", "graphql", "-f", f"query={query}"]
    for k, v in variables.items():
        args += ["-F", f"{k}={v}"]
    out = gh(args, mutating=mutating)
    return json.loads(out) if out else None


# ── board objects ───────────────────────────────────────────────────────────

def find_project() -> dict | None:
    data = gh_json(["project", "list", "--owner", OWNER, "--format", "json"])
    for p in (data or {}).get("projects", []):
        if p["title"] == PROJECT_TITLE:
            return p
    return None


def ensure_project() -> dict:
    proj = find_project()
    if proj:
        print(f"• project '{PROJECT_TITLE}' exists (#{proj['number']})")
        return proj
    print(f"• creating project '{PROJECT_TITLE}'")
    gh(["project", "create", "--owner", OWNER, "--title", PROJECT_TITLE], mutating=True)
    if DRY:
        return {"number": 0, "id": "<project-id>"}
    proj = find_project()
    gh(["project", "link", str(proj["number"]), "--owner", OWNER, "--repo", REPO],
       mutating=True)
    return proj


def ensure_status_field(project_number: int, project_id: str) -> tuple[str, dict[str, str]]:
    """Ensure the Status single-select has our 4 options. Returns (fieldId, {name:optionId})."""
    if DRY and project_number == 0:
        print("  [dry] would set Status options: Todo / In Progress / Done / Blocked")
        return "<status-field-id>", {n: f"<{n}-id>" for n, _, _ in STATUS.values()}

    fields = gh_json(["project", "field-list", str(project_number),
                      "--owner", OWNER, "--format", "json"])
    status = next((f for f in fields["fields"] if f["name"] == "Status"), None)
    if not status:
        fail("built-in Status field not found on project")

    have = {o["name"] for o in status.get("options", [])}
    want = [name for name, _, _ in STATUS.values()]
    if set(want) <= have:
        print("• Status options already complete")
    else:
        print(f"• updating Status options -> {', '.join(want)}")
        opts = ", ".join(
            f'{{name: "{name}", color: {color}, description: ""}}'
            for name, color, _ in STATUS.values()
        )
        mutation = (
            "mutation($field: ID!) { updateProjectV2Field(input: {fieldId: $field, "
            f"singleSelectOptions: [{opts}]}}) {{ projectV2Field {{ "
            "... on ProjectV2SingleSelectField { id } } } }"
        )
        graphql(mutation, mutating=True, field=status["id"])

    # re-read to get fresh option ids
    fields = gh_json(["project", "field-list", str(project_number),
                      "--owner", OWNER, "--format", "json"])
    status = next(f for f in fields["fields"] if f["name"] == "Status")
    ids = {o["name"]: o["id"] for o in status["options"]}
    return status["id"], ids


def ensure_milestones(files: dict[int, Path], titles: dict[int, str]) -> dict[int, int]:
    existing = gh_json(["api", f"repos/{REPO}/milestones?state=all&per_page=100"]) or []
    by_title = {m["title"]: m["number"] for m in existing}
    out: dict[int, int] = {}
    for n in sorted(files):
        title = f"Stage {n:02d} — {titles[n].split('—', 1)[-1].strip()}"
        if title in by_title:
            out[n] = by_title[title]
            continue
        print(f"• creating milestone '{title}'")
        res = gh_json(["api", f"repos/{REPO}/milestones", "-f", f"title={title}",
                       "-f", f"description=Backtrack {titles[n]}"], mutating=True)
        out[n] = res["number"] if res else -n
    return out


def existing_issues() -> dict[str, dict]:
    data = gh_json(["issue", "list", "--repo", REPO, "--state", "all",
                    "--limit", "300", "--json", "number,title,state,url"]) or []
    out = {}
    for it in data:
        m = ISSUE_TITLE_ID.match(it["title"])
        if m:
            out[m.group(1)] = it
    return out


# ── commands ────────────────────────────────────────────────────────────────

def cmd_provision(only_stage: int | None = None) -> None:
    tasks = load_tasks()
    files = stage_files()
    titles = stage_titles(files)
    if only_stage is not None:
        tasks = [t for t in tasks if t.stage == only_stage]
        files = {only_stage: files[only_stage]}
        print(f"Filtering to Stage {only_stage:02d} only.")
    print(f"Loaded {len(tasks)} tasks across {len(files)} stage(s).\n")

    proj = ensure_project()
    pnum, pid = proj["number"], proj["id"]
    status_field_id, opt_ids = ensure_status_field(pnum, pid)
    milestones = ensure_milestones(files, titles)
    issues = existing_issues()

    created = skipped = 0
    for t in tasks:
        name, _, _ = STATUS[t.state]
        if t.tid in issues:
            skipped += 1
            continue
        milestone_title = f"Stage {t.stage:02d} — {titles[t.stage].split('—', 1)[-1].strip()}"
        title = f"{t.tid} — {t.short_title}"
        body = f"{t.body}\n\n---\nStage {t.stage:02d} · task `{t.tid}` · tracked in "\
               f"[progress.md](https://github.com/{REPO}/blob/main/backtrack_plan/progress.md)"
        print(f"• issue: {title}  [{name}]")
        url = gh(["issue", "create", "--repo", REPO, "--title", title,
                  "--body", body, "--milestone", milestone_title], mutating=True)
        created += 1
        if DRY:
            continue
        url = url.strip().splitlines()[-1]
        item = gh_json(["project", "item-add", str(pnum), "--owner", OWNER,
                        "--url", url, "--format", "json"], mutating=True)
        gh(["project", "item-edit", "--id", item["id"], "--project-id", pid,
            "--field-id", status_field_id,
            "--single-select-option-id", opt_ids[name]], mutating=True)

    print(f"\nprovision: {created} to create, {skipped} already present.")
    if DRY:
        print("(dry run — nothing changed. re-run with --execute to apply.)")


def cmd_sync() -> None:
    tasks = load_tasks()
    proj = find_project()
    if not proj:
        fail("project not found — run `provision` first")
    pnum, pid = proj["number"], proj["id"]
    status_field_id, opt_ids = ensure_status_field(pnum, pid)
    issues = existing_issues()

    # map issue number -> project item id + current status
    items = gh_json(["project", "item-list", str(pnum), "--owner", OWNER,
                     "--limit", "200", "--format", "json"]) or {"items": []}
    item_by_num = {}
    for it in items.get("items", []):
        content = it.get("content", {})
        if content.get("type") == "Issue":
            item_by_num[content["number"]] = it

    changes = 0
    for t in tasks:
        name, _, closed = STATUS[t.state]
        iss = issues.get(t.tid)
        if not iss:
            warn(f"{t.tid}: no issue on GitHub (run provision)")
            continue
        # open/closed
        want_state = "CLOSED" if closed else "OPEN"
        if iss["state"].upper() != want_state:
            changes += 1
            action = "close" if closed else "reopen"
            print(f"• {t.tid}: {action} issue #{iss['number']}")
            gh(["issue", action, str(iss["number"]), "--repo", REPO], mutating=True)
        # status column
        item = item_by_num.get(iss["number"])
        if item is None:
            print(f"• {t.tid}: add issue #{iss['number']} to board [{name}]")
            if not DRY:
                added = gh_json(["project", "item-add", str(pnum), "--owner", OWNER,
                                 "--url", iss["url"], "--format", "json"], mutating=True)
                item = {"id": added["id"], "status": None}
        cur = (item or {}).get("status")
        if item and cur != name:
            changes += 1
            print(f"• {t.tid}: status {cur!r} -> {name!r}")
            gh(["project", "item-edit", "--id", item["id"], "--project-id", pid,
                "--field-id", status_field_id,
                "--single-select-option-id", opt_ids[name]], mutating=True)

    print(f"\nsync: {changes} change(s).")
    if DRY:
        print("(dry run — nothing changed. re-run with --execute to apply.)")


def main() -> None:
    global DRY
    ap = argparse.ArgumentParser(description="Backtrack GitHub board tooling")
    ap.add_argument("command", choices=["provision", "sync"])
    ap.add_argument("--execute", action="store_true",
                    help="actually mutate GitHub (default is a dry run)")
    ap.add_argument("--stage", type=int, default=None,
                    help="provision only this stage number (smoke test)")
    args = ap.parse_args()
    DRY = not args.execute
    mode = "EXECUTE" if args.execute else "DRY RUN"
    print(f"=== board {args.command} ({mode}) ===\n")
    if args.command == "provision":
        cmd_provision(only_stage=args.stage)
    else:
        cmd_sync()


if __name__ == "__main__":
    main()
