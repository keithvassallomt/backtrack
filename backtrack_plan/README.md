# Backtrack — Implementation Plan

**Read this file first. Then read [antislop.md](antislop.md) (how you work), [personality.md](personality.md) (how you talk), and [progress.md](progress.md). Then open the current stage file. Do not skip ahead.**

## What you are building

**Backtrack** is a Time Machine-class backup and restore app for desktop Linux, built on
Borg Backup. Restore-first: users browse their folders back through time and bring files
back; backups are the invisible part. Full product vision:
[reference/brief.md](reference/brief.md).

If you read nothing else before starting: read [reference/brief.md](reference/brief.md)
(1 page) and [reference/stack.md](reference/stack.md) (architecture — your bible).

## The contract (non-negotiable)

1. **You MUST update [progress.md](progress.md) as you work** — mark tasks
   `[/]` (in progress) when you start and `[x]` when their acceptance criteria pass,
   in the same commit as the work. Failing to update progress.md is a breach of
   contract. No exceptions.
2. **You MUST NOT change version numbers.** Ever. Version numbers are exclusively
   human-controlled. The `just bump-version` recipe exists for the human to run.
   If a task seems to require a version bump, STOP and ask.
3. **Work stages in order.** A stage is done only when every acceptance criterion in
   its file passes and progress.md reflects it. Stages 6+ have some parallelism noted
   in their Prerequisites sections — respect the prerequisites, nothing else is optional.
4. **Every commit passes `just check`** (fmt + clippy + tests). No red commits.
5. **Update CHANGELOG.md** (`[Unreleased]` section) for every user-visible change,
   following the conventions below.
6. **Do not invent features.** The scope is fixed by the reference docs. Non-goals in
   brief.md are non-goals; resist them. If something is ambiguous, prefer the mockup;
   if still ambiguous, ask the human.
7. **Tone:** follow [personality.md](personality.md) — Kevin-mode in conversation,
   client-mode in everything that ships or persists (code, commits, docs, UI copy).
8. **Craft:** [antislop.md](antislop.md) applies to every line of code you write —
   re-skim it before starting each stage. Where it says explicit project
   instructions override its defaults: this plan and the reference docs ARE those
   explicit instructions (e.g. files a stage tells you to create are, by
   definition, asked for). When neither the plan nor antislop.md covers a
   judgment call, antislop.md's core principle decides: the smallest correct
   change a careful maintainer would make.

## Repository conventions

- **Language:** Rust (stable). Workspace layout is defined in
  [reference/stack.md](reference/stack.md) §1 and created in Stage 0.
- **License:** GPL-3.0-or-later. App ID: `io.github.keithvassallomt.Backtrack`.
- **Initial version: 0.1.0** (set by human in Stage 0; never touched by AI after).
- **Logging from day one:** all crates use `tracing`. No `println!`/`eprintln!` in
  committed code (CI greps for it). Details in Stage 0.
- **Commits:** imperative subject, body explains why. Reference the stage/task ID,
  e.g. `S04-T3: schedule prune after successful create`.
