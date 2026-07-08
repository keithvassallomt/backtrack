# Stage 2 — Borg Adapter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `backtrack-core` one trait, `BackupEngine`, that drives Borg 1.2/1.4 for every product operation (create, list, extract, prune, compact, check, repo init/import/key-export) with typed errors, streamed progress, and the passphrase supplied from the system keyring.

**Architecture:** A `#[async_trait]` `BackupEngine` trait held by the daemon as `Arc<dyn BackupEngine>`. Job-style operations return a concrete `JobStream` (a `Stream` over a `tokio::sync::mpsc` channel) that owns the Borg child and a `CancellationToken`; the terminal outcome is delivered in-band as the final `JobEvent::Finished(Result<..>)`. The real implementation, `BorgCli`, spawns `borg --log-json` via `tokio::process` and parses stderr JSONL into events. A separate `backtrack-testkit` crate provides `MockEngine`/`MockSecretStore` as dev-dependencies for later stages.

**Tech Stack:** Rust, tokio (process + mpsc), async-trait, futures (`Stream`/`AsyncRead`/`BoxStream`), tokio-util (`CancellationToken`), oo7 (Secret Service), thiserror, serde_json. Reuses Stage 1's `index::BorgItem` / `parse_borg_mtime`.

## Global Constraints

- **Borg floor:** require `borg >= 1.2` at `BorgCli` construction; below that → `EngineError::BorgMissing { needed: ">=1.2", found }`.
- **License headers:** every file under `crates/**/*.rs` must start with the two SPDX lines (see any existing source file). `just check-license-headers` enforces it.
- **No `println!`/`eprintln!`/`dbg!` under `crates/` outside `#[cfg(test)]`** — CI greps for it. Use `tracing`.
- **Passphrase never on disk:** injected into the Borg child's env as `BORG_PASSPHRASE` only.
- **Locale + relocation pinned on every borg call:** `LC_ALL=C.UTF-8`, `LANG=C.UTF-8`, `BORG_RELOCATED_REPO_ACCESS_IS_OK=no`.
- **Quality gate is `just check`** (fmt, `clippy -D warnings`, `cargo test --workspace`, license headers). Integration tests are `just test-integration` (`cargo test --workspace --features integration`), gated on borg presence.
- **Board contract (CLAUDE.md):** flip the `progress.md` checkbox for an `S02-T#` in the *same commit* as the work; run `just sync-board-apply` after any checkbox change. `[/]` when started, `[x]` only when that task's acceptance passes.
- **Version inheritance:** new crates use `version.workspace = true` and the shared `[lints] workspace = true`.

**Task → S02-T# map** (checkbox flips noted per task):

| Plan task | S02-T# | Checkbox action |
|---|---|---|
| 1 Error taxonomy | T1 | mark T1 `[/]` |
| 2 Trait + JobStream + spec | T1 | — |
| 3 SecretStore | T3 | mark T3 `[x]` |
| 4 testkit MockEngine/MockSecretStore | T1 | mark T1 `[x]` |
| 5 log-json parser | T2 | mark T2 `[/]` |
| 6 exit classification | T2 | — |
| 7 BorgCli implementation | T2 | — |
| 8 Borg integration suite | T2 | mark T2 `[x]` |
| 9 Repo lifecycle | T4 | mark T4 `[x]` |
| 10 CI flake gate + notes | T5 | mark T5 `[x]` |

---

### Task 1: Engine error taxonomy + health mapping (S02-T1)

**Files:**
- Create: `crates/backtrack-core/src/engine/mod.rs` (module root; grows over tasks 1–2)
- Create: `crates/backtrack-core/src/engine/error.rs`
- Modify: `crates/backtrack-core/src/lib.rs` (add `pub mod engine;`)
- Modify: `backtrack_plan/progress.md` (mark S02-T1 `[/]`)
- Test: inline `#[cfg(test)]` in `error.rs`

**Interfaces:**
- Produces: `EngineError` (enum), `type Result<T> = std::result::Result<T, EngineError>`, `HealthFailure` (enum) with `HealthFailure::ALL: &[HealthFailure]`, and `EngineError::health_failure(&self) -> HealthFailure`.

- [ ] **Step 1: Add the module to the crate root**

In `crates/backtrack-core/src/lib.rs`, after `pub mod index;` add:

```rust
pub mod engine;
```

Create `crates/backtrack-core/src/engine/mod.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The Borg adapter: one trait, [`BackupEngine`], behind which every
//! Borg-specific operation lives, plus its typed error taxonomy and streamed
//! job events. Borg 2 later is a second implementation, not a rewrite.

mod error;

pub use error::{EngineError, HealthFailure, Result};
```

- [ ] **Step 2: Write the failing test**

Create `crates/backtrack-core/src/engine/error.rs` with only the test module first:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// One representative EngineError per variant.
    fn one_of_each() -> Vec<EngineError> {
        vec![
            EngineError::RepoUnreachable,
            EngineError::PassphraseMissing,
            EngineError::PassphraseWrong,
            EngineError::AuthFailed,
            EngineError::DestinationFull,
            EngineError::LocalDiskFull,
            EngineError::RepoCorrupt,
            EngineError::LockedByOther,
            EngineError::BorgMissing { needed: ">=1.2".into(), found: None },
            EngineError::BorgFailed { code: 2, stderr: "boom".into() },
        ]
    }

    #[test]
    fn every_health_row_is_covered_by_some_error() {
        let produced: HashSet<HealthFailure> =
            one_of_each().iter().map(|e| e.health_failure()).collect();
        let expected: HashSet<HealthFailure> = HealthFailure::ALL.iter().copied().collect();
        assert_eq!(
            produced, expected,
            "every engine-relevant health.md row must map to at least one EngineError"
        );
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p backtrack-core engine::error`
Expected: FAIL to compile — `EngineError` / `HealthFailure` not found.

- [ ] **Step 4: Write the implementation**

Prepend to `crates/backtrack-core/src/engine/error.rs` (above the test module):

```rust
//! Typed engine errors and their mapping to the health.md failure catalogue.
//!
//! Each **engine-relevant** row of health.md maps to exactly one [`HealthFailure`];
//! rows that are not engine failures (index corruption — SQLite; interrupted
//! backup — checkpoint; snapshot-taken-but-indexing-failed — ingest) are
//! deliberately absent, because the engine cannot raise them.

/// A convenience result alias for the engine layer.
pub type Result<T> = std::result::Result<T, EngineError>;

/// Everything the Borg adapter can fail with. Maps 1:1 onto the engine-relevant
/// rows of the health.md failure catalogue via [`EngineError::health_failure`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    #[error("the backup destination is unreachable")]
    RepoUnreachable,
    #[error("no passphrase is stored for this repository")]
    PassphraseMissing,
    #[error("the stored passphrase no longer matches the repository")]
    PassphraseWrong,
    #[error("authentication to the destination failed")]
    AuthFailed,
    #[error("the backup destination is full")]
    DestinationFull,
    #[error("not enough space on this computer")]
    LocalDiskFull,
    #[error("the repository is corrupt and needs repair")]
    RepoCorrupt,
    #[error("the repository is locked by another process")]
    LockedByOther,
    #[error("borg {needed} is required (found {found:?})")]
    BorgMissing { needed: String, found: Option<String> },
    #[error("borg exited with code {code}: {stderr}")]
    BorgFailed { code: i32, stderr: String },
}

/// The engine-relevant rows of health.md's failure catalogue. Used to prove the
/// error taxonomy is exhaustive over failures the engine can actually detect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HealthFailure {
    PassphraseMissing,
    PassphraseWrong,
    AuthExpired,
    DestinationFull,
    LocalDiskFull,
    RepoCorrupt,
    BorgMissing,
    RepoUnreachable,
    LockedByOther,
    UncategorisedBorgFailure,
}

impl HealthFailure {
    /// Every catalogue row, for the exhaustiveness test.
    pub const ALL: &'static [HealthFailure] = &[
        HealthFailure::PassphraseMissing,
        HealthFailure::PassphraseWrong,
        HealthFailure::AuthExpired,
        HealthFailure::DestinationFull,
        HealthFailure::LocalDiskFull,
        HealthFailure::RepoCorrupt,
        HealthFailure::BorgMissing,
        HealthFailure::RepoUnreachable,
        HealthFailure::LockedByOther,
        HealthFailure::UncategorisedBorgFailure,
    ];
}

