# Backtrack — UX Prototype

This is the proposed user experience, based on the research in [open-questions.md](open-questions.md).
Each screen is shown first as an ASCII wireframe (the layout intent), followed by a generated
high-fidelity mockup image.

The design follows the research conclusions:

- The time-travel experience lives in **Backtrack's own window**, launched from the file managers.
- The primary time control is a **grouped snapshot list + step arrows + calendar popover**,
  with a thin **timeline density strip** kept as a secondary "slide through time" control.
- The folder view **stays put while time changes** — Time Machine's one great idea.
- Restores use **Cancel / Keep Both / Replace** with a safety stash and Undo toast.

---

## Screen 1 — The main Backtrack window

The heart of the app. Alice opens it, picks her folder, and moves back through time while
the file list updates instantly (served from the local catalogue, never live from Borg).

```
┌──────────────────────────────────────────────────────────────────────────────────────────┐
│ ◷ Backtrack          🏠 Home ▸ Documents ▸ Projects                    🔍      ≡   ─ □ ✕ │
├────────────────────────┬─────────────────────────────────────────────────────────────────┤
│  SNAPSHOTS        [📅] │                                                                 │
│  ─────────────────────  │   ◀ Older        Wed 12 Jun 2026, 09:00 · backup 3 of 47        │
│  ▾ Today               │                  (You are viewing the past)         Newer ▶     │
│     ● 16:00            │  ───────────────────────────────────────────────────────────────│
│     ● 15:00            │   Name                     Size     Modified      Status        │
│     ● 14:00            │   ─────────────────────────────────────────────────────────────  │
│     ● 13:00            │   📁 old-client-folder      —       11 Jun 18:22  ⚠ deleted     │
│  ▾ Yesterday           │                                                    after this   │
│     ● 22:00            │   📄 invoice-may.pdf        182 KB  02 Jun 10:11               │
│     ● 08:00            │   📄 invoice-june-draft.pdf  96 KB  12 Jun 08:47  ✎ changed    │
│  ▾ This week           │                                                    since then   │
│     ● Tue 10 Jun       │   📄 report.odt              45 KB  12 Jun 08:52  ✎ changed    │
│     ● Mon 9 Jun        │   📄 tax-notes.txt            4 KB  28 May 09:30               │
│  ▸ Last week      (7)  │                                                                 │
│  ▸ May 2026      (31)  │  ┌───────────── Preview: report.odt ─────────────┐              │
│  ▸ April 2026    (30)  │  │  [ thumbnail of the document as it was       ]│              │
│                        │  │  [ at 09:00 on 12 Jun — text preview         ]│              │
│  ⟳ Cataloguing         │  └───────────────────────────────────────────────┘              │
│    April… (12 left)    │                                                                 │
├────────────────────────┴─────────────────────────────────────────────────────────────────┤
│  TIMELINE  ▁▁▂▁▁▁▁▂▁▁▁▁▁▂▁▁▃▁▁▁▁▂▁▁▁▁▅▁▁▁▂▁▁▇▉   ← density strip; drag to scrub,        │
│            Apr          May          Jun  ▲Now      snaps to nearest snapshot            │
├───────────────────────────────────────────────────────────────────────────────────────────┤
│            [ Compare with Today ]      [ Restore To… ]      [ ▣ Restore ]                │
└───────────────────────────────────────────────────────────────────────────────────────────┘
```

Key interactions:

- **Snapshot list (left)** — the primary control. Grouped like Google Docs version history;
  groups absorb Borg's uneven hourly/daily/weekly retention. Badge counts on collapsed groups.
- **◀ / ▶ arrows** — step one snapshot at a time (`Ctrl+←/→`). Long-press ▶ offers
  *"Next change to selected file"* — skips the 40 identical hourly snapshots.
- **📅 calendar button** — popover with backup-days shaded; clicking a day scrolls the list.
- **Timeline strip (bottom)** — the "slider", demoted to garnish: shows snapshot density,
  draggable, always snaps to a real snapshot, never the only way to navigate.
- **Status badges** — "deleted after this", "changed since then" make Alice's and Charlie's
  hunts trivial: deleted files simply *reappear* as you go back.
