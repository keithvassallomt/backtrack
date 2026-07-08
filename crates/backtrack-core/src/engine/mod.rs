// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>

//! The Borg adapter: one trait, [`BackupEngine`], behind which every
//! Borg-specific operation lives, plus its typed error taxonomy and streamed
//! job events. Borg 2 later is a second implementation, not a rewrite.

mod borg;
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
