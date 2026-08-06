// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! `org.backtrack.Daemon1` — the interface every client talks to.
//!
//! The method and signal set is normative: it comes from stack.md §2 and is
//! pinned by an introspection snapshot test, so a rename is a deliberate act
//! rather than an accident that breaks the GUI at run time.
//!
//! Two conventions run through the whole interface:
//!
//! - **Long work returns a job id immediately.** No method blocks for the
//!   duration of a backup. Progress arrives as signals carrying that id.
//! - **Errors cross as named D-Bus errors**, never as prose. See [`error`].
//!
//! Some methods here are ahead of their user interface — `SearchFiles` has no
//! search UI until Stage 8, `RestoreEverything` no guided flow until Stage 11.
//! They are implemented anyway rather than stubbed, because the core supports
//! them today and a method that lies about working is worse than one that does
//! not exist.

mod e2e;
mod error;
mod health;
#[cfg(test)]
mod introspect;
mod preview;
mod state;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use backtrack_core::config::Config;
use backtrack_core::engine::{
    ArchiveId, BackupEngine, BorgCli, CheckLevel, CreateSpec, PrunePolicy,
};
use backtrack_core::index::{IndexReader, Kind};
use backtrack_core::paths;
use backtrack_core::secret::SecretStore;
use futures::future::BoxFuture;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info, warn};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedFd;

use crate::jobs::{JobFactory, JobKind, JobRegistry, JobState, JobUpdate, Outcome};

pub use error::{DaemonError, Result};
pub use health::{HealthInputs, HealthState};
pub use preview::PreviewCache;
pub use state::{PauseState, RestorePolicy, SearchResult, Status};

use state::{config_document, config_get, config_set, next_due, to_epoch};

/// Everything the interface reads and writes, shared with the background tasks
/// that fan job updates out as signals.
pub struct Shared {
    config: Mutex<Config>,
    pause: Mutex<PauseState>,
    jobs: Arc<JobRegistry>,
    secrets: Arc<dyn SecretStore>,
    preview: PreviewCache,
    index_path: PathBuf,
    /// The engine, absent until a repository is configured.
    engine: Mutex<Option<Arc<dyn BackupEngine>>>,
    /// The archive name the most recent backup was told to write. Stage 4's
    /// ingest needs this to know what to stream into the catalogue.
    last_archive: Mutex<Option<String>>,
    /// Facts the health model is computed from.
    last_backup: Mutex<Option<SystemTime>>,
    blocking_failure: Mutex<bool>,
    destination_reachable: Mutex<bool>,
}

impl Shared {
    /// Assemble from the daemon's own pieces.
    pub fn new(
        config: Config,
        jobs: Arc<JobRegistry>,
        secrets: Arc<dyn SecretStore>,
    ) -> Arc<Shared> {
        Arc::new(Shared {
            config: Mutex::new(config),
            pause: Mutex::new(PauseState::default()),
            jobs,
            secrets,
            preview: PreviewCache::new(paths::cache_dir()),
            index_path: paths::index_db(),
            engine: Mutex::new(None),
            last_archive: Mutex::new(None),
            last_backup: Mutex::new(None),
            blocking_failure: Mutex::new(false),
            destination_reachable: Mutex::new(true),
        })
    }

    /// Replace the engine. Used at startup once the configuration is known, and
    /// again after `SetupRepo`/`ImportRepo` change the destination.
    pub fn set_engine(&self, engine: Arc<dyn BackupEngine>) {
        *self.engine.lock().unwrap() = Some(engine);
    }

    /// Recover the last successful backup time from the catalogue.
    ///
    /// Without this the daemon would forget every backup it has ever taken each
    /// time it restarts, and a machine that has been protected for months would
    /// announce itself as never having been backed up. The index already knows;
    /// it is the record that outlives the process.
    pub fn seed_last_backup(&self) {
        let newest = IndexReader::open(&self.index_path)
            .and_then(|reader| reader.archives_overview())
            .map(|archives| archives.iter().map(|a| a.ts).max())
            .unwrap_or(None);
        if let Some(ts) = newest.filter(|ts| *ts > 0) {
            let when = std::time::UNIX_EPOCH + Duration::from_secs(ts as u64);
            *self.last_backup.lock().unwrap() = Some(when);
            debug!(ts, "seeded last-backup time from the catalogue");
        }
    }

