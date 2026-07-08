# Stage 2 — Borg Adapter: Design

*Design date: 2026-07-08. Implements [backtrack_plan/stages/stage-02-borg-adapter.md](../../../backtrack_plan/stages/stage-02-borg-adapter.md).
Reference reading: [stack.md](../../../backtrack_plan/reference/stack.md) §4–5,
[open-questions.md](../../../backtrack_plan/reference/open-questions.md) Q3,
[health.md](../../../backtrack_plan/reference/health.md) failure catalogue.*

## Objective

`backtrack-core` can drive Borg 1.2/1.4 for every operation the product needs —
create, list, extract, prune, compact, check, repo init/import/key-export — with
typed errors, streamed progress, and the passphrase supplied from the system
keyring. Everything Borg-specific lives behind one trait so Borg 2 later is a
second implementation, not a rewrite.

## Key decisions

1. **Dynamic dispatch via `async-trait` + `Arc<dyn BackupEngine>`.** The whole
   architecture swaps engines (Borg 1 now, Borg 2 later, `MockEngine` in tests),
   which is the textbook case for a trait object. Making the daemon generic over
   `<E: BackupEngine>` would infect the job queue, scheduler, and D-Bus state
   with a type parameter to save one heap allocation per Borg call — negligible
   next to spawning a subprocess. Native async-in-traits cannot return
   `impl Stream` from a `dyn` trait anyway, so a runtime Borg-2 switch would force
   boxed streams regardless. Therefore: `async-trait`, boxed streams, and a
   **concrete `JobStream` struct** rather than `impl Trait` returns.

5. **`MockEngine` lives in a separate `backtrack-testkit` crate**, not in
   `backtrack-core`. The production library never compiles test scaffolding, and
   there is no `mock` feature flag to leak into a real daemon. Downstream crates
   (daemon, GUI) depend on `backtrack-testkit` as a **dev-dependency only**.

2. **Terminal job outcome is delivered in-band** as the final
   `JobEvent::Finished(Result<JobSummary, EngineError>)`. The `create()` future
   returns `Ok(JobStream)` as soon as the child spawns — long before Borg
   finishes or fails — so a job that fails mid-run must surface its error through
   the stream, not the constructor future.

3. **Cancellation lives on `JobStream`.** It owns the Borg child and a
   `CancellationToken`; `cancel()` sends SIGTERM, `Drop` kills the child. No job
   registry in core — the daemon (Stage 3) tracks job IDs and holds the handles.

4. **`CreateSpec` is minimal-but-honest.** Model only what a real backup and the
   integration tests exercise now (sources, archive name, excludes, compression,
   one-file-system). Retention policy, exclude-from files, checkpoint interval,
   and chunker params land in Stage 4 when the pipeline actually consumes them.

## Module layout (new in `backtrack-core`)

```
src/engine/
  mod.rs        # BackupEngine trait, JobStream, JobEvent, JobSummary, re-exports
  error.rs      # EngineError taxonomy + health.md mapping (+ exhaustiveness test)
  spec.rs       # CreateSpec, PrunePolicy, CheckLevel, RepoSpec, RepoInfo, ArchiveId
  borg/
    mod.rs      # BorgCli: the real BackupEngine impl
    invoke.rs   # process spawn, env, arg building, --version probe
    logjson.rs  # --log-json stderr JSONL → JobEvent parser
    classify.rs # exit-code + stderr-pattern → EngineError table
src/secret/
  mod.rs        # SecretStore trait
  keyring.rs    # oo7 (Secret Service) implementation
  file.rs       # BACKTRACK_DEV=1 file-backed store (headless CI)
```

The mock engine lives in a **new workspace crate** rather than in core:

```
crates/backtrack-testkit/
  src/lib.rs    # MockEngine (scripted JobEvents) impl BackupEngine,
                # plus a MockSecretStore and shared fixture helpers.
```

`backtrack-testkit` depends on `backtrack-core` (for the `BackupEngine` /
`SecretStore` traits it implements) and is added to the workspace `members` list.
Downstream crates list it under `[dev-dependencies]` only. It is unpublished
(`publish = false`) and, like `xtask`, sits outside the strict production lint
gates where that helps its scripted-fixture role.

New dependencies for `backtrack-core`: `tokio` (workspace), `async-trait`,
`futures` (Stream / AsyncRead / BoxStream), `tokio-util` (CancellationToken),
`oo7` (workspace). `async-trait`, `futures`, and `tokio-util` are added to the
workspace `[workspace.dependencies]` table and pinned there. `backtrack-testkit`
depends on `backtrack-core`, `async-trait`, and `futures`.

