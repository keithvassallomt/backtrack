# Backtrack — Technical Architecture & Stack

*Decided 4 July 2026. Companion documents: [open-questions.md](open-questions.md) (research),
[prototype.md](prototype.md) (UX), [offline-strategy.md](offline-strategy.md) (offline design).*

## Decisions at a glance

| Area | Decision |
|---|---|
| Core language | **Rust** (Cargo workspace) |
| UI toolkit | **GTK4 + libadwaita** (gtk4-rs) |
| Process model | **Daemon (`backtrackd`) + thin clients over D-Bus** |
| Backup engine | **Borg 1.2/1.4 CLI subprocess** behind an adapter trait |
| Index | **SQLite** (WAL), interval-encoded schema, FTS5 filename search |
| Local API | **D-Bus** session service `org.backtrack.Daemon1` + `backtrack` CLI |
| Network API | None in v1 |
| Packaging | **Flatpak and distro-native (RPM/DEB) from day one** |
| License / identity | GPL-3.0-or-later · app ID `io.github.keithvassallomt.Backtrack` (see [brief.md](brief.md)) |
| v1 extras | Compare view, btrfs snapshot mode, cross-snapshot search, guided disaster recovery, health model ([health.md](health.md)) |
| Deferred to v1.x | `backtrack:/` KIO worker, compiled Dolphin FileItemAction plugin, Borg 2, cloud destinations |

Rationale highlights: Rust matches the always-on daemon's reliability/memory needs and
has precedent (Pika Backup is Rust); the 19 mockups are already Adwaita-styled; KDE
integration lives in Dolphin plugins/KIO, not the app toolkit, so a GTK app does not
penalise KDE users; Borg is Python, so linking is unrealistic — a CLI adapter also makes
the Borg 2 migration a second implementation of one trait.

## 1. Repository layout

```
backtrack/
├─ crates/
│  ├─ backtrack-core     # library: borg adapter, index, spool, restore engine, config
│  ├─ backtrackd         # daemon binary: scheduler, D-Bus service, job queue
│  ├─ backtrack-gtk      # GTK4/libadwaita app (timeline, wizard, prefs, compare)
│  └─ backtrack-cli      # thin CLI over D-Bus (backup now, status, restore, doctor)
├─ integrations/
│  ├─ nautilus/          # ~100-line nautilus-python MenuProvider extension
│  └─ dolphin/           # .desktop service menu (v1); KF6 C++ plugin + KIO worker (v1.x)
└─ packaging/            # flatpak manifest, RPM spec, deb rules, systemd user units
```

Three processes at runtime: `backtrackd` (systemd user service, D-Bus-activatable),
the GTK app, and transient `borg` subprocesses owned by the daemon.

**Single-writer rule:** only the daemon touches the Borg repos or writes the index.
The GUI reads the index directly (SQLite WAL permits concurrent readers) and performs
every mutation via D-Bus. This eliminates cross-process locking entirely.

## 2. D-Bus API — `org.backtrack.Daemon1` (session bus)

**Methods**

| Method | Purpose |
|---|---|
| `BackupNow()` | Manual backup trigger |
| `Pause(until: timestamp)` / `Resume()` | Self-expiring pause (menu options map here) |
| `GetStatus()` | State, last/next backup, destination reachability, spool usage |
| `RestoreFiles(archive, paths[], dest, policy) → job` | policy = ask / keep-both / replace / skip-identical |
| `PreviewFile(archive, path) → fd` | Extracted content over a pipe fd; daemon caches by chunk hash |
| `CompareFile(archive, path) → job` | Produces a diff artifact for the compare view |
| `Prune()` / `Verify()` / `Compact()` | Maintenance (Preferences → Advanced) |
| `CancelJob(id)` / `PauseJob(id)` | Cancel / pause any long-running job |
| `SetConfig(key, value)` / `GetConfig()` | GUI edits config through the daemon |
| `SetupRepo(...)` / `ImportRepo(path)` | Wizard operations (create vs adopt existing repo) |
| `SearchFiles(query) → results` | FTS5 cross-snapshot search over the index (grouped-by-file results incl. deleted files) |
| `RestoreEverything(archive, policy) → job` | Guided disaster recovery: per-top-level-folder extract jobs, resumable |