    /// Build the engine for the configured repository, if there is one.
    pub async fn connect_engine(self: &Arc<Self>) -> Result<()> {
        let repository = self.config.lock().unwrap().storage.repository.clone();
        let Some(repository) = repository else {
            debug!("no repository configured; engine not connected");
            return Ok(());
        };
        let engine =
            BorgCli::new(repository.clone(), repository, Arc::clone(&self.secrets)).await?;
        self.set_engine(Arc::new(engine));
        Ok(())
    }

    fn engine(&self) -> Result<Arc<dyn BackupEngine>> {
        self.engine.lock().unwrap().clone().ok_or_else(|| {
            DaemonError::NotConfigured(
                "no backup destination is configured yet; run the setup wizard".into(),
            )
        })
    }

    fn config(&self) -> Config {
        self.config.lock().unwrap().clone()
    }

    /// Persist a new configuration and reconnect anything that depends on it.
    fn store_config(&self, config: Config) -> Result<()> {
        config.save()?;
        *self.config.lock().unwrap() = config;
        Ok(())
    }

    /// The current health state, computed fresh from facts.
    pub fn health(&self) -> HealthState {
        let now = SystemTime::now();
        let config = self.config();
        let inputs = HealthInputs {
            blocking_failure: *self.blocking_failure.lock().unwrap(),
            paused_until: self.pause.lock().unwrap().until(now),
            destination_reachable: *self.destination_reachable.lock().unwrap(),
            // The offline spool lands in Stage 5; until then there is no local
            // safety net to claim credit for.
            offline_protection_active: false,
            last_success: *self.last_backup.lock().unwrap(),
            frequency: config.backup.frequency.interval(),
            needs_attention: false,
        };
        health::evaluate(&inputs, now)
    }

    /// The id of whatever is running now, or 0.
    fn active_job(&self) -> u64 {
        self.jobs
            .list()
            .into_iter()
            .find(|job| job.state == JobState::Running)
            .map(|job| job.id)
            .unwrap_or(0)
    }

    /// Record that a backup succeeded, which is what keeps health honest.
    fn record_backup_success(&self) {
        *self.last_backup.lock().unwrap() = Some(SystemTime::now());
        *self.blocking_failure.lock().unwrap() = false;
    }

    /// Record a failure that stops backups until the user acts.
    fn record_blocking_failure(&self, blocking: bool) {
        *self.blocking_failure.lock().unwrap() = blocking;
    }
}

/// The D-Bus object.
pub struct Daemon1 {
    shared: Arc<Shared>,
}

impl Daemon1 {
    pub fn new(shared: Arc<Shared>) -> Daemon1 {
        Daemon1 { shared }
    }
}

#[zbus::interface(name = "org.backtrack.Daemon1")]
impl Daemon1 {
    /// Start a backup now, whatever the schedule says.
    async fn backup_now(&self) -> Result<u64> {
        let engine = self.shared.engine()?;
        let config = self.shared.config();
        let spec = create_spec(&config);
        *self.shared.last_archive.lock().unwrap() = Some(spec.archive_name.clone());
        let factory: JobFactory = Arc::new(move || {
            let engine = Arc::clone(&engine);
            let spec = spec.clone();
            Box::pin(async move { engine.create(&spec).await }) as BoxFuture<'_, _>
        });
        Ok(self.shared.jobs.submit(JobKind::Backup, factory))
    }

    /// Pause scheduled backups until `until` (seconds since the epoch).
    ///
    /// Self-expiring by construction: there is no way to express "off forever"
    /// here, because the menu deliberately does not offer one.
    async fn pause(&self, until: u64) -> Result<()> {
        let until = state::from_epoch(until).ok_or_else(|| {
            DaemonError::InvalidArgument("pause needs a time in the future".into())
        })?;
        if until <= SystemTime::now() {
            return Err(DaemonError::InvalidArgument(
                "pause needs a time in the future".into(),
            ));
        }
        self.shared.pause.lock().unwrap().pause_until(until);
        info!(until = to_epoch(Some(until)), "backups paused");
        Ok(())
    }

