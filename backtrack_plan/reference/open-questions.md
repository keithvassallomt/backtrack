# Backtrack — Answers to the Open Questions

*Research completed 4 July 2026. Everything below was checked against current documentation (GNOME/Nautilus API 4.1, KDE Frameworks 6, Borg 1.x/2.0-beta) — not just remembered.*

---

## Question 1: How deeply can we integrate with Nautilus and Dolphin?

**Short answer: right-click menus are easy in both. A slider *inside* the file manager window is not possible in either — and that's fine, because launching our own window from the right-click menu is the proven, stable pattern.**

### What each file manager allows

| What we want | Nautilus (GNOME) | Dolphin (KDE) |
|---|---|---|
| Right-click a file/folder → "Backtrack" | ✅ Easy | ✅ Easy |
| Right-click empty space in a folder → "Browse backups here" | ✅ Easy | ✅ Easy |
| A Backtrack button in the toolbar | ❌ Not possible — no API for it | ❌ Not possible — no API for it |
| Slider embedded in the window, folder contents morphing live | ❌ Not possible | ❌ Not possible without forking Dolphin |
| Backups appearing as a browsable "location" (like a network drive) | ⚠️ Workaround only (see below) | ✅ Doable properly (see below) |
| Little badges on files that are backed up ("emblems") | ⚠️ Doable with effort | ⚠️ Doable with effort |

### Why the embedded slider is off the table

- **Nautilus** used to have an API that let extensions put widgets inside the window (`LocationWidgetProvider`). It was **removed in Nautilus 43 with no replacement** — extensions can now only add menu items, columns, and badges. The best-known app that relied on it (nautilus-terminal) was abandoned in 2025 for exactly this reason.
- **Dolphin** is friendlier overall, but it also has no way for third parties to add toolbar buttons or panels. Its plugin points are menus, badges, and virtual locations.
- There's also a cautionary tale: **Deja Dup (GNOME's own backup app) used to have Nautilus menu integration and removed it** because it kept breaking between GNOME releases. Lesson: keep the file-manager plugin as thin and boring as possible — just a launcher.

### The bonus opportunity in Dolphin

KDE has a mechanism called **KIO workers** that lets apps invent browsable locations. Dolphin already ships `timeline:/` (browse files by date) built this way. We could ship a `backtrack:/` location — open it and see your snapshots as ordinary folders. On the Nautilus side there's no equivalent plugin system; the closest workaround is mounting a snapshot with `borg mount` and adding a sidebar bookmark.

### Recommended shape

```
 Nautilus / Dolphin                       Backtrack app
┌─────────────────────┐                ┌──────────────────────┐
│ right-click a file  │   passes the   │  Our own window:     │
│ or folder           │ ─────────────► │  timeline, previews, │
│ "Backtrack…"        │  path clicked  │  restore buttons     │
└─────────────────────┘                └──────────────────────┘
   (tiny, boring plugin —                (all the magic lives
    survives OS updates)                  here, fully ours)
```

This matches what the earlier AI conversation suspected — and it's now confirmed against the current APIs.

---

## Question 2: Is the slider the best time-travel metaphor?

**Short answer: no — not as the *primary* control. Sliders are great for continuous things (volume, brightness) and bad for picking one item from a discrete, unevenly-spaced set — which is exactly what backups are. The winning design is a hybrid: a snapshot *list* as the main control, big back/forward arrows, and a calendar for long jumps. A slim timeline strip can stay as a visual garnish.**

### Why a plain slider fights us

- Backups aren't evenly spaced: hourly for today, daily for the month, weekly after that. On a slider, **the last 24 hours (the part people need most) get squashed into a few pixels**, while one lonely snapshot from March gets acres of space.
- Picking "the 14:05 backup, not the 15:05 one" is a precision task — the textbook case where usability research (Nielsen Norman Group) says *don't* use a slider.
- Sliders are fiddly on touchpads, awkward for people with motor difficulties, and hide useful information (how many backups exist? where are the gaps?).
- Field evidence: Apple's own Time Machine timeline (the tick bars on the right edge) is one of its most complained-about elements, and Apple progressively stripped back the whole cinematic UI over the years. Meanwhile **every Linux backup tool (Pika, Vorta, Back In Time, Déjà Dup) independently converged on lists, not sliders.**