impl EngineError {
    /// Which health.md catalogue row this error surfaces as. The `match` is total,
    /// so the compiler guarantees every error variant is classified.
    pub fn health_failure(&self) -> HealthFailure {
        match self {
            EngineError::RepoUnreachable => HealthFailure::RepoUnreachable,
            EngineError::PassphraseMissing => HealthFailure::PassphraseMissing,
            EngineError::PassphraseWrong => HealthFailure::PassphraseWrong,
            EngineError::AuthFailed => HealthFailure::AuthExpired,
            EngineError::DestinationFull => HealthFailure::DestinationFull,
            EngineError::LocalDiskFull => HealthFailure::LocalDiskFull,
            EngineError::RepoCorrupt => HealthFailure::RepoCorrupt,
            EngineError::LockedByOther => HealthFailure::LockedByOther,
            EngineError::BorgMissing { .. } => HealthFailure::BorgMissing,
            EngineError::BorgFailed { .. } => HealthFailure::UncategorisedBorgFailure,
        }
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p backtrack-core engine::error`
Expected: PASS (`every_health_row_is_covered_by_some_error`).

- [ ] **Step 6: Mark the task in progress and commit**

In `backtrack_plan/progress.md`, change `- [ ] S02-T1 …` to `- [/] S02-T1 …`.

```bash
git add crates/backtrack-core/src/lib.rs crates/backtrack-core/src/engine backtrack_plan/progress.md
git commit -m "S02-T1: engine error taxonomy + health.md mapping"
```

(Board sync happens when T1 is marked `[x]` in Task 4.)

---

### Task 2: BackupEngine trait, JobStream, and spec types (S02-T1)

**Files:**
- Create: `crates/backtrack-core/src/engine/spec.rs`
- Create: `crates/backtrack-core/src/engine/job.rs`
- Modify: `crates/backtrack-core/src/engine/mod.rs` (trait + re-exports)
- Modify: `crates/backtrack-core/Cargo.toml` (deps)
- Modify: `Cargo.toml` (workspace deps)
- Test: inline `#[cfg(test)]` in `job.rs`

**Interfaces:**
- Consumes: `EngineError`, `Result` (Task 1); `index::BorgItem` (Stage 1).
- Produces:
  - `enum JobEvent { Progress { current: u64, total: Option<u64>, phase: String }, Log { level: LogLevel, msg: String }, ItemDone { path: String }, Finished(std::result::Result<JobSummary, EngineError>) }`
  - `enum LogLevel { Debug, Info, Warning, Error }`
  - `struct JobSummary { pub archive_id: Option<String> }` (Default)
  - `struct JobStream` with `pub fn cancel(&self)`, `pub fn from_events(impl IntoIterator<Item = JobEvent> + Send + 'static) -> JobStream`, `pub(crate) fn new(mpsc::Receiver<JobEvent>, CancellationToken) -> JobStream`, `impl Stream<Item = JobEvent>`
  - `struct CreateSpec`, `enum Compression`, `struct PrunePolicy`, `enum CheckLevel`, `struct RepoSpec`, `enum Encryption`, `struct RepoInfo`, `struct ArchiveId`
  - `trait BackupEngine` (see Step 4)

- [ ] **Step 1: Add dependencies**

In the workspace `Cargo.toml` `[workspace.dependencies]` table, add:

```toml
async-trait = "0.1"
futures = "0.3"
tokio-util = "0.7"
```

In `crates/backtrack-core/Cargo.toml` under `[dependencies]`, add (keep existing lines):

```toml
tokio.workspace = true
async-trait.workspace = true
futures.workspace = true
tokio-util.workspace = true
oo7.workspace = true
```

> If `cargo build` reports `CancellationToken` missing, change the workspace line to `tokio-util = { version = "0.7", features = ["rt"] }`.

- [ ] **Step 2: Write the failing test**

Create `crates/backtrack-core/src/engine/job.rs` with just the header + test:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn from_events_yields_each_event_then_ends() {
        let mut s = JobStream::from_events(vec![
            JobEvent::Progress { current: 1, total: Some(2), phase: "archiving".into() },
            JobEvent::ItemDone { path: "home/a".into() },
            JobEvent::Finished(Ok(JobSummary::default())),
        ]);
        assert!(matches!(s.next().await, Some(JobEvent::Progress { current: 1, .. })));
        assert!(matches!(s.next().await, Some(JobEvent::ItemDone { .. })));
        assert!(matches!(s.next().await, Some(JobEvent::Finished(Ok(_)))));
        assert!(s.next().await.is_none());
    }

    #[tokio::test]
    async fn cancel_stops_the_stream_early() {
        let mut s = JobStream::from_events(
            (0..1000).map(|i| JobEvent::ItemDone { path: format!("f{i}") }),
        );
        assert!(s.next().await.is_some());
        s.cancel();
        // Drain: cancellation makes the feeder stop; the stream ends.
        while s.next().await.is_some() {}
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p backtrack-core engine::job`
Expected: FAIL to compile — `JobStream` / `JobEvent` / `JobSummary` not found.

- [ ] **Step 4: Write the implementation**

Prepend to `crates/backtrack-core/src/engine/job.rs` (above the tests):

```rust
//! Streamed job events and the [`JobStream`] that carries them. A job's terminal
//! outcome is delivered in-band as the final [`JobEvent::Finished`], because a
//! Borg process only fails *after* the `create()` future has already returned.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::EngineError;

/// Severity of a `log_message` line from Borg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

/// A summary of a finished job. Minimal in Stage 2; extended with byte/file stats
/// when the backup pipeline (Stage 4) needs them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobSummary {
    pub archive_id: Option<String>,
}

/// One event from a running Borg job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobEvent {
    /// Progress tick. `total` is `None` when Borg cannot give a denominator
    /// (e.g. partial extracts — never trust `progress_percent` there).
    Progress { current: u64, total: Option<u64>, phase: String },
    /// A forwarded `log_message`.
    Log { level: LogLevel, msg: String },
    /// One item finished (from `file_status`).
    ItemDone { path: String },
    /// Terminal event: the job's outcome. The stream ends after this.
    Finished(std::result::Result<JobSummary, EngineError>),
}

/// A live Borg job as a stream of [`JobEvent`]s. Owns the Borg child (via the
/// spawner) and a [`CancellationToken`]; [`JobStream::cancel`] and `Drop` both
/// trip the token, which the reader task turns into a SIGTERM/kill.
pub struct JobStream {
    rx: mpsc::Receiver<JobEvent>,
    cancel: CancellationToken,
}

impl JobStream {
    /// Construct from a channel + token. Used by [`BorgCli`](super) once it has
    /// spawned the reader task.
    pub(crate) fn new(rx: mpsc::Receiver<JobEvent>, cancel: CancellationToken) -> JobStream {
        JobStream { rx, cancel }
    }

    /// Build a stream that yields a fixed sequence of events. For mocks and tests
    /// (used by `backtrack-testkit`); requires a Tokio runtime.
    pub fn from_events(
        events: impl IntoIterator<Item = JobEvent> + Send + 'static,
    ) -> JobStream {
        let (tx, rx) = mpsc::channel(64);
        let cancel = CancellationToken::new();
        let tc = cancel.clone();
        tokio::spawn(async move {
            for ev in events {
                tokio::select! {
                    _ = tc.cancelled() => break,
                    r = tx.send(ev) => {
                        if r.is_err() {
                            break;
                        }
                    }
                }
            }
        });
        JobStream::new(rx, cancel)
    }

    /// Request cancellation: trips the token so the reader task kills the child.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

impl Drop for JobStream {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Stream for JobStream {
    type Item = JobEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<JobEvent>> {
        self.rx.poll_recv(cx)
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p backtrack-core engine::job`
Expected: PASS (both tests).

- [ ] **Step 6: Add spec types**

Create `crates/backtrack-core/src/engine/spec.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Inputs to the engine operations. Minimal-but-honest for Stage 2: only what a
//! real backup and the integration tests exercise. Retention detail, exclude
//! files, checkpoint interval and chunker params arrive with the Stage 4
//! pipeline that consumes them.

use std::path::PathBuf;

/// A `borg create` request.
#[derive(Debug, Clone)]
pub struct CreateSpec {
    pub archive_name: String,
    pub sources: Vec<PathBuf>,
    pub excludes: Vec<String>,
    pub compression: Compression,
    pub one_file_system: bool,
}

/// Compression algorithm passed to `--compression`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compression {
    #[default]
    Zstd,
    Lz4,
    None,
}

impl Compression {
    pub fn as_borg_arg(self) -> &'static str {
        match self {
            Compression::Zstd => "zstd",
            Compression::Lz4 => "lz4",
            Compression::None => "none",
        }
    }
}

/// Retention counts for `borg prune`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrunePolicy {
    pub keep_hourly: u32,
    pub keep_daily: u32,
    pub keep_weekly: u32,
    pub keep_monthly: u32,
}

/// Depth of a `borg check`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckLevel {
    /// `--repository-only`
    Repository,
    /// `--archives-only`
    Archives,
    /// Full check (default).
    Full,
}

/// A repository to create.
#[derive(Debug, Clone)]
pub struct RepoSpec {
    /// Local path, `ssh://…`, or a mounted-share path.
    pub path: String,
    pub encryption: Encryption,
}

/// Encryption mode for `borg init`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encryption {
    #[default]
    RepokeyBlake2,
}

impl Encryption {
    pub fn as_borg_arg(self) -> &'static str {
        match self {
            Encryption::RepokeyBlake2 => "repokey-blake2",
        }
    }
}

/// What `repo_info` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    pub repository_id: String,
    pub archive_count: usize,
}

/// A Borg archive name or hex id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveId(pub String);
```

- [ ] **Step 7: Define the trait and wire re-exports**

Replace the body of `crates/backtrack-core/src/engine/mod.rs` with:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The Borg adapter: one trait, [`BackupEngine`], behind which every
//! Borg-specific operation lives, plus its typed error taxonomy and streamed
//! job events. Borg 2 later is a second implementation, not a rewrite.

mod error;
mod job;
mod spec;

use std::path::Path;
use std::pin::Pin;

use async_trait::async_trait;
use futures::stream::BoxStream;
use tokio::io::AsyncRead;

use crate::index::BorgItem;

pub use error::{EngineError, HealthFailure, Result};
pub use job::{JobEvent, JobStream, JobSummary, LogLevel};
pub use spec::{
    ArchiveId, CheckLevel, Compression, CreateSpec, Encryption, PrunePolicy, RepoInfo, RepoSpec,
};

/// The one interface every backup engine implements. The daemon holds an
/// `Arc<dyn BackupEngine>`; `BorgCli` is the v1 implementation and
/// `backtrack-testkit::MockEngine` the test double.
#[async_trait]
pub trait BackupEngine: Send + Sync {
    /// Create the repository (`borg init`).
    async fn init_repo(&self, spec: &RepoSpec) -> Result<()>;

    /// Read repository id + archive count (`borg info`). Doubles as the
    /// passphrase check used by import.
    async fn repo_info(&self) -> Result<RepoInfo>;

    /// Export the text recovery key (`borg key export`).
    async fn key_export(&self) -> Result<String>;

    /// Create an archive (`borg create`).
    async fn create(&self, spec: &CreateSpec) -> Result<JobStream>;

    /// Stream one archive's file list (`borg list --json-lines`).
    async fn list_archive(&self, id: &ArchiveId) -> Result<BoxStream<'static, Result<BorgItem>>>;

    /// Extract paths into `dest` (`borg extract`).
    async fn extract(&self, id: &ArchiveId, paths: &[String], dest: &Path) -> Result<JobStream>;

    /// Stream a single file's bytes to a reader (`borg extract --stdout`).
    async fn extract_stdout(
        &self,
        id: &ArchiveId,
        path: &str,
    ) -> Result<Pin<Box<dyn AsyncRead + Send>>>;

    /// Apply a retention policy (`borg prune`).
    async fn prune(&self, policy: &PrunePolicy) -> Result<JobStream>;

    /// Free repository space (`borg compact`).
    async fn compact(&self) -> Result<JobStream>;

    /// Consistency check (`borg check`).
    async fn check(&self, level: CheckLevel) -> Result<JobStream>;
}
```

- [ ] **Step 8: Re-export from the crate root**