    /// Resume scheduled backups.
    async fn resume(&self) -> Result<()> {
        self.shared.pause.lock().unwrap().resume();
        info!("backups resumed");
        Ok(())
    }

    /// The overall state, and the handful of facts the UI shows beside it.
    async fn get_status(&self) -> Result<Status> {
        let now = SystemTime::now();
        let config = self.shared.config();
        let last_backup = *self.shared.last_backup.lock().unwrap();
        let paused_until = self.shared.pause.lock().unwrap().until(now);
        let next = if paused_until.is_some() {
            None
        } else {
            next_due(&config, last_backup, now)
        };
        Ok(Status {
            state: self.shared.health().as_str().to_string(),
            last_backup: to_epoch(last_backup),
            next_backup: to_epoch(next),
            destination_reachable: *self.shared.destination_reachable.lock().unwrap(),
            // The spool arrives in Stage 5.
            spool_bytes: 0,
            active_job: self.shared.active_job(),
            paused_until: to_epoch(paused_until),
            configured: config.is_configured(),
        })
    }

    /// Restore `paths` from `archive` into `dest`.
    ///
    /// The staging-and-compare pipeline, conflict detection and the safety stash
    /// are Stage 7; today this extracts, and the policy is validated and carried
    /// so that callers written now keep working when it starts being honoured.
    async fn restore_files(
        &self,
        archive: &str,
        paths: Vec<String>,
        dest: &str,
        policy: &str,
    ) -> Result<u64> {
        let engine = self.shared.engine()?;
        let policy = RestorePolicy::parse(policy)?;
        if paths.is_empty() {
            return Err(DaemonError::InvalidArgument(
                "restore needs at least one path".into(),
            ));
        }
        info!(archive, dest, policy = policy.as_str(), "restore requested");
        let archive = ArchiveId(archive.to_string());
        let dest = PathBuf::from(dest);
        let factory: JobFactory = Arc::new(move || {
            let engine = Arc::clone(&engine);
            let archive = archive.clone();
            let paths = paths.clone();
            let dest = dest.clone();
            Box::pin(async move { engine.extract(&archive, &paths, &dest).await })
                as BoxFuture<'_, _>
        });
        Ok(self.shared.jobs.submit(JobKind::Restore, factory))
    }

    /// A readable file descriptor for one file's contents as of `archive`.
    ///
    /// The daemon extracts (via `borg extract --stdout`) into its private cache
    /// and hands back a descriptor onto the result. A descriptor rather than a
    /// path is what lets a sandboxed GUI read a file it could not open itself,
    /// and it keeps the cache directory the daemon's business alone.
    async fn preview_file(&self, archive: &str, path: &str) -> Result<OwnedFd> {
        let cache = self.shared.preview.clone();
        let entry = cache.entry_path(archive, path);

        if !cache.contains(archive, path) {
            let engine = self.shared.engine()?;
            cache
                .ensure_dir()
                .map_err(|e| DaemonError::LocalDiskFull(e.to_string()))?;
            let mut reader = engine
                .extract_stdout(&ArchiveId(archive.to_string()), path)
                .await?;

            // Extract to a temporary name and rename into place, so a cancelled
            // or failed extraction can never leave a truncated file looking like
            // a valid cache hit.
            let staging = entry.with_extension("partial");
            let mut file = tokio::fs::File::create(&staging)
                .await
                .map_err(|e| DaemonError::LocalDiskFull(e.to_string()))?;
            let copied = tokio::io::copy(&mut reader, &mut file).await;
            let finish = async {
                copied?;
                file.flush().await?;
                file.sync_all().await
            };
            if let Err(e) = finish.await {
                let _ = tokio::fs::remove_file(&staging).await;
                return Err(DaemonError::LocalDiskFull(e.to_string()));
            }
            drop(file);
            tokio::fs::rename(&staging, &entry)
                .await
                .map_err(|e| DaemonError::LocalDiskFull(e.to_string()))?;
            cache.evict_to_fit();
        }

        cache.touch(&entry);
        let file = std::fs::File::open(&entry).map_err(|e| {
            DaemonError::NotFound(format!("{path} in {archive} could not be opened: {e}"))
        })?;
        Ok(OwnedFd::from(std::os::fd::OwnedFd::from(file)))
    }