### What each metaphor is good at

| Metaphor | Good | Bad |
|---|---|---|
| Continuous slider | Feels "time-like", demos beautifully | Imprecise, squashes recent history, weak accessibility |
| Slider with snap-to ticks | Fixes precision a bit | Only works with ~4–12 steps; backups have hundreds |
| **Version list sidebar** (Google Docs style) | Precise, scannable, shows counts and gaps, keyboard/screen-reader friendly for free | Less visually exciting; needs grouping to stay tidy |
| **Back/forward arrows** | Instantly understood; perfect for "the version just before this" | Hopeless for jumping back three months |
| **Calendar popover** | Great for "I know roughly when" | Useless for choosing between several same-day backups |
| Time Machine "tunnel" (stacked windows) | Strong wow factor | Modal, expensive to build, ages badly — even Apple toned it down |

### Recommended design for Backtrack

Keep the **one genuinely great idea from Time Machine**: the folder view stays put while *time* changes around it. Then drive it with:

1. **Snapshot list in a sidebar** (the primary control) — newest first, grouped like Google Docs: "Today" (hourly entries), "Yesterday", "This week", collapsed months. Grouping absorbs the uneven spacing instead of fighting it.
2. **Big ◀ ▶ arrows** to step one snapshot at a time (with keyboard shortcuts). Power move: a "jump to previous version where *this file* changed" step — turns hunting through 40 identical hourly snapshots into two clicks.
3. **A small calendar popover** for long jumps, with days that have backups lightly shaded.
4. **Optional: a thin timeline strip** at the bottom showing snapshot density and current position — as an *indicator and coarse jump*, never the only way to select. This keeps the "sliding through time" feeling from the user stories without the precision problems.
5. Always show the current position in words: *"Wed 12 Jun 2026, 09:00 — backup 3 of 47"*.

So Alice's story survives intact — she still drags back through time with live previews — but the thing she drags is honest about being a set of discrete snapshots.

### Things to explicitly avoid