In `crates/backtrack-core/src/lib.rs`, the `pub mod engine;` line already exposes everything via `engine::…`. No further change.

- [ ] **Step 9: Run the gate and commit**

Run: `cargo test -p backtrack-core` then `cargo clippy -p backtrack-core --all-targets -- -D warnings`
Expected: PASS; no clippy warnings.

```bash
git add crates/backtrack-core Cargo.toml
git commit -m "S02-T1: BackupEngine trait, JobStream, and spec types"
```

---

### Task 3: SecretStore — trait, file store, keyring (S02-T3)

**Files:**
- Create: `crates/backtrack-core/src/secret/mod.rs`
- Create: `crates/backtrack-core/src/secret/file.rs`
- Create: `crates/backtrack-core/src/secret/keyring.rs`
- Modify: `crates/backtrack-core/src/lib.rs` (add `pub mod secret;`)
- Modify: `backtrack_plan/progress.md` (mark S02-T3 `[x]`)
- Test: inline `#[cfg(test)]` in `file.rs`

**Interfaces:**
- Consumes: `engine::{EngineError, Result}` (Task 1).
- Produces:
  - `trait SecretStore: Send + Sync` with async `get(&self, repo_id: &str) -> Result<String>`, `set(&self, repo_id, passphrase) -> Result<()>`, `delete(&self, repo_id) -> Result<()>`. Missing entry → `EngineError::PassphraseMissing`.
  - `struct FileSecretStore` with `pub fn new(path: PathBuf) -> Self` and `pub fn dev_default() -> Result<Self>`.
  - `struct KeyringSecretStore` with `pub fn new() -> Self`.
  - `pub const APP_ID: &str = "io.github.keithvassallomt.Backtrack"`.
  - `pub fn default_store() -> Arc<dyn SecretStore>` (file store when `BACKTRACK_DEV=1`, else keyring).

- [ ] **Step 1: Add the module**

In `crates/backtrack-core/src/lib.rs`, after `pub mod engine;` add:

```rust
pub mod secret;
```

Create `crates/backtrack-core/src/secret/mod.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Passphrase storage. The real store is the Secret Service (system keyring) via
//! `oo7`; `BACKTRACK_DEV=1` selects a file-backed store so CI stays headless.
//!
//! A missing entry is [`EngineError::PassphraseMissing`], never a prompt —
//! prompting is the UI's job (see health.md), and this exact path powers the
//! passphrase-recovery dialog (mockup 24) later.

mod file;
mod keyring;

use std::sync::Arc;

use async_trait::async_trait;

use crate::engine::{EngineError, Result};

pub use file::FileSecretStore;
pub use keyring::KeyringSecretStore;

/// Stable Secret Service attribute: the application id.
pub const APP_ID: &str = "io.github.keithvassallomt.Backtrack";

/// Get/set/delete a repository passphrase under a stable attribute set
/// (`app-id` + `repo-id`).
#[async_trait]
pub trait SecretStore: Send + Sync {
    /// Returns the stored passphrase, or [`EngineError::PassphraseMissing`].
    async fn get(&self, repo_id: &str) -> Result<String>;
    async fn set(&self, repo_id: &str, passphrase: &str) -> Result<()>;
    async fn delete(&self, repo_id: &str) -> Result<()>;
}

/// The store the daemon uses by default: file-backed under `BACKTRACK_DEV=1`,
/// otherwise the system keyring.
pub fn default_store() -> Result<Arc<dyn SecretStore>> {
    if std::env::var("BACKTRACK_DEV").as_deref() == Ok("1") {
        Ok(Arc::new(FileSecretStore::dev_default()?))
    } else {
        Ok(Arc::new(KeyringSecretStore::new()))
    }
}

/// Shared helper: a keyring/file lookup that found nothing.
pub(crate) fn missing() -> EngineError {
    EngineError::PassphraseMissing
}
```

- [ ] **Step 2: Write the failing test**

Create `crates/backtrack-core/src/secret/file.rs` with header + test only:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineError;
    use crate::secret::SecretStore;

    #[tokio::test]
    async fn set_get_delete_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSecretStore::new(dir.path().join("secrets.json"));

        // Missing → PassphraseMissing.
        assert!(matches!(
            store.get("repo-1").await,
            Err(EngineError::PassphraseMissing)
        ));

        store.set("repo-1", "hunter2").await.unwrap();
        store.set("repo-2", "other").await.unwrap();
        assert_eq!(store.get("repo-1").await.unwrap(), "hunter2");
        assert_eq!(store.get("repo-2").await.unwrap(), "other");

        store.delete("repo-1").await.unwrap();
        assert!(matches!(
            store.get("repo-1").await,
            Err(EngineError::PassphraseMissing)
        ));
        // Deleting a second store instance sees the persisted file.
        let store2 = FileSecretStore::new(dir.path().join("secrets.json"));
        assert_eq!(store2.get("repo-2").await.unwrap(), "other");
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p backtrack-core secret::file`
Expected: FAIL to compile — `FileSecretStore` not found.

- [ ] **Step 4: Write the file store**

Prepend to `crates/backtrack-core/src/secret/file.rs`:

```rust
//! File-backed passphrase store for `BACKTRACK_DEV=1` / headless CI. Stores a
//! flat JSON map `{ repo_id: passphrase }`. Not for production use.

use std::collections::BTreeMap;
use std::path::PathBuf;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::engine::{EngineError, Result};
use crate::secret::{missing, SecretStore};

/// A JSON-file passphrase store, serialised under a mutex for atomic updates.
pub struct FileSecretStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileSecretStore {
    /// A store backed by the given file (created on first `set`).
    pub fn new(path: PathBuf) -> FileSecretStore {
        FileSecretStore {
            path,
            lock: Mutex::new(()),
        }
    }

    /// The default dev location: `$XDG_DATA_HOME/backtrack-dev/secrets.json`
    /// (falling back to `~/.local/share`).
    pub fn dev_default() -> Result<FileSecretStore> {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .ok_or_else(|| EngineError::BorgFailed {
                code: -1,
                stderr: "neither XDG_DATA_HOME nor HOME is set".into(),
            })?;
        Ok(FileSecretStore::new(base.join("backtrack-dev/secrets.json")))
    }

    fn read_map(&self) -> Result<BTreeMap<String, String>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| EngineError::BorgFailed {
                code: -1,
                stderr: format!("corrupt dev secret store: {e}"),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(EngineError::BorgFailed {
                code: -1,
                stderr: format!("reading dev secret store: {e}"),
            }),
        }
    }

    fn write_map(&self, map: &BTreeMap<String, String>) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| EngineError::BorgFailed {
                code: -1,
                stderr: format!("creating dev secret dir: {e}"),
            })?;
        }
        let bytes = serde_json::to_vec_pretty(map).map_err(|e| EngineError::BorgFailed {
            code: -1,
            stderr: format!("serialising dev secret store: {e}"),
        })?;
        std::fs::write(&self.path, bytes).map_err(|e| EngineError::BorgFailed {
            code: -1,
            stderr: format!("writing dev secret store: {e}"),
        })
    }
}

#[async_trait]
impl SecretStore for FileSecretStore {
    async fn get(&self, repo_id: &str) -> Result<String> {
        let _g = self.lock.lock().await;
        self.read_map()?.get(repo_id).cloned().ok_or_else(missing)
    }

    async fn set(&self, repo_id: &str, passphrase: &str) -> Result<()> {
        let _g = self.lock.lock().await;
        let mut map = self.read_map()?;
        map.insert(repo_id.to_string(), passphrase.to_string());
        self.write_map(&map)
    }