## Component 1 — The trait and streaming types (S02-T1)

```rust
#[async_trait]
pub trait BackupEngine: Send + Sync {
    async fn init_repo(&self, spec: &RepoSpec) -> Result<()>;
    async fn repo_info(&self) -> Result<RepoInfo>;
    async fn key_export(&self) -> Result<String>;
    async fn create(&self, spec: &CreateSpec) -> Result<JobStream>;
    async fn list_archive(&self, id: &ArchiveId)
        -> Result<BoxStream<'static, Result<BorgItem>>>;
    async fn extract(&self, id: &ArchiveId, paths: &[String], dest: &Path)
        -> Result<JobStream>;
    async fn extract_stdout(&self, id: &ArchiveId, path: &str)
        -> Result<Pin<Box<dyn AsyncRead + Send>>>;
    async fn prune(&self, policy: &PrunePolicy) -> Result<JobStream>;
    async fn compact(&self) -> Result<JobStream>;
    async fn check(&self, level: CheckLevel) -> Result<JobStream>;
}
```

- **`JobStream`** — concrete struct implementing `Stream<Item = JobEvent>` over a
  `tokio::sync::mpsc` receiver. Owns the Borg child process and a
  `CancellationToken`. Exposes `fn cancel(&self)`; `Drop` kills the child.
- **`JobEvent`** — `Progress { current, total, phase }` · `Log { level, msg }` ·
  `ItemDone { path }` · `Finished(Result<JobSummary, EngineError>)`. The stream
  ends after `Finished`.
- **`BorgItem`** is reused from Stage 1 (`index::BorgItem`), including its
  `from_json_line` parser and `parse_borg_mtime`. `list_archive` feeds the same
  type the index already ingests.

`spec.rs` types (minimal-but-honest):

```rust
pub struct CreateSpec {
    pub archive_name: String,
    pub sources: Vec<PathBuf>,
    pub excludes: Vec<String>,
    pub compression: Compression,   // default Zstd
    pub one_file_system: bool,
}
pub enum Compression { Zstd, Lz4, None }      // extended in Stage 4 if needed
pub struct PrunePolicy { /* keep-{hourly,daily,weekly,monthly} counts */ }
pub enum CheckLevel { Repository, Archives, Full }
pub struct RepoSpec { pub path: String, pub encryption: Encryption } // repokey-blake2
pub struct RepoInfo { /* id, last-modified, archive count, usable space if known */ }
pub struct ArchiveId(pub String);             // borg archive name or hex id
```

**Accept (T1):** trait compiles; `backtrack-testkit::MockEngine` implements it and
drives scripted `JobEvent`s (verified by a unit test in that crate); a unit test
in core asserts the health.md → `EngineError` mapping table is exhaustive over the
engine-relevant catalogue rows.

## Component 2 — Error taxonomy and health mapping (S02-T1)

`EngineError` (thiserror), exactly the stage's set:

```rust
pub enum EngineError {
    RepoUnreachable, PassphraseMissing, PassphraseWrong, AuthFailed,
    DestinationFull, LocalDiskFull, RepoCorrupt, LockedByOther,
    BorgMissing { needed: String, found: Option<String> },
    BorgFailed { code: i32, stderr: String },
}
```

A `const` mapping associates each **engine-relevant** row of health.md's failure
catalogue with a variant. The exhaustiveness test iterates the catalogue rows and
asserts each has a mapped variant. Rows that are *not* engine errors are
explicitly excluded with a documented reason:

| health.md row | Maps to |
|---|---|
| Passphrase missing | `PassphraseMissing` |
| Wrong passphrase | `PassphraseWrong` |
| Destination credentials expired (SMB/SSH auth) | `AuthFailed` |
| Destination full | `DestinationFull` |
| Local disk full (spool/staging) | `LocalDiskFull` |
| Repo corruption | `RepoCorrupt` |
| Borg missing / wrong version | `BorgMissing` |
| Repo unreachable (destination offline) | `RepoUnreachable` |
| Repo locked by another process | `LockedByOther` |
| (uncategorised borg failure) | `BorgFailed` |
| Index corruption | *excluded — SQLite layer, not the engine* |
| Backup interrupted (checkpoint) | *excluded — pipeline concern (Stage 4)* |
| Snapshot taken but indexing failed | *excluded — ingest concern (Stage 1/4)* |

`classify.rs` maps a finished process to an `EngineError`: Borg exit codes first
(≥2 = error, 1 = warning), then stderr `msgid`/pattern matching for the specific
ones (`Repository.DoesNotExist`, `PassphraseWrong`, `LockTimeout`,
`Repository.CheckNeeded`, ENOSPC text, ssh/mount auth failures). The mapping table
is documented in code.

