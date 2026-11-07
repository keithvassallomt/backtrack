# Backtrack's documentation

## Guides

Each guide walks you through one task in Backtrack, step by step, and says
what you should see after each step. They are listed in the order you are
likely to need them.

- [Install Backtrack](installing.md)
- [Go back through a folder's history with Rewind](rewind.md)
- [Get back a file you deleted between two backups](trash.md)
- [Take snapshots between backups with Snapper](snapper.md)
- [Add Backtrack to your file manager's menu](file-manager-integration.md)

## For developers

These notes explain how parts of Backtrack work, and how they are checked by
hand. The plan for every stage, the architecture and the design notes are in
[backtrack_plan](../backtrack_plan/README.md).

- [How the file manager items work](file-manager-integration-internals.md)
- [The tray icon, and Backtrack running in the background](tray-and-background.md)
- [The development VMs](development-vms.md)
- [The disaster recovery drill](disaster-recovery-drill.md)
- [Taking the screenshots](screenshots.md)
- [The Borg adapter's design](superpowers/specs/2026-07-08-stage-02-borg-adapter-design.md)
  and [its implementation plan](superpowers/plans/2026-07-08-stage-02-borg-adapter.md),
  from Stage 2