    async fn delete(&self, repo_id: &str) -> Result<()> {
        let _g = self.lock.lock().await;
        let mut map = self.read_map()?;
        map.remove(repo_id);
        self.write_map(&map)
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p backtrack-core secret::file`
Expected: PASS.

- [ ] **Step 6: Write the keyring store (compile-checked; not unit-tested headless)**

Create `crates/backtrack-core/src/secret/keyring.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Secret Service (system keyring) passphrase store via `oo7`. Not exercised in
//! headless CI (no Secret Service); the file store is the tested path. The
//! attribute set (`app-id` + `repo-id`) is stable so entries survive upgrades.

use std::collections::HashMap;

use async_trait::async_trait;

use crate::engine::{EngineError, Result};
use crate::secret::{missing, SecretStore, APP_ID};

/// A passphrase store backed by the org.freedesktop.Secret service.
pub struct KeyringSecretStore;

impl KeyringSecretStore {
    pub fn new() -> KeyringSecretStore {
        KeyringSecretStore
    }
}

impl Default for KeyringSecretStore {
    fn default() -> Self {
        KeyringSecretStore::new()
    }
}

fn attrs(repo_id: &str) -> HashMap<&str, &str> {
    HashMap::from([("app-id", APP_ID), ("repo-id_", repo_id)])
}

fn svc_err(e: impl std::fmt::Display) -> EngineError {
    EngineError::BorgFailed {
        code: -1,
        stderr: format!("keyring: {e}"),
    }
}

#[async_trait]
impl SecretStore for KeyringSecretStore {
    async fn get(&self, repo_id: &str) -> Result<String> {
        // NOTE: verify method names against the pinned oo7 0.6 API during impl.
        let keyring = oo7::Keyring::new().await.map_err(svc_err)?;
        let attributes = attrs(repo_id);
        let items = keyring.search_items(&attributes).await.map_err(svc_err)?;
        let item = items.into_iter().next().ok_or_else(missing)?;
        let secret = item.secret().await.map_err(svc_err)?;
        String::from_utf8(secret.to_vec()).map_err(svc_err)
    }

    async fn set(&self, repo_id: &str, passphrase: &str) -> Result<()> {
        let keyring = oo7::Keyring::new().await.map_err(svc_err)?;
        let attributes = attrs(repo_id);
        keyring
            .create_item(
                "Backtrack backup passphrase",
                &attributes,
                passphrase.as_bytes(),
                true, // replace
            )
            .await
            .map_err(svc_err)?;
        Ok(())
    }

    async fn delete(&self, repo_id: &str) -> Result<()> {
        let keyring = oo7::Keyring::new().await.map_err(svc_err)?;
        let attributes = attrs(repo_id);
        keyring.delete(&attributes).await.map_err(svc_err)?;
        Ok(())
    }
}
```

> The `repo-id_` attribute key intentionally avoids a name collision risk; keep it stable once shipped. If oo7 0.6's method names differ, adjust — the trait contract (get/set/delete, missing → `PassphraseMissing`) is what matters.

- [ ] **Step 7: Run the gate and commit; mark T3 done**

Run: `cargo build -p backtrack-core` (compiles the keyring path), then `cargo test -p backtrack-core secret` and `cargo clippy -p backtrack-core --all-targets -- -D warnings`.
Expected: builds; file-store test passes; no warnings.

In `backtrack_plan/progress.md`, change `- [ ] S02-T3 …` to `- [x] S02-T3 …`.

```bash
git add crates/backtrack-core/src/lib.rs crates/backtrack-core/src/secret backtrack_plan/progress.md
git commit -m "S02-T3: SecretStore trait, file store, oo7 keyring"
just sync-board-apply
```

---

### Task 4: backtrack-testkit — MockEngine + MockSecretStore (S02-T1)

**Files:**
- Create: `crates/backtrack-testkit/Cargo.toml`
- Create: `crates/backtrack-testkit/src/lib.rs`
- Modify: `Cargo.toml` (add `crates/backtrack-testkit` to `members`)
- Modify: `backtrack_plan/progress.md` (mark S02-T1 `[x]`)
- Test: inline `#[cfg(test)]` in `lib.rs`

**Interfaces:**
- Consumes: `backtrack_core::engine::{BackupEngine, JobEvent, JobStream, JobSummary, EngineError, Result, ArchiveId, CreateSpec, PrunePolicy, CheckLevel, RepoSpec, RepoInfo}`, `backtrack_core::index::BorgItem`, `backtrack_core::secret::SecretStore`.
- Produces: `MockEngine` (impl `BackupEngine`), `MockSecretStore` (impl `SecretStore`).

- [ ] **Step 1: Register the crate**

In the workspace `Cargo.toml`, add to `members`:

```toml
    "crates/backtrack-testkit",
```

Create `crates/backtrack-testkit/Cargo.toml`:

```toml
[package]
name = "backtrack-testkit"
description = "Test doubles for backtrack-core (MockEngine, MockSecretStore). Not published."
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
publish = false

[lints]
workspace = true

[dependencies]
backtrack-core.workspace = true
async-trait.workspace = true
futures.workspace = true
tokio.workspace = true
```

- [ ] **Step 2: Write the failing test**

Create `crates/backtrack-testkit/src/lib.rs` with header + test only:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

#[cfg(test)]
mod tests {
    use super::*;
    use backtrack_core::engine::{BackupEngine, JobEvent, JobSummary};
    use backtrack_core::secret::SecretStore;
    use futures::StreamExt;

    #[tokio::test]
    async fn mock_engine_drives_a_scripted_job_to_finished() {
        let engine = MockEngine::default().with_create_events(vec![
            JobEvent::Progress { current: 1, total: Some(1), phase: "archiving".into() },
            JobEvent::ItemDone { path: "home/a".into() },
            JobEvent::Finished(Ok(JobSummary { archive_id: Some("mock-1".into()) })),
        ]);

        let spec = backtrack_core::engine::CreateSpec {
            archive_name: "a".into(),
            sources: vec!["/tmp".into()],
            excludes: vec![],
            compression: Default::default(),
            one_file_system: true,
        };
        let events: Vec<JobEvent> = engine.create(&spec).await.unwrap().collect().await;
        assert!(matches!(events.last(), Some(JobEvent::Finished(Ok(_)))));
        assert_eq!(events.len(), 3);
    }

    #[tokio::test]
    async fn mock_secret_store_round_trips_and_reports_missing() {
        let store = MockSecretStore::default();
        assert!(store.get("r").await.is_err());
        store.set("r", "p").await.unwrap();
        assert_eq!(store.get("r").await.unwrap(), "p");
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p backtrack-testkit`
Expected: FAIL to compile — `MockEngine` / `MockSecretStore` not found.

- [ ] **Step 4: Write the mocks**

Prepend to `crates/backtrack-testkit/src/lib.rs`:

```rust
//! Test doubles for `backtrack-core`. Downstream crates depend on this crate
//! under `[dev-dependencies]` only, so no scaffolding compiles into production.

use std::collections::BTreeMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::Mutex;

use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};
use tokio::io::AsyncRead;

use backtrack_core::engine::{
    ArchiveId, BackupEngine, CheckLevel, CreateSpec, EngineError, JobEvent, JobStream, PrunePolicy,
    RepoInfo, RepoSpec, Result,
};
use backtrack_core::index::BorgItem;
use backtrack_core::secret::SecretStore;

/// A scriptable [`BackupEngine`]. Set the events `create`/`prune`/etc. should
/// emit and the items `list_archive` should yield; optionally force every call
/// to fail with a fixed error.
#[derive(Default)]
pub struct MockEngine {
    create_events: Vec<JobEvent>,
    items: Vec<BorgItem>,
    key: String,
    info: Option<RepoInfo>,
    fail: Option<EngineError>,
}

impl MockEngine {
    pub fn with_create_events(mut self, events: Vec<JobEvent>) -> Self {
        self.create_events = events;
        self
    }
    pub fn with_items(mut self, items: Vec<BorgItem>) -> Self {
        self.items = items;
        self
    }
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }
    pub fn with_info(mut self, info: RepoInfo) -> Self {
        self.info = Some(info);
        self
    }
    pub fn failing(mut self, err: EngineError) -> Self {
        self.fail = Some(err);
        self
    }

    fn check_fail(&self) -> Result<()> {
        match &self.fail {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }
    fn job(&self, events: Vec<JobEvent>) -> Result<JobStream> {
        self.check_fail()?;
        Ok(JobStream::from_events(events))
    }
}

#[async_trait]
impl BackupEngine for MockEngine {
    async fn init_repo(&self, _spec: &RepoSpec) -> Result<()> {
        self.check_fail()
    }
    async fn repo_info(&self) -> Result<RepoInfo> {
        self.check_fail()?;
        Ok(self
            .info
            .clone()
            .unwrap_or(RepoInfo { repository_id: "mock-repo".into(), archive_count: 0 }))
    }
    async fn key_export(&self) -> Result<String> {
        self.check_fail()?;
        Ok(self.key.clone())
    }
    async fn create(&self, _spec: &CreateSpec) -> Result<JobStream> {
        self.job(self.create_events.clone())
    }
    async fn list_archive(&self, _id: &ArchiveId) -> Result<BoxStream<'static, Result<BorgItem>>> {
        self.check_fail()?;
        Ok(stream::iter(self.items.clone().into_iter().map(Ok)).boxed())
    }
    async fn extract(&self, _id: &ArchiveId, _paths: &[String], _dest: &Path) -> Result<JobStream> {
        self.job(vec![JobEvent::Finished(Ok(Default::default()))])
    }
    async fn extract_stdout(
        &self,
        _id: &ArchiveId,
        _path: &str,
    ) -> Result<Pin<Box<dyn AsyncRead + Send>>> {
        self.check_fail()?;
        Ok(Box::pin(tokio::io::empty()))
    }
    async fn prune(&self, _policy: &PrunePolicy) -> Result<JobStream> {
        self.job(vec![JobEvent::Finished(Ok(Default::default()))])
    }
    async fn compact(&self) -> Result<JobStream> {
        self.job(vec![JobEvent::Finished(Ok(Default::default()))])
    }
    async fn check(&self, _level: CheckLevel) -> Result<JobStream> {
        self.job(vec![JobEvent::Finished(Ok(Default::default()))])
    }
}

/// An in-memory [`SecretStore`]; missing entries return
/// [`EngineError::PassphraseMissing`].
#[derive(Default)]
pub struct MockSecretStore {
    map: Mutex<BTreeMap<String, String>>,
}

#[async_trait]
impl SecretStore for MockSecretStore {
    async fn get(&self, repo_id: &str) -> Result<String> {
        self.map
            .lock()
            .unwrap()
            .get(repo_id)
            .cloned()
            .ok_or(EngineError::PassphraseMissing)
    }
    async fn set(&self, repo_id: &str, passphrase: &str) -> Result<()> {
        self.map
            .lock()
            .unwrap()
            .insert(repo_id.to_string(), passphrase.to_string());
        Ok(())
    }
    async fn delete(&self, repo_id: &str) -> Result<()> {
        self.map.lock().unwrap().remove(repo_id);
        Ok(())
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p backtrack-testkit`
Expected: PASS (both tests).

- [ ] **Step 6: Run the full gate and commit; mark T1 done**

Run: `just check`
Expected: fmt clean, clippy clean, all tests pass, license headers present.

In `backtrack_plan/progress.md`, change `- [/] S02-T1 …` to `- [x] S02-T1 …`.

```bash
git add crates/backtrack-testkit Cargo.toml backtrack_plan/progress.md
git commit -m "S02-T1: backtrack-testkit crate with MockEngine and MockSecretStore"
just sync-board-apply
```

---

### Task 5: log-json line parser (S02-T2)

**Files:**
- Create: `crates/backtrack-core/src/engine/borg/mod.rs` (module root; grows over tasks 5–9)
- Create: `crates/backtrack-core/src/engine/borg/logjson.rs`
- Modify: `crates/backtrack-core/src/engine/mod.rs` (add `mod borg;`)
- Modify: `backtrack_plan/progress.md` (mark S02-T2 `[/]`)
- Test: inline `#[cfg(test)]` in `logjson.rs`

**Interfaces:**
- Consumes: `LogLevel` (Task 2).
- Produces: `enum Parsed { Progress { current: u64, total: Option<u64>, phase: String }, ItemDone { path: String }, Log { level: LogLevel, msgid: Option<String>, message: String }, Ignore }` and `fn parse_log_line(line: &str) -> Parsed`.

- [ ] **Step 1: Add the borg submodule**

In `crates/backtrack-core/src/engine/mod.rs`, add `mod borg;` alongside the other `mod` lines.

Create `crates/backtrack-core/src/engine/borg/mod.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The real [`BackupEngine`](super::BackupEngine): `BorgCli`. Spawns
//! `borg --log-json` and parses its stderr JSONL into [`JobEvent`](super::JobEvent)s.

mod logjson;
```

- [ ] **Step 2: Write the failing test**

Create `crates/backtrack-core/src/engine/borg/logjson.rs` with header + test only:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::LogLevel;

    #[test]
    fn parses_progress_percent_with_total() {
        let line = r#"{"type":"progress_percent","message":"Calculating","current":40,"total":100,"finished":false,"msgid":"extract"}"#;
        assert_eq!(
            parse_log_line(line),
            Parsed::Progress { current: 40, total: Some(100), phase: "Calculating".into() }
        );
    }

    #[test]
    fn progress_percent_negative_total_is_unknown() {
        let line = r#"{"type":"progress_percent","message":"","current":5,"total":-1,"finished":false}"#;
        assert_eq!(
            parse_log_line(line),
            Parsed::Progress { current: 5, total: None, phase: "".into() }
        );
    }

    #[test]
    fn parses_archive_progress_as_progress() {
        let line = r#"{"type":"archive_progress","original_size":2048,"compressed_size":1024,"nfiles":3,"path":"home/a"}"#;
        assert_eq!(
            parse_log_line(line),
            Parsed::Progress { current: 2048, total: None, phase: "archiving".into() }
        );
    }

    #[test]
    fn parses_file_status_as_item_done() {
        let line = r#"{"type":"file_status","status":"A","path":"home/report.odt"}"#;
        assert_eq!(parse_log_line(line), Parsed::ItemDone { path: "home/report.odt".into() });
    }

    #[test]
    fn parses_error_log_message_with_msgid() {
        let line = r#"{"type":"log_message","levelname":"ERROR","name":"borg","message":"Repository does not exist.","msgid":"Repository.DoesNotExist"}"#;
        assert_eq!(
            parse_log_line(line),
            Parsed::Log {
                level: LogLevel::Error,
                msgid: Some("Repository.DoesNotExist".into()),
                message: "Repository does not exist.".into(),
            }
        );
    }

    #[test]
    fn unknown_type_and_garbage_are_ignored() {
        assert_eq!(parse_log_line(r#"{"type":"question_prompt"}"#), Parsed::Ignore);
        assert_eq!(parse_log_line("not json"), Parsed::Ignore);
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p backtrack-core engine::borg::logjson`
Expected: FAIL to compile — `Parsed` / `parse_log_line` not found.

- [ ] **Step 4: Write the parser**

Prepend to `crates/backtrack-core/src/engine/borg/logjson.rs`:

```rust
//! Parse one `borg --log-json` stderr line into a [`Parsed`] record.
//!
//! Mapping rules (per the Stage 2 research):
//! - `progress_percent` → [`Parsed::Progress`]; a negative `total` means the
//!   denominator is unknown (`None`). Never trusted for partial extracts —
//!   honest restore % is computed from index byte totals downstream.
//! - `archive_progress` → [`Parsed::Progress`] keyed on `original_size`.
//! - `file_status` → [`Parsed::ItemDone`].
//! - `log_message` → [`Parsed::Log`] (retained for error classification).
//! - anything else / non-JSON → [`Parsed::Ignore`].

use serde::Deserialize;

use crate::engine::LogLevel;

/// A single classified log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Progress { current: u64, total: Option<u64>, phase: String },
    ItemDone { path: String },
    Log { level: LogLevel, msgid: Option<String>, message: String },
    Ignore,
}

#[derive(Deserialize)]
struct Raw {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    msgid: Option<String>,
    #[serde(default)]
    levelname: Option<String>,
    #[serde(default)]
    current: Option<i64>,
    #[serde(default)]
    total: Option<i64>,
    #[serde(default)]
    original_size: Option<i64>,
    #[serde(default)]
    path: Option<String>,
}

fn level_from(name: Option<&str>) -> LogLevel {
    match name.unwrap_or("INFO") {
        "DEBUG" => LogLevel::Debug,
        "WARNING" => LogLevel::Warning,
        "ERROR" | "CRITICAL" => LogLevel::Error,
        _ => LogLevel::Info,
    }
}

/// Parse one stderr line. Never fails: unrecognised input is [`Parsed::Ignore`].
pub fn parse_log_line(line: &str) -> Parsed {
    let raw: Raw = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(_) => return Parsed::Ignore,
    };
    match raw.kind.as_str() {
        "progress_percent" => Parsed::Progress {
            current: raw.current.unwrap_or(0).max(0) as u64,
            total: match raw.total {
                Some(t) if t >= 0 => Some(t as u64),
                _ => None,
            },
            phase: raw.message.unwrap_or_default(),
        },
        "archive_progress" => Parsed::Progress {
            current: raw.original_size.unwrap_or(0).max(0) as u64,
            total: None,
            phase: "archiving".to_string(),
        },
        "file_status" => match raw.path {
            Some(path) => Parsed::ItemDone { path },
            None => Parsed::Ignore,
        },
        "log_message" => Parsed::Log {
            level: level_from(raw.levelname.as_deref()),
            msgid: raw.msgid,
            message: raw.message.unwrap_or_default(),
        },
        _ => Parsed::Ignore,
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p backtrack-core engine::borg::logjson`
Expected: PASS (all six tests).

- [ ] **Step 6: Mark T2 in progress and commit**

In `backtrack_plan/progress.md`, change `- [ ] S02-T2 …` to `- [/] S02-T2 …`.

```bash
git add crates/backtrack-core/src/engine backtrack_plan/progress.md
git commit -m "S02-T2: --log-json stderr line parser"
```

---

### Task 6: exit-code + stderr classification (S02-T2)

**Files:**
- Create: `crates/backtrack-core/src/engine/borg/classify.rs`
- Modify: `crates/backtrack-core/src/engine/borg/mod.rs` (add `mod classify;`)
- Test: inline `#[cfg(test)]` in `classify.rs`

**Interfaces:**
- Consumes: `EngineError` (Task 1).
- Produces: `struct ErrLine { pub msgid: Option<String>, pub message: String }` and `fn classify(code: i32, errors: &[ErrLine]) -> EngineError`.

- [ ] **Step 1: Add the submodule**

In `crates/backtrack-core/src/engine/borg/mod.rs`, add `mod classify;`.

- [ ] **Step 2: Write the failing test**

Create `crates/backtrack-core/src/engine/borg/classify.rs` with header + test only:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineError;

    fn line(msgid: Option<&str>, msg: &str) -> ErrLine {
        ErrLine { msgid: msgid.map(str::to_string), message: msg.to_string() }
    }

    #[test]
    fn repo_missing_maps_to_unreachable() {
        let errs = [line(Some("Repository.DoesNotExist"), "Repository /mnt/x does not exist.")];
        assert_eq!(classify(2, &errs), EngineError::RepoUnreachable);
    }

    #[test]
    fn wrong_passphrase_by_message() {
        let errs = [line(None, "passphrase supplied in BORG_PASSPHRASE is incorrect.")];
        assert_eq!(classify(2, &errs), EngineError::PassphraseWrong);
    }

    #[test]
    fn lock_timeout_maps_to_locked() {
        let errs = [line(Some("LockTimeout"), "Failed to create/acquire the lock")];
        assert_eq!(classify(2, &errs), EngineError::LockedByOther);
    }

    #[test]
    fn check_needed_maps_to_corrupt() {
        let errs = [line(Some("Repository.CheckNeeded"), "Inconsistency detected.")];
        assert_eq!(classify(2, &errs), EngineError::RepoCorrupt);
    }

    #[test]
    fn enospc_maps_to_destination_full() {
        let errs = [line(None, "[Errno 28] No space left on device")];
        assert_eq!(classify(2, &errs), EngineError::DestinationFull);
    }

    #[test]
    fn ssh_auth_maps_to_auth_failed() {
        let errs = [line(None, "Permission denied (publickey).")];
        assert_eq!(classify(2, &errs), EngineError::AuthFailed);
    }

    #[test]
    fn ssh_unreachable_maps_to_unreachable() {
        let errs = [line(None, "ssh: connect to host nas.local port 22: No route to host")];
        assert_eq!(classify(2, &errs), EngineError::RepoUnreachable);
    }

    #[test]
    fn unrecognised_falls_back_to_borg_failed() {
        let errs = [line(None, "something weird happened")];
        assert_eq!(
            classify(2, &errs),
            EngineError::BorgFailed { code: 2, stderr: "something weird happened".into() }
        );
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p backtrack-core engine::borg::classify`
Expected: FAIL to compile — `ErrLine` / `classify` not found.

- [ ] **Step 4: Write the classifier**

Prepend to `crates/backtrack-core/src/engine/borg/classify.rs`:

```rust
//! Map a finished Borg process (exit code + captured error lines) to an
//! [`EngineError`]. Precedence: specific `msgid`/message patterns first, then a
//! `BorgFailed` fallback. The table below is the documented mapping; keep it in
//! sync with health.md.
//!
//! | Signal (msgid or message substring) | EngineError |
//! |---|---|
//! | `Repository.DoesNotExist`, "does not exist", ssh "No route to host"/"Connection refused"/"Connection closed"/"Network is unreachable"/"Could not resolve hostname" | `RepoUnreachable` |
//! | "passphrase … is incorrect"/"wrong passphrase"/`PassphraseWrong` | `PassphraseWrong` |
//! | ssh "Permission denied"/"Authentication failed"/"Host key verification failed" | `AuthFailed` |
//! | "No space left on device"/ENOSPC | `DestinationFull` |
//! | `LockTimeout`, "Failed to create/acquire the lock" | `LockedByOther` |
//! | `Repository.CheckNeeded`, "Inconsistency detected"/"Data integrity error"/"manifest" | `RepoCorrupt` |
//! | anything else with a non-zero code | `BorgFailed { code, stderr }` |

use crate::engine::EngineError;

/// A captured error-level line from Borg's `--log-json` stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrLine {
    pub msgid: Option<String>,
    pub message: String,
}

fn any<'a>(errors: &'a [ErrLine], needles: &[&str]) -> bool {
    errors.iter().any(|e| {
        needles.iter().any(|n| {
            e.message.to_lowercase().contains(&n.to_lowercase())
                || e.msgid.as_deref().map(|m| m.eq_ignore_ascii_case(n)).unwrap_or(false)
        })
    })
}

/// Classify a failed Borg invocation. `code` is the process exit code; `errors`
/// are the error-level `log_message` lines captured from stderr.
pub fn classify(code: i32, errors: &[ErrLine]) -> EngineError {
    // Order matters: check the most specific signals before the generic fallback.
    if any(errors, &["PassphraseWrong", "passphrase supplied", "is incorrect", "wrong passphrase"]) {
        return EngineError::PassphraseWrong;
    }
    if any(
        errors,
        &["Permission denied", "Authentication failed", "Host key verification failed"],
    ) {
        return EngineError::AuthFailed;
    }
    if any(
        errors,
        &[
            "Repository.DoesNotExist",
            "does not exist",
            "No route to host",
            "Connection refused",
            "Connection closed",
            "Network is unreachable",
            "Could not resolve hostname",
        ],
    ) {
        return EngineError::RepoUnreachable;
    }
    if any(errors, &["No space left on device", "Errno 28"]) {
        return EngineError::DestinationFull;
    }
    if any(errors, &["LockTimeout", "Failed to create/acquire the lock"]) {
        return EngineError::LockedByOther;
    }
    if any(
        errors,
        &["Repository.CheckNeeded", "Inconsistency detected", "Data integrity error", "manifest"],
    ) {
        return EngineError::RepoCorrupt;
    }
    let stderr = errors
        .iter()
        .map(|e| e.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    EngineError::BorgFailed { code, stderr }
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p backtrack-core engine::borg::classify`
Expected: PASS (all eight tests).

- [ ] **Step 6: Commit**

```bash
git add crates/backtrack-core/src/engine/borg
git commit -m "S02-T2: exit-code + stderr → EngineError classification"
```

---

### Task 7: BorgCli implementation (S02-T2)

**Files:**
- Create: `crates/backtrack-core/src/engine/borg/invoke.rs`
- Modify: `crates/backtrack-core/src/engine/borg/mod.rs` (`BorgCli` + `impl BackupEngine`)
- Modify: `crates/backtrack-core/src/engine/mod.rs` (re-export `BorgCli`)
- Test: inline `#[cfg(test)]` in `invoke.rs` (version parsing — borg-free)

**Interfaces:**
- Consumes: everything from tasks 1, 2, 3, 5, 6; `index::BorgItem`.
- Produces: `struct BorgCli` with `pub async fn new(repo: String, repo_id: String, secrets: Arc<dyn SecretStore>) -> Result<BorgCli>`; `impl BackupEngine for BorgCli`. Helper `fn parse_borg_version(s: &str) -> Option<(u32, u32)>`.

- [ ] **Step 1: Write the failing test (version parsing)**

Create `crates/backtrack-core/src/engine/borg/invoke.rs` with header + test only:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_borg_version_banner() {
        assert_eq!(parse_borg_version("borg 1.2.8"), Some((1, 2)));
        assert_eq!(parse_borg_version("borg 1.4.0\n"), Some((1, 4)));
        assert_eq!(parse_borg_version("borg 2.0.0b14"), Some((2, 0)));
        assert_eq!(parse_borg_version("garbage"), None);
    }

    #[test]
    fn version_floor_rejects_pre_1_2() {
        assert!(meets_floor((1, 2)));
        assert!(meets_floor((1, 4)));
        assert!(meets_floor((2, 0)));
        assert!(!meets_floor((1, 1)));
        assert!(!meets_floor((0, 9)));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p backtrack-core engine::borg::invoke`
Expected: FAIL to compile — `parse_borg_version` / `meets_floor` not found.

- [ ] **Step 3: Write invoke.rs (spawn helpers, env, version probe, streaming)**

Prepend to `crates/backtrack-core/src/engine/borg/invoke.rs`:

```rust
//! Process plumbing for `BorgCli`: environment, the version probe, and the
//! stderr-reader task that turns `--log-json` output into a [`JobStream`].

use std::path::Path;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::engine::borg::classify::{classify, ErrLine};
use crate::engine::borg::logjson::{parse_log_line, Parsed};
use crate::engine::{EngineError, JobEvent, JobStream, JobSummary, LogLevel, Result};

/// The minimum supported Borg version.
pub(super) const BORG_FLOOR: (u32, u32) = (1, 2);

/// Parse the `borg --version` banner ("borg 1.4.0") into `(major, minor)`.
pub(super) fn parse_borg_version(s: &str) -> Option<(u32, u32)> {
    let ver = s.split_whitespace().nth(1)?;
    let mut parts = ver.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    // Minor may carry a suffix (e.g. "0b14"); take the leading digits.
    let minor_str = parts.next()?;
    let digits: String = minor_str.chars().take_while(|c| c.is_ascii_digit()).collect();
    let minor: u32 = digits.parse().ok()?;
    Some((major, minor))
}

/// Is this `(major, minor)` at or above the floor?
pub(super) fn meets_floor(v: (u32, u32)) -> bool {
    v >= BORG_FLOOR
}

/// Run `borg --version` and enforce the floor.
pub(super) async fn probe_version(bin: &Path) -> Result<()> {
    let out = Command::new(bin)
        .arg("--version")
        .output()
        .await
        .map_err(|_| EngineError::BorgMissing { needed: ">=1.2".into(), found: None })?;
    let banner = String::from_utf8_lossy(&out.stdout);
    match parse_borg_version(&banner) {
        Some(v) if meets_floor(v) => Ok(()),
        found => Err(EngineError::BorgMissing {
            needed: ">=1.2".into(),
            found: found.map(|(a, b)| format!("{a}.{b}")),
        }),
    }
}

/// Common environment for every borg invocation.
pub(super) fn base_command(bin: &Path, repo: &str, passphrase: &str) -> Command {
    let mut cmd = Command::new(bin);
    cmd.env("BORG_PASSPHRASE", passphrase)
        .env("BORG_RELOCATED_REPO_ACCESS_IS_OK", "no")
        .env("BORG_EXIT_CODES", "modern")
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8")
        .arg("--log-json");
    let _ = repo; // repo is embedded in per-subcommand args by the caller
    cmd
}

/// Spawn a job-style borg command (progress on stderr) and return its stream.
/// Stdout is discarded; the stderr reader forwards events and, on exit,
/// classifies failure into the terminal [`JobEvent::Finished`].
pub(super) fn spawn_streamed(mut cmd: Command) -> Result<JobStream> {
    cmd.stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| EngineError::BorgFailed { code: -1, stderr: format!("spawning borg: {e}") })?;
    let stderr = child.stderr.take().expect("stderr piped");

    let (tx, rx) = mpsc::channel(64);
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();

    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut errbuf: Vec<ErrLine> = Vec::new();
        let mut cancelled = false;
        loop {
            tokio::select! {
                _ = task_cancel.cancelled() => {
                    cancelled = true;
                    let _ = child.start_kill();
                    break;
                }
                next = lines.next_line() => match next {
                    Ok(Some(line)) => forward_line(&line, &tx, &mut errbuf).await,
                    Ok(None) | Err(_) => break,
                }
            }
        }
        // Drain any remaining lines after the child closes stderr (non-cancel path).
        if !cancelled {
            while let Ok(Some(line)) = lines.next_line().await {
                forward_line(&line, &tx, &mut errbuf).await;
            }
        }
        let result = match child.wait().await {
            Ok(status) if status.success() && !cancelled => Ok(JobSummary::default()),
            Ok(status) => Err(classify(status.code().unwrap_or(-1), &errbuf)),
            Err(e) => Err(EngineError::BorgFailed { code: -1, stderr: e.to_string() }),
        };
        let _ = tx.send(JobEvent::Finished(result)).await;
    });

    Ok(JobStream::new(rx, cancel))
}

async fn forward_line(line: &str, tx: &mpsc::Sender<JobEvent>, errbuf: &mut Vec<ErrLine>) {
    let event = match parse_log_line(line) {
        Parsed::Progress { current, total, phase } => JobEvent::Progress { current, total, phase },
        Parsed::ItemDone { path } => JobEvent::ItemDone { path },
        Parsed::Log { level, msgid, message } => {
            if level == LogLevel::Error {
                errbuf.push(ErrLine { msgid, message: message.clone() });
            }
            JobEvent::Log { level, msg: message }
        }
        Parsed::Ignore => return,
    };
    let _ = tx.send(event).await;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p backtrack-core engine::borg::invoke`
Expected: PASS (both tests).

- [ ] **Step 5: Write BorgCli and its BackupEngine impl**

Replace the body of `crates/backtrack-core/src/engine/borg/mod.rs` with:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The real [`BackupEngine`](super::BackupEngine): `BorgCli`. Spawns
//! `borg --log-json` and parses its stderr JSONL into [`JobEvent`](super::JobEvent)s.

mod classify;
mod invoke;
mod logjson;

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;

use crate::engine::{
    ArchiveId, BackupEngine, CheckLevel, CreateSpec, EngineError, PrunePolicy, RepoInfo, RepoSpec,
    Result,
};
use crate::index::BorgItem;
use crate::secret::SecretStore;

use invoke::{base_command, probe_version, spawn_streamed};

/// The v1 backup engine: drives the `borg` CLI (1.2/1.4).
pub struct BorgCli {
    repo: String,
    repo_id: String,
    secrets: Arc<dyn SecretStore>,
    bin: PathBuf,
}

impl BorgCli {
    /// Construct against `repo` (local path / `ssh://` / mounted share), keyed by
    /// `repo_id` for passphrase lookup. Probes `borg --version` and enforces the
    /// `>=1.2` floor.
    pub async fn new(
        repo: String,
        repo_id: String,
        secrets: Arc<dyn SecretStore>,
    ) -> Result<BorgCli> {
        let bin = PathBuf::from("borg");
        probe_version(&bin).await?;
        Ok(BorgCli { repo, repo_id, secrets, bin })
    }

    /// A borg command with the passphrase + environment applied.
    async fn cmd(&self) -> Result<Command> {
        let pass = self.secrets.get(&self.repo_id).await?;
        Ok(base_command(&self.bin, &self.repo, &pass))
    }

    fn archive_ref(&self, id: &ArchiveId) -> String {
        format!("{}::{}", self.repo, id.0)
    }
}

#[async_trait]
impl BackupEngine for BorgCli {
    async fn init_repo(&self, spec: &RepoSpec) -> Result<()> {
        let mut cmd = self.cmd().await?;
        cmd.arg("init")
            .arg("--encryption")
            .arg(spec.encryption.as_borg_arg())
            .arg(&spec.path);
        // init is short; run it as a streamed job and drain to its outcome.
        drain_to_result(spawn_streamed(cmd)?).await
    }

    async fn repo_info(&self) -> Result<RepoInfo> {
        let mut cmd = self.cmd().await?;
        cmd.arg("info").arg("--json").arg(&self.repo);
        let out = run_json(cmd).await?;
        let repository_id = out
            .get("repository")
            .and_then(|r| r.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let archive_count = out
            .get("archives")
            .and_then(|a| a.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        Ok(RepoInfo { repository_id, archive_count })
    }

    async fn key_export(&self) -> Result<String> {
        let mut cmd = self.cmd().await?;
        cmd.arg("key").arg("export").arg(&self.repo); // to stdout
        run_stdout_string(cmd).await
    }

    async fn create(&self, spec: &CreateSpec) -> Result<JobStreamAlias> {
        let mut cmd = self.cmd().await?;
        cmd.arg("create")
            .arg("--json")
            .arg("--list")
            .arg("--filter")
            .arg("AME")
            .arg("--compression")
            .arg(spec.compression.as_borg_arg());
        if spec.one_file_system {
            cmd.arg("--one-file-system");
        }
        for ex in &spec.excludes {
            cmd.arg("--exclude").arg(ex);
        }
        cmd.arg(format!("{}::{}", self.repo, spec.archive_name));
        for src in &spec.sources {
            cmd.arg(src);
        }
        spawn_streamed(cmd)
    }

    async fn list_archive(&self, id: &ArchiveId) -> Result<BoxStream<'static, Result<BorgItem>>> {
        let mut cmd = self.cmd().await?;
        cmd.arg("list").arg("--json-lines").arg(self.archive_ref(id));
        cmd.stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| EngineError::BorgFailed {
            code: -1,
            stderr: format!("spawning borg list: {e}"),
        })?;
        let stdout = child.stdout.take().expect("stdout piped");
        let lines = BufReader::new(stdout).lines();

        // Own the child in the stream state so it lives as long as the stream
        // (kill_on_drop reaps it if the consumer drops early).
        let stream = stream::unfold((lines, child), |(mut lines, mut child)| async move {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let item = BorgItem::from_json_line(&line).map_err(|e| EngineError::BorgFailed {
                        code: -1,
                        stderr: format!("parsing borg list line: {e}"),
                    });
                    Some((item, (lines, child)))
                }
                Ok(None) => {
                    let _ = child.wait().await;
                    None
                }
                Err(e) => Some((
                    Err(EngineError::BorgFailed { code: -1, stderr: e.to_string() }),
                    (lines, child),
                )),
            }
        });
        Ok(stream.boxed())
    }

    async fn extract(&self, id: &ArchiveId, paths: &[String], dest: &Path) -> Result<JobStreamAlias> {
        let mut cmd = self.cmd().await?;
        cmd.current_dir(dest)
            .arg("extract")
            .arg("--list")
            .arg(self.archive_ref(id));
        for p in paths {
            cmd.arg(p);
        }
        spawn_streamed(cmd)
    }

    async fn extract_stdout(
        &self,
        id: &ArchiveId,
        path: &str,
    ) -> Result<Pin<Box<dyn AsyncRead + Send>>> {
        let mut cmd = self.cmd().await?;
        cmd.arg("extract")
            .arg("--stdout")
            .arg(self.archive_ref(id))
            .arg(path)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| EngineError::BorgFailed {
            code: -1,
            stderr: format!("spawning borg extract --stdout: {e}"),
        })?;
        let stdout = child.stdout.take().expect("stdout piped");
        // Keep the child alive alongside its stdout; kill_on_drop reaps it.
        Ok(Box::pin(ChildStdoutReader { _child: child, stdout }))
    }

    async fn prune(&self, policy: &PrunePolicy) -> Result<JobStreamAlias> {
        let mut cmd = self.cmd().await?;
        cmd.arg("prune")
            .arg("--list")
            .arg(format!("--keep-hourly={}", policy.keep_hourly))
            .arg(format!("--keep-daily={}", policy.keep_daily))
            .arg(format!("--keep-weekly={}", policy.keep_weekly))
            .arg(format!("--keep-monthly={}", policy.keep_monthly))
            .arg(&self.repo);
        spawn_streamed(cmd)
    }

    async fn compact(&self) -> Result<JobStreamAlias> {
        let mut cmd = self.cmd().await?;
        cmd.arg("compact").arg(&self.repo);
        spawn_streamed(cmd)
    }

    async fn check(&self, level: CheckLevel) -> Result<JobStreamAlias> {
        let mut cmd = self.cmd().await?;
        cmd.arg("check");
        match level {
            CheckLevel::Repository => {
                cmd.arg("--repository-only");
            }
            CheckLevel::Archives => {
                cmd.arg("--archives-only");
            }
            CheckLevel::Full => {}
        }
        cmd.arg(&self.repo);
        spawn_streamed(cmd)
    }
}

/// Local alias so the signatures above read cleanly.
type JobStreamAlias = crate::engine::JobStream;

/// A reader that streams a borg-extracted file and keeps the child alive.
struct ChildStdoutReader {
    _child: tokio::process::Child,
    stdout: tokio::process::ChildStdout,
}

impl AsyncRead for ChildStdoutReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdout).poll_read(cx, buf)
    }
}

