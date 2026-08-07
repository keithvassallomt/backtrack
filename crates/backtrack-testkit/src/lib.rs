// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

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
use backtrack_core::index::{ArchiveMeta, BorgItem};
use backtrack_core::secret::SecretStore;

/// A scriptable [`BackupEngine`]. Set the events `create`/`prune`/etc. should
/// emit and the items `list_archive` should yield; optionally force every call
/// to fail with a fixed error.
///
/// It also keeps a pretend repository: `create` adds an archive to it and
/// `prune` removes the oldest, so a caller can watch the catalogue follow the
/// repository without a real Borg anywhere near the test.
#[derive(Default)]
pub struct MockEngine {
    create_events: Vec<JobEvent>,
    create_pending: bool,
    items: Vec<BorgItem>,
    key: String,
    info: Option<RepoInfo>,
    fail: Option<EngineError>,
    archives: Mutex<Vec<ArchiveMeta>>,
    /// How many archives `prune` leaves behind; `None` means it removes none.
    prune_keeps: Option<usize>,
    /// Fails `list_archive` after its items, mimicking a listing cut short.
    listing_error: Option<EngineError>,
    /// Fails `prune`, so retention failing can be told apart from backing up
    /// failing.
    prune_error: Option<EngineError>,
}

impl MockEngine {
    pub fn with_create_events(mut self, events: Vec<JobEvent>) -> Self {
        self.create_events = events;
        self
    }

    /// Pre-populate the pretend repository.
    pub fn with_archives(self, archives: Vec<ArchiveMeta>) -> Self {
        *self.archives.lock().unwrap() = archives;
        self
    }

    /// Make `prune` keep only the newest `n` archives.
    pub fn keeping(mut self, n: usize) -> Self {
        self.prune_keeps = Some(n);
        self
    }

    /// The pretend repository's current contents.
    pub fn archives(&self) -> Vec<ArchiveMeta> {
        self.archives.lock().unwrap().clone()
    }

    /// Make `create` return a backup that keeps running until it is cancelled
    /// (see [`JobStream::pending`]). For exercising anything that acts on a job
    /// mid-flight — cancellation, queuing behind it, status while it runs.
    pub fn with_create_pending(mut self) -> Self {
        self.create_pending = true;
        self
    }
    pub fn with_items(mut self, items: Vec<BorgItem>) -> Self {
        self.items = items;
        self
    }

    /// Make `list_archive` yield its items and then fail — a Borg subprocess
    /// dying part way through a listing, which is the case that decides whether
    /// a half-read archive can be mistaken for a catalogued one.
    pub fn with_truncated_listing(mut self, err: EngineError) -> Self {
        self.listing_error = Some(err);
        self
    }
    /// Make `prune` fail with `err`, leaving `create` working.
    pub fn with_prune_error(mut self, err: EngineError) -> Self {
        self.prune_error = Some(err);
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
        Ok(self.info.clone().unwrap_or(RepoInfo {
            repository_id: "mock-repo".into(),
            archive_count: 0,
        }))
    }
    async fn key_export(&self) -> Result<String> {
        self.check_fail()?;
        Ok(self.key.clone())
    }
    async fn create(&self, spec: &CreateSpec) -> Result<JobStream> {
        if self.create_pending {
            self.check_fail()?;
            return Ok(JobStream::pending());
        }
        let stream = self.job(self.create_events.clone())?;
        // The archive exists as soon as create succeeds — which is exactly the
        // window a crash can fall into, and what reconciliation has to notice.
        let mut archives = self.archives.lock().unwrap();
        let ts = archives.last().map(|a| a.ts + 1_000).unwrap_or(1_000);
        let borg_id = Some(format!("mock-{}", archives.len() + 1));
        archives.push(ArchiveMeta {
            borg_id,
            name: spec.archive_name.clone(),
            ts,
        });
        Ok(stream)
    }
    async fn list_archives(&self) -> Result<Vec<ArchiveMeta>> {
        self.check_fail()?;
        Ok(self.archives.lock().unwrap().clone())
    }
    async fn list_archive(&self, _id: &ArchiveId) -> Result<BoxStream<'static, Result<BorgItem>>> {
        self.check_fail()?;
        let items = self.items.clone().into_iter().map(Ok);
        match self.listing_error.clone() {
            Some(err) => Ok(stream::iter(items.chain(std::iter::once(Err(err)))).boxed()),
            None => Ok(stream::iter(items).boxed()),
        }
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
        if let Some(err) = self.prune_error.clone() {
            return self.job(vec![JobEvent::Finished(Err(err))]);
        }
        let stream = self.job(vec![JobEvent::Finished(Ok(Default::default()))])?;
        if let Some(keep) = self.prune_keeps {
            let mut archives = self.archives.lock().unwrap();
            let drop_count = archives.len().saturating_sub(keep);
            archives.drain(..drop_count);
        }
        Ok(stream)
    }
    async fn delete_archives(&self, ids: &[ArchiveId]) -> Result<JobStream> {
        let stream = self.job(vec![JobEvent::Finished(Ok(Default::default()))])?;
        let names: Vec<&str> = ids.iter().map(|id| id.0.as_str()).collect();
        self.archives
            .lock()
            .unwrap()
            .retain(|a| !names.contains(&a.name.as_str()));
        Ok(stream)
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

#[cfg(test)]
mod tests {
    use super::*;
    use backtrack_core::engine::{BackupEngine, EngineError, JobEvent, JobSummary};
    use backtrack_core::secret::SecretStore;
    use futures::StreamExt;

    #[tokio::test]
    async fn mock_engine_drives_a_scripted_job_to_finished() {
        let engine = MockEngine::default().with_create_events(vec![
            JobEvent::Progress {
                current: 1,
                total: Some(1),
                phase: "archiving".into(),
            },
            JobEvent::ItemDone {
                path: "home/a".into(),
            },
            JobEvent::Finished(Ok(JobSummary {
                archive_id: Some("mock-1".into()),
            })),
        ]);

        let spec = backtrack_core::engine::CreateSpec {
            archive_name: "a".into(),
            sources: vec!["/tmp".into()],
            excludes: vec![],
            compression: Default::default(),
            one_file_system: true,
            created_at: std::time::SystemTime::UNIX_EPOCH,
            paths: vec![],
        };
        let events: Vec<JobEvent> = engine.create(&spec).await.unwrap().collect().await;
        assert!(matches!(events.last(), Some(JobEvent::Finished(Ok(_)))));
        assert_eq!(events.len(), 3);
    }

    #[tokio::test]
    async fn mock_secret_store_round_trips_and_reports_missing() {
        let store = MockSecretStore::default();
        assert!(matches!(
            store.get("r").await,
            Err(EngineError::PassphraseMissing)
        ));
        store.set("r", "p").await.unwrap();
        assert_eq!(store.get("r").await.unwrap(), "p");
    }
}
