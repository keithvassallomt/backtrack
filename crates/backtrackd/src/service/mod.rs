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
use backtrack_core::index::{IndexReader, IndexWriter, Kind};
use backtrack_core::paths;
use backtrack_core::restore::{self, Decision, Decisions};
use backtrack_core::secret::SecretStore;
use backtrack_core::state::RuntimeState;
use futures::future::BoxFuture;
use futures::StreamExt;
use tokio::io::AsyncWriteExt;
use tokio::sync::Notify;
use tracing::{debug, info, warn};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedFd;

use crate::jobs::{JobFactory, JobKind, JobRegistry, JobState, JobUpdate, Outcome};
use crate::pipeline;
use crate::preflight::{self, Facts, SystemProbe, UnknownProbe, Verdict};
use crate::reachability::{DestinationProbe, Reach, RealProbe};
use crate::schedule::{self, ScheduleInput};

pub use backtrack_core::dbus::{ReplacedFile, RestorePreview, SearchResult, Status};
pub use error::{DaemonError, Result};
pub use health::{HealthInputs, HealthState};
pub use preview::PreviewCache;
pub use state::{PauseState, RestorePolicy};

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
    /// Bookkeeping that has to survive a restart: the pause, the attempt clock,
    /// the compaction clock. See [`backtrack_core::state`].
    persisted: Mutex<RuntimeState>,
    /// Where the configuration is written. A field rather than a call to
    /// [`paths::config_file`], for the same reason `state_path` is one: a test
    /// that exercises `SetupRepo` or `SetConfig` would otherwise rewrite the
    /// developer's own configuration, pointing their daemon at a temporary
    /// directory that no longer exists.
    config_path: PathBuf,
    /// Where that bookkeeping is written. A field rather than a call to
    /// [`paths::state_file`] at each use, so tests exercise the real persistence
    /// path without writing into the developer's own data directory.
    state_path: PathBuf,
    /// Nudges the scheduler when something happens that changes its answer.
    /// Absent until the scheduler is running, which is why every use goes
    /// through [`Shared::wake_scheduler`].
    waker: Mutex<Option<Arc<Notify>>>,
    /// Answers questions about the machine — battery, metered connection.
    /// [`UnknownProbe`] until the system bus is reached, which keeps a daemon
    /// in a container backing up rather than gated on services it cannot see.
    probe: Mutex<Arc<dyn SystemProbe>>,
    /// Something wants eventual attention: today, a local disk low enough to
    /// mention. Feeds the health model's `DEGRADED`.
    needs_attention: Mutex<bool>,
    /// The last preflight skip announced, so a machine that sits on battery all
    /// afternoon logs the reason once rather than once a minute.
    last_skip: Mutex<Option<preflight::Skip>>,
    /// Restores that have been worked out but not yet carried out, and the
    /// move logs of ones that have. Held here because a restore is two calls
    /// with a decision in between, and the plan has to survive the wait.
    restores: Arc<crate::restore::Restores>,
    /// Where restores stage their extracted copies, and where the files they
    /// replace are kept. Fields rather than calls to `paths::` so tests keep
    /// their filesystem effects inside a temporary directory.
    staging_dir: PathBuf,
    replaced_dir: PathBuf,
    /// The one catalogue writer in the process. Shared rather than owned by the
    /// daemon module because the backup pipeline writes through it too, and
    /// there must never be a second.
    index: Mutex<Option<Arc<Mutex<IndexWriter>>>>,
    /// Answers "is the destination there?". A field so the flap tests can
    /// supply a sequence of answers instead of unplugging something.
    destination_probe: Mutex<Arc<dyn DestinationProbe>>,
    /// Tripped when something *other than a job* changes what health would
    /// report — today, the destination coming or going. Without it a machine
    /// that lost its backup drive would keep announcing its old state until the
    /// next job happened to finish.
    health_changed: Arc<Notify>,
    /// The engine for the local spool repository, built on first use.
    spool: Mutex<Option<Arc<dyn BackupEngine>>>,
    /// Where the local spool repository lives. A field for the same reason
    /// `state_path` is one: a test that exercises the real spool must not write
    /// a Borg repository into the developer's own data directory.
    spool_dir: PathBuf,
    /// Where read-only filesystem snapshots are kept, when this machine can
    /// take them.
    snapshots_dir: PathBuf,
    /// The detected local-protection mode, worked out on first use. `None`
    /// means "not asked yet", which is also how a configuration change forces
    /// it to be asked again.
    offline_mode: Mutex<Option<crate::offline::Mode>>,
    /// Set when local protection could not run at all, which is what turns
    /// "the drive isn't reachable, and that's fine" into something the user
    /// needs to know about.
    offline_broken: Mutex<bool>,
    /// Set when the spool is at its storage limit. `DEGRADED`, not a failure:
    /// protection is still happening, it is just constrained.
    offline_degraded: Mutex<bool>,
    /// Lets a finished local backup report what it did. Nested because the job
    /// factory only learns the handle once the job actually starts.
    #[allow(clippy::type_complexity)]
    offline_handle: Mutex<Option<Arc<Mutex<Option<pipeline::OfflineHandle>>>>>,
}

impl Shared {
    /// Assemble from the daemon's own pieces, writing to the real data
    /// directory.
    pub fn new(
        config: Config,
        jobs: Arc<JobRegistry>,
        secrets: Arc<dyn SecretStore>,
    ) -> Arc<Shared> {
        Shared::assemble(
            config,
            jobs,
            secrets,
            Layout {
                config_path: paths::config_file(),
                index_path: paths::index_db(),
                cache_dir: paths::cache_dir(),
                state_path: paths::state_file(),
                spool_dir: paths::spool_dir(),
                snapshots_dir: paths::snapshots_dir(),
                staging_dir: paths::staging_dir(),
                replaced_dir: paths::replaced_dir(),
            },
        )
    }

    /// Assemble with every file this daemon writes kept inside `dir`. Test-only,
    /// so a test that exercises the real persistence path cannot scribble on the
    /// developer's own backups.
    #[cfg(test)]
    pub(crate) fn in_dir(
        config: Config,
        jobs: Arc<JobRegistry>,
        secrets: Arc<dyn SecretStore>,
        dir: &std::path::Path,
    ) -> Arc<Shared> {
        Shared::assemble(
            config,
            jobs,
            secrets,
            Layout {
                config_path: dir.join("config.toml"),
                index_path: dir.join("index.db"),
                cache_dir: dir.join("cache"),
                state_path: dir.join("state.toml"),
                spool_dir: dir.join("spool"),
                snapshots_dir: dir.join("snapshots"),
                staging_dir: dir.join("staging"),
                replaced_dir: dir.join("replaced"),
            },
        )
    }

    fn assemble(
        config: Config,
        jobs: Arc<JobRegistry>,
        secrets: Arc<dyn SecretStore>,
        layout: Layout,
    ) -> Arc<Shared> {
        let Layout {
            config_path,
            index_path,
            cache_dir,
            state_path,
            spool_dir,
            snapshots_dir,
            staging_dir,
            replaced_dir,
        } = layout;
        Arc::new(Shared {
            config: Mutex::new(config),
            pause: Mutex::new(PauseState::default()),
            jobs,
            secrets,
            preview: PreviewCache::new(cache_dir),
            index_path,
            config_path,
            engine: Mutex::new(None),
            last_archive: Mutex::new(None),
            last_backup: Mutex::new(None),
            blocking_failure: Mutex::new(false),
            destination_reachable: Mutex::new(true),
            persisted: Mutex::new(RuntimeState::default()),
            state_path,
            waker: Mutex::new(None),
            probe: Mutex::new(Arc::new(UnknownProbe)),
            needs_attention: Mutex::new(false),
            last_skip: Mutex::new(None),
            index: Mutex::new(None),
            restores: Arc::new(crate::restore::Restores::default()),
            staging_dir,
            replaced_dir,
            destination_probe: Mutex::new(Arc::new(RealProbe)),
            health_changed: Arc::new(Notify::new()),
            spool: Mutex::new(None),
            spool_dir,
            snapshots_dir,
            offline_mode: Mutex::new(None),
            offline_broken: Mutex::new(false),
            offline_degraded: Mutex::new(false),
            offline_handle: Mutex::new(None),
        })
    }

    /// Swap the destination probe. Used by the tests to script a sequence of
    /// answers, which is the only way to exercise a flapping link.
    #[cfg(test)]
    pub(crate) fn set_destination_probe(&self, probe: Arc<dyn DestinationProbe>) {
        *self.destination_probe.lock().unwrap() = probe;
    }

    /// Ask the destination whether it is there.
    pub async fn probe_destination(&self) -> Reach {
        let repository = self.config().storage.repository.unwrap_or_default();
        let probe = Arc::clone(&*self.destination_probe.lock().unwrap());
        let engine = self.engine.lock().unwrap().clone();
        probe.probe(&repository, engine).await
    }

