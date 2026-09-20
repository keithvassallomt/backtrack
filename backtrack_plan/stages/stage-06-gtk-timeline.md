# Stage 6 — GTK App Shell & Timeline Browser

## Objective
The heart of the product: a libadwaita window where the folder view stays put while
time changes around it. Everything renders from the index (instant, offline);
Borg is touched only for previews.

## Prerequisites
Stages 0–3 (daemon running against demo-repo; Stages 4–5 helpful but not required).
**Normative UI references:** [../reference/prototype.md](../reference/prototype.md)
Screens 1 + 6–8 wireframes, mockups
[1](../mockups/1-backtrack-main-window.png), [6](../mockups/6-main-window-dark.png),
[7](../mockups/7-calendar-popover.png), [14](../mockups/14-primary-menu.png).
Also read open-questions.md Q2 (why the design is list-first, slider-demoted).

## UI rules for every GTK stage
- Adwaita widgets and HIG defaults; if the mockup and a stock widget disagree on
  minor styling, the stock widget wins.
- Every control keyboard-reachable; labels for screen readers; no information
  conveyed by color alone (badges have text).
- Both themes always (mockups 1 vs 6): test with `adw_style_manager` forced each way.
- The GUI process NEVER blocks on I/O: index reads on a worker (they're fast but
  still off-main-thread), previews streamed async with spinner + cancel.
- Current position always visible in words: "Wed 12 Jun 2026, 09:00 — backup 3 of 47".

## Tasks

### S06-T1 — Shell
`AdwApplicationWindow`, header bar (app icon, breadcrumb path, search button
(stub → Stage 8), primary-menu button), CSS for badge pills. Launch contract:
`backtrack-gtk [--path DIR] [--select FILE]`. Connects to daemon (D-Bus,
auto-activating), opens read-only index.
**Accept:** window matches mockup 1's frame in both themes; `--path`/`--select`
honored; daemon down → auto-activation brings it up.

### S06-T2 — Snapshot sidebar
`GtkListView` + section model over `archives_overview()`: groups Today (hourly
entries), Yesterday, This week (day entries), then collapsed month groups with
counts — grouping absorbs uneven retention (see research). Spool/fs-snapshot
archives get the "on this computer" suffix badge. Selection drives an
`AppState.current_seq` observable everything else binds to.
**Accept:** demo-repo renders the exact grouping the fixture implies (snapshot
test on the group model); selecting entries updates the position label.

### S06-T3 — File pane
Column view (Name, Size, Modified, Status) bound to `folder_at(path, seq)`.
Status badges: orange "deleted after this", blue "changed since then" (flags from
S01-T3). Double-click folder navigates (breadcrumb updates); double-click file =
preview focus. Selection drives the action bar (Restore / Restore To… / Compare
stubs wired in Stages 7–8).
**Accept:** the Alice walkthrough on demo-repo: navigate to the fixture folder,
step back until `old-client-folder` appears with badge — matches mockup 1 rows.

### S06-T4 — Time stepping
Older/Newer buttons + `Ctrl+←/→`; disabled at the ends. Long-press / menu-arrow on
the buttons exposes "Previous/Next change to selected file" using
`next_change()` — the 40-identical-hourlies killer feature.
**Accept:** stepping is <16 ms perceived (no visible relayout jank on demo data);
next-change jumps land exactly on the fixture's known change points.

### S06-T5 — Calendar popover
Per mockup 7: month grid, shaded days = has snapshots (from overview counts),
today ringed, click day → sidebar scrolls to that day's first snapshot; month nav
arrows; caption "Shaded days have backups". Days without backups must be visibly
plain (the mockup undersells this — prototype.md notes it).
**Accept:** fixture has known gap days → rendered unshaded and unclickable.

### S06-T6 — Density strip
Thin custom widget (GtkDrawingArea): per-day bars over the whole history, current
position marker, month labels. Click/drag = coarse jump snapping to nearest
snapshot (updates AppState). It is an indicator+shortcut, never the only control;
full slider ARIA semantics (`GtkAccessible` role slider, arrow keys step snapshots).
**Accept:** keyboard-only operation works; drag always lands on a real snapshot;
matches mockup 1's strip visually.

### S06-T7 — Preview pane
On selection: instant metadata (name/size/mtime from index) + async content via
`PreviewFile` fd — text (first 64 KB, monospace), images (thumbnail), PDF (first
page via poppler-glib if trivially available, else icon), other → icon + "Open
after restoring". Spinner + cancel on slow fetch; LRU-cached daemon-side (S03-T3).
**Accept:** preview of demo `report.odt`-style text file appears; selecting rapidly
across 20 files leaves no zombie fetches (cancellation test via logs).

### S06-T8 — Primary menu
Mockup 14 exactly: Back Up Now, Pause Backups ▸ (1 h / until tomorrow / until I
resume), Recently Replaced Files (stub → Stage 7), Preferences (stub → Stage 9),
Keyboard Shortcuts (`GtkShortcutsWindow`, real accels), Help (docs URL), About
(`AdwAboutDialog` — version string read from Cargo metadata, never hardcoded).
**Accept:** Back Up Now triggers daemon backup with toast on completion; Pause
updates status line and self-expires (dev: 1 h option shortened via
BACKTRACK_DEV to 1 min for testing).

### S06-T9 — Demo fixture dated relative to now
`xtask` dates its 30 snapshots from fixed calendar dates in June 2026, so the
sidebar's Today / Yesterday / This week bands are empty on any day after that
month and the whole history collapses into two month groups. The scripted
*content* (files appearing, changing, `old-client-folder` deleted after entry
15) stays exactly as it is; only the timestamps move, ending with a few hours
apart today so every band has something in it. Borg's `--timestamp` does not
round-trip through `{time:%s}` unchanged, so the offset is measured rather than
assumed.
**Accept:** `just demo-repo` produces a history whose newest snapshots are
today, and the sidebar renders Today, Yesterday, This week, Last week and at
least one month group; the existing acceptance check (`old-client-folder`
flagged deleted at snapshot 10) still passes.

### S06-T10 — JobFinished signal
The interface has no way to learn that a job ended: `StatusChanged` fires only
when the health *state* moves, which a successful backup on an already-healthy
machine does not do, so the window polls `GetStatus` to know when a backup it
started has finished. Add `JobFinished(job, kind, outcome)` to
`org.backtrack.Daemon1`, emitted from the signal fan-out on any terminal job
state, and use it in place of the polling. Stage 7's restores need the same
thing.
**Accept:** the introspection snapshot carries the new signal; the GTK app
toasts on completion without polling; a daemon test asserts the signal is
emitted once per terminal job with the right outcome.

## Definition of Done
The Alice story runs end to end on demo-repo (browse → step back → deleted folder
reappears → preview) in both themes with keyboard only; screenshots of light+dark
attached to the PR; progress.md + CHANGELOG updated.