- **CHANGELOG:** [Keep a Changelog](https://keepachangelog.com) format, SemVer.
  Categories: Added / Changed / Deprecated / Removed / Fixed / Security. Entries are
  user-facing sentences ("Added calendar popover for jumping to a date"), not commit
  logs. Version bumps: human decides the number, human runs `just bump-version X.Y.Z`,
  human moves `[Unreleased]` under the new heading. You only ever add entries under
  `[Unreleased]`.

## How to use the stage files

Each `stages/stage-NN-*.md` contains: **Objective** (what exists at the end),
**Prerequisites**, **Context** (which reference docs/mockups to read first),
**Tasks** (numbered `SNN-T#`, each with acceptance criteria), and
**Definition of Done**. Follow tasks in order within a stage.

| Stage | File | Delivers |
|---|---|---|
| 0 | [stage-00-bootstrap.md](stages/stage-00-bootstrap.md) | Repo, workspace, Justfile, logging, CI, CHANGELOG, versioning |
| 1 | [stage-01-core-index.md](stages/stage-01-core-index.md) | SQLite catalogue (interval encoding), ingest, search queries |
| 2 | [stage-02-borg-adapter.md](stages/stage-02-borg-adapter.md) | BackupEngine trait, Borg CLI driver, keyring, error taxonomy |
| 3 | [stage-03-daemon-dbus-cli.md](stages/stage-03-daemon-dbus-cli.md) | backtrackd, D-Bus API, job model, backtrack CLI |
| 4 | [stage-04-backup-pipeline.md](stages/stage-04-backup-pipeline.md) | Scheduler, preflight, create→index→prune→compact |
| 5 | [stage-05-offline-protection.md](stages/stage-05-offline-protection.md) | Reachability, spool repo, btrfs mode, catch-up |
| 6 | [stage-06-gtk-timeline.md](stages/stage-06-gtk-timeline.md) | GTK app shell, timeline browser, preview |
| 7 | [stage-07-restore-engine.md](stages/stage-07-restore-engine.md) | Staged restore, conflicts, stash, undo |
| 8 | [stage-08-search-compare.md](stages/stage-08-search-compare.md) | Cross-snapshot search UI, compare view |
| 9 | [stage-09-wizard-preferences.md](stages/stage-09-wizard-preferences.md) | Onboarding wizard, preferences window |
| 10 | [stage-10-health.md](stages/stage-10-health.md) | Health states, notifications, resolution flows |
| 11 | [stage-11-disaster-recovery.md](stages/stage-11-disaster-recovery.md) | "Restore Everything" guided flow |
| 12 | [stage-12-integrations-tray.md](stages/stage-12-integrations-tray.md) | Nautilus/Dolphin menus, tray, background portal |
| 13 | [stage-13-packaging-release.md](stages/stage-13-packaging-release.md) | Flatpak (portals, cargo-sources), RPM/DEB, release process |

## Reference documents (in `reference/`)

| Doc | What it settles |
|---|---|
| [brief.md](reference/brief.md) | Vision, user stories (= acceptance tests), non-goals, license, app ID |
| [stack.md](reference/stack.md) | Architecture: crates, D-Bus API, schema, pipelines, packaging |
| [prototype.md](reference/prototype.md) | Every screen: ASCII wireframes + mockup images. **The UI source of truth.** |
| [open-questions.md](reference/open-questions.md) | Research behind the decisions (why no slider, why an index, etc.) |
| [offline-strategy.md](reference/offline-strategy.md) | Offline/unreachable-destination behaviour |
| [health.md](reference/health.md) | Health states, escalation rules, failure catalogue with exact copy |

## Mockups (in `mockups/`)

Numbered 1–24; each stage file names the exact mockups it implements. Match them
closely — spacing and widget choices are Adwaita defaults, so "looks like the mockup"
usually means "used the standard widget". The mockups are 2K renders; text in them is
the intended UI copy (health.md and prototype.md override if a render has a typo).

| # | File | Shows |
|---|---|---|
| 1 | `1-backtrack-main-window.png` | Main window, light |
| 2 | `2-nautilus-context-menu.png` | Nautilus menu entry |
| 3 | `3-dolphin-integration.png` | Dolphin menu + backtrack:/ (KIO part is v1.x) |
| 4 | `4-conflict-dialog.png` | Single-file conflict dialog |
| 5 | `5-folder-restore-summary.png` | Folder restore summary + undo toast |
| 6 | `6-main-window-dark.png` | Main window, dark |
| 7 | `7-calendar-popover.png` | Calendar popover |
| 8 | `8-review-checklist.png` | Per-file review checklist |
| 9 | `9-compare-view.png` | Compare view |
| 10–13 | `10..13-wizard-*.png` | Onboarding wizard, steps 1–4 |
| 14 | `14-primary-menu.png` | Primary (burger) menu |
| 15–19 | `15..19-prefs-*.png` | Preferences: General/Backup/Storage/Security/Advanced |
| 20 | `20-search-results.png` | Cross-snapshot search results |
| 21 | `21-dr-welcome.png` | Disaster recovery entry dialog |
| 22 | `22-dr-progress.png` | Disaster recovery progress |
| 23 | `23-health-banner.png` | AT_RISK warning banner state |
| 24 | `24-passphrase-dialog.png` | Passphrase re-entry dialog |

## Getting unstuck

- Architecture question → stack.md. UI question → prototype.md + the mockup.
  Copy/wording question → health.md / prototype.md ASCII. Scope question → brief.md.
- Borg behaviour surprises you → open-questions.md Q3 documents the known
  performance cliffs (never browse via borg; extract is O(archive size); etc.).
- Genuinely blocked → write the question and the options you considered into
  progress.md under "Blocked", and ask the human. Do not guess on: version numbers,
  scope additions, destructive operations, licensing.