/// Drain a short job stream to its terminal outcome (used by `init_repo`).
async fn drain_to_result(mut stream: crate::engine::JobStream) -> Result<()> {
    use crate::engine::JobEvent;
    let mut outcome = Err(EngineError::BorgFailed { code: -1, stderr: "no outcome".into() });
    while let Some(ev) = stream.next().await {
        if let JobEvent::Finished(r) = ev {
            outcome = r.map(|_| ());
        }
    }
    outcome
}

/// Run a borg command that prints one JSON document to stdout; parse it.
async fn run_json(mut cmd: Command) -> Result<serde_json::Value> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = cmd.output().await.map_err(|e| EngineError::BorgFailed {
        code: -1,
        stderr: format!("running borg: {e}"),
    })?;
    if !out.status.success() {
        return Err(classify::classify(
            out.status.code().unwrap_or(-1),
            &collect_json_errors(&out.stderr),
        ));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| EngineError::BorgFailed {
        code: -1,
        stderr: format!("parsing borg --json: {e}"),
    })
}

/// Run a borg command and return its stdout as a UTF-8 string (e.g. key export).
async fn run_stdout_string(mut cmd: Command) -> Result<String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = cmd.output().await.map_err(|e| EngineError::BorgFailed {
        code: -1,
        stderr: format!("running borg: {e}"),
    })?;
    if !out.status.success() {
        return Err(classify::classify(
            out.status.code().unwrap_or(-1),
            &collect_json_errors(&out.stderr),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Extract error-level lines from captured `--log-json` stderr, for the
/// non-streaming (`output()`) commands.
fn collect_json_errors(stderr: &[u8]) -> Vec<classify::ErrLine> {
    use crate::engine::LogLevel;
    use logjson::{parse_log_line, Parsed};
    String::from_utf8_lossy(stderr)
        .lines()
        .filter_map(|l| match parse_log_line(l) {
            Parsed::Log { level: LogLevel::Error, msgid, message } => {
                Some(classify::ErrLine { msgid, message })
            }
            _ => None,
        })
        .collect()
}

use futures::StreamExt as _;
```

> `JobStreamAlias` exists only so the long method signatures read cleanly; it is the same `JobStream`. If clippy flags the alias or the `use futures::StreamExt as _;` placement, move the import to the top with the others.

- [ ] **Step 6: Re-export BorgCli**

In `crates/backtrack-core/src/engine/mod.rs`, add to the `pub use` block:

```rust
pub use borg::BorgCli;
```

And ensure `mod borg;` is declared (from Task 5).

- [ ] **Step 7: Run the gate and commit**

Run: `cargo test -p backtrack-core` then `cargo clippy -p backtrack-core --all-targets -- -D warnings`
Expected: unit tests pass; no warnings. (No real borg is invoked by unit tests.)

```bash
git add crates/backtrack-core/src/engine
git commit -m "S02-T2: BorgCli — spawn, env, version probe, all operations"
```

---

### Task 8: Borg integration suite (S02-T2)

**Files:**
- Create: `crates/backtrack-core/tests/borg_engine.rs` (feature-gated `integration`)
- Modify: `backtrack_plan/progress.md` (mark S02-T2 `[x]`)

**Interfaces:**
- Consumes: `backtrack_core::engine::{BorgCli, BackupEngine, CreateSpec, PrunePolicy, CheckLevel, RepoSpec, Encryption, ArchiveId, Compression, JobEvent, EngineError}`, `backtrack_core::secret::FileSecretStore`.

- [ ] **Step 1: Write the integration test**

Create `crates/backtrack-core/tests/borg_engine.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Real-borg integration tests. Run via `just test-integration`
//! (`cargo test --features integration`); skipped otherwise.
#![cfg(feature = "integration")]

use std::sync::Arc;

use backtrack_core::engine::{
    ArchiveId, BackupEngine, BorgCli, CheckLevel, Compression, CreateSpec, EngineError, Encryption,
    JobEvent, PrunePolicy, RepoSpec,
};
use backtrack_core::secret::{FileSecretStore, SecretStore};
use futures::StreamExt;
use tokio::io::AsyncReadExt;

const PASS: &str = "integration-passphrase";

/// A fixture: temp dir with a repo path, a source tree, and a store holding the
/// passphrase under `repo_id = "test"`.
struct Fixture {
    _dir: tempfile::TempDir,
    repo: String,
    src: std::path::PathBuf,
    secrets: Arc<dyn SecretStore>,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo").to_str().unwrap().to_string();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("hello.txt"), b"hello world").unwrap();
    std::fs::write(src.join("data.bin"), vec![7u8; 1024]).unwrap();

    let store = FileSecretStore::new(dir.path().join("secrets.json"));
    store.set("test", PASS).await.unwrap();
    Fixture { _dir: dir, repo, src, secrets: Arc::new(store) }
}

async fn engine(f: &Fixture) -> BorgCli {
    BorgCli::new(f.repo.clone(), "test".into(), f.secrets.clone())
        .await
        .expect("borg >= 1.2 available")
}

async fn run_to_finish(mut s: backtrack_core::engine::JobStream) -> Result<(), EngineError> {
    let mut outcome = Ok(());
    while let Some(ev) = s.next().await {
        if let JobEvent::Finished(r) = ev {
            outcome = r.map(|_| ());
        }
    }
    outcome
}

#[tokio::test]
async fn full_round_trip() {
    let f = fixture().await;
    let eng = engine(&f).await;

    // init
    eng.init_repo(&RepoSpec { path: f.repo.clone(), encryption: Encryption::RepokeyBlake2 })
        .await
        .unwrap();

    // two archives
    for name in ["arch-1", "arch-2"] {
        let spec = CreateSpec {
            archive_name: name.into(),
            sources: vec![f.src.clone()],
            excludes: vec![],
            compression: Compression::Zstd,
            one_file_system: false,
        };
        run_to_finish(eng.create(&spec).await.unwrap()).await.unwrap();
    }

    // list streams the fixture files
    let items: Vec<_> = eng
        .list_archive(&ArchiveId("arch-1".into()))
        .await
        .unwrap()
        .collect()
        .await;
    let paths: Vec<String> = items.into_iter().map(|i| i.unwrap().path).collect();
    assert!(paths.iter().any(|p| p.ends_with("hello.txt")));
    assert!(paths.iter().any(|p| p.ends_with("data.bin")));

    // extract_stdout matches source bytes
    let rel = format!("{}/hello.txt", f.src.strip_prefix("/").unwrap().display());
    let mut reader = eng.extract_stdout(&ArchiveId("arch-1".into()), &rel).await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"hello world");

    // prune keeps at least one, compact runs
    run_to_finish(
        eng.prune(&PrunePolicy { keep_hourly: 0, keep_daily: 0, keep_weekly: 0, keep_monthly: 1 })
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    run_to_finish(eng.compact().await.unwrap()).await.unwrap();
    run_to_finish(eng.check(CheckLevel::Repository).await.unwrap()).await.unwrap();
}

#[tokio::test]
async fn wrong_passphrase_yields_passphrase_wrong() {
    let f = fixture().await;
    let eng = engine(&f).await;
    eng.init_repo(&RepoSpec { path: f.repo.clone(), encryption: Encryption::RepokeyBlake2 })
        .await
        .unwrap();

    // Overwrite the stored passphrase with the wrong one.
    f.secrets.set("test", "not-the-passphrase").await.unwrap();
    let err = eng.repo_info().await.unwrap_err();
    assert_eq!(err, EngineError::PassphraseWrong);
}

#[tokio::test]
async fn unreachable_path_yields_repo_unreachable() {
    let f = fixture().await;
    // Point at a repo that does not exist; never initialised.
    let eng = BorgCli::new(
        format!("{}/does-not-exist", f.repo),
        "test".into(),
        f.secrets.clone(),
    )
    .await
    .unwrap();
    let err = eng.repo_info().await.unwrap_err();
    assert_eq!(err, EngineError::RepoUnreachable);
}
```

- [ ] **Step 2: Run the integration test locally**

Run: `just test-integration`
Expected: PASS for `borg_engine` (borg present). If borg is absent the recipe prints a skip and exits 0 — install borg to actually exercise it.

> If `extract_stdout`'s `rel` path fails to match Borg's stored path, print the `list` output first and align the path exactly (Borg stores source paths without a leading `/`). Adjust `rel` to the exact stored string.

- [ ] **Step 3: Mark T2 done and commit**

In `backtrack_plan/progress.md`, change `- [/] S02-T2 …` to `- [x] S02-T2 …`.

```bash
git add crates/backtrack-core/tests/borg_engine.rs backtrack_plan/progress.md
git commit -m "S02-T2: real-borg integration suite (create/list/extract/prune/compact + error paths)"
just sync-board-apply
```

---

### Task 9: Repo lifecycle integration (S02-T4)

**Files:**
- Create: `crates/backtrack-core/tests/borg_repo_lifecycle.rs` (feature-gated `integration`)
- Modify: `backtrack_plan/progress.md` (mark S02-T4 `[x]`)

**Interfaces:**
- Consumes: same as Task 8. `init_repo` / `key_export` / `repo_info` are already implemented in Task 7; this task proves the S02-T4 acceptance (init → key_export non-empty → import with right/wrong passphrase).

- [ ] **Step 1: Write the lifecycle integration test**

Create `crates/backtrack-core/tests/borg_repo_lifecycle.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! S02-T4: repo init / key export / import (open + verify passphrase).
#![cfg(feature = "integration")]

use std::sync::Arc;

use backtrack_core::engine::{
    BackupEngine, BorgCli, EngineError, Encryption, RepoSpec,
};
use backtrack_core::secret::{FileSecretStore, SecretStore};

const PASS: &str = "lifecycle-pass";

async fn store(dir: &std::path::Path) -> Arc<dyn SecretStore> {
    let s = FileSecretStore::new(dir.join("secrets.json"));
    s.set("test", PASS).await.unwrap();
    Arc::new(s)
}

#[tokio::test]
async fn init_export_then_import_right_and_wrong() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo").to_str().unwrap().to_string();
    let secrets = store(dir.path()).await;

    // init
    let eng = BorgCli::new(repo.clone(), "test".into(), secrets.clone()).await.unwrap();
    eng.init_repo(&RepoSpec { path: repo.clone(), encryption: Encryption::RepokeyBlake2 })
        .await
        .unwrap();

    // key_export is non-empty text
    let key = eng.key_export().await.unwrap();
    assert!(key.contains("BORG") || !key.trim().is_empty(), "recovery key should be text");

    // import (a fresh engine over the same repo) with the right passphrase: repo_info works
    let eng2 = BorgCli::new(repo.clone(), "test".into(), secrets.clone()).await.unwrap();
    let info = eng2.repo_info().await.unwrap();
    assert!(!info.repository_id.is_empty());

    // import with the wrong passphrase
    secrets.set("test", "wrong").await.unwrap();
    let eng3 = BorgCli::new(repo.clone(), "test".into(), secrets.clone()).await.unwrap();
    assert_eq!(eng3.repo_info().await.unwrap_err(), EngineError::PassphraseWrong);
}
```

- [ ] **Step 2: Run the integration test**

Run: `just test-integration`
Expected: PASS for `borg_repo_lifecycle`.

- [ ] **Step 3: Mark T4 done and commit**

In `backtrack_plan/progress.md`, change `- [ ] S02-T4 …` to `- [x] S02-T4 …`.

```bash
git add crates/backtrack-core/tests/borg_repo_lifecycle.rs backtrack_plan/progress.md
git commit -m "S02-T4: repo lifecycle integration (init, key export, import)"
just sync-board-apply
```

---

### Task 10: CI flake gate, decision notes, close-out (S02-T5)

**Files:**
- Modify: `backtrack_plan/progress.md` (mark S02-T5 `[x]`, append decision notes)

**Interfaces:** none (process task).

- [ ] **Step 1: Confirm CI already runs the integration suite**

Read `.github/workflows/ci.yml`. The `Integration tests (real borg)` step already runs `just test-integration` in the Fedora container (borg installed). No workflow change is needed — the new `tests/*.rs` are picked up automatically.

- [ ] **Step 2: Verify the whole gate locally**

Run: `just check` then `just test-integration`
Expected: fmt clean, `clippy -D warnings` clean, all unit tests pass, license headers present, integration suite green.

- [ ] **Step 3: Flake check — two consecutive green integration runs**

Run: `just test-integration && just test-integration`
Expected: both runs PASS. (Each test uses its own temp dir, so they are order-independent and parallel-safe.)

- [ ] **Step 4: Append decision notes to progress.md**

Under `## Notes / decisions made during implementation`, add a dated entry recording: the exact borg flags chosen per operation; that `list --json-lines` carries `size`/`mtime`/`mode`/`type` by default (so no explicit `--format` is needed, a deviation from the stage's literal wording); `BORG_EXIT_CODES=modern`; the locale pinning; the classification precedence order; the in-band terminal-error and SIGTERM-on-cancel model; and that `MockEngine` lives in `backtrack-testkit` (dev-dependency only) rather than in core.

- [ ] **Step 5: Mark T5 done, commit, sync**

In `backtrack_plan/progress.md`, change `- [ ] S02-T5 …` to `- [x] S02-T5 …`.

```bash
git add backtrack_plan/progress.md
git commit -m "S02-T5: CI integration gate confirmed + Stage 2 decision notes"
just sync-board-apply
```

- [ ] **Step 6: Finish the branch**

All of S02-T1…T5 are `[x]`. Use the finishing-a-development-branch skill to open the PR to `main` (CI must be green, including the two-run integration gate).

---

## Self-Review

**Spec coverage:**
- Trait + `JobStream` + `JobEvent` typed events → Task 2. ✓
- Error taxonomy (all 10 variants) + health.md exhaustiveness test → Task 1. ✓
- `MockEngine` for later tests → Task 4 (in `backtrack-testkit`). ✓
- `BorgCli` create/list/extract/extract_stdout/prune/compact/check + `--log-json` parsing → Tasks 5, 7. ✓
- Env (`BORG_PASSPHRASE`, relocation, locale) + version probe → Task 7. ✓
- Exit-code + stderr → taxonomy mapping table → Task 6. ✓
- `SecretStore` (oo7 + file dev store, missing → `PassphraseMissing`) → Task 3. ✓
- Repo lifecycle (init/import/key_export) → Task 7 (impl) + Task 9 (accept). ✓
- Integration suite in CI, <30 s, flake gate → Tasks 8, 9, 10. ✓
- Definition of done (progress.md per task, board sync, decision notes) → Tasks 1–10. ✓

**Placeholder scan:** No "TBD"/"handle edge cases"/"similar to". Two flagged verification points (oo7 0.6 method names in Task 3; extract_stdout `rel` path in Task 8) are explicit, with concrete fallback instructions — not placeholders.

**Type consistency:** `EngineError`/`Result` shared across engine + secret; `JobStream`/`JobEvent`/`JobSummary`/`LogLevel` defined in Task 2 and used unchanged in tasks 4, 7, 8, 9; `Parsed` (Task 5) consumed only in Task 7; `ErrLine`/`classify` (Task 6) consumed in Task 7. `BorgItem` reused from Stage 1 unchanged. `FileSecretStore::new(PathBuf)` / `dev_default()` signatures match between Tasks 3, 8, 9. `MockEngine` builder methods (`with_create_events`, etc.) are consistent within Task 4.