    /// What the daemon currently believes about the destination.
    pub fn destination_reachable(&self) -> bool {
        *self.destination_reachable.lock().unwrap()
    }

    /// Check a restore request and work out where it will stage its copy.
    ///
    /// Everything that can be refused before a job exists is refused here, so
    /// a client hears "that path is empty" as an error to its call rather than
    /// as a job that fails a moment later.
    fn restore_preflight(
        &self,
        archive: &str,
        paths: &[String],
        dest: &str,
    ) -> Result<PendingRestore> {
        if paths.is_empty() {
            return Err(DaemonError::InvalidArgument(
                "restore needs at least one path".into(),
            ));
        }
        Ok(PendingRestore {
            engine: self.engine()?,
            archive: ArchiveId(archive.to_string()),
            paths: paths.to_vec(),
            dest: PathBuf::from(dest),
            into_dest: false,
        })
    }

    /// Submit a restore job, giving its plan the id it was admitted under.
    ///
    /// The staging directory is named for the job, and the prepared plan is
    /// filed under the same id — which is why the factory needs to know it.
    fn submit_restore(
        &self,
        pending: PendingRestore,
        start: impl Fn(crate::restore::PreparePlan) -> backtrack_core::engine::JobStream
            + Send
            + Sync
            + 'static,
    ) -> u64 {
        let restores = Arc::clone(&self.restores);
        let staging_root = self.staging_dir.clone();
        let factory: JobFactory = Arc::new(move |job| {
            let plan = crate::restore::PreparePlan {
                engine: Arc::clone(&pending.engine),
                archive: pending.archive.clone(),
                paths: pending.paths.clone(),
                dest: pending.dest.clone(),
                into_dest: pending.into_dest,
                staging: staging_root.join(job.to_string()),
                restores: Arc::clone(&restores),
                job,
            };
            let stream = start(plan);
            Box::pin(async move { Ok(stream) }) as BoxFuture<'_, _>
        });
        self.jobs.submit(JobKind::Restore, factory)
    }

    /// Where restores stage their extracted copies. Exposed so start-up can
    /// clear what a previous daemon left behind.
    pub fn staging_root(&self) -> &std::path::Path {
        &self.staging_dir
    }

    /// Where replaced files are kept. Exposed so the daily expiry pass knows
    /// which directory it is bounding.
    pub fn replaced_root(&self) -> &std::path::Path {
        &self.replaced_dir
    }

    /// A handle that makes the signal fan-out re-evaluate health.
    pub fn health_waker(&self) -> Arc<Notify> {
        Arc::clone(&self.health_changed)
    }

    /// Adopt a change in the destination's reachability.
    ///
    /// Reported by [`crate::reachability`] only after its hysteresis has been
    /// satisfied, so this is a real transition rather than a flap, and is worth
    /// both a log line and a `StatusChanged`.
    pub async fn destination_changed(&self, reachable: bool) {
        *self.destination_reachable.lock().unwrap() = reachable;
        if reachable {
            info!("the backup destination is reachable again");
            self.on_reconnected().await;
        } else {
            info!("the backup destination is no longer reachable; protecting changes locally");
            // Bring the schedule round promptly: the next tick is what starts
            // local protection, and waiting out a full interval would leave a
            // gap at exactly the moment the user stopped being protected.
            self.wake_scheduler();
        }
        self.health_changed.notify_one();
    }

    /// What to do the moment the destination comes back.
    ///
    /// Back up **now**, not at the next scheduled tick. Until that backup lands,
    /// the current state of every file exists in exactly one place — this
    /// machine — and the whole offline design rests on closing that window as
    /// soon as it can be closed. Waiting up to an hour because the cadence says
    /// so would be the wrong answer to the one event the user cannot see.
    ///
    /// It still goes through preflight: reconnecting on a phone tether, on
    /// battery, is a reason to wait, and those gates already know it.
    async fn on_reconnected(&self) {
        match self.start_scheduled_backup().await {
            Ok(Some(job)) => info!(job, "catching up now the destination is back"),
            // A gate declined; the reason is logged there and the schedule will
            // come round again.
            Ok(None) => {}
            Err(e) => warn!("could not start the catch-up backup: {e}"),
        }
        self.wake_scheduler();
    }

    /// Fold a successful backup *to the real destination* into the local
    /// safety net's bookkeeping.
    ///
    /// This is the moment everything changes for the local snapshots: the
    /// current state of every file is now off the machine, so what they still
    /// hold is only the intermediate versions from the offline window. Those
    /// get a clock, and the ones whose clock has run out are removed.
    ///
    /// Deliberately not called for a local backup — that is the whole reason
    /// `JobKind::Offline` exists as its own kind. Marking snapshots discardable
    /// because *another local snapshot* succeeded would throw away the only
    /// copy of the versions they hold.
    async fn after_catch_up(&self) {
        let Ok(index) = self.index() else { return };
        let now = SystemTime::now();
        let at = backtrack_core::state::to_epoch(Some(now)).unwrap_or(0) as i64;

        let marking = Arc::clone(&index);
        let marked =
            tokio::task::spawn_blocking(move || marking.lock().unwrap().mark_expirable(at)).await;
        match marked {
            Ok(Ok(0)) => {}
            Ok(Ok(count)) => info!(
                count,
                "the destination has caught up; local snapshots will expire in 30 days"
            ),
            Ok(Err(e)) => return warn!("could not date the local snapshots: {e}"),
            Err(e) => return warn!("could not date the local snapshots: {e}"),
        }

        if let Err(e) = self.expire_local_snapshots(now).await {
            // Housekeeping. The user is protected either way.
            warn!("expired local snapshots could not be cleared: {e}");
        }
    }

    /// Remove local snapshots whose time is up, from their store and the
    /// catalogue together.
    async fn expire_local_snapshots(&self, now: SystemTime) -> Result<()> {
        let index = self.index()?;
        let reading = Arc::clone(&index);
        let held = tokio::task::spawn_blocking(move || reading.lock().unwrap().local_archives())
            .await
            .map_err(|e| DaemonError::InvalidConfig(e.to_string()))?
            .map_err(DaemonError::from)?;

        let (snapshots, spooled): (Vec<_>, Vec<_>) = held
            .into_iter()
            .filter(|row| crate::offline::expired(row, now))
            .partition(|row| row.repo == backtrack_core::index::Repo::FsSnapshot.as_str());

        if !snapshots.is_empty() {
            let named: Vec<(i64, String)> =
                snapshots.iter().map(|r| (r.seq, r.name.clone())).collect();
            pipeline::expire_snapshots(&index, &self.snapshots_dir, &named).await?;
        }

        if !spooled.is_empty() {
            let engine = self.spool_engine().await?;
            let ids: Vec<ArchiveId> = spooled.iter().map(|r| ArchiveId(r.name.clone())).collect();
            let mut stream = engine.delete_archives(&ids).await?;
            while let Some(event) = stream.next().await {
                if let backtrack_core::engine::JobEvent::Finished(result) = event {
                    result?;
                }
            }
            let seqs: Vec<i64> = spooled.iter().map(|r| r.seq).collect();
            let index = Arc::clone(&index);
            tokio::task::spawn_blocking(move || index.lock().unwrap().remove_archives(&seqs))
                .await
                .map_err(|e| DaemonError::InvalidConfig(e.to_string()))??;
            info!(
                count = spooled.len(),
                "expired local snapshots removed from the spool"
            );
        }
        Ok(())
    }

    /// Adopt a probe that can answer for the machine, once one is available.
    pub fn set_probe(&self, probe: Arc<dyn SystemProbe>) {
        *self.probe.lock().unwrap() = probe;
    }

    /// Reload the bookkeeping this daemon left behind last time it ran.
    ///
    /// A pause is the reason this exists: "pause for four hours" must mean four
    /// hours, not "until something restarts the daemon" — and on a laptop, a
    /// restart is a lid closing.
    pub fn restore_persisted_state(&self) {
        self.adopt_state(RuntimeState::load_from(&self.state_path));
    }

    /// Take on a previously-saved state. Split from the load so the rules can be
    /// tested without a filesystem.
    fn adopt_state(&self, state: RuntimeState) {
        match backtrack_core::state::from_epoch(state.paused_until) {
            // A pause that ran out while the daemon was not running has already
            // expired: honouring it now would extend it by the downtime.
            Some(until) if until > SystemTime::now() => {
                self.pause.lock().unwrap().pause_until(until);
                info!(
                    until = state.paused_until,
                    "restored a pause that outlived the daemon"
                );
            }
            _ => {}
        }
        *self.persisted.lock().unwrap() = state;
    }