## Component 3 — `BorgCli` implementation (S02-T2)

- Spawns `borg --log-json <subcommand> …` via `tokio::process::Command`, stderr
  piped. A reader task splits stderr into lines, parses each JSON object in
  `logjson.rs`, and forwards `JobEvent`s down the mpsc channel.
- Log-json message handling per the research rules: `archive_progress`,
  `progress_percent`, `progress_message`, `log_message`. **Never trust
  `progress_percent` for partial extracts** — the adapter forwards Borg's numbers
  tagged with a phase; honest restore percentages are computed later from index
  byte totals (Stages 4/7). `log_message` at error level is retained for
  classification.
- `list_archive` runs `borg list --json-lines` with an explicit `--format`
  carrying size, mtime, mode, and type, parsed via `BorgItem::from_json_line`.
- Environment: `BORG_PASSPHRASE` injected from the `SecretStore` into the child
  env only (never written to disk), `BORG_RELOCATED_REPO_ACCESS_IS_OK=no`, and
  locale pinned (`LC_ALL=C.UTF-8`, `LANG=C.UTF-8`) so message parsing is stable.
- **Version probe** on construction: `borg --version` parsed; require ≥ 1.2, else
  `BorgMissing { needed: ">=1.2", found }`.

**Accept (T2):** feature-gated integration tests (real borg, CI) — init temp repo
→ create 2 archives from a fixture tree → `list` streams the correct items →
`extract_stdout` of a single file matches the source bytes → prune + compact runs;
a wrong passphrase yields `PassphraseWrong`; an unplugged/absent path yields
`RepoUnreachable`.

## Component 4 — Passphrase provider (S02-T3)

```rust
#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn get(&self, repo_id: &str) -> Result<String>;   // missing → PassphraseMissing
    async fn set(&self, repo_id: &str, passphrase: &str) -> Result<()>;
    async fn delete(&self, repo_id: &str) -> Result<()>;
}
```

Keyed by a stable attribute set: `app-id = io.github.keithvassallomt.Backtrack`
plus `repo-id`. `oo7` (Secret Service) implementation for real use; a file-backed
implementation gated on `BACKTRACK_DEV=1` keeps CI headless. A missing entry
returns `PassphraseMissing` — never a prompt; prompting is the UI's job
(health.md), and this exact path powers mockup 24 later.

**Accept (T3):** round-trip test against the file store; the daemon integration
test (Stage 3) will assert that deleting the key makes the next backup fail with
`PassphraseMissing`.

## Component 5 — Repo lifecycle operations (S02-T4)

- `init_repo` — `borg init --encryption=repokey-blake2` on a local path,
  `ssh://…`, or a mounted-share path.
- `import_repo` — open an existing repo, verify the passphrase via a cheap
  authenticated operation (e.g. `repo_info`/`list` of archives), and read the
  archive list.
- `key_export` — `borg key export` text form for the wizard's Save/Print recovery
  key.

SMB/NFS destinations are treated as mounted paths; GIO/gvfs mount handling is
Stage 9's problem — core just takes a `Path`.

**Accept (T4):** integration test — init → `key_export` non-empty → `import_repo`
with the right passphrase succeeds and lists archives, with the wrong passphrase
yields `PassphraseWrong`.

## Component 6 — CI integration suite (S02-T5)

`just test-integration` (existing recipe, `--features integration`) runs the
real-borg tests; the CI job installs borg in the Fedora container and runs them.
Each test < 30 s, parallel temp dirs. Add a flake gate: the suite must be green
two consecutive runs.

**Accept (T5):** suite green in CI two consecutive runs.

## Testing strategy

- **Unit (default `cargo test`, borg-free):** in `backtrack-core` — log-json
  line → `JobEvent` parsing against fixture JSONL; `classify.rs`
  exit-code/stderr → `EngineError` cases; the health.md mapping exhaustiveness
  test; `SecretStore` file-store round-trip. In `backtrack-testkit` —
  `MockEngine` drives a scripted job to `Finished`.
- **Integration (`--features integration`, real borg):** the T2/T4 round trips
  above.

## Definition of done

All accept criteria pass; `MockEngine` available via the `backtrack-testkit`
crate for later stages; `progress.md`
checkboxes updated per task in the same commit as the work; decision notes (exact
borg flags chosen, `--format` string, classification patterns) appended to
`progress.md` Notes; board synced via `just sync-board-apply`.
