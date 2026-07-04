# Backtrack — Product Brief

*One page. What this is, who it's for, and — as importantly — what it is not.*

> **Backtrack** — *Browse your files as they were.*
> A Time Machine-class backup and restore experience for Linux, built on Borg.

## The problem

Linux has excellent backup *engines* (Borg) and several capable backup *managers*
(Pika, Vorta, Back In Time). What it lacks is the opinionated, beautiful **restore**
experience: pick a folder, move through time, bring a file back — without knowing what
an "archive" is. Backtrack is restore-first: backups are the invisible part.

## Target user

A desktop Linux user (GNOME or KDE) who would use Time Machine if they had a Mac.
They know what a folder is; they should never need to know what a repository, archive,
chunk, or prune policy is. Power users are welcome but are not the design target.

## The four stories (acceptance tests for v1)

1. **Alice** deletes a file, empties the trash, and gets it back hours later by sliding
   back through time.
2. **Bob** right-clicks a document in his file manager and restores this morning's
   version after a bad rewrite.
3. **Charlie** can't find a file he downloaded last week — search across time finds it
   in a snapshot even though it was deleted since.
4. **Dave** previews old versions of an image from Dolphin and restores the one he wants.

Plus the fifth, unspoken story: **the laptop dies.** On a new machine, Backtrack
connects to the existing repo and guides a full restore of the home folder.

## v1 scope (fixed)

Hourly encrypted backups · timeline browser (snapshot list + arrows + calendar +
density strip) · cross-snapshot search incl. deleted files · restore with conflict
handling, safety stash, and undo · compare view · onboarding wizard · preferences ·
Nautilus/Dolphin context menus · offline protection (btrfs snapshots or spool repo) ·
guided full-home disaster recovery · health monitoring with honest failure UX.

## Non-goals (decided 4 Jul 2026 — resist these as feature requests)

| Non-goal | Why |
|---|---|
| System files / bootable restore | Userspace only. TimeShift territory stays TimeShift's. |
| Multiple destinations (3-2-1) | One primary destination + the offline safety net. Mirroring is a different product. |
| Multi-machine shared repos | One repo per machine; Borg discourages sharing and the cache/locking pain isn't worth it. |
| Multi-user machines | Backtrack protects the logged-in user's files; each account runs its own instance. |
| Telemetry, plugins, web UI, multi-engine | None serve the four stories. |

## Project facts

- **License:** GPL-3.0-or-later.
- **Distribution:** public repo; Flatpak (Flathub) + Fedora COPR + Ubuntu PPA from v1.
- **App ID:** `io.github.keithvassallomt.Backtrack` (Flathub-compatible; revisit if a
  domain is acquired).
- **Naming note:** "Backtrack" collides with the retired BackTrack Linux pentest distro
  in search results. Accepted for now — the association is dated and the name is right —
  but branding (icon, tagline, "Backtrack Backup" in store listings) should lean away
  from security-tool aesthetics. Decision owner: Keith.

## Document map

| Doc | Contents |
|---|---|
| [open-questions.md](open-questions.md) | Research: FM integration, UX metaphor, performance, conflicts |
| [prototype.md](prototype.md) | Full UX: 19+ mockups and ASCII wireframes |
| [offline-strategy.md](offline-strategy.md) | Offline/unreachable-destination design |
| [health.md](health.md) | Backup health model and failure UX |
| [stack.md](stack.md) | Architecture and technical decisions |
| [backtrack.md](backtrack.md) | Original conversation transcript (historical) |