- Current position is always spelled out in words ("backup 3 of 47").

---

## Screen 2 — Nautilus (GNOME Files) integration

A thin MenuProvider extension. Right-click a file **or** the folder background:

```
┌──────────────────────────────────────────────────────────────────────────┐
│  ⊞  ◀ ▶   🏠 Home ▸ Documents ▸ Projects                    🔍  ≡  ─ □ ✕ │
├──────────┬───────────────────────────────────────────────────────────────┤
│ ⭐ Starred│   📁              📁              📄            📄            │
│ 🏠 Home   │   designs         old-client…     report.odt    invoice-…    │
│ 📄 Docume…│                  ┌──────────────────────────────┐            │
│ ⬇ Downloa…│                  │ Open with Text Editor        │            │
│ 🖼 Picture…│                  │ Open With…                  │            │
│ 🗑 Trash  │                  ├──────────────────────────────┤            │
│           │                  │ Cut                          │            │
│ + Other L…│                  │ Copy                         │            │
│           │                  │ Move to Trash                │            │
│           │                  ├──────────────────────────────┤            │
│           │                  │ ◷ Restore Previous Version…  │  ← Backtrack│
│           │                  ├──────────────────────────────┤            │
│           │                  │ Rename…                      │            │
│           │                  │ Properties                   │            │
│           │                  └──────────────────────────────┘            │
│           │        (background right-click adds:                         │
│           │         "◷ Browse Backups of This Folder…")                  │
└──────────┴───────────────────────────────────────────────────────────────┘
```

- **On a file/folder:** *Restore Previous Version…* → opens Backtrack with that item selected.
- **On empty space:** *Browse Backups of This Folder…* → opens Backtrack at that path.
- That's the entire extension — deliberately boring, so it survives GNOME upgrades
  (the mistake Deja Dup made was going deeper).

---

## Screen 3 — Dolphin (KDE) integration

Same two menu entries (service menu + KAbstractFileItemActionPlugin), plus the KDE-only
bonus: a `backtrack:/` virtual location in Places, built exactly like KDE's own `timeline:/`.

```
┌──────────────────────────────────────────────────────────────────────────┐
│  Dolphin   ◀ ▶ ↑   backtrack:/Projects/                       🔍  ≡ ─ □ ✕│
├─────────────┬────────────────────────────────────────────────────────────┤
│ Places      │   📁 Today, 16:00        📁 Today, 15:00                   │
│  🏠 Home    │   📁 Today, 14:00        📁 Yesterday, 22:00               │
│  📄 Documents│  📁 Tue 10 June         📁 Mon 9 June                     │
│  ⬇ Downloads│   📁 Week of 2 June      📁 May 2026                       │
│  🗑 Trash   │                                                            │
│ ────────────│   Each folder = that snapshot of Projects/,                │
│ Backtrack   │   browsable read-only like any directory.                  │
│  ◷ backtrack:/ ← the KIO worker                                          │
│  ⏱ timeline:/  │                                                         │
├─────────────┴──────────────────────────────────────────────────────────  │
│  In a normal folder, right-click a file:                                 │
│                  ┌──────────────────────────────┐                        │
│                  │ Open                         │                        │
│                  │ Open With ▸                  │                        │
│                  ├──────────────────────────────┤                        │
│                  │ Cut    Copy    Rename        │                        │
│                  │ Move to Trash                │                        │
│                  ├──────────────────────────────┤                        │
│                  │ ◷ Backtrack ▸ Restore Previous Version…│              │
│                  │             ▸ Browse Backups of This Folder…│         │
│                  ├──────────────────────────────┤                        │
│                  │ Properties                   │                        │
│                  └──────────────────────────────┘                        │
└───────────────────────────────────────────────────────────────────────────┘
```

- Dave's story: right-click the image in Dolphin → *Backtrack ▸ Restore Previous Version…*
  → Backtrack opens with the image selected, preview pane showing each version.

---

## Screen 4 — Single-file conflict dialog

Shown only when the file on disk *actually differs* from the backup version
(identical files are silently skipped — "already up to date").

