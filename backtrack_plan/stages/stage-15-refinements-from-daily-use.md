# Stage 15 — Refinements from Daily Use

## Objective
Changes that came out of using Backtrack on a real computer every day. More
tasks join this stage as they are found.

The first: the timeline lists only the backups in which the folder being viewed
is different. Today the sidebar, the stepping buttons, the density strip and
the calendar offer every backup, so a folder that changed once in a week is
forty near-identical hourly rows to search through. The "next change" buttons
(S06-T4) ease that for a single file, but leave the list itself as long as it
was.

## The rule
For the folder being viewed, a backup is **listed** if it holds a version of
some file at or below that folder which is changed or gone in the next newer
backup. The newest backup is always listed.

In the index's terms (interval encoding, Stage 1): every version spans
`first_seq..=last_seq`, so the listed backups are the distinct `last_seq`
values of the versions under the folder, plus the newest catalogued backup.

Worked through, with A and B in the folder today and backups numbered from the
newest backwards:

| Backup | What it holds | Listed |
|---|---|---|
| 1 | A and B as they are now | Yes: the newest backup is always listed |
| 2 to 5 | A and B as they are now | No: backup 1 holds them |
| 6 | The newest copy of an earlier B | Yes |
| 7, 8 | The same earlier B | No: 6 holds it |
| 9 | A as now, and no B yet | No: a file that did not exist yet is nothing to restore |

A file deleted since is covered by the same rule: the last backup that still
held it is listed.

**What makes hiding safe:** every file version in a backup that is not listed
is also in the next newer listed backup, which may hold newer files besides.
Hiding a backup never hides anything that could be restored from it.

Decided with Keith, 2026-10-20:
- **Files and symlinks count; a folder's own record does not.** Adding a file
  changes its folder's modification time, so counting folders would list every
  backup in which a file was merely created, which is the case the rule exists
  to leave out. A deleted folder still counts through the files it held; a
  deleted empty folder does not, and that is accepted.
- **The whole subtree counts**, not only what is directly inside the folder.
  Otherwise a backup in which only `Taxes/2025/receipt.pdf` changed would be
  hidden while viewing Documents, and Documents as it was at that backup could
  not be restored. S15-T4 shows where in the subtree the difference is.
- **The list follows the folder, not the selected file.** A list that rewrote
  itself on every click in the file pane would be impossible to keep one's
  place in. The "next change" buttons go on serving the single file.
- **The density strip and the calendar follow the same list.** A gap in the
  strip then means that nothing changed in this folder, not that no backup
  ran. Missed backups are health's to report (Stage 10), and it does.

## Prerequisites
Stage 1 (interval encoding, the `versions_last_seq` index from schema v3),
Stage 6 (the time controls), S01-T6 (prune keeps every `last_seq` on an archive
that still exists).

## Order of work
T1 first: its timing on a real index decides how T2 asks the question. T2
before T3, since both read the list T2 introduces. T4 last.

T5 and T6 are defects found on 2026-10-20 while planning Stage 17, in what
users already have. They stand apart from T1 to T4 and can go first.

## Tasks

### S15-T1 — The backups that differ under a folder
A reader query in `backtrack-core` (`crates/backtrack-core/src/index/reader.rs`)
returning the listed backups for a folder, newest first, by the rule in this
stage file: the distinct `last_seq` of every non-folder version at or below the
folder, plus the newest catalogued backup. The tree root lists every catalogued
backup. Backups not yet catalogued have no versions, so they are not the
query's to report; the window adds them (S15-T2).

The query walks the subtree through `paths_parent_name`, as `sizes_inside`
already does. On the home folder that is every file the user has, so it is
timed, read-only, against Keith's own index before anything else is built. If
the home folder cannot be answered in under 250 ms, the answer is kept per
folder as backups are catalogued instead (a schema migration), and the timing
and the choice are recorded in the commit.

**Accept:** unit tests pin the stage file's worked example exactly; a file
deleted since lists the last backup that held it; a backup in which files were
only created is not listed; a backup in which only a folder's modification time
changed is not listed; a change three folders down is listed at every folder
above it; after a prune, every backup returned still exists; the tree root
lists every catalogued backup. The home folder's timing on Keith's index is
recorded.

### S15-T2 — The sidebar and the stepping list only those backups
The window's state (`crates/backtrack-gtk/src/state.rs`) gains the listed
backups for the folder being viewed, asked of the index off the main thread
whenever the folder or the set of backups changes.
- The sidebar groups the listed backups, and its group counts count them.
- Older and Newer, and Ctrl+← and Ctrl+→, step through the listed backups.
- Moving to a backup that is not listed lands on the nearest newer listed one,
  which by the rule shows every file the requested backup held. That covers
  walking into a folder where the current backup is not listed, Search's View
  in Timeline, "next change" towards the present (which lands on the first
  backup of the next version, not the last), and a new backup arriving after
  the one being viewed.
- Backups still being catalogued stay in the list, marked "cataloguing…" as
  now, since nothing is known yet about what they hold.