**Signals:** `BackupProgress(job, phase, current, total)`, `RestoreProgress(job, bytes, total)`,
`IndexingProgress(archive, pct)`, `StatusChanged(state)`. Long operations return a job ID;
signals carry it. Restore progress is computed from the index's byte totals, not Borg's
percent (unreliable on partial extracts — see research).

The CLI maps this interface one-to-one and doubles as the debug tool
(`backtrack status --json`, `backtrack doctor`).

## 3. Data layout

All under `~/.local/share/backtrack/`:

```
index.db      # SQLite, WAL mode
spool/        # offline spool repo (absent when btrfs mode is active)
replaced/     # 30-day safety stash from restores
staging/      # restore staging area (same fs as targets where possible)
logs/         # rotating JSONL logs
config.toml   # source of truth; GUI edits via D-Bus SetConfig
```

**Index schema** (interval encoding — one row per *version*, not per file×archive):

```sql
archives(seq INTEGER PRIMARY KEY, borg_id TEXT, name TEXT, ts INTEGER,
         repo TEXT CHECK(repo IN ('primary','spool','fs-snapshot')), status TEXT);
paths(id INTEGER PRIMARY KEY, parent_id INTEGER, name TEXT);        -- deduped tree
versions(path_id INTEGER, first_seq INTEGER, last_seq INTEGER,
         size INTEGER, mtime INTEGER, mode INTEGER, kind TEXT, chunk_hash TEXT);
fts_names USING fts5(name);                                          -- filename search
meta(key TEXT PRIMARY KEY, value);       -- schema_version, last_network_archive, ...
```

Timeline query: `WHERE parent_id=? AND first_seq<=?1 AND last_seq>=?1` — indexed, instant,
works offline. Expected size ≈ 50–150 MB at 500k files × 100 archives. Spool and
filesystem-snapshot archives share these tables (distinguished by `repo`), which makes
the "on this computer" badge and unified restores trivial.

**Ingest:** after each `borg create`, stream `borg list --json-lines` for the new archive
into one transaction, diffing against the previous archive's rows in SQL to open/close
validity intervals (never `borg diff` — it reads two metadata streams; see research).
First run on an existing repo indexes newest-first so the app is useful within minutes.

## 4. Borg adapter

`trait BackupEngine` in backtrack-core; v1 implementation `BorgCli` targeting Borg 1.2/1.4:

- Spawns `borg` with `--log-json`; parses stderr JSONL into typed progress/log events.
- Passphrase via Secret Service API (system keyring); exported to the child as
  `BORG_PASSPHRASE` only, never written to disk.
- Exit codes and error messages mapped to a typed error taxonomy.
- Flatpak bundles a pinned borg; native packages depend on the distro's borg
  (minimum version enforced at startup, surfaced by `backtrack doctor`).
- Borg 2 later = a second `BackupEngine` implementation; `borg transfer` will then allow
  consolidating spool archives into the primary repo (see offline-strategy.md).

## 5. Backup pipeline

Timer tick (systemd user timer or internal tokio timer) or `BackupNow()`:

1. Preflight: pause state, battery (UPower) and metered (NetworkManager) checks per config.
2. Probe destination reachability.
3. **Reachable:** `borg create` → stream-index → `borg prune` per policy (compact on its
   own schedule, off-peak) → update `meta.last_network_archive`. If spool/fs-snapshot
   archives exist from an offline window, this backup is the catch-up; mark them expirable
   (kept ~30 days).
4. **Unreachable:** per offline-strategy.md —
   - home on **btrfs** (v1): read-only subvolume snapshot, registered in `archives` as
     `repo='fs-snapshot'`, auto-expiring;
   - otherwise: changed-file list computed from the index (`mtime/size` vs
     `last_network_archive`) → `borg create` into the capped **spool** repo (same
     passphrase), indexed like any archive.