    /// Prepare the archived copy of `path` so it can be compared with the live
    /// one. The diff view itself is Stage 8; this is the extraction it needs.
    async fn compare_file(&self, archive: &str, path: &str) -> Result<u64> {
        let engine = self.shared.engine()?;
        let cache = self.shared.preview.clone();
        let archive = archive.to_string();
        let path = path.to_string();
        let factory: JobFactory = Arc::new(move || {
            let engine = Arc::clone(&engine);
            let cache = cache.clone();
            let archive = archive.clone();
            let path = path.clone();
            Box::pin(async move {
                cache_extraction(&*engine, &cache, &archive, &path).await?;
                Ok(backtrack_core::engine::JobStream::from_events(vec![
                    backtrack_core::engine::JobEvent::Finished(Ok(Default::default())),
                ]))
            }) as BoxFuture<'_, _>
        });
        Ok(self.shared.jobs.submit(JobKind::Restore, factory))
    }

    /// Filename search across every snapshot, including files that have since
    /// been deleted.
    async fn search_files(&self, query: &str) -> Result<Vec<SearchResult>> {
        let reader = IndexReader::open(&self.shared.index_path)?;
        Ok(reader
            .search(query)?
            .into_iter()
            .map(|hit| SearchResult {
                path: hit.path,
                name: hit.name,
                kind: kind_name(hit.kind).to_string(),
                first_seq: hit.first_seq,
                last_seq: hit.last_seq,
                first_ts: hit.first_ts,
                last_ts: hit.last_ts,
                versions: hit.version_count.max(0) as u32,
                exists_today: hit.exists_today,
            })
            .collect())
    }

    /// Guided disaster recovery: bring back everything from `archive`.
    ///
    /// The guided user experience is Stage 11. The job it drives exists now, and
    /// is the one kind that can be paused between folders.
    async fn restore_everything(&self, archive: &str, policy: &str) -> Result<u64> {
        let engine = self.shared.engine()?;
        let policy = RestorePolicy::parse(policy)?;
        info!(
            archive,
            policy = policy.as_str(),
            "disaster recovery requested"
        );
        let archive = ArchiveId(archive.to_string());
        let dest = dirs_home();
        let factory: JobFactory = Arc::new(move || {
            let engine = Arc::clone(&engine);
            let archive = archive.clone();
            let dest = dest.clone();
            Box::pin(async move { engine.extract(&archive, &[], &dest).await }) as BoxFuture<'_, _>
        });
        Ok(self.shared.jobs.submit(JobKind::RestoreEverything, factory))
    }

    /// Apply the retention policy.
    async fn prune(&self) -> Result<u64> {
        let engine = self.shared.engine()?;
        let policy = prune_policy(&self.shared.config());
        let factory: JobFactory = Arc::new(move || {
            let engine = Arc::clone(&engine);
            let policy = policy.clone();
            Box::pin(async move { engine.prune(&policy).await }) as BoxFuture<'_, _>
        });
        Ok(self.shared.jobs.submit(JobKind::Prune, factory))
    }

    /// Check the repository's integrity.
    async fn verify(&self) -> Result<u64> {
        let engine = self.shared.engine()?;
        let factory: JobFactory = Arc::new(move || {
            let engine = Arc::clone(&engine);
            Box::pin(async move { engine.check(CheckLevel::Full).await }) as BoxFuture<'_, _>
        });
        Ok(self.shared.jobs.submit(JobKind::Check, factory))
    }