- A full-screen modal takeover (Time Machine's most-repeated complaint).
- Freezing the UI while the backup engine thinks (its second-most-repeated complaint) — the list must appear instantly from a local catalogue.
- Unlabelled tick marks anywhere.

---

## Question 3: How do we keep it fast with large files and folders?

**Short answer: never make the UI talk to Borg while browsing. Borg has no quick way to answer "what was in this folder?" — every such question costs a full scan of that snapshot's file catalogue (tens of seconds to minutes on big backups). So we build our own little catalogue (a local SQLite index) once per backup, and the timeline UI reads only that: instant, and it even works when the backup disk is unplugged. Borg gets involved only when actually previewing or restoring.**

### What's fast and slow in Borg (measured, from Borg docs and real bug reports)

| Operation | Speed |
|---|---|
| Listing the snapshots themselves | Fast (seconds) |
| Listing *files inside* one snapshot | Slow — grows with file count; minutes on big archives |
| Browsing via `borg mount` (FUSE) | 37–121 seconds *just to open* one archive on a large repo |
| Extracting one small file | Not instant — Borg still scans the catalogue to find it (real case: 280 s) |
| Restoring a whole folder with `borg extract` | Fast — this is the recommended bulk path |
| An hourly backup when little changed | Fast — ~1 million unchanged files in ~4 minutes |

### The architecture that follows

```
 hourly backup finishes
        │
        ▼
 background indexer reads that ONE new snapshot's file list
        │
        ▼
 local SQLite catalogue  ◄──── the timeline UI reads ONLY this
        │                       (instant browsing, works offline)
        ▼
 Borg is touched only for:
   • preview  → extract that one file (with spinner + cancel)
   • restore  → borg extract into a staging folder, then move into place
   • fallback → borg mount + rsync for resumable restores on flaky networks
```

### Won't the catalogue be huge?

Not if we're smart. The naive approach (one record per file per snapshot: 500,000 files × 100 snapshots = 50 million records, several GB) is out. Instead each record says *"this version of the file existed from snapshot 12 through snapshot 87"* — an unchanged file costs **one record no matter how many snapshots it appears in**. With realistic day-to-day change rates that's roughly **50–150 MB** for 500k files × 100 snapshots. Cheap.

### Other practical points

- **Index newest-first** on first run, so the app is useful within minutes of pointing it at an existing repo; older snapshots fill in quietly in the background ("cataloguing…" badge on the ones still pending).
- **Restores get real progress bars**: Borg emits machine-readable progress, and since our catalogue knows the exact total size of what's being restored, we can show an honest percentage and byte count.
- **Big restores are split** into per-folder jobs — natural checkpoints, per-job retry.
- **Cache previews** — thanks to deduplication, the same file content across 50 snapshots is a single cache entry.
- **Pruning** keeps the Time Machine feel affordable: keep hourly for a day, daily for a week, weekly for a month, monthly for six months (Borg has this built in).
- **Target Borg 1.x for v1.** Borg 2 is still in beta (as of July 2026, marked "do not use for production"). Keep our Borg calls behind one adapter layer so switching later is painless.

---

## Question 4: How should we handle conflicts when restoring?

**Short answer: your instinct — Replace / Keep Both / Cancel — is right, and it's exactly what Time Machine uses. Three upgrades make it meaningfully better than anyone else's version: show both versions' dates and sizes in the dialog, summarise folder restores in one screen instead of a storm of pop-ups, and quietly keep a safety copy of anything we overwrite so "Replace" is never truly destructive.**

### What the neighbours do

| | Buttons | Shows dates/sizes of both versions? | Keeps a safety copy of what it overwrites? |
|---|---|---|---|
| Time Machine | Keep Original / Keep Both / Replace | ❌ (a known complaint) | ❌ |
| Windows Explorer | Replace / Skip / Compare both | ✅ in the compare view | ❌ (bypasses the Recycle Bin!) |
| GNOME Files | Cancel / Skip / Replace (+ rename option) | ✅ always | ❌ |
| **Backtrack (proposed)** | Cancel / Keep Both / Replace | ✅ always | ✅ — our differentiator |

No mainstream tool protects the file being overwritten. That's our chance to be genuinely better.

### Single-file conflict dialog

Only shown when there's a *real* difference — if the file on disk is identical to the backup version, we silently skip and say "already up to date" (a trick borrowed from Windows).

```
┌──────────────────────────────────────────────────────┐
│            Replace "report.odt"?                     │
│                                                      │
│  A file with this name already exists in Documents.  │
│                                                      │
│  📄 Current file     Modified 3 Jul, 14:02   48 KB   │
│                      (newer than the backup)         │
│  🕘 Backup version   From 28 Jun, 09:00      45 KB   │
│                                                      │
│     [ Cancel ]   [ Keep Both ]   [ Replace ]         │
│                                   └─ red, and NOT    │
│                                      the default     │
└──────────────────────────────────────────────────────┘
```

- **Keep Both** = the restored file takes the real name; the current file becomes `report (current).odt`. (In a *restore* tool, the user's intent is "give me the backup version back" — so it wins the name. Same logic as Time Machine.)
- The dialog always says which side is **newer** — restoring over a newer file is the risky case, so it gets a bolder warning.
- Buttons are verbs, Cancel comes first, Replace is styled red and never pre-selected — straight from the GNOME design guidelines.

### Folder restores: one summary, not fifty pop-ups

Before touching anything, we compare and show one screen:

```
Restore "Projects" from the backup of 28 Jun?

  • 214 files are identical — left alone
  •   6 files will be replaced (3 are newer on disk)   [Review…]
  •   2 files exist only in the backup — will be added
  •   4 files exist only on your disk — kept, nothing is deleted

  Replaced files are kept as safety copies for 30 days.

     [ Cancel ]   [ Keep Both Versions ]   [ Replace Changed Files ]
```

- "Review…" opens a checklist for people who want per-file control.
- Restores **merge — they never delete** files that only exist on disk. That's the number-one fear in folder restores, so the dialog says it outright.

### The safety net (recommended, and honest)

- **Don't promise "the old file is in your backup"** — that's only true if a backup happened to run after the file's last edit. It often hasn't.
- Instead: before overwriting anything, **move the current version into a Backtrack-managed "Recently replaced" stash** (auto-expires after ~30 days, browsable inside the app).
- After every restore, show a toast: **"Restored report.odt — Undo"**. GNOME's design guidance explicitly says undo beats confirmation dialogs, because people click through dialogs without reading.
- Mechanically, restores go: extract to a temporary folder → compare → move files into place one by one. That structure makes Keep Both, the stash, and Undo all easy and safe.

### Edge cases we've flagged for the design

- File open in another app → can't reliably detect on Linux; use atomic replace so nothing corrupts.
- File on disk vs folder in backup (or vice versa), and symlinks → shown as "type changed" in the review list, never silently replaced; restore symlinks as links, never follow them.
- No permission to write → fail that one file with a clear message and offer "restore somewhere else", don't abort the whole job.
- Low disk space → checked *before* starting (staging + stash briefly needs double space), not discovered halfway through.

---

## The one-paragraph takeaway

Everything the user stories describe is buildable, with one adjustment: the time-travel experience lives in **Backtrack's own window**, launched from a right-click in Nautilus or Dolphin (both support that cleanly; neither allows an embedded slider, and GNOME's own backup app got burned trying deeper integration). Inside that window, the "slider" becomes a **grouped snapshot list + step arrows + calendar**, with an optional slim timeline strip for the sliding-through-time feel. Browsing is instant because it reads a **local catalogue** instead of poking Borg, and restores use **Replace / Keep Both / Cancel** with a safety stash and Undo — which makes Backtrack *safer* than Time Machine, Windows, or any current Linux tool.

<details>
<summary>Key sources (click to expand)</summary>

**File manager APIs:** [Nautilus extension API 4.1](https://gnome.pages.gitlab.gnome.org/nautilus/) · [nautilus-python migration guide (LocationWidgetProvider removal)](https://gnome.pages.gitlab.gnome.org/nautilus-python/nautilus-python-migrating-to-4.html) · [nautilus-terminal end-of-life](https://github.com/flozz/nautilus-terminal) · [KDE service menus](https://develop.kde.org/docs/apps/dolphin/service-menus/) · [KIO framework](https://api.kde.org/kio-index.html) · [Deja Dup Nautilus woes](https://bugs.debian.org/cgi-bin/bugreport.cgi?bug=894384)

**UX / metaphor:** [NN/g on slider controls](https://www.nngroup.com/articles/gui-slider-controls/) · [Time Machine UI complaints](https://discussions.apple.com/thread/255041503) · [Windows File History](https://support.microsoft.com/en-us/windows/backup-and-restore-with-file-history-7bf065bf-f1ea-0a78-c1cf-7dcf51cc8bfc) · [Figma version history](https://help.figma.com/hc/en-us/articles/360038006754-View-a-file-s-version-history) · [Pika Backup recovery](https://help.gnome.org/pika-backup/recovery-pika.html) · [Back In Time](https://github.com/bit-team/backintime)

**Borg performance:** [Borg FAQ](https://borgbackup.readthedocs.io/en/stable/faq.html) · [borg mount docs](https://borgbackup.readthedocs.io/en/stable/usage/mount.html) · [BorgBase: mount vs extract](https://docs.borgbase.com/restore/borg/cli) · slow-mount issue [#7173](https://github.com/borgbackup/borg/issues/7173) · slow single-file extract [#4362](https://github.com/borgbackup/borg/issues/4362) · [borg prune](https://borgbackup.readthedocs.io/en/stable/usage/prune.html) · [Borg 2 status (beta)](https://www.borgbackup.org/releases/)

**Conflicts:** [Time Machine restore (Apple)](https://support.apple.com/guide/mac-help/restore-files-mh11422/mac) · [GNOME HIG — dialogs & undo](https://developer.gnome.org/hig/patterns/feedback/dialogs.html) · [Nautilus conflict dialog source](https://github.com/GNOME/nautilus/blob/master/src/nautilus-file-conflict-dialog.c) · [Windows conflict dialog history](https://devblogs.microsoft.com/oldnewthing/20190604-00/?p=102539) · [Borg extract limitations](https://borgbackup.readthedocs.io/en/stable/usage/notes.html)

</details>