    /// Apply a change to the persisted bookkeeping and write it out.
    ///
    /// A write failure is logged, never propagated: failing a `Pause` call
    /// because a disk is full would be a worse answer than a pause that is
    /// honoured now and forgotten after a restart.
    fn update_persisted(&self, change: impl FnOnce(&mut RuntimeState)) {
        let mut state = self.persisted.lock().unwrap();
        change(&mut state);
        if let Err(e) = state.save_to(&self.state_path) {
            warn!("could not persist daemon state: {e}");
        }
    }

    /// Hand the scheduler's waker over, once it is running.
    pub fn set_waker(&self, waker: Arc<Notify>) {
        *self.waker.lock().unwrap() = Some(waker);
    }

    /// Make the scheduler reconsider now rather than at its next tick.
    fn wake_scheduler(&self) {
        if let Some(waker) = self.waker.lock().unwrap().as_ref() {
            waker.notify_one();
        }
    }

    /// The facts the schedule is decided from, assembled fresh.
    pub fn schedule_input(&self) -> ScheduleInput {
        let config = self.config();
        ScheduleInput {
            interval: schedule::interval_for(&config),
            last_attempt: backtrack_core::state::from_epoch(
                self.persisted.lock().unwrap().last_attempt,
            ),
            paused_until: self.pause.lock().unwrap().until(SystemTime::now()),
            configured: config.is_configured(),
            busy: self.busy(),
            // Owned by the scheduler loop, which is the only thing that knows
            // whether it has already served a catch-up delay.
            catch_up_deferred: false,
        }
    }

    /// Whether any job is running or waiting to run.
    fn busy(&self) -> bool {
        self.jobs.list().iter().any(|job| !job.state.is_terminal())
    }

    /// Gather everything preflight decides from.
    async fn preflight_facts(&self) -> Facts {
        let config = self.config();
        let probe = Arc::clone(&*self.probe.lock().unwrap());
        let repository = config.storage.repository.clone().unwrap_or_default();
        let (on_battery, metered) = futures::join!(probe.on_battery(), probe.metered());
        debug!(?on_battery, ?metered, "machine state read for preflight");
        // Read out before the struct literal, not inside it. A lock guard in a
        // struct expression lives until the whole expression finishes, and this
        // one now spans an await — which would make the scheduler's future
        // non-`Send` and, worse, hold a lock across a network probe.
        let paused = self
            .pause
            .lock()
            .unwrap()
            .until(SystemTime::now())
            .is_some();
        let destination_reachable = self.probe_destination().await.definite();
        Facts {
            paused,
            on_battery,
            allow_on_battery: config.backup.on_battery,
            metered,
            allow_on_metered: config.backup.on_metered,
            destination_is_remote: preflight::destination_is_remote(&repository),
            // Probed fresh rather than read from the tracked state. The
            // hysteresis in `reachability` exists to keep the *status* steady;
            // the question here is "can this backup run right now", and
            // answering it from a value that is up to a minute old would start
            // a backup into a drive that has just been unplugged.
            destination_reachable,
            free_local_bytes: preflight::free_bytes(&paths::data_dir()),
        }
    }

    /// Start the backup the schedule asked for.
    ///
    /// Returns the job id, or `None` if a preflight gate declined the run. A
    /// decline is not an error: being on battery is a decision, not a fault, and
    /// the attempt clock deliberately does not advance — the backup happens when
    /// the charger goes in, not at the top of the next hour.
    pub async fn start_scheduled_backup(&self) -> Result<Option<u64>> {
        let facts = self.preflight_facts().await;

        // The reachability probe feeds the status line whether or not it stops
        // this run: "the drive is not plugged in" is worth saying.
        if let Some(reachable) = facts.destination_reachable {
            *self.destination_reachable.lock().unwrap() = reachable;
        }
        *self.needs_attention.lock().unwrap() = preflight::local_disk_needs_attention(&facts);

        match preflight::evaluate(&facts) {
            Verdict::Go => {
                self.announce_skip(None);
                Ok(Some(self.submit_backup().await?))
            }
            // The destination is away. This is not a skipped backup, it is the
            // moment local protection takes over — the promise the wizard makes
            // ("keeps protecting your changes on this computer and catches up
            // when it reconnects"), kept.
            Verdict::Skip(preflight::Skip::Unreachable) => {
                self.announce_skip(Some(preflight::Skip::Unreachable));
                match self.start_offline_protection().await {
                    Ok(job) => {
                        *self.offline_broken.lock().unwrap() = false;
                        Ok(job)
                    }
                    Err(e) => {
                        // The safety net itself is not working, which the user
                        // does need to know about — being away from the drive
                        // was supposed to be covered.
                        warn!("changes cannot be protected on this computer: {e}");
                        *self.offline_broken.lock().unwrap() = true;
                        self.health_changed.notify_one();
                        Ok(None)
                    }
                }
            }
            Verdict::Skip(reason) => {
                self.announce_skip(Some(reason));
                Ok(None)
            }
        }
    }

    /// Log a preflight outcome, but only when it differs from the last one.
    ///
    /// A laptop on battery all afternoon would otherwise write the same line to
    /// the log every minute, burying anything that mattered.
    fn announce_skip(&self, reason: Option<preflight::Skip>) {
        let mut last = self.last_skip.lock().unwrap();
        if *last == reason {
            return;
        }
        match &reason {
            Some(skip) => info!(
                reason = skip.as_str(),
                "scheduled backup skipped: {}",
                skip.explain()
            ),
            None if last.is_some() => {
                info!("the condition that was holding backups up has cleared")
            }
            None => {}
        }
        *last = reason;
    }

    /// Queue a backup and record the attempt.
    ///
    /// The attempt clock advances here rather than on success, so a destination
    /// that has been unplugged for a week produces one attempt per interval
    /// instead of one per tick.
    async fn submit_backup(&self) -> Result<u64> {
        let engine = self.engine()?;
        let index = self.index()?;
        let config = self.config();

        // A name the repository does not already hold. Two backups starting in
        // the same second collide, and borg refuses the second outright — which
        // became reachable the moment reconnecting started an immediate
        // catch-up that can land in the same second as a scheduled tick.
        let reading = Arc::clone(&index);
        let taken: Vec<String> = tokio::task::spawn_blocking(move || {
            reading
                .lock()
                .unwrap()
                .archives_in(backtrack_core::index::Repo::Primary)
                .unwrap_or_default()
        })
        .await
        .map_err(|e| DaemonError::InvalidConfig(e.to_string()))?
        .into_iter()
        .map(|row| row.name)
        .collect();

        let host = pipeline::hostname();
        let (archive_name, created_at) =
            pipeline::next_free_name(SystemTime::now(), &taken, |at| {
                pipeline::archive_name(&host, at)
            });
        let spec = CreateSpec {
            archive_name,
            created_at,
            ..create_spec(&config)
        };
        *self.last_archive.lock().unwrap() = Some(spec.archive_name.clone());
        self.update_persisted(|state| {
            state.last_attempt = backtrack_core::state::to_epoch(Some(SystemTime::now()));
        });
        // Retention is applied as part of the backup rather than on a schedule
        // of its own: a repository is only ever over its policy in the moment
        // after a backup, so that is the only moment worth checking.
        let prune = Some(prune_policy(&config));
        let factory: JobFactory = Arc::new(move |_job| {
            let plan = pipeline::BackupPlan {
                engine: Arc::clone(&engine),
                index: Arc::clone(&index),
                spec: spec.clone(),
                prune: prune.clone(),
            };
            Box::pin(async move { Ok(pipeline::start_backup(plan)) }) as BoxFuture<'_, _>
        });
        Ok(self.jobs.submit(JobKind::Backup, factory))
    }

    /// The engine for the local spool repository, creating the repository on
    /// first use.
    ///
    /// Keyed to the *primary* repository's secret, so the spool is encrypted
    /// with the same passphrase and the user never meets a second one — or has
    /// to know this repository exists at all. A machine whose keyring is locked
    /// therefore cannot spool either, which is correct: that is already a
    /// blocking health failure with its own banner.
    async fn spool_engine(&self) -> Result<Arc<dyn BackupEngine>> {
        if let Some(engine) = self.spool.lock().unwrap().clone() {
            return Ok(engine);
        }
        let repository = self
            .config()
            .storage
            .repository
            .ok_or_else(|| DaemonError::NotConfigured("no destination is configured".into()))?;
        let path = self.spool_dir.clone();
        std::fs::create_dir_all(&path)
            .map_err(|e| DaemonError::LocalDiskFull(format!("cannot create the spool: {e}")))?;

        let engine = BorgCli::new(
            path.display().to_string(),
            repository,
            Arc::clone(&self.secrets),
        )
        .await?;
        // An empty directory is not yet a repository. Decided by looking for
        // Borg's own `config` file rather than by asking Borg and reading the
        // error: an empty directory reports "not a valid repository" (exit 15),
        // which is not the same error as an absent one, and inferring "needs
        // creating" from an error string is exactly the kind of thing that
        // works until a Borg release rewords it.
        if !path.join("config").exists() {
            engine
                .init_repo(&backtrack_core::engine::RepoSpec {
                    path: path.display().to_string(),
                    encryption: Default::default(),
                })
                .await?;
            info!(path = %path.display(), "local safety net prepared");
        }

        let engine: Arc<dyn BackupEngine> = Arc::new(engine);
        *self.spool.lock().unwrap() = Some(Arc::clone(&engine));
        Ok(engine)
    }