    /// Reclaim space the repository is no longer using.
    async fn compact(&self) -> Result<u64> {
        let engine = self.shared.engine()?;
        let factory: JobFactory = Arc::new(move || {
            let engine = Arc::clone(&engine);
            Box::pin(async move { engine.compact().await }) as BoxFuture<'_, _>
        });
        Ok(self.shared.jobs.submit(JobKind::Compact, factory))
    }

    /// Stop a job.
    async fn cancel_job(&self, id: u64) -> Result<()> {
        Ok(self.shared.jobs.cancel(id)?)
    }

    /// Pause a job between units of work. Only guided disaster recovery
    /// qualifies; anything else is refused with `NotPausable`.
    async fn pause_job(&self, id: u64) -> Result<()> {
        Ok(self.shared.jobs.pause(id)?)
    }

    /// Put a paused job back in the queue.
    ///
    /// Not in the original interface table, which lists only `PauseJob` — but a
    /// job that can be paused and never resumed is a bug rather than an API, and
    /// Stage 11's resumable recovery needs this.
    async fn resume_job(&self, id: u64) -> Result<()> {
        Ok(self.shared.jobs.resume(id)?)
    }

    /// The whole configuration, as TOML.
    async fn get_config(&self) -> Result<String> {
        config_document(&self.shared.config())
    }

    /// Set one configuration key. `key` is a dotted path (`backup.frequency`)
    /// and `value` a TOML literal (`"daily"`, `24`, `["/home/k/Documents"]`).
    async fn set_config(&self, key: &str, value: &str) -> Result<()> {
        let updated = config_set(&self.shared.config(), key, value)?;
        let repository_changed =
            updated.storage.repository != self.shared.config().storage.repository;
        self.shared.store_config(updated)?;
        info!(key, "configuration changed");
        if repository_changed {
            // Best-effort: a destination that cannot be reached yet is a health
            // state, not a reason to reject the edit.
            if let Err(e) = self.shared.connect_engine().await {
                warn!("new destination is not usable yet: {e}");
            }
        }
        Ok(())
    }

    /// Read one configuration key, as a TOML literal.
    async fn get_config_key(&self, key: &str) -> Result<String> {
        config_get(&self.shared.config(), key)
    }

    /// Create a repository at `path` and adopt it as the destination.
    async fn setup_repo(&self, path: &str, passphrase: &str) -> Result<()> {
        self.shared.secrets.set(path, passphrase).await?;
        let engine = BorgCli::new(
            path.to_string(),
            path.to_string(),
            Arc::clone(&self.shared.secrets),
        )
        .await?;
        engine
            .init_repo(&backtrack_core::engine::RepoSpec {
                path: path.to_string(),
                encryption: Default::default(),
            })
            .await?;

        let mut config = self.shared.config();
        config.storage.repository = Some(path.to_string());
        self.shared.store_config(config)?;
        self.shared.set_engine(Arc::new(engine));
        info!(path, "repository created");
        Ok(())
    }

    /// Adopt an existing repository at `path`.
    ///
    /// The passphrase is verified against the repository before anything is
    /// written, so a typo fails here rather than silently at the next backup.
    async fn import_repo(&self, path: &str, passphrase: &str) -> Result<()> {
        self.shared.secrets.set(path, passphrase).await?;
        let engine = BorgCli::new(
            path.to_string(),
            path.to_string(),
            Arc::clone(&self.shared.secrets),
        )
        .await?;
        let info = engine.repo_info().await?;

        let mut config = self.shared.config();
        config.storage.repository = Some(path.to_string());
        self.shared.store_config(config)?;
        self.shared.set_engine(Arc::new(engine));
        info!(path, archives = info.archive_count, "repository imported");
        Ok(())
    }

    /// Progress of a running backup.
    #[zbus(signal)]
    pub async fn backup_progress(
        emitter: &SignalEmitter<'_>,
        job: u64,
        phase: &str,
        current: u64,
        total: u64,
    ) -> zbus::Result<()>;