- The position label counts the listed backups, and its wording makes clear
  that it is counting backups in which this folder differs, not every backup.
- Until the list for a new folder arrives, the previous one stays on screen,
  so walking into a folder never flashes the full history.

**Accept:** unit tests on the state pin stepping and landing, including landing
from a backup the new folder does not list; on the demo repository, a folder
with a quiet history lists fewer backups than its parent, Older and Newer never
stop on a hidden backup, and walking into a subfolder from a backup it does not
list lands on the next newer listed one with the same files on screen.

### S15-T3 — The density strip and the calendar show the same backups
`crates/backtrack-gtk/src/ui/strip.rs` and `crates/backtrack-gtk/src/ui/calendar.rs`
draw from S15-T2's list instead of every backup.
- The strip's bars, its readout and its keyboard control (arrows, Home, End)
  cover the listed backups only. The strip now redraws when the folder changes;
  `state.rs`'s module comment, which says walking into a folder must not redraw
  it, is corrected.
- The calendar marks only the days with a listed backup, and picking a day
  goes to its newest listed backup.

**Accept:** unit tests on `model/density.rs` and `model/calendar.rs` fed a
listed subset; on the demo repository, for a folder with a quiet history the
strip and the calendar show only its listed backups, and both redraw on walking
into a subfolder.

### S15-T4 — A folder says when something inside it changed
With the whole subtree counting (S15-T1), a listed backup can show a folder
exactly as the next one does, because the difference is two folders down.
Today a folder's "changed since then" badge comes from the folder's own
modification time, which changes when an entry directly inside it is added or
removed: nothing a user can act on.

For a folder, "changed since then" comes to mean that something at or below it
in the backup being viewed is changed or gone in the newest backup, the same
test the rule applies, measured against the present. Following the badges from
any listed backup then leads to the files that make it different. Files keep
the badge as they have it now.

**Accept:** unit tests: a change three folders down badges every folder above
it in each listing; an entry added inside a folder, with nothing changed or
removed, does not badge it; on the demo repository, the badges lead from a
listed backup down to the file that differs, and a folder listing still answers
as quickly as before on Keith's index.

### S15-T5 — Preview and restore from the offline spool
Preview, the compare view and the restore pipeline ask the destination's
repository for every archive (`restore_preflight`, `preview_file` and
`compare_file` in `crates/backtrackd/src/service/mod.rs`). The spool's local
snapshots (Stage 5) are in the spool's own repository. So while the
destination is away, a restore from one fails with `RepoUnreachable`, and once
it is back, Borg answers that the archive does not exist. These are the
snapshots that exist so that work done away from the destination can be got
back, and on a stock desktop the spool is what takes them.
- Each of the three chooses its engine by the archive's kind: the
  destination's for `primary`, the spool's for `spool`. Stage 5's btrfs
  snapshots (`fs-snapshot`) are S17-T3's, which adds snapshot folders to the
  same choice.

**Accept:**
`a_local_snapshot_is_previewed_and_restored_from_while_the_destination_is_away`
and `a_local_snapshot_is_previewed_and_restored_from_once_the_destination_is_back`
in `crates/backtrackd/src/service/e2e.rs` pass with their `#[ignore]` removed
(as written on 2026-10-20 they fail: an empty preview, then `RepoUnreachable`
or "Archive … does not exist"); a compare from a spool snapshot reads the
spool's copy. On a VM, with the destination unplugged, a file edited and then
lost is restored from its local snapshot through the window.

### S15-T6 — A preview that could not be fetched is an error, not an empty file
`extract_stdout` (`crates/backtrack-core/src/engine/borg/mod.rs`) discards
Borg's error output and never reads its exit status, so a failed extraction
reads as an empty file. The preview (`preview_file`) and the compare view
(`cache_extraction`) then keep that empty file in the preview cache as though
it were the real one, and go on serving it after the cause has gone. A laptop
away from its destination shows every file previewed as empty, and they stay
empty when it is back.
- The reader reports Borg's failure once the output ends, with its exit code
  and error output classified as every other Borg call's are.
- Nothing reaches the cache unless the extraction succeeded.
- The preview pane and the compare view say what went wrong instead of showing
  an empty file.
- Empty files already cached by this defect are cleared at every start of
  the daemon. Not once: the cache counts every file in its folder as an
  entry, so a marker saying it had been done would be evicted with the rest,
  and clearing at every start costs only extracting a genuinely empty file
  again.

**Accept:** `a_preview_that_could_not_be_fetched_is_not_kept_as_an_empty_file`
in `crates/backtrackd/src/service/e2e.rs` passes with its `#[ignore]` removed
(as written on 2026-10-20 it fails: an empty preview while the destination is
away, and still empty once it is back); a Borg engine test: `extract_stdout`
from a missing archive is an error, not an empty read; a test that the clear
removes the empty entries and keeps the rest; on a VM, previewing with the destination unplugged shows the
reason, and plugging it back in shows the file.

## Definition of Done
All tasks done; the CHANGELOG's Unreleased section says what changed for the
user; the timeline browser on the GNOME VM behaves as described on the demo
repository.