```
        ┌────────────────────────────────────────────────────┐
        │                                                    │
        │              Replace "report.odt"?                 │
        │                                                    │
        │   A file with this name already exists in          │
        │   Documents. The file on disk is NEWER than        │
        │   the backup version.                              │
        │                                                    │
        │   ┌──────────────────────────────────────────┐     │
        │   │ 📄 Current file                          │     │
        │   │    Modified 3 Jul 2026, 14:02 · 48 KB    │     │
        │   │    ⚠ newer than the backup               │     │
        │   ├──────────────────────────────────────────┤     │
        │   │ ◷ Backup version                         │     │
        │   │    From backup of 28 Jun 2026, 09:00     │     │
        │   │    45 KB                                 │     │
        │   └──────────────────────────────────────────┘     │
        │                                                    │
        │   Replaced files are kept as safety copies         │
        │   for 30 days.                                     │
        │                                                    │
        │   [ Cancel ]   [ Keep Both ]   [ 🔴 Replace ]      │
        │      ↑first          ↑              ↑red,          │
        │                 renames current      never the     │
        │                 → "report (current).odt"  default  │
        └────────────────────────────────────────────────────┘

   …and after any restore, a toast in the main window:

        ┌───────────────────────────────────────────┐
        │  ✓ Restored report.odt          [ Undo ]  │
        └───────────────────────────────────────────┘
```

---

## Screen 5 — Folder restore summary (no dialog storms)

Bob restores a whole folder: one summary screen, computed before anything is touched.

```
        ┌──────────────────────────────────────────────────────┐
        │                                                      │
        │        Restore "Projects" from 28 June?              │
        │                                                      │
        │   Restoring from the backup of 28 Jun 2026, 09:00    │
        │                                                      │
        │    ✓  214 files are identical — left alone           │
        │    ↺    6 files will be replaced          [Review…]  │
        │         (3 of them are newer on disk)                │
        │    ＋   2 files exist only in the backup — added     │
        │    ▣    4 files exist only on your disk — kept.      │
        │         Nothing is ever deleted.                     │
        │                                                      │
        │   Replaced files are kept as safety copies           │
        │   for 30 days.                                       │
        │                                                      │
        │  [ Cancel ]  [ Keep Both Versions ]  [ 🔴 Replace    │
        │                                       Changed Files ]│
        └──────────────────────────────────────────────────────┘
```