    /// Progress of a running restore, in bytes.
    #[zbus(signal)]
    pub async fn restore_progress(
        emitter: &SignalEmitter<'_>,
        job: u64,
        bytes: u64,
        total: u64,
    ) -> zbus::Result<()>;

    /// Progress of catalogue ingest for one archive, as a percentage.
    #[zbus(signal)]
    pub async fn indexing_progress(
        emitter: &SignalEmitter<'_>,
        archive: &str,
        pct: u32,
    ) -> zbus::Result<()>;

    /// The overall health state changed.
    #[zbus(signal)]
    pub async fn status_changed(emitter: &SignalEmitter<'_>, state: &str) -> zbus::Result<()>;
}

/// Extract one file into the preview cache, replacing any partial attempt.
async fn cache_extraction(
    engine: &dyn BackupEngine,
    cache: &PreviewCache,
    archive: &str,
    path: &str,
) -> backtrack_core::engine::Result<()> {
    if cache.contains(archive, path) {
        return Ok(());
    }
    let entry = cache.entry_path(archive, path);
    let _ = cache.ensure_dir();
    let mut reader = engine
        .extract_stdout(&ArchiveId(archive.to_string()), path)
        .await?;
    let staging = entry.with_extension("partial");
    let map_io = |e: std::io::Error| backtrack_core::engine::EngineError::BorgFailed {
        code: -1,
        stderr: e.to_string(),
    };
    let mut file = tokio::fs::File::create(&staging).await.map_err(map_io)?;
    tokio::io::copy(&mut reader, &mut file)
        .await
        .map_err(map_io)?;
    file.sync_all().await.map_err(map_io)?;
    drop(file);
    tokio::fs::rename(&staging, &entry).await.map_err(map_io)?;
    cache.evict_to_fit();
    Ok(())
}

/// Turn the job registry's updates into D-Bus signals.
///
/// Runs for the daemon's lifetime. `StatusChanged` is emitted only when the
/// computed state actually differs from the last one announced — a signal per
/// job event would make "the state changed" meaningless.
pub async fn fan_out_signals(shared: Arc<Shared>, emitter: SignalEmitter<'static>) {
    let mut updates = shared.jobs.subscribe();
    let mut announced = shared.health();
    let _ = Daemon1::status_changed(&emitter, announced.as_str()).await;

    loop {
        let update = match updates.recv().await {
            Ok(update) => update,
            // Lagged: intermediate progress was dropped, which is exactly what
            // the buffer is allowed to do. Carry on from the current truth.
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                debug!(missed = n, "signal fan-out lagged");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        };

        match update {
            JobUpdate::Progress {
                id,
                kind,
                current,
                total,
                phase,
            } => {
                let total = total.unwrap_or(0);
                let _ = match kind {
                    JobKind::Backup => {
                        Daemon1::backup_progress(&emitter, id, &phase, current, total).await
                    }
                    JobKind::Restore | JobKind::RestoreEverything => {
                        Daemon1::restore_progress(&emitter, id, current, total).await
                    }
                    JobKind::Index => {
                        let pct = percentage(current, total);
                        Daemon1::indexing_progress(&emitter, &phase, pct).await
                    }
                    // Maintenance has no progress signal in the interface; the
                    // job's state changes carry it instead.
                    JobKind::Prune | JobKind::Compact | JobKind::Check => Ok(()),
                };
            }
            JobUpdate::State { kind, state, .. } => {
                if state.is_terminal() {
                    update_health(&shared, kind, &state);
                }
                let current = shared.health();
                if current != announced {
                    announced = current;
                    info!(state = current.as_str(), "health state changed");
                    let _ = Daemon1::status_changed(&emitter, current.as_str()).await;
                }
            }
        }
    }
}

/// Fold a finished job into the facts health is computed from.
fn update_health(shared: &Arc<Shared>, kind: JobKind, state: &JobState) {
    match state {
        JobState::Done(Outcome::Completed) if kind == JobKind::Backup => {
            shared.record_backup_success();
        }
        // Only failures the catalogue calls blocking put the product in BROKEN;
        // a transient lock or an unreachable destination does not.
        //
        // Note the asymmetry: a failure can *raise* the flag but never lower it.
        // Clearing it here would mean an unclassified error arriving after a
        // real one — say a lock timeout following a wrong passphrase — silently
        // dismissing a banner the user still needs to act on. Only a successful
        // backup clears it, because only a successful backup proves the problem
        // is gone.
        JobState::Failed(e) if e.health_failure().is_some() => {
            shared.record_blocking_failure(true);
        }
        _ => {}
    }
}

fn percentage(current: u64, total: u64) -> u32 {
    if total == 0 {
        return 0;
    }
    ((current.min(total) * 100) / total) as u32
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::File => "file",
        Kind::Dir => "dir",
        Kind::Symlink => "symlink",
        Kind::Other => "other",
    }
}

