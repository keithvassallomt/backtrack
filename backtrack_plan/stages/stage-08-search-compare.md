# Stage 8 — Search & Compare

## Objective
Charlie's story done properly (cross-snapshot search with deleted-file discovery)
and Bob's pre-restore confidence (compare view).

## Prerequisites
Stages 1, 3, 6; Stage 7 for the restore actions. **Normative references:**
prototype.md Screens 11 (search) + 8 wireframe & Screen "Compare" (mockup 9);
mockups [20](../mockups/20-search-results.png), [9](../mockups/9-compare-view.png).

## Tasks

### S08-T1 — SearchFiles (daemon)
Wire S01-T4 `search()` through the existing D-Bus method: query → hits with path,
lifespan (first/last timestamps), version count, exists-today, kind. Debounce is
client-side; daemon enforces result cap (200) + query minimum (2 chars).
**Accept:** zbus test: fixture queries return ranked hits, deleted-first.

### S08-T2 — Search UI
Header search button / `Ctrl+F` → search mode (revealer over the main pane, per
mockup 20): result cards grouped BY FILE: icon, name, orange "no longer on your
disk" tag when gone, breadcrumb path, "Existed: X – Y · N versions · size" line.
Results stream in as typed (150 ms debounce); "N files matched across M backups"
caption; empty state with hint text.
**Accept:** layout matches mockup 20; Charlie fixture (file deleted 3 archives
ago) appears first with correct lifespan text.

### S08-T3 — Search actions
Per-card: **View in Timeline** → closes search, navigates to parent folder at the
LAST snapshot where the file existed, selects it (uses `--select` machinery from
S06-T1). **Restore Latest Version** (only when gone or changed) → Stage 7 pipeline
with the file's last version, original location, standard conflict handling.
Folder hits get Restore Folder analogously.
**Accept:** end-to-end Charlie walkthrough on demo-repo: search → restore latest →
file back on disk; View-in-Timeline lands selected at the right snapshot.

### S08-T4 — Compare view
Action-bar "Compare with Today" (file selected, changed-since-then) opens the
compare window per mockup 9: two panes with headers "Backup — date · size" /
"Today — date · size"; legend (green added-since-backup on the Today side, red
removed on the backup side). Text files: line diff (use the `similar` crate)
rendered with GtkSourceView background tags; images: side-by-side scaled previews
+ metadata only; binaries: metadata panel + "these files differ" (byte compare).
Footer: "N sections differ", buttons **Restore This Version** (→ Stage 7 single-file
flow) / **Keep Current Version** (close). Backup side content via PreviewFile fd.
**Accept:** text fixture with known 2-hunk diff renders 2 highlighted sections and
the footer count; image and binary fallbacks render; Restore This Version round-trips.

## Definition of Done
Charlie's story is a <30-second GUI walkthrough on demo data; compare handles
text/image/binary gracefully; CI green; progress.md + CHANGELOG updated.
