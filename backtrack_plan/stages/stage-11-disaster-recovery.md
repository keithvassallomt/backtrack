# Stage 11 — Disaster Recovery ("Restore Everything")

## Objective
The fifth user story: laptop dies, new machine, everything back — guided,
resumable, honest about progress. Entered from the wizard's import path.

## Prerequisites
Stages 4 (backfill), 7 (restore pipeline), 9 (wizard/import). **Normative
references:** prototype.md Screen 12 wireframes; mockups
[21](../mockups/21-dr-welcome.png), [22](../mockups/22-dr-progress.png).

## Tasks

### S11-T1 — RestoreEverything job
Core: given archive seq + policy → enumerate top-level dirs of the backup root →
one sub-job per top-level dir (Documents, Pictures, …) through the Stage 7
staging pipeline, executed serially (repo read contention; simpler resume), with
a persisted job manifest (`~/.local/share/backtrack/dr-job.json`: archive, dir
list, per-dir status, byte totals from index). Resume: daemon restart mid-DR →
job continues from first incomplete dir (staging dir of the interrupted sub-job
discarded and redone). Pause = finish current file, hold (manifest persists).
Progress: bytes-restored / index-total (honest ETA: rolling rate over 60 s).
**Accept:** integration: DR of demo home; kill daemon at 50% → restart resumes and
completes; final tree hash-identical to archive content; ETA within sanity bounds
in logs.

### S11-T2 — Entry dialog
Per mockup 21, after ImportRepo succeeds in the wizard (and newest archive is
indexed): "Welcome back. Restore this computer?" — Restore everything
(recommended; snapshot dropdown listing recent archives), Restore selected
folders… (checklist of top-level dirs with sizes), Just browse. Info line copy
verbatim (never-overwrite promise, backups-start-after note).
**Accept:** all three paths function; snapshot dropdown drives the job's archive.

### S11-T3 — Progress UI
Per mockup 22: window with overall bar, "61% · 72 of 118 GB · about 40 min left",
current file line (rate-limited), per-top-dir checklist (done/current/pending),
Pause/Cancel (cancel confirms: "Keep what's restored so far / Discard"). Window
closable — job continues in daemon; reopening the app reattaches (GetStatus
exposes active DR job). Completion notification + summary toast.
**Accept:** close-and-reattach mid-run; pause/resume; cancel-keep leaves completed
dirs intact and manifest cleaned.

### S11-T4 — Post-restore handoff
Conflict policy during DR: existing-different files go through ONE summary
(Stage 7 folder machinery) per the entry dialog's promise — on a fresh machine
this is near-empty; skip UI entirely when zero conflicts. Backup schedule stays
disabled during DR (manifest presence blocks scheduler) and enables automatically
on completion; first backup after DR runs within 5 min (it should be nearly
no-op thanks to files cache… but it validates the loop).
**Accept:** scheduler provably paused during DR (mock clock test) and first
post-DR backup succeeds; conflict summary appears when the target home was
seeded with a modified file.

## Definition of Done
Full drill on a scratch `BACKTRACK_DEV` home: wizard → Import → Restore
everything → interrupt+resume → completion → automatic first backup — documented
as a walkthrough in the repo docs with timings; progress.md + CHANGELOG updated.
