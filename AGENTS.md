# Backtrack — working agreements

## Task tracking: progress.md and the GitHub board move together

`backtrack_plan/progress.md` is the **single source of truth** for task state.
Every task (`S##-T#`) also exists as a GitHub issue on the **Backtrack** Project
board (<https://github.com/users/keithvassallomt/projects/2>), grouped by stage
milestone. The board is a mirror — never edit task *state* on the board by hand,
edit the checkbox in progress.md and let the sync push it.

**The contract — do this whenever task state changes:**

1. Update the checkbox in `progress.md` in the SAME COMMIT as the work
   (`[ ]` todo · `[/]` in progress · `[x]` done · `[!]` blocked). Only mark `[x]`
   when the stage file's acceptance criteria actually pass.
2. Run `just sync-board` (dry run) to preview, then `just sync-board-apply` to
   push the state to the board. This closes/reopens issues and moves Status
   columns automatically. It is idempotent — safe to run any time.

Mapping: `[ ]`→Todo, `[/]`→In Progress, `[x]`→Done (issue closed), `[!]`→Blocked.

**When new tasks are discovered** (scope grows mid-stage):
1. Add the task to `progress.md` with the next free `S##-T#` id for its stage,
   and add its `### S##-T# — Title` section (with an `**Accept:**` line) to the
   relevant `backtrack_plan/stages/stage-XX-*.md` file.
2. Run `just provision-board-apply` — it creates the missing issue(s) and skips
   everything that already exists.

**Comments / discussion** go directly on the GitHub issue (`gh issue comment`) —
those are richer than a checkbox and don't belong in progress.md.

Board tooling lives in `scripts/board.py`. Both commands default to a dry run;
nothing hits GitHub without `--execute` (or the `-apply` Just recipes).

## Tone

Chat/explanations follow `backtrack_plan/personality.md` (the "Kevin" voice).
Everything that persists — code, commits, docs, issues, UI copy — stays
strictly professional. The board and its issues are client-facing: professional.