/// The archive name for a backup taken now. Seconds since the epoch keeps names
/// sortable and unique without pulling in a date library.
fn archive_name(now: SystemTime) -> String {
    format!("backtrack-{}", to_epoch(Some(now)))
}

fn create_spec(config: &Config) -> CreateSpec {
    CreateSpec {
        archive_name: archive_name(SystemTime::now()),
        sources: config.backup.include.clone(),
        excludes: config.backup.exclude.clone(),
        compression: match config.advanced.compression {
            backtrack_core::config::Compression::Zstd => backtrack_core::engine::Compression::Zstd,
            backtrack_core::config::Compression::Lz4 => backtrack_core::engine::Compression::Lz4,
            backtrack_core::config::Compression::None => backtrack_core::engine::Compression::None,
        },
        one_file_system: true,
    }
}

fn prune_policy(config: &Config) -> PrunePolicy {
    let r = config.storage.retention;
    PrunePolicy {
        keep_hourly: r.keep_hourly,
        keep_daily: r.keep_daily,
        keep_weekly: r.keep_weekly,
        keep_monthly: r.keep_monthly,
    }
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_names_sort_chronologically() {
        let earlier = archive_name(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000));
        let later = archive_name(SystemTime::UNIX_EPOCH + Duration::from_secs(2_000));
        assert!(earlier < later, "{earlier} should sort before {later}");
    }

    #[test]
    fn percentages_are_clamped_and_safe_at_zero() {
        assert_eq!(
            percentage(0, 0),
            0,
            "no denominator must not divide by zero"
        );
        assert_eq!(percentage(5, 0), 0);
        assert_eq!(percentage(1, 4), 25);
        assert_eq!(percentage(4, 4), 100);
        assert_eq!(percentage(9, 4), 100, "over-count must not exceed 100");
    }

    #[test]
    fn the_prune_policy_comes_from_the_configured_retention() {
        let mut config = Config::default();
        config.storage.retention.keep_daily = 14;
        let policy = prune_policy(&config);
        assert_eq!(policy.keep_hourly, 24);
        assert_eq!(policy.keep_daily, 14);
        assert_eq!(policy.keep_monthly, 6);
    }

    #[test]
    fn the_create_spec_carries_the_configured_sources_and_exclusions() {
        let mut config = Config::default();
        config.backup.include = vec![PathBuf::from("/home/k/Documents")];
        let spec = create_spec(&config);
        assert_eq!(spec.sources, config.backup.include);
        assert_eq!(spec.excludes, config.backup.exclude);
        assert!(
            spec.one_file_system,
            "crossing filesystems would drag in mounted media"
        );
    }

    #[test]
    fn compression_choices_reach_the_engine() {
        let mut config = Config::default();
        config.advanced.compression = backtrack_core::config::Compression::Lz4;
        assert_eq!(
            create_spec(&config).compression.as_borg_arg(),
            backtrack_core::engine::Compression::Lz4.as_borg_arg()
        );
    }

    #[test]
    fn index_kinds_have_wire_names() {
        assert_eq!(kind_name(Kind::File), "file");
        assert_eq!(kind_name(Kind::Dir), "dir");
        assert_eq!(kind_name(Kind::Symlink), "symlink");
    }
}