    /// Protect what has changed since the last snapshot, on this computer.
    ///
    /// Runs in place of a scheduled backup when the destination does not
    /// answer. Returns the job id, or `None` when there is nothing to do or
    /// local protection is switched off.
    pub async fn start_offline_protection(&self) -> Result<Option<u64>> {
        let config = self.config();
        if !config.storage.offline.enabled {
            return Ok(None);
        }
        if config.backup.include.is_empty() {
            return Ok(None);
        }

        let index = self.index()?;
        let excludes = effective_excludes(&config);

        // Which safety net this machine can actually use. Worked out once and
        // remembered; re-detected when the sources change, since the answer
        // depends on which filesystem they live on.
        let mode = self.offline_mode().await;
        debug!(mode = mode.as_str(), "protecting changes on this computer");
        if let crate::offline::Mode::FsSnapshot(subvolume) = &mode {
            let plan = pipeline::SnapshotPlan {
                index,
                subvolume: subvolume.root.clone(),
                snapshots_dir: self.snapshots_dir.clone(),
                walk: backtrack_core::walk::WalkSpec {
                    sources: config.backup.include.clone(),
                    excludes: backtrack_core::pattern::ExcludeSet::compile(&excludes),
                    one_file_system: true,
                    never: vec![self.spool_dir.clone(), self.snapshots_dir.clone()],
                    include_dirs: true,
                },
                excludes,
                created_at: SystemTime::now(),
            };
            self.update_persisted(|state| {
                state.last_attempt = backtrack_core::state::to_epoch(Some(SystemTime::now()));
            });
            let plan = Arc::new(Mutex::new(Some(plan)));
            let factory: JobFactory = Arc::new(move |_job| {
                let plan = plan.lock().unwrap().take();
                Box::pin(async move {
                    let plan = plan.ok_or(backtrack_core::engine::EngineError::Cancelled)?;
                    Ok(pipeline::start_snapshot_backup(plan))
                }) as BoxFuture<'_, _>
            });
            return Ok(Some(self.jobs.submit(JobKind::Offline, factory)));
        }

        let engine = self.spool_engine().await?;
        let plan = pipeline::OfflinePlan {
            engine,
            index,
            index_path: self.index_path.clone(),
            walk: backtrack_core::walk::WalkSpec {
                sources: config.backup.include.clone(),
                excludes: backtrack_core::pattern::ExcludeSet::compile(&excludes),
                one_file_system: true,
                // The two directories that would feed the walk its own
                // output. Deliberately *not* the whole data directory: a
                // backup source can legitimately live inside it — the
                // development fixture's does — and pruning it wholesale made
                // every local snapshot empty with nothing to say so. The rest
                // of our storage is handled by `effective_excludes`.
                never: vec![self.spool_dir.clone(), self.snapshots_dir.clone()],
                // A delta for borg, so no directory entries — see `WalkSpec`.
                include_dirs: false,
            },
            excludes,
            compression: create_spec(&config).compression,
            cap_bytes: u64::from(config.storage.offline.space_limit_gb) * 1024 * 1024 * 1024,
            spool_dir: self.spool_dir.clone(),
            created_at: SystemTime::now(),
        };

        // The attempt clock advances here as it does for a network backup: this
        // *is* the scheduled run for this hour. Without it the tick would fire
        // again immediately and walk the whole home directory every few seconds.
        self.update_persisted(|state| {
            state.last_attempt = backtrack_core::state::to_epoch(Some(SystemTime::now()));
        });

        let plan = Arc::new(Mutex::new(Some(plan)));
        let handle: Arc<Mutex<Option<pipeline::OfflineHandle>>> = Arc::new(Mutex::new(None));
        let factory_handle = Arc::clone(&handle);
        let factory: JobFactory = Arc::new(move |_job| {
            let plan = plan.lock().unwrap().take();
            let handle = Arc::clone(&factory_handle);
            Box::pin(async move {
                let plan = plan.ok_or(backtrack_core::engine::EngineError::Cancelled)?;
                let (stream, started) = pipeline::start_offline_backup(plan);
                *handle.lock().unwrap() = Some(started);
                Ok(stream)
            }) as BoxFuture<'_, _>
        });
        let job = self.jobs.submit(JobKind::Offline, factory);
        *self.offline_handle.lock().unwrap() = Some(handle);
        Ok(Some(job))
    }

    /// How this machine holds changes while the destination is away.
    ///
    /// Detected once and remembered, because the probe takes a real snapshot;
    /// re-detected when the sources change, since the answer depends on which
    /// filesystem they are on.
    pub async fn offline_mode(&self) -> crate::offline::Mode {
        if let Some(mode) = self.offline_mode.lock().unwrap().clone() {
            return mode;
        }
        let sources = self.config().backup.include;
        let mode = crate::offline::detect(&sources, &self.snapshots_dir).await;
        *self.offline_mode.lock().unwrap() = Some(mode.clone());
        mode
    }

    /// Forget the detected mode, so the next offline tick works it out again.
    fn forget_offline_mode(&self) {
        *self.offline_mode.lock().unwrap() = None;
    }

    /// What the local safety net is currently holding, for `GetStatus`.
    ///
    /// Read from the catalogue rather than from a counter kept in step by hand:
    /// the catalogue is what the timeline shows, so a number derived from
    /// anything else could disagree with what the user is looking at.
    async fn local_protection(&self) -> LocalProtection {
        let config = self.config();
        let mode = if config.storage.offline.enabled {
            // Not `offline_mode()`: that probes, and probing takes a real
            // filesystem snapshot. `GetStatus` is called whenever a window
            // opens. Report what has already been worked out, and let the
            // offline path do the finding out.
            self.offline_mode
                .lock()
                .unwrap()
                .as_ref()
                .map(|mode| mode.as_str())
                .unwrap_or(crate::offline::Mode::Spool.as_str())
        } else {
            "off"
        };

        let Ok(index) = self.index() else {
            return LocalProtection {
                mode: mode.to_string(),
                ..LocalProtection::default()
            };
        };
        let held = tokio::task::spawn_blocking(move || index.lock().unwrap().local_archives())
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default();

        LocalProtection {
            bytes: crate::offline::directory_bytes(&self.spool_dir),
            snapshots: held.len() as u32,
            expirable: held.iter().filter(|r| r.expirable_at.is_some()).count() as u32,
            mode: mode.to_string(),
        }
    }

    /// Whether the local safety net is in a position to protect anything.
    ///
    /// Feeds health.md's `PROTECTED_LOCALLY`, which is the difference between
    /// "your backup drive isn't reachable" reading as reassurance or as an
    /// alarm. True whenever offline protection is switched on and has not
    /// failed — the promise the wizard makes is that changes *are* being kept,
    /// and claiming it only after the first local snapshot would leave the first
    /// hour offline reporting nothing at all.
    fn offline_protection_active(&self) -> bool {
        self.config().storage.offline.enabled && !*self.offline_broken.lock().unwrap()
    }

    /// Fold a finished local backup into the health facts.
    fn record_offline_outcome(&self) {
        let handle = self.offline_handle.lock().unwrap().clone();
        let Some(outcome) = handle.and_then(|h| h.lock().unwrap().clone()) else {
            return;
        };
        let outcome = outcome.outcome();
        *self.offline_degraded.lock().unwrap() = outcome.degraded;
    }

    /// Catalogue anything the repository has that the index does not.
    ///
    /// Submitted as a job rather than run inline, for two reasons: it takes only
    /// a shared repository lock, so a restore or the next backup can proceed
    /// alongside it; and a repository with a year of history takes minutes to
    /// read, which is far too long to hold up a daemon that has not yet told
    /// systemd it is ready.
    ///
    /// Returns `None` when there is no destination to reconcile against.
    pub async fn reconcile_catalogue(&self) -> Option<u64> {
        // A destination that is not there cannot be reconciled against, and
        // submitting a job that fails immediately would write an error to the
        // log on every start — which is how people learn to ignore logs. An
        // unplugged drive is a status, not a fault.
        if self.probe_destination().await == Reach::No {
            debug!("destination not reachable; catalogue reconciliation deferred");
            return None;
        }
        let engine = self.engine().ok()?;
        let index = self.index().ok()?;
        let factory: JobFactory = Arc::new(move |_job| {
            let plan = pipeline::CataloguePlan {
                engine: Arc::clone(&engine),
                index: Arc::clone(&index),
            };
            Box::pin(async move { Ok(pipeline::start_catalogue(plan)) }) as BoxFuture<'_, _>
        });
        Some(self.jobs.submit(JobKind::Index, factory))
    }

