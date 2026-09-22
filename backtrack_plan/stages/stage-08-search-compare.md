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

### S08-T5 — Real images in the demo fixtures
Every "image" in the demo fixtures is ASCII text under a `.jpg`/`.png` name
(`Pictures/vacation.jpg` is the string `JPEG-BINARY`), so the preview pane's
texture branch has never rendered against demo data and S08-T4's image compare
has nothing to exercise. Give the `xtask` generator real, small image bytes:
`Pictures/vacation.jpg` appearing at day 10 and *changing* at least once after
it, so "an older version of this photo" is something a person can look at, plus
the `img/` files `just demo-conflicts` stages. Keep the assets small enough that
the repository stays light (synthesise them, or check in a few KB). The photo
is renamed to `.png` as part of this: the bytes are real now, and the name
should say what they are.
**Accept:** the preview pane renders `Pictures/vacation.png` as a picture at two
different snapshots, showing different images; S08-T4's image branch draws both
sides from the fixture.

### S08-T6 — "Not on your disk" has to be a fact about the disk
Both badges in the file pane are measured inside the catalogue: `deleted_after`
means "absent from the newest catalogued archive" and `changed_since` means "a
newer version opens after this one". At the newest backup both are false by
construction, so the Status column is structurally empty exactly where a person
spends most of their time — and a file deleted from disk ten minutes ago, which
is the product's whole reason for existing, is reported as unremarkable.

`SearchFiles` has the same hole under a more confident name: `exists_today` is
`last_seq == global_max`, "present in the latest archive", and S08-T2's orange
"no longer on your disk" tag is specified to be driven by it.

Give the daemon a live presence check — one `read_dir` per folder rather than a
stat per file — and a `PathsOnDisk` method to serve it, because the GUI cannot
be assumed to reach the user's files under Flatpak. Tri-state: present, absent,
or unknown, where unknown shows nothing; an archive from another machine, or a
folder that cannot be read, must never produce a claim that something was
deleted. One status per row: the new badge fills the case where the catalogue
believes the file is current and the disk disagrees, leaving Stage 6's
navigation signals as they are. S08-T1 then consumes the same resolver so
search's tag and its deleted-first ranking mean what they say.
**Accept:** a file present in the newest backup and deleted from disk is badged
in the file pane at that backup; a folder the daemon cannot read yields no
badge at all rather than a false one; zbus test over `PathsOnDisk` covering
present, absent and unknown.

## Definition of Done
Charlie's story is a <30-second GUI walkthrough on demo data; compare handles
text/image/binary gracefully; CI green; progress.md + CHANGELOG updated.
