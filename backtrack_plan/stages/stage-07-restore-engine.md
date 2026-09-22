# Stage 7 — Restore Engine & Conflict UX

## Objective
Restores that are safer than Time Machine's: staging → compare → atomic move,
skip-identical, Keep Both, a 30-day safety stash for anything replaced, Undo,
and folder restores as one summary instead of dialog storms.

## Prerequisites
Stages 0–3, 6. **Normative references:**
[../reference/open-questions.md](../reference/open-questions.md) Q4 (the researched
design + exact dialog wording), prototype.md Screens 4–5 + 7 wireframes, mockups
[4](../mockups/4-conflict-dialog.png), [5](../mockups/5-folder-restore-summary.png),
[8](../mockups/8-review-checklist.png).

## Tasks

### S07-T1 — Core pipeline
In backtrack-core: `plan_restore(archive, paths, dest) -> RestorePlan` then
`execute(plan, decisions) -> JobStream`. Plan phase: extract targets to
`staging/<job-id>/` (borg extract with prefix), walk staging vs destination →
per-file classification: `Identical` (size+mtime, hash tie-break), `OnlyInBackup`,
`OnlyOnDisk`, `Conflict{disk_newer: bool}`, `TypeChanged`, `SymlinkEntry`.
Execute: apply decisions — moves are atomic `rename()` (same-fs staging; if
staging can't be same-fs, copy+fsync+rename fallback), replaced disk files first
moved to `replaced/<timestamp>/<original-path>` preserving metadata. Symlinks
restored as links, never followed (path-traversal test mandatory). Per-file
permission errors collected, not fatal. Free-space preflight (staging+stash ≈ 2×).
**Accept:** unit matrix over fixture trees covering every classification;
traversal test (malicious symlink in archive cannot escape dest); crash-mid-execute
leaves either old or new file at every path (no partials — rename atomicity test).

### S07-T2 — Merge semantics
Folder restores merge: `OnlyOnDisk` files are ALWAYS kept (brief.md fear #1);
`Identical` silently skipped; summary counts computed from the plan. "Keep Both"
resolution renames the CURRENT file to `name (current).ext` (collision → ` (current 2)`)
and the backup version takes the real name — restore-tool semantics per research
(inverse of copy dialogs; rationale in open-questions.md Q4).
**Accept:** merge property test: no execution may ever delete or rename an
OnlyOnDisk file except explicit Keep-Both renames of conflicting paths.

### S07-T3 — Single-file conflict dialog
`AdwAlertDialog` per mockup 4: title `Replace "name"?`; body states which side is
NEWER (bolder when disk is newer); comparison box with both versions' icon,
date, size, orange `newer` tag; caption "Replaced files are kept as safety copies
for 30 days."; buttons Cancel / Keep Both / Replace — Replace `destructive-action`
and never default; Cancel first. Identical files never reach this dialog.
**Accept:** screenshot matches mockup 4 layout; disk-newer vs backup-newer copy
variants both rendered (unit test on the copy selection).

### S07-T4 — Folder summary + review
Summary dialog per mockup 5 (counts, "Nothing is ever deleted", safety-copy line,
Cancel / Keep Both Versions / Replace Changed Files in red). "Review…" opens the
checklist (mockup 8): checkbox per conflict, both versions' dates+sizes,
"newer on disk" tags, Select All/None, button counts selections
("Replace 5 Files"). Unchecked = skipped.
**Accept:** Bob walkthrough on demo data produces the mockup-5 numbers for a
scripted fixture; review de-selections carried into execution.

### S07-T5 — Stash
`replaced/` layout as in stack.md; expiry job (30 days, size-capped 5 GB — evict
oldest); "Recently Replaced Files" window listing stash entries with per-entry
Restore (= put back) and the primary-menu entry wired. Stash writes preserve
mtime/mode.
**Accept:** replace → entry appears with correct metadata; expiry (mock clock)
removes it; put-back restores byte-identical file.

### S07-T6 — Undo
After any restore job: toast "Restored <name> — Undo" (multi-file: "Restored N
files — Undo"). Undo = revert the job's move log (restored files removed or
replaced by their stash copies; Keep-Both renames reverted). Toast lifetime 10 s;
job move-log kept until next restore or app exit.
**Accept:** restore → Undo → tree byte-identical to before (hash the fixture tree).

### S07-T7 — Restore To…
Folder picker (portal-based `FileDialog` — must work under Flatpak later);
restoring elsewhere bypasses conflict UI entirely (fresh dir guaranteed by
creating a subfolder `Restored <name> <date>/`).
**Accept:** restore-elsewhere of a folder from demo repo; no dialogs; correct tree.

### S07-T8 — The demo fixture has to be restorable
`just demo-repo` named its source relatively (`borg create … home`, from inside
the fixture directory), so the archives' members were `home/Documents/…` —
paths that match nothing on the machine. Browsable, which is all Stage 6 asked
of them, and impossible to restore: an in-place restore reproduces the member
path under `/`, so Alice's deleted folder would have been aimed at
`/home/old-client-folder`. It also made a second, disjoint tree inside one
index, because the daemon's own backups of those same files are stored under
their absolute path — stepping from a real backup into the scripted history
reported "not in this backup" for every folder in it. Name the source
absolutely so the two halves share a root.
**Accept:** after `just demo-repo` and one daemon backup the index holds a
single path root, and the scripted history's paths are the ones the daemon
itself writes.

## Definition of Done
Alice + Bob + Dave stories complete end-to-end in the GUI on demo-repo;
the property/atomicity/traversal tests green in CI; progress.md + CHANGELOG
("Added: restore with conflict handling, safety copies and Undo").