    /// Take on a repository's existing history: newest snapshot now, the rest
    /// in the background.
    ///
    /// The two halves are deliberately different kinds of work. The first is
    /// synchronous because the caller is a wizard about to show a timeline, and
    /// one browsable snapshot is the difference between "set up" and "broken".
    /// The second is a job because a year of history is minutes of reading, and
    /// nothing should be waiting on it — including this method's reply.
    pub async fn adopt_catalogue(self: &Arc<Self>) -> Result<()> {
        let plan = pipeline::CataloguePlan {
            engine: self.engine()?,
            index: self.index()?,
        };
        let remaining = pipeline::catalogue_newest(&plan).await?;
        if remaining > 0 {
            info!(
                remaining,
                "cataloguing the rest of the history in the background"
            );
            self.reconcile_catalogue().await;
        }
        Ok(())
    }

    /// How many backups exist but cannot be browsed yet.
    ///
    /// health.md's "snapshot taken but indexing failed" row: `DEGRADED`, badged
    /// "1 backup not yet browsable". Stage 10 surfaces it; this is the number.
    ///
    /// Asynchronous, and the reason is a deadlock this deliberately cannot have.
    /// Taking the catalogue lock directly on a runtime thread pairs badly with
    /// an ingest in flight: the ingest's blocking half holds that lock while
    /// waiting for the next batch of items, and the only thing that can send one
    /// is the async half — which cannot run while the runtime thread is parked
    /// on the lock. Going through `spawn_blocking` puts the wait on the blocking
    /// pool, where nothing else is depending on it.
    pub async fn uncatalogued_count(&self) -> usize {
        let Ok(index) = self.index() else { return 0 };
        tokio::task::spawn_blocking(move || {
            index
                .lock()
                .unwrap()
                .pending_archives()
                .map(|pending| pending.len())
                .unwrap_or(0)
        })
        .await
        .unwrap_or(0)
    }

    /// Run `borg compact` if it is due, on its own daily cadence.
    ///
    /// Separate from the backup because it is a different kind of work: a backup
    /// protects new data, compaction reclaims space that pruned archives were
    /// still occupying. Folding it into every backup would put a repository
    /// rewrite between the user and their hourly protection.
    pub async fn maybe_compact(&self) {
        let last = backtrack_core::state::from_epoch(self.persisted.lock().unwrap().last_compact);
        // First start with a destination: begin the clock rather than compacting
        // a repository that may not have been backed up to yet.
        if last.is_none() {
            self.update_persisted(|state| {
                state.last_compact = backtrack_core::state::to_epoch(Some(SystemTime::now()));
            });
            return;
        }
        if !schedule::compact_due(last, SystemTime::now(), self.busy()) {
            return;
        }
        let Ok(engine) = self.engine() else { return };
        self.update_persisted(|state| {
            state.last_compact = backtrack_core::state::to_epoch(Some(SystemTime::now()));
        });
        let factory: JobFactory = Arc::new(move |_job| {
            let engine = Arc::clone(&engine);
            Box::pin(async move { engine.compact().await }) as BoxFuture<'_, _>
        });
        let job = self.jobs.submit(JobKind::Compact, factory);
        info!(job, "reclaiming repository space");
    }

