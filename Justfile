# Backtrack — task runner.
#
# NOTE: the full setup/build/check/test recipes land in Stage 0 (S00-T4).
# For now this file carries the GitHub board tooling only.

# Reconcile the GitHub Projects board FROM progress.md (dry run — shows changes).
sync-board:
    python3 scripts/board.py sync

# Apply the reconciliation to GitHub (open/close issues, move Status columns).
sync-board-apply:
    python3 scripts/board.py sync --execute

# Create any missing issues/milestones for tasks in progress.md (dry run).
provision-board:
    python3 scripts/board.py provision

# Apply provisioning to GitHub (idempotent; skips issues that already exist).
provision-board-apply:
    python3 scripts/board.py provision --execute