- **Review…** opens a per-file checklist (each row: both versions' dates/sizes, checkbox).
- Restores merge; they never delete disk-only files — and the dialog says so.

---

## Screen 6 — Calendar popover (long jumps)

Charlie knows *roughly* when he downloaded that file. The calendar button at the top of
the snapshot sidebar opens a popover; days with backups are shaded, clicking one scrolls
the snapshot list to that day.

```
   SNAPSHOTS  [📅]◄─ click
              ┌───────────────────────────┐
              │      ◀   June 2026   ▶    │
              │   Mo Tu We Th Fr Sa Su    │
              │    1  2  3  4  5  ６  ７   │
              │   ◉8 ◉9 ◉10 ◉11 (12)13 14 │   ◉ = has backups
              │  ◉15 ◉16 ◉17 ◉18 ◉19 20 21│  (12) = selected day
              │  ◉22 ◉23 ◉24 ◉25 ◉26 27 28│   plain = no backups
              │  ◉29 ◉30                  │
              │                           │
              │  Shaded days have backups │
              └───────────────────────────┘
```

## Screen 7 — Review checklist (per-file control in folder restores)

Opened from "Review…" in the folder-restore summary. Every conflicting file, both
versions side by side, checkbox per row.

```
        ┌──────────────────────────────────────────────────────┐
        │          Review files to be replaced                 │
        │     Restoring "Projects" from 28 Jun 2026, 09:00     │
        │                                                      │
        │   Select All · Select None                           │
        │   ┌────────────────────────────────────────────┐     │
        │   │ ☑ report.odt              ⚠ newer on disk  │     │
        │   │     On disk: 3 Jul, 14:02 · 48 KB          │     │
        │   │     Backup:  28 Jun, 09:00 · 45 KB         │     │
        │   │ ☑ budget.ods              ⚠ newer on disk  │     │
        │   │ ☐ notes.txt               ⚠ newer on disk  │     │
        │   │ ☑ logo-draft.png                           │     │
        │   │ ☑ summary.pdf                              │     │
        │   │ ☑ todo.md                                  │     │
        │   └────────────────────────────────────────────┘     │
        │   Unchecked files will be skipped.                   │
        │   Replaced files are kept as safety copies 30 days.  │
        │                                                      │
        │          [ Back ]        [ 🔴 Replace 5 Files ]      │
        └──────────────────────────────────────────────────────┘
```

## Screen 8 — Compare view

Bob's story: before restoring, see exactly what changed between the backup and today.

```
┌──────────────────────────────────────────────────────────────────────────┐
│                     Compare: report.odt                          ─ □ ✕   │
├───────────────────────────────────┬──────────────────────────────────────┤
│ ◷ Backup — 12 Jun, 09:00 · 45 KB  │ 📄 Today — 3 Jul, 14:02 · 48 KB      │
├───────────────────────────────────┼──────────────────────────────────────┤
│  Quarterly Report                 │  Quarterly Report                    │
│                                   │                                      │
│  1. Overview                      │  1. Overview                         │
│  This report provides…            │  This report provides…               │
│                                   │                                      │
│ ▓▓ 2. Old budget section ▓▓       │ ░░ 2. Rewritten budget section ░░    │
│ ▓▓ (removed since backup) ▓▓      │ ░░ (added since backup)        ░░    │
│                                   │                                      │
│  3. Next Steps                    │  3. Next Steps                       │
├───────────────────────────────────┴──────────────────────────────────────┤
│  ⚠ 2 sections differ     [ Restore This Version ]  [ Keep Current ]      │
└──────────────────────────────────────────────────────────────────────────┘
     ▓ = removed since backup      ░ = added since backup
```

---

## Screen 9 — Onboarding wizard

Four visible steps; everything else (exclusions, schedule, retention) lives behind
"Advanced" disclosures with safe defaults. An existing Borg repo short-circuits
straight to indexing.

```
  ┌─────────┐   ┌──────────┐   ┌──────────┐   ┌──────────────┐
  │ Welcome  │ ► │ 2. What  │ ► │ 3. Where │ ► │ 4. Protect   │ ► first backup
  │          │   │ to back  │   │ to back  │   │    & Go      │    starts,
  │          │   │ up       │   │ up to    │   │ (encryption) │    index builds
  └─────────┘   └──────────┘   └────┬─────┘   └──────────────┘
                                    │
                                    └─► "Existing Backtrack/Borg backup found —
                                         use it?" → skip creation, start indexing
```

### Step 1 — Welcome
```
┌──────────────────────────────────────────────────────┐
│                                                      │
│                     ◷  (app icon)                    │
│                                                      │
│              Welcome to Backtrack                    │
│         Browse your files as they were.              │
│                                                      │
│   ⏱  Backs up every hour, automatically             │
│   🕰  Slide back in time to any version              │
│   🔒  Encrypted, storage-efficient backups           │
│   🗂  Restore right from your file manager           │
│                                                      │
│                    [ Get Started ]                   │
│              Already have backups? Import…           │
└──────────────────────────────────────────────────────┘
```

### Step 2 — What to back up
```
┌──────────────────────────────────────────────────────┐
│  What should be backed up?              Step 2 of 4  │
│                                                      │
│   ◉ My personal files (recommended)                  │
│      Home folder: documents, pictures, music,        │
│      downloads, desktop — 118 GB                     │
│   ○ Let me choose folders…                           │
│                                                      │
│   ▸ Advanced: exclusions                             │
│     (pre-filled: Trash, caches, browser profiles,    │
│      node_modules, VM images — editable list)        │
│                                                      │
│                        [ Back ]   [ Continue ]       │
└──────────────────────────────────────────────────────┘
```

### Step 3 — Where to back up to
```
┌──────────────────────────────────────────────────────┐
│  Where should backups go?               Step 3 of 4  │
│                                                      │
│   ⬚ 💾 External drive        "WD My Passport, 2 TB   │
│                               free" (detected)       │
│   ⬚ 🖧  Network folder        SMB / NFS share         │
│   ⬚ 🔑 SSH server            Any Linux box, NAS,     │
│                               or BorgBase account    │
│   ⬚ ☁  Cloud storage         Coming later            │
│                               (greyed out)           │
│                                                      │
│   ✓ Selection fits: 118 GB needed, 2 TB free         │
│   ⓘ If this location isn't reachable, Backtrack      │
│     keeps protecting your changes on this computer   │
│     and catches up when it reconnects.               │
│     (see offline-strategy.md)                        │
│                                                      │
│                        [ Back ]   [ Continue ]       │
└──────────────────────────────────────────────────────┘
```

### Step 4 — Protect & Go
```
┌──────────────────────────────────────────────────────┐
│  Protect your backups                   Step 4 of 4  │
│                                                      │
│   Passphrase        [__________________________]     │
│   Confirm           [__________________________]     │
│   ▓▓▓▓▓▓▓░░░  strong                                 │
│   ☑ Remember it so backups run automatically         │
│                                                      │
│   ⚠ IMPORTANT: without the passphrase OR the         │
│     recovery key, backups cannot be read — by        │
│     anyone, ever.                                    │
│   [ ⬇ Save Recovery Key… ]   [ 🖨 Print… ]            │
│   (Continue stays disabled until one is done)        │
│                                                      │
│   ▸ Advanced: schedule & retention                   │
│     (hourly; keep hourly 24h · daily 1w · weekly     │
│      1m · monthly 6m; pause on battery — defaults)   │
│                                                      │
│              [ Back ]   [ Start First Backup ]       │
└──────────────────────────────────────────────────────┘
     └─► "First backup may take a few hours. After
          that, hourly backups take minutes. You can
          close this window."
```

---

## Screen 10 — Menu & Preferences

### The primary ("burger") menu

Follows the GNOME primary-menu convention: actions first, then app-level entries.

```
                                        ┌────────────────────────────┐
   header bar:   🔍  [ ≡ ]◄─ click      │  ⟳  Back Up Now            │
                                        │  ⏸  Pause Backups        ▸ │──┐
                                        ├────────────────────────────┤  │ For 1 hour
                                        │  🕘 Recently Replaced Files │  │ Until tomorrow
                                        ├────────────────────────────┤  │ Until I resume
                                        │  ⚙  Preferences            │  └────────────
                                        │  ⌨  Keyboard Shortcuts     │
                                        │  ❓ Help                    │
                                        │  ⓘ  About Backtrack        │
                                        └────────────────────────────┘
```

- **Back Up Now** — manual trigger, always available.
- **Pause Backups** — temporary, self-expiring options only; no permanent "off" buried
  in a menu (that lives in Preferences → Backup, deliberately).
- **Recently Replaced Files** — the safety stash from the conflict design, one click away.

### Preferences window (libadwaita, sidebar + pages)

```
┌────────────────────────────────────────────────────────────────────┐
│  Preferences                                              ─  □  ✕ │
├───────────────┬────────────────────────────────────────────────────┤
│ ▶ General     │  (selected page's content)                         │
│   Backup      │                                                    │
│   Storage     │                                                    │
│   Security    │                                                    │
│   Advanced    │                                                    │
└───────────────┴────────────────────────────────────────────────────┘
```

**General**
```
  Behaviour
  ├─ Run in background                                   [ ON  ⬤ ]
  │    Hourly backups continue while the window is closed
  └─ Notifications              [ Only when attention is needed ▾ ]

  File manager integration
  ├─ GNOME Files (Nautilus)  ✓ installed                 [ ON  ⬤ ]
  └─ Dolphin                 not installed               [ Install… ]

  Setup
  └─ Run Welcome Wizard Again…
       Reconfigure from the start — your backups are kept
```

**Backup**
```
  Schedule
  ├─ Frequency                                    [ Every hour  ▾ ]
  ├─ Back up on battery power                            [ OFF ○ ]
  ├─ Back up on metered connections                      [ OFF ○ ]
  └─ Next backup: today at 17:00

  What to back up
  ├─ 🏠 Home  ▸  Documents, Pictures, Music, Downloads, Desktop
  └─ [ Add Folder… ]

  Exclusions
  ├─ Trash · Caches · node_modules · VM images · *.iso
  └─ [ Add Exclusion… ]
```

**Storage**
```
  Destination
  ├─ Repository        smb://nas.local/backups/keith     [ Change… ]
  ├─ Space used        412 GB of 2 TB   (1.9 TB before dedup)
  └─ Last backup       Today, 16:00  ✓

  Retention
  └─ Automatic (recommended)                             [ ON  ⬤ ]
       keeps hourly 24h · daily 1 week · weekly 1 month ·
       monthly 6 months          (custom steppers when OFF)

  When the destination is unreachable        (see offline-strategy.md)
  ├─ Keep protecting changes on this computer            [ ON  ⬤ ]
  ├─ Space limit for local snapshots              [ 10 GB  ▾ ]
  └─ Currently using: 1.2 GB · 14 snapshots on this computer
```

**Security**
```
  Encryption
  ├─ 🛡 Encrypted — AES-256, key stored in repository
  ├─ Change Passphrase…
  ├─ Remember passphrase (system keyring)                [ ON  ⬤ ]
  └─ [ ⬇ Save Recovery Key… ]   [ 🖨 Print Recovery Key ]

  ⚠ Without the passphrase or the recovery key, backups
    cannot be read — by anyone, ever.
```

**Advanced**
```
  Catalogue
  ├─ 47 backups indexed · 132 MB on disk
  └─ [ Rebuild Catalogue ]

  Repository maintenance
  ├─ Verify Repository Health…        (borg check)
  ├─ Free Up Space Now…               (borg compact)
  └─ Compression                              [ zstd (recommended) ▾ ]

  Network
  └─ Limit backup upload speed               [ OFF ○ ]  ( __ MB/s )

  Troubleshooting
  ├─ Open Logs
  └─ Reset All Settings…            (red; backups are never deleted)
```

Notes:
- **Re-run wizard** lives in General → Setup, non-destructive by design.
- Anything destructive (Reset) is red, at the bottom of Advanced, and explicitly
  says backups survive it.
- The offline spool from [offline-strategy.md](offline-strategy.md) is user-visible
  and capped in Storage.

---

## Screen 11 — Cross-snapshot search (Charlie's story, done properly)

The search button in the header opens a search view that answers *"where did this
file go?"* — results are grouped **by file**, each showing its lifespan across
snapshots, including files that no longer exist today.

```
┌──────────────────────────────────────────────────────────────────────────┐
│  🔍  [ contract                                    ✕ ]           ─ □ ✕   │
├──────────────────────────────────────────────────────────────────────────┤
│  3 files matched across 47 backups                                       │
│                                                                          │
│  📄 contract-final.pdf                     ⚠ no longer on your disk      │
│      Downloads › contract-final.pdf                                      │
│      Existed: 26 Jun – 1 Jul · 8 versions · last 214 KB                  │
│      [ View in Timeline ]  [ ⚡ Restore Latest Version ]                  │
│                                                                          │
│  📄 contract-draft.odt                                                   │
│      Documents › Clients › contract-draft.odt                            │
│      Existed: 12 May – today · 31 versions                               │
│      [ View in Timeline ]                                                │
│                                                                          │
│  📁 old-contracts                          ⚠ no longer on your disk      │
│      Documents › Archive › old-contracts                                 │
│      Existed: 3 Mar – 14 Jun · folder, 12 files                          │
│      [ View in Timeline ]  [ ⚡ Restore Folder ]                          │
└──────────────────────────────────────────────────────────────────────────┘
```

- Search is instant (FTS5 over the local catalogue — no repo access).
- "No longer on your disk" results are ranked first: if you're searching, it's
  usually for something that's gone.
- **View in Timeline** jumps to the folder with the file selected, at the last
  snapshot where it existed — from there, normal preview/restore applies.
- **Restore Latest Version** is the one-click path for Charlie: latest version,
  original location, standard conflict handling.

## Screen 12 — Disaster recovery ("Restore Everything")

Entered from the wizard's "Already have backups? Import…" on a fresh machine
(or Preferences → Storage → Change… → import). After connecting the repo and
entering the passphrase, indexing starts newest-first and the flow offers:

```
┌──────────────────────────────────────────────────────────────────────┐
│              Welcome back. Restore this computer?                    │
│                                                                      │
│   Backups found for keith@fedora                                     │
│   Latest: Yesterday, 22:00 · 118 GB · 47 snapshots                   │
│                                                                      │
│   ◉ Restore everything (recommended)                                 │
│      Home folder as of [ Yesterday, 22:00 ▾ ]                        │
│   ○ Restore selected folders…                                        │
│   ○ Just browse — restore things later                               │
│                                                                      │
│   ⓘ Files already on this computer are never overwritten             │
│     without asking. New backups start after the restore              │
│     finishes.                                                        │
│                                                                      │
│                       [ Back ]   [ Start Restore ]                   │
└──────────────────────────────────────────────────────────────────────┘
          │
          ▼  (running — closable, survives via the daemon)
┌──────────────────────────────────────────────────────────────────────┐
│  Restoring your files…                             ⏸ Pause  ✕ Cancel │
│  ████████████████░░░░░░░░░░  61% · 72 of 118 GB · about 40 min left  │
│  Restoring: Pictures/2025/holiday/IMG_2041.jpg                       │
│  ✓ Documents  ✓ Music  ▶ Pictures  ○ Downloads  ○ Desktop            │
└──────────────────────────────────────────────────────────────────────┘
```

- Runs as per-top-level-folder extract jobs (staging → move), so it's resumable
  after interruption and progress is honest (byte totals from the catalogue).
- Conflict policy for DR: existing-and-different files go through the standard
  summary (typically near-zero on a fresh machine).
- Hourly backups begin automatically only after the restore completes, to avoid
  backing up a half-restored home.

---

## High-fidelity mockups

Generated with GPT Image 2 (via Higgsfield), saved in [mockups/](../mockups/).

### 1. The main Backtrack window
The snapshot list (left), step arrows, "backup 3 of 47" position label, status badges
("deleted after this", "changed since then"), live preview pane, timeline density strip,
and the Restore actions.

![Backtrack main window](../mockups/1-backtrack-main-window.png)

### 2. Nautilus integration
The single menu item the GNOME extension adds: *Restore Previous Version…*
(background right-click adds *Browse Backups of This Folder…*).

![Nautilus context menu](../mockups/2-nautilus-context-menu.png)

### 3. Dolphin integration
Both KDE integrations in one shot: the `backtrack:/` virtual location in Places
(snapshots browsable as ordinary time-named folders), and the *Backtrack* context-menu
submenu with both actions.

![Dolphin integration](../mockups/3-dolphin-integration.png)

### 4. Single-file conflict dialog
Cancel / Keep Both / Replace, with both versions' dates and sizes, the "newer" warning tag,
and the safety-copy reassurance line. Replace is red and not the default.

![Conflict dialog](../mockups/4-conflict-dialog.png)

### 5. Folder restore summary + Undo toast
One summary instead of a dialog storm; "nothing is deleted" stated outright;
the Undo toast that follows every restore.

![Folder restore summary](../mockups/5-folder-restore-summary.png)

### 6. Main window — dark theme
The same layout in Adwaita dark; badges and the selected snapshot stay legible.

![Backtrack main window, dark theme](../mockups/6-main-window-dark.png)

### 7. Calendar popover
The long-jump control: month view anchored to the sidebar's calendar button,
backup days shaded, selected day highlighted. (Note for the real build: days
*without* backups should render fully plain — the mockup shades almost every
day, which undersells the contrast.)

![Calendar popover](../mockups/7-calendar-popover.png)

### 8. Review checklist
Per-file control inside a folder restore: checkbox per file, both versions'
dates and sizes, "newer on disk" warnings, and a button that counts what it
will do ("Replace 5 Files").

![Review checklist](../mockups/8-review-checklist.png)

### 9. Compare view
Bob's pre-restore check: backup version and today's version side by side,
removed text flagged red on the left, added text green on the right,
"2 sections differ" summary, and Restore This Version / Keep Current Version.

![Compare view](../mockups/9-compare-view.png)

### 10. Onboarding — Welcome
The four-promise pitch and the "Already have backups? Import…" path for
existing Borg/Pika/Vorta users.

![Wizard: welcome](../mockups/10-wizard-welcome.png)

### 11. Onboarding — What to back up
"My personal files (Recommended)" vs "Let me choose folders…", with exclusions
behind the Advanced expander (Trash, caches, node_modules, VM images pre-excluded).

![Wizard: what to back up](../mockups/11-wizard-what.png)

### 12. Onboarding — Where to back up to
Detected external drive, network folder, SSH server (covers NAS/BorgBase),
cloud greyed out as "Coming later"; the space check and the offline
expectation-setting line — backups *continue locally* when the destination
is unreachable and catch up on reconnect (see [offline-strategy.md](offline-strategy.md)).

![Wizard: where to back up](../mockups/12-wizard-where.png)

### 13. Onboarding — Protect & Go
Passphrase with strength meter, keyring opt-in, the amber "cannot be read —
by anyone, ever" warning, mandatory Save Recovery Key / Print step, and
schedule & retention defaults behind Advanced. Ends with "Start First Backup".

![Wizard: protect and go](../mockups/13-wizard-protect.png)

### 14. Primary ("burger") menu
Back Up Now, self-expiring Pause options (for 1 hour / until tomorrow /
until I resume), Recently Replaced Files, then Preferences, Keyboard
Shortcuts, Help, About.

![Primary menu](../mockups/14-primary-menu.png)

### 15. Preferences — General
Run in background, notification policy ("Only when attention is needed"),
per-file-manager integration toggles with install state, and
**Run Welcome Wizard Again** ("your backups are kept").

![Preferences: General](../mockups/15-prefs-general.png)

### 16. Preferences — Backup
Frequency (hourly), battery/metered switches, next-backup time, the
backed-up folder list with Add Folder…, and the editable exclusion list
(Trash, Caches, node_modules, VM images, *.iso) with Add Exclusion….

![Preferences: Backup](../mockups/16-prefs-backup.png)

### 17. Preferences — Storage
Repository location + Change…, deduplicated space usage with progress bar,
last-backup status, Automatic retention (hourly 24h · daily a week · weekly
a month · monthly 6 months), and the offline local-protection controls from
[offline-strategy.md](offline-strategy.md): on/off, space cap, current usage.

![Preferences: Storage](../mockups/17-prefs-storage.png)

### 18. Preferences — Security
Encryption status, Change Passphrase…, keyring toggle, Save/Print Recovery
Key (available again any time, not just during onboarding), and the standing
warning card.

![Preferences: Security](../mockups/18-prefs-security.png)

### 19. Preferences — Advanced
Catalogue status + Rebuild, repository maintenance (Verify Health, Free Up
Space, compression), upload speed limit, logs, and red Reset All Settings —
which explicitly never deletes backups.

![Preferences: Advanced](../mockups/19-prefs-advanced.png)

### 20. Cross-snapshot search (Screen 11)
Results grouped by file, "no longer on your disk" ranked first, lifespan lines,
View in Timeline / Restore Latest Version actions.

![Search results](../mockups/20-search-results.png)

### 21. Disaster recovery — entry (Screen 12)
"Welcome back. Restore this computer?" with the three paths and the
never-overwrite / backups-start-after promises.

![DR welcome](../mockups/21-dr-welcome.png)

### 22. Disaster recovery — progress (Screen 12)
Honest byte-based progress, current file, per-folder checklist, Pause/Cancel.

![DR progress](../mockups/22-dr-progress.png)

### 23. Health — AT_RISK banner (see health.md)
The yellow "No successful backups for 3 days" banner with Fix… button.

![Health banner](../mockups/23-health-banner.png)

### 24. Health — passphrase recovery dialog (see health.md)
Re-entry dialog with keyring re-store checkbox and the recovery-key escape hatch.
*(Note: the dimmed background window in this render invents a different layout —
only the dialog is normative.)*

![Passphrase dialog](../mockups/24-passphrase-dialog.png)