    /// The catalogue writer, which only exists once a data directory does.
    fn index(&self) -> Result<Arc<Mutex<IndexWriter>>> {
        self.index
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| DaemonError::NotConfigured("the catalogue is not open".into()))
    }

    /// Hand over the writer the daemon opened at startup. There is exactly one
    /// in the process — the single-writer rule the whole architecture rests on.
    pub fn set_index(&self, index: Arc<Mutex<IndexWriter>>) {
        *self.index.lock().unwrap() = Some(index);
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
        config.save_to(&self.config_path)?;
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
            offline_protection_active: self.offline_protection_active(),
            last_success: *self.last_backup.lock().unwrap(),
            frequency: config.backup.frequency.interval(),
            needs_attention: *self.needs_attention.lock().unwrap()
                || *self.offline_degraded.lock().unwrap(),
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

/// What the local safety net is holding, as `GetStatus` reports it.
#[derive(Debug, Default)]
struct LocalProtection {
    bytes: u64,
    snapshots: u32,
    expirable: u32,
    mode: String,
}

/// Every file and directory the daemon writes.
///
/// Grouped rather than passed one by one so a test can redirect the whole lot
/// into a temporary directory in one move — a suite that scribbled a Borg
/// repository into the developer's own data directory would be a nasty
/// surprise.
struct Layout {
    config_path: PathBuf,
    index_path: PathBuf,
    cache_dir: PathBuf,
    state_path: PathBuf,
    spool_dir: PathBuf,
    snapshots_dir: PathBuf,
    staging_dir: PathBuf,
    replaced_dir: PathBuf,
}

/// A restore request that has passed its checks but has no job yet.
struct PendingRestore {
    engine: Arc<dyn BackupEngine>,
    archive: ArchiveId,
    paths: Vec<String>,
    dest: PathBuf,
    /// Whether what was asked for lands inside `dest` rather than at the
    /// absolute path it came from. See [`crate::restore::PreparePlan`].
    into_dest: bool,
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
    ///
    /// Deliberately bypasses the pause rather than refusing: someone who paused
    /// backups this morning and is now pressing "Back Up Now" has said what they
    /// want, and it would be perverse to answer "no, you paused it". The pause
    /// itself is untouched, so the schedule stays paused afterwards — the bypass
    /// is for this one run.
    async fn backup_now(&self) -> Result<u64> {
        let paused = self
            .shared
            .pause
            .lock()
            .unwrap()
            .until(SystemTime::now())
            .is_some();
        if paused {
            info!("manual backup requested while paused; running it anyway");
        }
        self.shared.submit_backup().await
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
        self.shared
            .update_persisted(|s| s.paused_until = backtrack_core::state::to_epoch(Some(until)));
        self.shared.wake_scheduler();
        info!(until = to_epoch(Some(until)), "backups paused");
        Ok(())
    }

    /// Resume scheduled backups.
    async fn resume(&self) -> Result<()> {
        self.shared.pause.lock().unwrap().resume();
        self.shared.update_persisted(|s| s.paused_until = None);
        self.shared.wake_scheduler();
        info!("backups resumed");
        Ok(())
    }

    /// The overall state, and the handful of facts the UI shows beside it.
    async fn get_status(&self) -> Result<Status> {
        let now = SystemTime::now();
        let config = self.shared.config();
        let last_backup = *self.shared.last_backup.lock().unwrap();
        let paused_until = self.shared.pause.lock().unwrap().until(now);
        // Reported from the same facts the scheduler decides on — a next-backup
        // time the schedule would not honour is worse than none at all.
        let schedule = self.shared.schedule_input();
        let next = if paused_until.is_some() || !schedule.configured {
            None
        } else {
            next_due(schedule.interval, schedule.last_attempt, now)
        };
        let local = self.shared.local_protection().await;
        Ok(Status {
            state: self.shared.health().as_str().to_string(),
            last_backup: to_epoch(last_backup),
            next_backup: to_epoch(next),
            destination_reachable: self.shared.destination_reachable(),
            spool_bytes: local.bytes,
            offline_mode: local.mode,
            local_snapshots: local.snapshots,
            expirable_snapshots: local.expirable,
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
    /// Restore `paths` from `archive` into `dest` with no one to ask.
    ///
    /// The same pipeline the interactive restore uses — staging, comparison,
    /// atomic moves, the safety stash — with `policy` standing in for the
    /// answers a person would give. `ask` means there is nobody to ask, so
    /// conflicts are left alone and reported rather than guessed at: a
    /// command-line restore must not overwrite work because it could not put
    /// the question.
    async fn restore_files(
        &self,
        archive: &str,
        paths: Vec<String>,
        dest: &str,
        policy: &str,
    ) -> Result<u64> {
        let policy = RestorePolicy::parse(policy)?;
        let prepared = self.shared.restore_preflight(archive, &paths, dest)?;
        info!(archive, dest, policy = policy.as_str(), "restore requested");

        let decisions = Decisions::all(match policy {
            RestorePolicy::Replace => Decision::Replace,
            RestorePolicy::KeepBoth => Decision::KeepBoth,
            RestorePolicy::Ask | RestorePolicy::SkipIdentical => Decision::Skip,
        });
        let stash = self.shared.replaced_dir.clone();
        Ok(self.shared.submit_restore(prepared, move |plan| {
            crate::restore::start_direct(plan, decisions.clone(), stash.clone())
        }))
    }

    /// Work out what restoring `paths` from `archive` into `dest` would do,
    /// without doing any of it.
    ///
    /// Returns the job that is working it out. When that job finishes, the
    /// answer is read with `GetRestorePreview` using the same id, applied with
    /// `ExecuteRestore`, and thrown away with `DiscardRestore`.
    async fn prepare_restore(&self, archive: &str, paths: Vec<String>, dest: &str) -> Result<u64> {
        let prepared = self.shared.restore_preflight(archive, &paths, dest)?;
        info!(archive, dest, "working out a restore");
        Ok(self
            .shared
            .submit_restore(prepared, crate::restore::start_prepare))
    }

    /// What the restore prepared under `job` would do.
    async fn get_restore_preview(&self, job: u64) -> Result<RestorePreview> {
        let plan = self.shared.restores.plan(job).ok_or_else(|| {
            DaemonError::NoSuchJob(format!("no restore is prepared under job {job}"))
        })?;
        Ok(crate::restore::preview(&plan))
    }

    /// Carry out the restore prepared under `job`.
    ///
    /// `blanket` answers every conflict the summary screen covered;
    /// `decisions` overrides individual paths, which is what unticking a row in
    /// the review list does. A change of type is never covered by the blanket
    /// answer and must be named here to happen at all.
    async fn execute_restore(
        &self,
        job: u64,
        blanket: &str,
        decisions: Vec<(String, String)>,
    ) -> Result<u64> {
        if !self.shared.restores.holds(job) {
            return Err(DaemonError::NoSuchJob(format!(
                "no restore is prepared under job {job}"
            )));
        }
        let blanket = Decision::parse(blanket).ok_or_else(|| {
            DaemonError::InvalidArgument(format!(
                "unknown decision {blanket:?}; expected one of replace, keep-both, skip"
            ))
        })?;
        let mut chosen = Decisions::all(blanket);
        for (path, decision) in decisions {
            let decision = Decision::parse(&decision).ok_or_else(|| {
                DaemonError::InvalidArgument(format!("unknown decision {decision:?} for {path:?}"))
            })?;
            chosen = chosen.except(PathBuf::from(path), decision);
        }

        let plan = crate::restore::ExecutePlan {
            restores: Arc::clone(&self.shared.restores),
            prepared: job,
            decisions: chosen,
            stash: self.shared.replaced_dir.clone(),
        };
        let plan = Arc::new(Mutex::new(Some(plan)));
        let factory: JobFactory = Arc::new(move |_job| {
            let plan = Arc::clone(&plan);
            Box::pin(async move {
                let plan = plan.lock().unwrap().take().ok_or_else(|| {
                    backtrack_core::engine::EngineError::Local(
                        "that restore has already been carried out".to_string(),
                    )
                })?;
                Ok(crate::restore::start_execute(plan))
            }) as BoxFuture<'_, _>
        });
        Ok(self.shared.jobs.submit(JobKind::Restore, factory))
    }

    /// Restore `paths` into `dest`, which must be a directory made for this.
    ///
    /// The whole point is that there is nothing to ask. `dest` is a folder that
    /// did not exist a moment ago, so nothing in it can clash with anything
    /// coming out of the backup — no conflicts, no summary, no dialog, and
    /// nothing on the machine is overwritten. It is the answer to "I want to
    /// look at the old version before I decide", which in-place restoring
    /// cannot be.
    ///
    /// One path at a time. The contents of what was asked for land directly in
    /// `dest`, and two folders' contents merged into one directory would be a
    /// different operation wearing this one's name.
    async fn restore_into(&self, archive: &str, paths: Vec<String>, dest: &str) -> Result<u64> {
        if paths.len() != 1 {
            return Err(DaemonError::InvalidArgument(format!(
                "RestoreInto takes exactly one path; got {}",
                paths.len()
            )));
        }
        let mut prepared = self.shared.restore_preflight(archive, &paths, dest)?;
        prepared.into_dest = true;

        // Made here rather than left to the moves, so a destination that cannot
        // be created fails now — with nothing extracted and nothing to undo —
        // rather than per-file halfway through.
        std::fs::create_dir_all(dest)
            .map_err(|e| DaemonError::RestoreFailed(format!("{dest} could not be created: {e}")))?;
        info!(archive, dest, "restoring into a folder of its own");

        // Nothing on disk to clash with, so every answer is the same answer.
        let decisions = Decisions::all(Decision::Replace);
        let stash = self.shared.replaced_dir.clone();
        Ok(self.shared.submit_restore(prepared, move |plan| {
            crate::restore::start_direct(plan, decisions.clone(), stash.clone())
        }))
    }

    /// The files the safety stash is keeping, newest restore first.
    ///
    /// `limit` bounds the answer: a restore of a large folder replaces as many
    /// files as it touches, and the window that shows this wants the recent
    /// ones rather than all of them marshalled across a bus.
    async fn list_replaced(&self, limit: u32) -> Result<Vec<ReplacedFile>> {
        let root = self.shared.replaced_dir.clone();
        let found = tokio::task::spawn_blocking(move || restore::list_stash(&root, limit as usize))
            .await
            .map_err(|e| DaemonError::RestoreFailed(e.to_string()))?;
        Ok(found
            .into_iter()
            .map(|entry| ReplacedFile {
                original: entry.original.to_string_lossy().to_string(),
                stashed: entry.stashed.to_string_lossy().to_string(),
                size: entry.size,
                replaced_at: entry.replaced_at,
                mtime: entry.mtime,
            })
            .collect())
    }

    /// Put one replaced file back where it came from.
    ///
    /// Whatever is standing in its place is stashed in turn rather than thrown
    /// away: putting a file back is a restore like any other, and the promise
    /// that the thing being overwritten survives the overwriting does not stop
    /// applying because the user is going the other way.
    ///
    /// Returns where the displaced file went, or an empty string if there was
    /// nothing in the way.
    async fn put_back_replaced(&self, stashed: &str) -> Result<String> {
        let root = self.shared.replaced_dir.clone();
        let wanted = PathBuf::from(stashed);
        // Named by its stashed path, which the client got from `ListReplaced`
        // — and looked up rather than trusted, because what follows is a
        // `rename` onto a path derived from it.
        let entry = tokio::task::spawn_blocking({
            let root = root.clone();
            move || restore::find_stashed(&root, &wanted)
        })
        .await
        .map_err(|e| DaemonError::RestoreFailed(e.to_string()))?
        .ok_or_else(|| DaemonError::NotFound(format!("the stash is not keeping {stashed:?}")))?;

        let original = entry.original.clone();
        let displaced = tokio::task::spawn_blocking(move || {
            restore::put_back(&entry, &root, SystemTime::now())
        })
        .await
        .map_err(|e| DaemonError::RestoreFailed(e.to_string()))?
        .map_err(|e| DaemonError::RestoreFailed(e.to_string()))?;
        info!(path = %original.display(), "a replaced file was put back");
        Ok(displaced
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default())
    }

    /// Put back everything the restore under `job` moved.
    ///
    /// This is what the Undo on the toast does, and it keeps working long after
    /// the toast has gone — the files it needs are in the stash, not in memory.
    async fn undo_restore(&self, job: u64) -> Result<u64> {
        if self.shared.restores.log(job).is_none() {
            return Err(DaemonError::NoSuchJob(format!(
                "job {job} has nothing recorded to undo"
            )));
        }
        let restores = Arc::clone(&self.shared.restores);
        let factory: JobFactory = Arc::new(move |_job| {
            let plan = crate::restore::UndoPlan {
                restores: Arc::clone(&restores),
                prepared: job,
            };
            Box::pin(async move { Ok(crate::restore::start_undo(plan)) }) as BoxFuture<'_, _>
        });
        Ok(self.shared.jobs.submit(JobKind::Restore, factory))
    }

    /// Throw away a prepared restore and the copy it extracted.
    ///
    /// Cancelling costs nothing because nothing was touched, but the extracted
    /// copy is real and has to go.
    async fn discard_restore(&self, job: u64) -> Result<()> {
        self.shared.restores.discard(job);
        Ok(())
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
        let factory: JobFactory = Arc::new(move |_job| {
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
        let factory: JobFactory = Arc::new(move |_job| {
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
        let factory: JobFactory = Arc::new(move |_job| {
            let engine = Arc::clone(&engine);
            let policy = policy.clone();
            Box::pin(async move { engine.prune(&policy).await }) as BoxFuture<'_, _>
        });
        Ok(self.shared.jobs.submit(JobKind::Prune, factory))
    }

    /// Check the repository's integrity.
    async fn verify(&self) -> Result<u64> {
        let engine = self.shared.engine()?;
        let factory: JobFactory = Arc::new(move |_job| {
            let engine = Arc::clone(&engine);
            Box::pin(async move { engine.check(CheckLevel::Full).await }) as BoxFuture<'_, _>
        });
        Ok(self.shared.jobs.submit(JobKind::Check, factory))
    }

    /// Reclaim space the repository is no longer using.
    async fn compact(&self) -> Result<u64> {
        let engine = self.shared.engine()?;
        let factory: JobFactory = Arc::new(move |_job| {
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
        let sources_changed = updated.backup.include != self.shared.config().backup.include;
        self.shared.store_config(updated)?;
        info!(key, "configuration changed");
        if sources_changed {
            // Which local safety net is usable depends on the filesystem the
            // sources are on, so a new source list is a new question.
            self.shared.forget_offline_mode();
        }
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
    ///
    /// Returns once the *newest* snapshot is browsable, with the rest of the
    /// history filling in behind. A repository holding a year of backups takes
    /// minutes to catalogue in full, and a wizard that says "you're set up" over
    /// an empty timeline reads as a failure — so this call waits for exactly the
    /// one snapshot the user is about to be shown, and no longer.
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

        self.shared.adopt_catalogue().await
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

    /// A job ended, however it ended.
    ///
    /// The one thing `StatusChanged` cannot tell a client. Health only moves
    /// when the *state* changes, so a successful backup on a machine that was
    /// already healthy announces nothing at all — leaving a client that started
    /// that backup with no way to learn it had finished except to poll. Every
    /// kind of job reports here, including the ones with no progress signal of
    /// their own.
    ///
    /// `outcome` is `completed`, `cancelled` or `failed`.
    #[zbus(signal)]
    pub async fn job_finished(
        emitter: &SignalEmitter<'_>,
        job: u64,
        kind: &str,
        outcome: &str,
    ) -> zbus::Result<()>;
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
    let health_changed = shared.health_waker();
    let mut announced = shared.health();
    let _ = Daemon1::status_changed(&emitter, announced.as_str()).await;

    loop {
        // Two sources, one destination. Jobs are the usual reason health moves,
        // but not the only one: the backup destination coming or going changes
        // the answer with no job involved at all, and a client that only ever
        // hears about jobs would sit on a stale banner until the next backup.
        let update = tokio::select! {
            update = updates.recv() => match update {
                Ok(update) => Some(update),
                // Lagged: intermediate progress was dropped, which is exactly
                // what the buffer is allowed to do. Carry on from the current
                // truth.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    debug!(missed = n, "signal fan-out lagged");
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            },
            _ = health_changed.notified() => None,
        };

        let Some(update) = update else {
            let current = shared.health();
            if current != announced {
                announced = current;
                info!(state = current.as_str(), "health state changed");
                let _ = Daemon1::status_changed(&emitter, current.as_str()).await;
            }
            continue;
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
                    // Local protection is a backup as far as anyone watching is
                    // concerned, and reports itself as one.
                    JobKind::Backup | JobKind::Offline => {
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
            JobUpdate::State { id, kind, state } => {
                if let Some(outcome) = state.outcome_token() {
                    let _ = Daemon1::job_finished(&emitter, id, kind.as_str(), outcome).await;
                }
                if state.is_terminal() {
                    update_health(&shared, kind, &state);
                    // A backup that reached the real destination is what starts
                    // the local safety net's clock. Awaited here rather than
                    // spawned so the marking cannot race the next backup.
                    if kind == JobKind::Backup && state == JobState::Done(Outcome::Completed) {
                        shared.after_catch_up().await;
                    }
                    // The repository is free again, so a backup the scheduler
                    // declined to queue while busy can be reconsidered now
                    // rather than at the next tick.
                    shared.wake_scheduler();
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
        JobState::Done(Outcome::Completed)
            if matches!(kind, JobKind::Backup | JobKind::Offline) =>
        {
            // A local snapshot counts, and health.md says so: the last success
            // is "the last backup that succeeded anywhere — network, spool, or
            // snapshot". A laptop that has been away for a week and protecting
            // itself hourly is not at risk, and must not be told it is.
            shared.record_backup_success();
            if kind == JobKind::Offline {
                shared.record_offline_outcome();
            }
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

/// Everything the user asked to exclude, plus the ones they should not have to
/// think of.
///
/// A source of `/home/k` contains Backtrack's own data directory, and parts of
/// it must never be archived: the offline spool would copy the backup into the
/// backup hourly, the snapshots directory the same, and `index.db` is a live
/// SQLite database that would be captured mid-write.
///
/// Deliberately *not* the whole data directory. `config.toml` and `state.toml`
/// are small, they change rarely, and they are exactly what somebody restoring
/// a machine would be glad to find — excluding them to save a few kilobytes
/// would be throwing away the user's settings for nothing.
fn effective_excludes(config: &Config) -> Vec<String> {
    let mut excludes = config.backup.exclude.clone();
    for dir in [
        paths::spool_dir(),
        paths::snapshots_dir(),
        paths::cache_dir(),
        paths::staging_dir(),
        paths::replaced_dir(),
        paths::log_dir(),
    ] {
        excludes.push(format!("pp:{}", dir.display()));
    }
    // The catalogue and its write-ahead log. A glob rather than a prefix,
    // because `index.db-wal` and `index.db-shm` are siblings of `index.db`
    // rather than children of it.
    excludes.push(format!("{}*", paths::index_db().display()));
    excludes
}

fn create_spec(config: &Config) -> CreateSpec {
    let now = SystemTime::now();
    CreateSpec {
        archive_name: pipeline::archive_name(&pipeline::hostname(), now),
        created_at: now,
        sources: config.backup.include.clone(),
        excludes: effective_excludes(config),
        paths: Vec::new(),
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
    use crate::jobs::JobRegistry;
    use backtrack_testkit::{MockEngine, MockSecretStore};

    /// A configured daemon with a backup that runs until it is cancelled, and
    /// every file it writes confined to `dir`. The repository directory exists,
    /// so preflight's reachability probe passes.
    fn configured(dir: &std::path::Path) -> Arc<Shared> {
        std::fs::create_dir_all(dir.join("repo")).unwrap();
        let mut config = Config::default();
        config.storage.repository = Some(dir.join("repo").display().to_string());
        config.backup.include = vec![dir.join("src")];
        let shared = Shared::in_dir(
            config,
            JobRegistry::new(),
            Arc::new(MockSecretStore::default()),
            dir,
        );
        shared.set_engine(Arc::new(MockEngine::default().with_create_pending()));
        shared.set_index(Arc::new(Mutex::new(
            IndexWriter::open(&dir.join("index.db")).unwrap(),
        )));
        shared
    }

    #[tokio::test]
    async fn a_pause_survives_a_restart() {
        // The acceptance criterion: "pause state persists across daemon
        // restarts". A pause set for the afternoon must not be undone by a lid
        // closing, which on a laptop is the most likely thing to happen next.
        let dir = tempfile::tempdir().unwrap();
        let until = SystemTime::now() + Duration::from_secs(3_600);

        let first = configured(dir.path());
        first.pause.lock().unwrap().pause_until(until);
        first.update_persisted(|s| s.paused_until = backtrack_core::state::to_epoch(Some(until)));

        // A second daemon over the same directory: a restart, as far as the
        // state on disk is concerned.
        let second = configured(dir.path());
        assert_eq!(second.schedule_input().paused_until, None, "not yet loaded");
        second.restore_persisted_state();
        assert!(
            second.schedule_input().paused_until.is_some(),
            "the pause must come back with the daemon"
        );
    }

    #[test]
    fn a_pause_that_expired_while_the_daemon_was_down_stays_expired() {
        // Otherwise every restart would silently extend the pause by however
        // long the machine was off.
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        let expired = SystemTime::now() - Duration::from_secs(60);
        shared.adopt_state(RuntimeState {
            paused_until: backtrack_core::state::to_epoch(Some(expired)),
            ..Default::default()
        });
        assert_eq!(shared.schedule_input().paused_until, None);
    }

    #[tokio::test]
    async fn a_scheduled_backup_advances_the_attempt_clock_and_persists_it() {
        // The clock has to be on disk, or a daemon restarted every few minutes
        // (by a crash loop, or a user logging in and out) would back up on every
        // start and never on schedule.
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        assert_eq!(shared.schedule_input().last_attempt, None);

        let job = shared
            .start_scheduled_backup()
            .await
            .expect("a configured daemon can start a backup");
        assert!(job.is_some());
        assert!(shared.schedule_input().last_attempt.is_some());

        let reloaded = RuntimeState::load_from(&shared.state_path);
        assert!(
            reloaded.last_attempt.is_some(),
            "the attempt clock must reach the disk, not just memory"
        );
        shared.jobs.cancel(job.unwrap()).unwrap();
    }

    /// A probe with fixed answers, standing in for UPower and NetworkManager.
    struct FixedProbe {
        on_battery: Option<bool>,
        metered: Option<bool>,
    }

    #[async_trait::async_trait]
    impl SystemProbe for FixedProbe {
        async fn on_battery(&self) -> Option<bool> {
            self.on_battery
        }
        async fn metered(&self) -> Option<bool> {
            self.metered
        }
    }

    #[tokio::test]
    async fn a_scheduled_backup_is_skipped_on_battery_without_burning_the_attempt() {
        // The whole point of the gate: skipping must not slide the schedule, or
        // a laptop unplugged for five minutes would lose an hour of protection.
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        shared.set_probe(Arc::new(FixedProbe {
            on_battery: Some(true),
            metered: Some(false),
        }));

        let job = shared.start_scheduled_backup().await.expect("not an error");
        assert_eq!(job, None, "a backup on battery is skipped, not attempted");
        assert_eq!(
            shared.schedule_input().last_attempt,
            None,
            "a skip must not advance the attempt clock"
        );
    }

    #[tokio::test]
    async fn plugging_in_lets_the_next_tick_through() {
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        shared.set_probe(Arc::new(FixedProbe {
            on_battery: Some(true),
            metered: None,
        }));
        assert_eq!(shared.start_scheduled_backup().await.unwrap(), None);

        shared.set_probe(Arc::new(FixedProbe {
            on_battery: Some(false),
            metered: None,
        }));
        let job = shared
            .start_scheduled_backup()
            .await
            .unwrap()
            .expect("on mains, the backup runs");
        shared.jobs.cancel(job).unwrap();
    }

    /// A probe that answers from a script, so a flapping link is a list rather
    /// than a router someone has to unplug. The last answer repeats once the
    /// script runs out.
    struct ScriptedProbe {
        answers: Mutex<std::collections::VecDeque<Reach>>,
        last: Mutex<Reach>,
    }

    impl ScriptedProbe {
        fn new(answers: impl IntoIterator<Item = Reach>) -> Arc<ScriptedProbe> {
            Arc::new(ScriptedProbe {
                answers: Mutex::new(answers.into_iter().collect()),
                last: Mutex::new(Reach::Unknown),
            })
        }
    }

    #[async_trait::async_trait]
    impl DestinationProbe for ScriptedProbe {
        async fn probe(&self, _repository: &str, _engine: Option<Arc<dyn BackupEngine>>) -> Reach {
            let next = self.answers.lock().unwrap().pop_front();
            match next {
                Some(reach) => {
                    *self.last.lock().unwrap() = reach;
                    reach
                }
                None => *self.last.lock().unwrap(),
            }
        }
    }

    #[tokio::test]
    async fn the_daemon_asks_the_probe_rather_than_deciding_for_itself() {
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        shared.set_destination_probe(ScriptedProbe::new([Reach::No, Reach::Yes]));

        assert_eq!(shared.probe_destination().await, Reach::No);
        assert_eq!(shared.probe_destination().await, Reach::Yes);
    }

    #[tokio::test]
    async fn a_probe_that_cannot_tell_does_not_stop_a_backup() {
        // The gate reads `== Some(false)`, so an unknown has to arrive as
        // `None` rather than being flattened into "unreachable" on the way.
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        shared.set_destination_probe(ScriptedProbe::new([Reach::Unknown]));

        let job = shared
            .start_scheduled_backup()
            .await
            .unwrap()
            .expect("a destination nobody could ask about must not block the backup");
        shared.jobs.cancel(job).unwrap();
    }

    #[tokio::test]
    async fn losing_the_destination_is_reported_and_announced() {
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        assert!(shared.destination_reachable());

        // Somebody has to be listening, or `notify_one` stores a permit and the
        // assertion below cannot tell the difference.
        let health = shared.health_waker();
        let listener = tokio::spawn(async move { health.notified().await });
        tokio::task::yield_now().await;

        shared.destination_changed(false).await;

        assert!(!shared.destination_reachable());
        assert!(
            tokio::time::timeout(Duration::from_secs(1), listener)
                .await
                .is_ok(),
            "the signal fan-out has to hear about it, or the banner goes stale"
        );
    }

    #[tokio::test]
    async fn an_absent_destination_is_recorded_and_skipped() {
        // No repository directory: an unplugged drive, or a share that is not
        // mounted. Stage 5 turns this into local protection; today it is
        // reported honestly and the run is deferred.
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        std::fs::remove_dir_all(dir.path().join("repo")).unwrap();
        assert_eq!(shared.start_scheduled_backup().await.unwrap(), None);
        assert!(
            !*shared.destination_reachable.lock().unwrap(),
            "the status line has to know the drive is not there"
        );
    }

    #[tokio::test]
    async fn a_manual_backup_ignores_every_preflight_gate() {
        // Preflight guards the *schedule*. Someone pressing the button on
        // battery, on a phone tether, has already decided.
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        shared.set_probe(Arc::new(FixedProbe {
            on_battery: Some(true),
            metered: Some(true),
        }));

        let daemon = Daemon1::new(Arc::clone(&shared));
        let job = daemon.backup_now().await.expect("runs regardless");
        shared.jobs.cancel(job).unwrap();
    }

    #[tokio::test]
    async fn back_up_now_overrides_a_pause_without_lifting_it() {
        // Someone who paused this morning and is now pressing the button has
        // said what they want. Refusing would be pedantic; silently cancelling
        // their pause would be worse.
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        let until = SystemTime::now() + Duration::from_secs(3_600);
        shared.pause.lock().unwrap().pause_until(until);

        let daemon = Daemon1::new(Arc::clone(&shared));
        let job = daemon.backup_now().await.expect("runs despite the pause");

        assert!(
            shared.schedule_input().paused_until.is_some(),
            "the pause is bypassed for this run, not cancelled"
        );
        shared.jobs.cancel(job).unwrap();
    }

    #[tokio::test]
    async fn the_schedule_reports_a_running_job_as_busy() {
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        assert!(!shared.schedule_input().busy);

        let job = shared.submit_backup().await.expect("submits");
        assert!(
            shared.schedule_input().busy,
            "a second backup must not be queued behind the first"
        );
        shared.jobs.cancel(job).unwrap();
    }

    #[test]
    fn an_unconfigured_machine_reports_no_schedule() {
        let dir = tempfile::tempdir().unwrap();
        let shared = Shared::in_dir(
            Config::default(),
            JobRegistry::new(),
            Arc::new(MockSecretStore::default()),
            dir.path(),
        );
        let input = shared.schedule_input();
        assert!(!input.configured);
        assert_eq!(
            input.interval,
            Some(Duration::from_secs(3_600)),
            "the frequency is still hourly; it is the destination that is missing"
        );
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
        for exclusion in &config.backup.exclude {
            assert!(
                spec.excludes.contains(exclusion),
                "the user's exclusion {exclusion} reached the engine"
            );
        }
        assert!(
            spec.paths.is_empty(),
            "an ordinary backup lets borg walk the sources"
        );
        assert!(
            spec.one_file_system,
            "crossing filesystems would drag in mounted media"
        );
    }

    #[test]
    fn the_volatile_parts_of_the_data_directory_are_never_backed_up() {
        // A source of `/home/k` contains the data directory. The spool would
        // copy the backup into the backup hourly, and `index.db` is a live
        // SQLite database that would be captured mid-write.
        let excludes = effective_excludes(&Config::default());
        let compiled = backtrack_core::pattern::ExcludeSet::compile(&excludes);
        let rel = |p: std::path::PathBuf| backtrack_core::walk::archive_path(&p);

        for must_go in [
            rel(paths::spool_dir().join("data/0/1")),
            rel(paths::snapshots_dir().join("bt-local-1/home/k/f")),
            rel(paths::cache_dir().join("something")),
            rel(paths::log_dir().join("backtrack.jsonl")),
            rel(paths::index_db()),
            format!("{}-wal", rel(paths::index_db())),
            format!("{}-shm", rel(paths::index_db())),
        ] {
            assert!(compiled.excludes(&must_go), "{must_go} should be excluded");
        }
    }

    #[test]
    fn the_users_settings_are_still_backed_up() {
        // Excluding the whole data directory to be safe would throw away
        // `config.toml` and `state.toml` — small, rarely changed, and exactly
        // what somebody restoring a machine would be glad to find.
        let excludes = effective_excludes(&Config::default());
        let compiled = backtrack_core::pattern::ExcludeSet::compile(&excludes);
        for keep in [paths::config_file(), paths::state_file()] {
            let path = backtrack_core::walk::archive_path(&keep);
            assert!(!compiled.excludes(&path), "{path} should be kept");
        }
    }

    #[test]
    fn a_source_inside_the_data_directory_is_still_backed_up() {
        // Found by running the daemon: excluding the whole data directory made
        // every archive on the development machine empty, because its demo
        // source tree lives inside it. Nothing said so — the backups "worked",
        // they just contained nothing.
        let excludes = effective_excludes(&Config::default());
        let compiled = backtrack_core::pattern::ExcludeSet::compile(&excludes);
        let source = backtrack_core::walk::archive_path(
            &paths::data_dir().join("demo-src/home/user/notes.txt"),
        );
        assert!(
            !compiled.excludes(&source),
            "{source} is a backup source, not our own storage"
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