5. Status copy while offline is informational, never an error.

## 6. Restore engine

One structure for every restore: **extract to `staging/` → per-file compare → atomic
`rename()` into place.** This yields, for free: skip-identical ("already up to date"),
Keep Both (current file renamed `name (current).ext`), the `replaced/` stash (30-day
expiry, browsable as "Recently Replaced Files"), Undo toasts, and the folder-merge
summary (counts computed during the compare pass, before anything is touched).

Rules: symlinks restored as links, never followed; type-changed entries surfaced in the
Review list, never silently replaced; per-file permission failures reported without
aborting the job; free-space check (staging + stash) up front; merge never deletes
disk-only files.

## 7. GTK app

- Timeline browser: snapshot sidebar (`GtkListView` over an index view-model, grouped
  Today/Yesterday/…), file pane for the selected folder+snapshot, preview pane fed by
  `PreviewFile` fds, thin timeline density strip (custom widget, indicator + coarse jump).
- Wizard: `AdwNavigationView` pages (mockups 10–13); repo operations via `SetupRepo`/`ImportRepo`.
- Preferences: `AdwPreferencesWindow` (mockups 15–19), all writes via `SetConfig`.
- Compare view (v1): text diff via GtkSourceView; images shown side-by-side only.
- Launch contract used by plugins: `backtrack --path <dir> [--select <file>]`.

## 8. File-manager integration (v1)

Launchers only, per research. Nautilus: nautilus-python extension adding
*Restore Previous Version…* (selection) and *Browse Backups of This Folder…* (background).
Dolphin: service-menu `.desktop` with the same two actions. Both ship **only in native
packages**; the Flatpak app detects their absence and points to the distro package from
Preferences → General. The compiled KF6 FileItemAction plugin and `backtrack:/` KIO
worker are v1.x.

## 9. Packaging (both from day one)

- **Flatpak:** app + daemon + bundled borg in one package; daemon autostart via the
  Background portal; repo/destination access via filesystem permissions. Accepted
  limitation: no file-manager plugins from the sandbox.
- **Native:** Fedora COPR + Ubuntu PPA initially; packages `backtrack`
  (app + daemon + CLI + systemd user units), `backtrack-nautilus`, `backtrack-dolphin`.
- CI builds all artifacts on every merge so the two packaging paths cannot drift.

## 10. Errors, logging, diagnostics

Typed errors in core (`RepoUnreachable`, `PassphraseMissing`, `DiskFull`,
`BorgFailed{code}`, `IndexCorrupt`, …) → named D-Bus errors → single source of GUI copy.
Rotating JSONL logs in `logs/`; `backtrack doctor` produces a redacted diagnostic bundle
(versions, config, recent log tail, repo/index health).

## 11. Testing

- **Unit (core):** index ingest/interval logic, changed-file detection, conflict
  resolution and stash behaviour against fixture trees, config migration.
- **Integration:** real borg in CI against temp repos — create/index/restore/prune round
  trips; offline-window simulation (spool + catch-up); btrfs path behind a loopback image.
- **GUI:** view-models tested against a mock D-Bus daemon; smoke test drives the real
  daemon headlessly.
- **Dev tooling:** `make demo-repo` seeds a repo + index with a scripted history for UI
  work and screenshots.

## 12. Roadmap

| Milestone | Contents |
|---|---|
| **v1** | Core (hourly backups, timeline browser, restore + conflicts + stash/undo, onboarding, preferences, offline spool, Nautilus/Dolphin menus) + Compare view + btrfs snapshot mode + cross-snapshot search + guided disaster recovery + health model per [health.md](health.md) |
| **v1.x** | `backtrack:/` KIO worker, compiled Dolphin plugin, Flathub listing polish, per-file "next change" power-step if not landed in v1 |
| **v2** | Borg 2 engine (repo migration, `borg transfer` spool consolidation), cloud destinations via borgstore/rclone |
