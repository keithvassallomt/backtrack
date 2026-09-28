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

mod dev;
mod e2e;
mod error;
mod escalation;
mod health;
#[cfg(test)]
mod introspect;
mod on_disk;
mod preview;
mod state;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use backtrack_core::config::Config;
use backtrack_core::dbus::Reason;
use backtrack_core::engine::{
    ArchiveId, BackupEngine, BorgCli, CheckLevel, CreateSpec, HealthFailure, PrunePolicy,
};
use backtrack_core::index::{IndexReader, IndexWriter, Kind};
use backtrack_core::paths;
use backtrack_core::restore::{self, Decision, Decisions};
use backtrack_core::secret::{SecretStore, SessionSecretStore};
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

pub use backtrack_core::dbus::{
    HealthReport, LocalStorage, ReplacedFile, RestorePreview, SearchResult, Status, StorageInfo,
};
pub use dev::Dev;
pub use error::{DaemonError, Result};
pub use escalation::Alert;
pub use health::{Health, HealthInputs, HealthState};
pub use preview::PreviewCache;
pub use state::{PauseState, RestorePolicy};

use state::{config_document, config_get, config_set, next_due, to_epoch};

/// How many backups in a row may fail for reasons nobody classified before
/// the repository is checked, rather than waiting for the month to come round.
/// health.md: repository damage is detected by the monthly check "and after
/// repeated failures", which is how it usually first shows itself.
const UNEXPLAINED_BEFORE_CHECK: u32 = 3;

/// How many of the newest archives the routine check reads the metadata of.
const CHECK_SAMPLE: u32 = 3;

/// The most search results the daemon will return.
///
/// Far more than anyone reads, and well short of what a two-character query
/// against a full home directory can match. The client's job is to show the
/// first screenful; the daemon's is to make sure a careless query cannot cost
/// a megabyte of D-Bus message and a directory read per row.
const SEARCH_LIMIT: usize = 200;

/// The shortest query the daemon will answer.
///
/// One character matches a large fraction of any real catalogue, and finding
/// that out is real work for an answer nobody can use. The application
/// debounces as the person types; this is what stops a client that does not.
const MIN_QUERY_CHARS: usize = 2;

/// Everything the interface reads and writes, shared with the background tasks
/// that fan job updates out as signals.
pub struct Shared {
    config: Mutex<Config>,
    pause: Mutex<PauseState>,
    jobs: Arc<JobRegistry>,
    /// Passphrases, in the keyring or only in memory as "Remember passphrase"
    /// says. See [`SessionSecretStore`].
    secrets: Arc<SessionSecretStore>,
    preview: PreviewCache,
    index_path: PathBuf,
    /// The engine, absent until a repository is configured.
    engine: Mutex<Option<Arc<dyn BackupEngine>>>,
    /// The archive name the most recent backup was told to write. Stage 4's
    /// ingest needs this to know what to stream into the catalogue.
    last_archive: Mutex<Option<String>>,
    /// Facts the health model is computed from.
    last_backup: Mutex<Option<SystemTime>>,
    /// The catalogue row a job failed with, held until a backup proves it is
    /// fixed. See [`update_health`] for why only that clears it.
    latched: Mutex<Option<HealthFailure>>,
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
    /// The local disk is low enough to mention: `DEGRADED`, the first half of
    /// health.md's "Local disk full" row.
    local_disk_low: Mutex<bool>,
    /// The local disk is too full for a backup to start: the second half,
    /// `BROKEN`. A fact measured at every preflight rather than a latch, so
    /// freeing space clears it at the next look.
    local_disk_full: Mutex<bool>,
    /// The local safety net held back a snapshot because it would not fit in
    /// its limit, so changes are not being protected. The same row, `BROKEN`.
    spool_held: Mutex<bool>,
    /// Backups that reached a repository and are not browsable yet. Counted
    /// when jobs end, since asking costs a catalogue query.
    pending: Mutex<usize>,
    /// The state last worked out, so a change can be recognised as one.
    announced: Mutex<Option<Health>>,
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
    /// Lets a finished local backup report what it did. Nested because the job
    /// factory only learns the handle once the job actually starts.
    #[allow(clippy::type_complexity)]
    offline_handle: Mutex<Option<Arc<Mutex<Option<pipeline::OfflineHandle>>>>>,
    /// The backup that will be this machine's first, if one is under way.
    /// Its end is announced, because the wizard told the person they could
    /// close the window and wait for it.
    first_backup: Mutex<Option<u64>>,
    /// Where notifications go. Nowhere until the session bus is reached.
    notifier: Mutex<Arc<dyn crate::notify::NotificationSink>>,
    /// A state shown in place of the real one, set only through the
    /// development interface.
    forced: Mutex<Option<Health>>,
    /// Backups that failed in a row for reasons nobody classified.
    unexplained_failures: Mutex<u32>,
    /// The catalogue was found damaged at start and is being read again from
    /// the repository: health.md's "Catalogue rebuilding…".
    catalogue_rebuilding: Mutex<bool>,
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
        let secrets = Arc::new(SessionSecretStore::new(
            secrets,
            config.security.remember_passphrase,
        ));
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
            latched: Mutex::new(None),
            destination_reachable: Mutex::new(true),
            persisted: Mutex::new(RuntimeState::default()),
            state_path,
            waker: Mutex::new(None),
            probe: Mutex::new(Arc::new(UnknownProbe)),
            local_disk_low: Mutex::new(false),
            local_disk_full: Mutex::new(false),
            spool_held: Mutex::new(false),
            pending: Mutex::new(0),
            announced: Mutex::new(None),
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
            offline_handle: Mutex::new(None),
            first_backup: Mutex::new(None),
            notifier: Mutex::new(Arc::new(crate::notify::NoDesktop)),
            forced: Mutex::new(None),
            unexplained_failures: Mutex::new(0),
            catalogue_rebuilding: Mutex::new(false),
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
            // A rebuild that had to wait for the destination can go on now.
            if *self.catalogue_rebuilding.lock().unwrap() {
                self.reconcile_catalogue().await;
            }
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

    /// Send notifications here from now on.
    pub fn set_notifier(&self, notifier: Arc<dyn crate::notify::NotificationSink>) {
        *self.notifier.lock().unwrap() = notifier;
    }

    /// Show `note` if the person's choice of notifications lets `kind`
    /// through.
    fn tell(&self, kind: crate::notify::Kind, note: crate::notify::Note) {
        let policy = self.config().general.notifications;
        if crate::notify::permits(policy, kind) {
            self.notifier.lock().unwrap().show(note);
        } else {
            debug!(
                title = note.title,
                ?policy,
                "a notification the person chose not to have"
            );
        }
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
            // A destination with nothing chosen to put in it has no schedule.
            // That is the state an import leaves a new computer in until the
            // person decides what to back up there, and a backup of nothing
            // would only fail, hourly.
            configured: config.is_configured() && !config.backup.include.is_empty(),
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

    /// Whether nothing is running or waiting to run.
    pub fn idle(&self) -> bool {
        !self.busy()
    }

    /// Whether backups are meant to carry on with no window open.
    pub fn run_in_background(&self) -> bool {
        self.config().general.run_in_background
    }

    /// Whether this daemon should start at login: there is something to back
    /// up to, and the person has not said to stop when the window closes.
    pub fn wants_background(&self) -> bool {
        let config = self.config();
        config.is_configured() && config.general.run_in_background
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
        *self.local_disk_low.lock().unwrap() = preflight::local_disk_needs_attention(&facts);
        *self.local_disk_full.lock().unwrap() = preflight::local_disk_full(&facts);

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
        // Asked again rather than refused: this is how "Check Again" finds
        // Borg once somebody has installed it.
        if self.engine.lock().unwrap().is_none() {
            self.connect_engine().await?;
        }
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
        let first = self.last_backup.lock().unwrap().is_none();
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
        let job = self.jobs.submit(JobKind::Backup, factory);
        if first {
            *self.first_backup.lock().unwrap() = Some(job);
        }
        Ok(job)
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

        let engine =
            BorgCli::new(path.display().to_string(), repository, self.secrets.clone()).await?;
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

    /// The passphrase the local safety net is encrypted with, read before a
    /// new destination's replaces it. `None` when there was no destination, or
    /// when it cannot be read, which is said.
    async fn spool_passphrase(&self, previous: Option<&str>) -> Option<String> {
        let previous = previous?;
        match self.secrets.get(previous).await {
            Ok(old) => Some(old),
            Err(e) => {
                warn!("the local snapshots cannot be carried over to the new destination: {e}");
                None
            }
        }
    }

    /// Carry the local safety net over to a new destination.
    ///
    /// The spool is encrypted with the passphrase of the destination it
    /// stands in for. A new destination brings a new passphrase, so without
    /// this the snapshots already on this computer would open until the daemon
    /// next restarted and never again after. They are re-keyed rather than
    /// thrown away because they may hold versions of files that nothing else
    /// has.
    ///
    /// `old` is read before the new passphrase is stored, because a new
    /// repository can have the old one's name: a fresh start after damage puts
    /// it exactly where the damaged one was.
    async fn rekey_spool(&self, old: Option<String>, passphrase: &str) {
        // Whatever happens next, the engine built for the old passphrase is
        // done.
        *self.spool.lock().unwrap() = None;
        let Some(old) = old else { return };
        if old == passphrase || !self.spool_dir.join("config").exists() {
            return;
        }
        let spool = match BorgCli::new(
            self.spool_dir.display().to_string(),
            self.spool_dir.display().to_string(),
            self.secrets.clone(),
        )
        .await
        {
            Ok(spool) => spool,
            Err(e) => {
                warn!("the local snapshots cannot be carried over to the new destination: {e}");
                return;
            }
        };
        match spool.change_passphrase(&old, passphrase).await {
            Ok(()) => info!("the local snapshots now open with the new destination's passphrase"),
            Err(e) => warn!("the local snapshots could not be moved to the new passphrase: {e}"),
        }
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

    /// Fold a finished local backup into the health facts, and say whether it
    /// protected what had changed.
    ///
    /// A snapshot held back by the storage limit ends as a successful job,
    /// because holding is the limit doing its job; but nothing was kept, and
    /// counting it as protection would let a machine sit behind a full spool
    /// for a week without ever being at risk.
    fn record_offline_outcome(&self) -> bool {
        let handle = self.offline_handle.lock().unwrap().clone();
        let Some(outcome) = handle.and_then(|h| h.lock().unwrap().clone()) else {
            return true;
        };
        let outcome = outcome.outcome();
        *self.spool_held.lock().unwrap() = outcome.held;
        !outcome.held
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

    /// Start the routine repository check if it is due, and return its job.
    ///
    /// Monthly, and sooner when backups keep failing for reasons nobody has
    /// classified. Low priority: only while nothing else has the repository,
    /// and only when the destination is there to be checked. Sampled, because
    /// the full verification reads every byte and takes hours on a large
    /// repository; that one stays in Preferences for anyone who wants it.
    ///
    /// Damage it finds ends the job with `RepoCorrupt`, which is the "backup
    /// needs repair" banner.
    pub fn maybe_check(&self, now: SystemTime) -> Option<u64> {
        let last = backtrack_core::state::from_epoch(self.persisted.lock().unwrap().last_check);
        let suspicious = *self.unexplained_failures.lock().unwrap() >= UNEXPLAINED_BEFORE_CHECK;
        if last.is_none() && !suspicious {
            // The first start with a destination begins the clock.
            self.update_persisted(|state| {
                state.last_check = backtrack_core::state::to_epoch(Some(now));
            });
            return None;
        }
        let busy = self.busy();
        let due = schedule::check_due(last, now, busy) || (suspicious && !busy);
        if !due || !self.destination_reachable() {
            return None;
        }
        let engine = self.engine().ok()?;
        self.update_persisted(|state| {
            state.last_check = backtrack_core::state::to_epoch(Some(now));
        });
        *self.unexplained_failures.lock().unwrap() = 0;
        let factory: JobFactory = Arc::new(move |_job| {
            let engine = Arc::clone(&engine);
            Box::pin(async move { engine.check(CheckLevel::Sampled(CHECK_SAMPLE)).await })
                as BoxFuture<'_, _>
        });
        let job = self.jobs.submit(JobKind::Check, factory);
        info!(job, suspicious, "checking the repository");
        Some(job)
    }

    /// Note that the catalogue is being read again from the repository, after
    /// it was found damaged.
    pub fn start_catalogue_rebuild(&self) {
        *self.catalogue_rebuilding.lock().unwrap() = true;
        self.health_changed.notify_one();
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
    ///
    /// Borg missing or too old is health.md's "backup engine is missing" row,
    /// and is recorded as one: without it there is no engine, every backup is
    /// refused before it starts, and nothing would ever say why.
    pub async fn connect_engine(&self) -> Result<()> {
        let repository = self.config.lock().unwrap().storage.repository.clone();
        let Some(repository) = repository else {
            debug!("no repository configured; engine not connected");
            return Ok(());
        };
        match BorgCli::new(repository.clone(), repository, self.secrets.clone()).await {
            Ok(engine) => {
                self.set_engine(Arc::new(engine));
                self.resolved(&[HealthFailure::BorgMissing]);
                Ok(())
            }
            Err(error) => {
                if let Some(failure) = error.health_failure() {
                    self.record_blocking_failure(failure);
                    self.health_changed.notify_one();
                }
                Err(error.into())
            }
        }
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

    /// The current health, computed fresh from facts.
    pub fn health(&self) -> Health {
        let now = SystemTime::now();
        self.shown(health::evaluate(&self.health_inputs(now), now))
    }

    /// The state to show: the real one, unless development has forced
    /// another.
    fn shown(&self, real: Health) -> Health {
        self.forced.lock().unwrap().unwrap_or(real)
    }

    /// Show `health` in place of the real state, or the real state again.
    fn force(&self, health: Option<Health>) {
        *self.forced.lock().unwrap() = health;
        self.health_changed.notify_one();
    }

    /// Everything health is computed from, as it stands.
    fn health_inputs(&self, now: SystemTime) -> HealthInputs {
        let schedule = self.schedule_input();
        // The engine's own failure first: it is the one a job actually hit.
        // The disk and the spool are facts about this computer that the next
        // look may find already cleared.
        let local_full = *self.local_disk_full.lock().unwrap() || *self.spool_held.lock().unwrap();
        let blocking = self
            .latched
            .lock()
            .unwrap()
            .or(local_full.then_some(HealthFailure::LocalDiskFull));
        // Most pressing first: a disk filling up turns into a stopped backup,
        // the other two only ever into a slower timeline, and a rebuild in
        // progress explains any backups not yet browsable.
        let attention = if *self.local_disk_low.lock().unwrap() || self.spool_over_limit() {
            Some(Reason::LocalDiskFull)
        } else if *self.catalogue_rebuilding.lock().unwrap() {
            Some(Reason::CatalogueRebuilding)
        } else if *self.pending.lock().unwrap() > 0 && !self.cataloguing() {
            Some(Reason::NotYetBrowsable)
        } else {
            None
        };
        HealthInputs {
            blocking,
            paused_until: self.pause.lock().unwrap().until(now),
            destination_reachable: *self.destination_reachable.lock().unwrap(),
            offline_protection_active: self.offline_protection_active(),
            last_success: *self.last_backup.lock().unwrap(),
            protected_since: backtrack_core::state::from_epoch(
                self.persisted.lock().unwrap().protected_since,
            ),
            // What the schedule will actually do: a destination with nothing
            // chosen to back up has no schedule, and cannot be late.
            frequency: schedule.interval.filter(|_| schedule.configured),
            attention,
        }
    }

    /// Whether the local safety net holds more than its limit: what a delta
    /// bigger than the whole limit, taken once, leaves behind.
    ///
    /// Measured from its size each time rather than remembered from the run
    /// that caused it, so a run with nothing to save cannot make it forgotten.
    /// It stays true until the limit is raised, the next local backup makes
    /// room, or the snapshots expire once the destination has them.
    fn spool_over_limit(&self) -> bool {
        let offline = self.config().storage.offline;
        let limit = u64::from(offline.space_limit_gb) * 1024 * 1024 * 1024;
        offline.enabled && limit > 0 && crate::offline::directory_bytes(&self.spool_dir) > limit
    }

    /// Whether something is busy making backups browsable. A backup waiting
    /// its turn behind one of these is not one that failed to catalogue.
    fn cataloguing(&self) -> bool {
        self.jobs.list().iter().any(|job| {
            !job.state.is_terminal()
                && matches!(
                    job.kind,
                    JobKind::Backup | JobKind::Offline | JobKind::Index
                )
        })
    }

    /// Count the backups that are not browsable yet, for the health model,
    /// and return the count.
    pub async fn refresh_pending(&self) -> usize {
        let count = self.uncatalogued_count().await;
        *self.pending.lock().unwrap() = count;
        count
    }

    /// Work out the health now and act on it: record a change, and tell the
    /// person if the escalation rules say this is the moment.
    ///
    /// Returns the new health when it changed, for the caller to announce.
    fn reassess(&self, now: SystemTime) -> Option<Health> {
        let inputs = self.health_inputs(now);
        let current = self.shown(health::evaluate(&inputs, now));
        let changed = {
            let mut announced = self.announced.lock().unwrap();
            let changed = *announced != Some(current);
            *announced = Some(current);
            changed
        };
        if changed {
            self.record_transition(current, now);
            // A notification about a problem that has been fixed is worse
            // than none: clicking it opens a fix for nothing.
            if !matches!(current.state, HealthState::AtRisk | HealthState::Broken) {
                self.notifier
                    .lock()
                    .unwrap()
                    .withdraw(crate::notify::Topic::Health);
            }
        }

        let told = self.persisted.lock().unwrap().notified.clone();
        let until = backtrack_core::state::to_epoch(health::protected_until(&inputs));
        let at = backtrack_core::state::to_epoch(Some(now)).unwrap_or(0);
        if let Some((alert, record)) = escalation::decide(current, until, &told, at) {
            // Recorded whether or not it is shown: someone who has switched
            // notifications off and back on should not be handed a backlog.
            self.update_persisted(|state| state.notified = record);
            info!(?alert, "backups need the person's attention");
            let destination = self.config().storage.repository.unwrap_or_default();
            self.tell(
                crate::notify::Kind::Attention,
                crate::notify::alert(alert, &backtrack_core::destination::name(&destination)),
            );
        }
        changed.then_some(current)
    }

    /// Add a change of state to the history, unless the history already ends
    /// with it — which is what a restart into the same state looks like.
    fn record_transition(&self, health: Health, now: SystemTime) {
        let last = self
            .persisted
            .lock()
            .unwrap()
            .health_history
            .last()
            .cloned();
        if last.is_some_and(|t| t.state == health.state.as_str() && t.reason == health.reason_str())
        {
            return;
        }
        info!(
            state = health.state.as_str(),
            reason = health.reason_str(),
            "health state changed"
        );
        let at = backtrack_core::state::to_epoch(Some(now)).unwrap_or(0);
        self.update_persisted(|state| {
            state.record_transition(backtrack_core::state::Transition {
                at,
                state: health.state.as_str().to_string(),
                reason: health.reason_str().to_string(),
            })
        });
    }

    /// When the current state began: its entry in the history, or now if it
    /// has not been recorded yet.
    fn state_since(&self, health: Health, now: SystemTime) -> u64 {
        let history = &self.persisted.lock().unwrap().health_history;
        history
            .last()
            .filter(|t| t.state == health.state.as_str() && t.reason == health.reason_str())
            .map(|t| t.at)
            .or_else(|| backtrack_core::state::to_epoch(Some(now)))
            .unwrap_or(0)
    }

    /// Whether `passphrase` is the one the backups use, asked without storing
    /// it.
    ///
    /// The destination answers when it can, and it is the authority when it
    /// is encrypted. An unencrypted one opens with any passphrase at all, so
    /// its yes proves nothing; the passphrase is then only used by the local
    /// safety net, which is always encrypted, and that is what has to accept
    /// it. Accepting whatever was typed would store a passphrase the safety
    /// net cannot open, and the next backup away from the destination would
    /// fail with the very banner this was meant to clear.
    ///
    /// Where the destination is away, the safety net answers alone, so that
    /// somebody whose keyring was reset on a train need not wait until they
    /// are home.
    async fn try_passphrase(&self, repository: &str, passphrase: &str) -> Result<()> {
        let trial: Arc<dyn SecretStore> = Arc::new(Trial(passphrase.to_string()));
        let engine = BorgCli::new(
            repository.to_string(),
            repository.to_string(),
            Arc::clone(&trial),
        )
        .await?;
        let spooled = self.spool_dir.join("config").exists();
        match engine.repo_info().await {
            Ok(info) if info.encrypted || !spooled => Ok(()),
            Ok(_) | Err(backtrack_core::engine::EngineError::RepoUnreachable) if spooled => {
                let spool_path = self.spool_dir.display().to_string();
                let spool = BorgCli::new(spool_path.clone(), spool_path, trial).await?;
                spool.repo_info().await?;
                Ok(())
            }
            answer => answer.map(|_| ()).map_err(DaemonError::from),
        }
    }

    /// Where the room on this computer has gone.
    fn local_storage(&self) -> LocalStorage {
        let config = self.config();
        let data = self.state_path.parent().unwrap_or(&self.state_path);
        LocalStorage {
            free: preflight::free_bytes(data).unwrap_or(0),
            snapshots: crate::offline::directory_bytes(&self.spool_dir),
            snapshot_limit: u64::from(config.storage.offline.space_limit_gb) * 1024 * 1024 * 1024,
            stash: crate::offline::directory_bytes(&self.replaced_dir),
            cache: self.preview.size_bytes(),
        }
    }

    /// Take the destination's backups out of the catalogue, leaving the local
    /// snapshots, which are not in the destination and could not be read back
    /// from it.
    async fn forget_primary_archives(&self) -> Result<()> {
        let index = self.index()?;
        tokio::task::spawn_blocking(move || {
            let mut writer = index.lock().unwrap();
            let seqs: Vec<i64> = writer
                .archives_in(backtrack_core::index::Repo::Primary)?
                .into_iter()
                .map(|row| row.seq)
                .collect();
            writer.remove_archives(&seqs)
        })
        .await
        .map_err(|e| DaemonError::IndexUnavailable(e.to_string()))??;
        Ok(())
    }

    /// The state, how it came about, and what has been said about it.
    fn health_report(&self) -> HealthReport {
        let now = SystemTime::now();
        let health = self.health();
        let since = self.state_since(health, now);
        let state = self.persisted.lock().unwrap().clone();
        HealthReport {
            state: health.state.as_str().to_string(),
            reason: health.reason_str().to_string(),
            since,
            history: state
                .health_history
                .iter()
                .map(|t| (t.at, t.state.clone(), t.reason.clone()))
                .collect(),
            errors: state
                .last_errors
                .iter()
                .map(|e| {
                    (
                        e.subsystem.clone(),
                        e.at,
                        e.reason.clone(),
                        e.message.clone(),
                    )
                })
                .collect(),
            at_risk_notices: state.notified.at_risk_count,
            at_risk_notified: state.notified.at_risk_at.unwrap_or(0),
            at_risk_since: state.notified.at_risk_since.unwrap_or(0),
            broken_notified_reason: state.notified.broken_reason.unwrap_or_default(),
            broken_notified: state.notified.broken_at.unwrap_or(0),
        }
    }

    /// Start the clock that "no successful backups" is measured on.
    fn start_protection_clock(&self) {
        self.update_persisted(|state| {
            state.protected_since = backtrack_core::state::to_epoch(Some(SystemTime::now()));
        });
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
    ///
    /// A backup to the destination proves every blocking failure fixed. One
    /// on this computer proves only the ones it went through: it needed the
    /// passphrase, Borg and room on the local disk, and it says nothing at all
    /// about whether the destination is still full or still refusing us.
    fn record_backup_success(&self, kind: JobKind) {
        *self.last_backup.lock().unwrap() = Some(SystemTime::now());
        if kind == JobKind::Backup {
            *self.unexplained_failures.lock().unwrap() = 0;
            // What the safety net could not hold is on the destination now.
            *self.spool_held.lock().unwrap() = false;
        }
        let mut latched = self.latched.lock().unwrap();
        let proven = match kind {
            JobKind::Offline => latched.is_some_and(|failure| {
                matches!(
                    failure,
                    HealthFailure::PassphraseMissing
                        | HealthFailure::PassphraseWrong
                        | HealthFailure::BorgMissing
                        | HealthFailure::LocalDiskFull
                )
            }),
            _ => true,
        };
        if proven {
            *latched = None;
        }
    }

    /// Record a failure that stops backups until the user acts.
    fn record_blocking_failure(&self, failure: HealthFailure) {
        *self.latched.lock().unwrap() = Some(failure);
    }

    /// Forget the failure holding backups up, for a repository that has not
    /// been tried yet.
    fn clear_blocking_failure(&self) {
        *self.latched.lock().unwrap() = None;
        self.health_changed.notify_one();
    }

    /// Clear the failure holding backups up if it is one of `fixed`: what a
    /// resolution has just proved, short of a whole backup.
    fn resolved(&self, fixed: &[HealthFailure]) {
        let mut latched = self.latched.lock().unwrap();
        if latched.is_some_and(|failure| fixed.contains(&failure)) {
            *latched = None;
            drop(latched);
            self.health_changed.notify_one();
        }
    }

    /// Record how a job failed, as the last word from its part of the daemon.
    fn record_error(&self, kind: JobKind, error: &backtrack_core::engine::EngineError) {
        let subsystem = subsystem(kind);
        let reason = error
            .health_failure()
            .map(|failure| Reason::from(failure).as_str().to_string())
            .unwrap_or_default();
        let at = backtrack_core::state::to_epoch(Some(SystemTime::now())).unwrap_or(0);
        let message = error.to_string();
        self.update_persisted(|state| {
            state.last_errors.retain(|e| e.subsystem != subsystem);
            state.last_errors.push(backtrack_core::state::LastError {
                subsystem: subsystem.to_string(),
                at,
                reason,
                message,
            });
        });
    }

    /// Take in a job that has ended: what it means for health, and whether
    /// it is worth a notification.
    fn job_ended(&self, id: u64, kind: JobKind, state: &JobState) {
        update_health(self, kind, state);
        self.announce_backup(id, kind, state);
    }

    /// Say how a backup went, to somebody who wants to know.
    ///
    /// The first backup always gets a word unless notifications are off,
    /// because the wizard told the person they could close the window and
    /// wait for it. Every other one only for somebody who asked to hear
    /// about every backup, and only when it went well: one that did not is
    /// the health model's to report, when it is worth reporting.
    fn announce_backup(&self, job: u64, kind: JobKind, state: &JobState) {
        let first = {
            let mut first = self.first_backup.lock().unwrap();
            let was = *first == Some(job);
            if was {
                // Cleared either way: a first backup that failed leaves the
                // next one as the first.
                *first = None;
            }
            was
        };
        let Some(outcome) = state.outcome_token() else {
            return;
        };
        if first {
            if let Some(note) = crate::notify::first_backup(outcome) {
                self.tell(crate::notify::Kind::FirstBackup, note);
            }
        } else if kind == JobKind::Backup && *state == JobState::Done(Outcome::Completed) {
            self.tell(
                crate::notify::Kind::Success,
                crate::notify::backup_complete(),
            );
        }
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
        // Refused here rather than left to Borg, which would fail with an
        // error about arguments that means nothing to the person who pressed
        // the button.
        if self.shared.config().backup.include.is_empty() {
            return Err(DaemonError::NotConfigured(
                "nothing has been chosen to back up yet".into(),
            ));
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
        let health = self.shared.health();
        Ok(Status {
            state: health.state.as_str().to_string(),
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
            reason: health.reason_str().to_string(),
            since: self.shared.state_since(health, now),
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

    /// A readable descriptor onto the *live* copy of a catalogued path.
    ///
    /// The other half of `PreviewFile`, and it exists for the same reason: a
    /// sandboxed application has no path to the user's files, so the side of a
    /// comparison that says "today" has to be handed over rather than opened.
    /// Reading the size and modification time is the caller's, from the
    /// descriptor it is given, so the two can never describe different files.
    ///
    /// Only paths the catalogue knows are served. Nothing about this daemon's
    /// other methods is narrower — a restore writes wherever it is told — but
    /// the comparison has no business asking about a file that was never
    /// backed up, and a method that will not answer a question nobody should
    /// ask is one less thing to reason about later.
    async fn live_file(&self, path: &str) -> Result<OwnedFd> {
        let catalogued = {
            let reader = IndexReader::open(&self.shared.index_path)?;
            reader
                .file_history(path)
                .map(|versions| !versions.is_empty())
        }?;
        if !catalogued {
            return Err(DaemonError::NotFound(format!(
                "{path} is not in the catalogue"
            )));
        }

        let full = std::path::Path::new("/").join(path.trim_start_matches('/'));
        let file = std::fs::File::open(&full).map_err(|e| {
            DaemonError::NotFound(format!("{} could not be opened: {e}", full.display()))
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

    /// Which of these catalogued paths are still on this computer.
    ///
    /// Answers come back in the order they were asked for, one byte each:
    /// 0 unknown, 1 absent, 2 present. Three-valued because the honest answer
    /// to "I could not read that folder" is not "the file is gone" — an
    /// archive taken on another machine describes paths this one never had,
    /// and a caller that painted those as deletions would be inventing them.
    ///
    /// The daemon answers this rather than the caller because a sandboxed
    /// application cannot be assumed to reach the user's files at all: it is
    /// handed archived contents as descriptors for exactly that reason.
    async fn paths_on_disk(&self, paths: Vec<String>) -> Result<Vec<u8>> {
        let answers = tokio::task::spawn_blocking(move || on_disk::resolve(&paths))
            .await
            .map_err(|error| {
                DaemonError::IndexUnavailable(format!("the folder listing task failed: {error}"))
            })?;
        Ok(answers.into_iter().map(on_disk::OnDisk::as_byte).collect())
    }

    /// Filename search across every snapshot, including files that have since
    /// been deleted.
    ///
    /// Two limits, both the daemon's to keep because a client cannot be relied
    /// on to. A query shorter than [`MIN_QUERY_CHARS`] is refused rather than
    /// answered: one character matches a large fraction of a home directory,
    /// and the work of finding that out is real even though the answer is
    /// useless. Results are capped at [`SEARCH_LIMIT`], which is far more than
    /// anyone reads and well short of what a broad query can produce.
    ///
    /// Debouncing is the client's, since only the client knows whether the
    /// person is still typing.
    async fn search_files(&self, query: &str) -> Result<Vec<SearchResult>> {
        let trimmed = query.trim();
        if trimmed.chars().count() < MIN_QUERY_CHARS {
            return Err(DaemonError::InvalidArgument(format!(
                "a search needs at least {MIN_QUERY_CHARS} characters"
            )));
        }

        let reader = IndexReader::open(&self.shared.index_path)?;
        let mut hits = reader.search(trimmed)?;
        // Capped before the disk is consulted, so a query matching ten thousand
        // paths costs ten thousand index rows rather than ten thousand
        // directory reads. The order it caps on is the catalogue's, which
        // already puts what the newest backup has lost at the top.
        hits.truncate(SEARCH_LIMIT);

        let paths: Vec<String> = hits.iter().map(|hit| hit.path.clone()).collect();
        let presence = tokio::task::spawn_blocking(move || on_disk::resolve(&paths))
            .await
            .map_err(|error| {
                DaemonError::IndexUnavailable(format!("the folder listing task failed: {error}"))
            })?;

        // What a person searching has lost is what they are looking for, so it
        // goes first. A *stable* sort, which leaves the catalogue's own
        // ranking — most recent existence, then relevance — intact inside each
        // group.
        let mut ranked: Vec<_> = hits.into_iter().zip(presence).collect();
        ranked.sort_by_key(|(_, presence)| *presence != on_disk::OnDisk::Absent);

        Ok(ranked
            .into_iter()
            .map(|(hit, presence)| SearchResult {
                path: hit.path,
                name: hit.name,
                kind: kind_name(hit.kind).to_string(),
                first_seq: hit.first_seq,
                last_seq: hit.last_seq,
                first_ts: hit.first_ts,
                last_ts: hit.last_ts,
                versions: hit.version_count.max(0) as u32,
                size: hit.size,
                gone_from_disk: presence == on_disk::OnDisk::Absent,
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
    async fn set_config(
        &self,
        key: &str,
        value: &str,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<()> {
        let before = self.shared.config();
        let updated = config_set(&before, key, value)?;
        let repository_changed = updated.storage.repository != before.storage.repository;
        let sources_changed = updated.backup.include != before.backup.include;
        let remember_changed =
            updated.security.remember_passphrase != before.security.remember_passphrase;
        let background_changed =
            updated.general.run_in_background != before.general.run_in_background;

        // The passphrase moves before the setting is written, so a keyring
        // that refuses leaves both saying what they said before.
        if remember_changed {
            self.shared
                .secrets
                .set_remember(
                    updated.security.remember_passphrase,
                    updated.storage.repository.as_deref(),
                )
                .await?;
        }
        let first_sources = before.backup.include.is_empty() && !updated.backup.include.is_empty();
        self.shared.store_config(updated)?;
        info!(key, "configuration changed");
        // A destination imported with nothing to back up has no schedule, and
        // nothing can be late until it has one.
        if first_sources {
            self.shared.start_protection_clock();
        }
        // The schedule reads the configuration fresh each time it decides, so
        // a new frequency or source list only needs it to decide again now.
        self.shared.wake_scheduler();
        if background_changed {
            crate::background::apply(connection, self.shared.wants_background()).await;
        }
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
    ///
    /// The schedule's clock starts here rather than the first backup being
    /// due at once. The wizard creates the repository partway through its
    /// last page, because the recovery key cannot be saved until there is a
    /// key, and takes the first backup itself when the person presses Start.
    /// A schedule that fired in between would start that backup while they
    /// were still reading the page.
    async fn setup_repo(
        &self,
        path: &str,
        passphrase: &str,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<()> {
        let previous = self.shared.config().storage.repository;
        let old = self.shared.spool_passphrase(previous.as_deref()).await;
        self.shared.secrets.set(path, passphrase).await?;
        let engine = BorgCli::new(
            path.to_string(),
            path.to_string(),
            self.shared.secrets.clone(),
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
        self.shared.update_persisted(|state| {
            state.last_attempt = backtrack_core::state::to_epoch(Some(SystemTime::now()));
        });
        self.shared.start_protection_clock();
        // Whatever was wrong with the last repository is not wrong with this
        // one, which has not been tried yet.
        self.shared.clear_blocking_failure();
        info!(path, "repository created");
        self.shared.rekey_spool(old, passphrase).await;

        // A new repository holds nothing, so a catalogue still describing
        // the previous destination's backups would offer restores from
        // archives this one does not have. They stay in the old repository,
        // which is where Import finds them.
        self.shared.reconcile_catalogue().await;
        crate::background::apply(connection, self.shared.wants_background()).await;
        Ok(())
    }

    /// What is at `repository` before anything is created there: `empty`,
    /// `existing` (a Borg repository), `occupied` (something else), or
    /// `unwritable`. Connection problems are errors, named as usual.
    ///
    /// The wizard asks before it offers to create anything, so that choosing a
    /// drive that already holds this computer's backups leads to them rather
    /// than to a failed attempt to create a second repository on top.
    async fn inspect_destination(&self, repository: &str) -> Result<String> {
        if repository.trim().is_empty() {
            return Err(DaemonError::InvalidArgument(
                "no destination was given".into(),
            ));
        }
        let engine = BorgCli::new(
            repository.to_string(),
            repository.to_string(),
            self.shared.secrets.clone(),
        )
        .await?;
        let presence = engine.presence().await?;
        debug!(repository, ?presence, "destination inspected");
        Ok(match presence {
            backtrack_core::engine::Presence::Empty => "empty",
            backtrack_core::engine::Presence::Existing => "existing",
            backtrack_core::engine::Presence::Occupied => "occupied",
            backtrack_core::engine::Presence::Unwritable => "unwritable",
        }
        .to_string())
    }

    /// Change the repository's passphrase.
    ///
    /// The local safety net is encrypted with the same passphrase, so it
    /// changes too, and the two never disagree: if the second change fails
    /// the first is undone. The recovery key saved earlier still opens the
    /// backups, but with the old passphrase, which the window says.
    async fn change_passphrase(&self, new: &str) -> Result<()> {
        if new.is_empty() {
            return Err(DaemonError::InvalidArgument(
                "a passphrase cannot be empty".into(),
            ));
        }
        let repository = self.shared.config().storage.repository.ok_or_else(|| {
            DaemonError::NotConfigured("no backup destination is configured yet".into())
        })?;
        let old = self.shared.secrets.get(&repository).await?;
        let engine = self.shared.engine()?;
        engine.change_passphrase(&old, new).await?;

        let undo = |error: DaemonError| async {
            if let Err(undo) = engine.change_passphrase(new, &old).await {
                warn!("the passphrase change could not be undone: {undo}");
            }
            error
        };
        if self.shared.spool_dir.join("config").exists() {
            let spool = match self.shared.spool_engine().await {
                Ok(spool) => spool,
                Err(error) => return Err(undo(error).await),
            };
            if let Err(error) = spool.change_passphrase(&old, new).await {
                return Err(undo(error.into()).await);
            }
        }
        if let Err(error) = self.shared.secrets.set(&repository, new).await {
            return Err(undo(error.into()).await);
        }
        info!("passphrase changed");
        Ok(())
    }

    /// The figures Preferences shows about the repository.
    ///
    /// Asked of Borg, which has to take the repository's lock to answer, so it
    /// is refused while a job is using the repository rather than risking the
    /// backup losing a race for that lock to a window being opened.
    async fn get_storage_info(&self) -> Result<StorageInfo> {
        let engine = self.shared.engine()?;
        if !self.shared.idle() {
            return Err(DaemonError::LockedByOther(
                "the backups are in use; ask again when the current job has finished".into(),
            ));
        }
        let stats = engine.repo_stats().await?;
        let repository = self.shared.config().storage.repository.unwrap_or_default();
        let (capacity, free) = filesystem_space(&repository);
        Ok(StorageInfo {
            encryption: stats.encryption,
            stored: stats.stored_bytes,
            original: stats.original_bytes,
            capacity,
            free,
        })
    }

    /// Throw the catalogue's record of the backups away and read it again
    /// from the repository. Returns the job doing the reading.
    ///
    /// Refused unless the repository is there to read from, since clearing
    /// the catalogue of a destination that is not plugged in would leave an
    /// empty timeline and nothing to fill it; and refused while a job is
    /// running, which may be writing to the catalogue at that moment. Local
    /// snapshots are left alone: they are not in the repository, so reading
    /// it again could not bring them back.
    async fn rebuild_catalogue(&self) -> Result<u64> {
        self.shared.engine()?;
        if !self.shared.idle() {
            return Err(DaemonError::LockedByOther(
                "wait for the current job to finish, then rebuild the catalogue".into(),
            ));
        }
        if self.shared.probe_destination().await == Reach::No {
            return Err(DaemonError::RepoUnreachable(
                "the backup destination is not reachable, so there is nothing to rebuild from"
                    .into(),
            ));
        }
        self.shared.forget_primary_archives().await?;
        info!("catalogue cleared for a rebuild");
        self.shared
            .reconcile_catalogue()
            .await
            .ok_or_else(|| DaemonError::RepoUnreachable("the backup destination went away".into()))
    }

    /// Put every setting back to its default, as before the wizard ran.
    ///
    /// Touches nothing else. The repository, the catalogue, the safety stash
    /// and the passphrase in the keyring are all left exactly where they are,
    /// so the wizard that follows can open the same backups again with
    /// Import.
    async fn reset_config(&self, #[zbus(connection)] connection: &zbus::Connection) -> Result<()> {
        let config = Config::default();
        self.shared
            .secrets
            .set_remember(config.security.remember_passphrase, None)
            .await?;
        self.shared.store_config(config)?;
        *self.shared.engine.lock().unwrap() = None;
        self.shared.forget_offline_mode();
        self.shared.wake_scheduler();
        crate::background::apply(connection, self.shared.wants_background()).await;
        info!("settings reset; backups and the catalogue are as they were");
        Ok(())
    }

    /// Take the passphrase again, after the keyring lost it or it stopped
    /// matching the repository, and carry on backing up.
    ///
    /// Tried against the repository before it is kept anywhere, so a typo is
    /// answered here, in the dialog it was typed into, rather than an hour
    /// later as the same banner again. `remember` is the dialog's "Remember it
    /// so backups run automatically", which is the Remember passphrase
    /// setting.
    ///
    /// `recovery_key`, when not empty, is put back into the repository first:
    /// the path for a repository whose key has changed since the key was
    /// saved, after which `passphrase` is the one that was in use then.
    ///
    /// Returns the backup it starts, or 0 when there is nothing chosen to
    /// back up.
    async fn unlock_backups(
        &self,
        passphrase: &str,
        remember: bool,
        recovery_key: &str,
    ) -> Result<u64> {
        if passphrase.is_empty() {
            return Err(DaemonError::InvalidArgument(
                "a passphrase cannot be empty".into(),
            ));
        }
        let repository = self.shared.config().storage.repository.ok_or_else(|| {
            DaemonError::NotConfigured("no backup destination is configured yet".into())
        })?;
        if !recovery_key.trim().is_empty() {
            self.shared.engine()?.key_import(recovery_key).await?;
            info!("a saved recovery key was put back into the repository");
        }
        self.shared.try_passphrase(&repository, passphrase).await?;

        // The setting moves before the passphrase is stored, so it is stored
        // where the setting now says.
        if remember != self.shared.config().security.remember_passphrase {
            self.shared
                .secrets
                .set_remember(remember, Some(&repository))
                .await?;
            let mut config = self.shared.config();
            config.security.remember_passphrase = remember;
            self.shared.store_config(config)?;
        }
        self.shared.secrets.set(&repository, passphrase).await?;
        info!(remember, "the passphrase was given again");
        self.shared.resolved(&[
            HealthFailure::PassphraseMissing,
            HealthFailure::PassphraseWrong,
        ]);
        if self.shared.config().backup.include.is_empty() {
            return Ok(0);
        }
        self.shared.submit_backup().await
    }

    /// Repair what a check found wrong (`borg check --repair`). Returns the
    /// job.
    ///
    /// Whatever is damaged beyond saving is removed so that the rest can be
    /// used again. The window says so in plain words before asking, and runs
    /// a check afterwards to find out whether it worked.
    async fn repair(&self) -> Result<u64> {
        let engine = self.shared.engine()?;
        if !self.shared.idle() {
            return Err(DaemonError::LockedByOther(
                "wait for the current job to finish, then repair".into(),
            ));
        }
        info!("repairing the repository");
        let factory: JobFactory = Arc::new(move |_job| {
            let engine = Arc::clone(&engine);
            Box::pin(async move { engine.repair().await }) as BoxFuture<'_, _>
        });
        Ok(self.shared.jobs.submit(JobKind::Check, factory))
    }

    /// Put a repository that could not be repaired aside, so that a new one
    /// can be made in its place. Returns where the old one now is.
    ///
    /// Nothing is deleted. The repository is renamed beside where it was, with
    /// everything in it, for whatever can still be salvaged by hand; the
    /// catalogue forgets the backups that were in it. The new repository is
    /// made by `SetupRepo`, as any other is.
    ///
    /// Refused for a repository on a server, which cannot be renamed from
    /// here: a fresh start there goes to a new location, and leaves the old
    /// one exactly as it is.
    async fn start_fresh(&self) -> Result<String> {
        let repository = self.shared.config().storage.repository.ok_or_else(|| {
            DaemonError::NotConfigured("no backup destination is configured yet".into())
        })?;
        let path = PathBuf::from(&repository);
        if !path.is_absolute() {
            return Err(DaemonError::InvalidArgument(
                "a repository on a server cannot be put aside from here; choose a new \
                 location for the fresh start instead"
                    .into(),
            ));
        }
        if !self.shared.idle() {
            return Err(DaemonError::LockedByOther(
                "wait for the current job to finish".into(),
            ));
        }
        let aside = set_aside(&path, SystemTime::now())?;
        *self.shared.engine.lock().unwrap() = None;
        self.shared.forget_primary_archives().await?;
        info!(
            aside = %aside.display(),
            "the damaged repository was put aside with nothing in it deleted"
        );
        Ok(aside.display().to_string())
    }

    /// Where the room on this computer has gone: the local safety net, the
    /// safety stash, and the preview cache.
    async fn get_local_storage(&self) -> Result<LocalStorage> {
        let shared = Arc::clone(&self.shared);
        tokio::task::spawn_blocking(move || shared.local_storage())
            .await
            .map_err(|e| DaemonError::LocalDiskFull(e.to_string()))
    }

    /// Remove the copies extracted for previews. Returns the bytes freed.
    async fn clear_preview_cache(&self) -> Result<u64> {
        let cache = self.shared.preview.clone();
        let freed = tokio::task::spawn_blocking(move || cache.clear())
            .await
            .map_err(|e| DaemonError::LocalDiskFull(e.to_string()))?;
        info!(bytes = freed, "the preview cache was cleared");
        Ok(freed)
    }

    /// Give up the files restores replaced, apart from the last hour's, which
    /// an Undo still on screen may be about to need. Returns the bytes freed.
    async fn empty_stash(&self) -> Result<u64> {
        let root = self.shared.replaced_dir.clone();
        let now = backtrack_core::state::to_epoch(Some(SystemTime::now())).unwrap_or(0) as i64;
        let report = tokio::task::spawn_blocking(move || restore::expire_stash(&root, now, 0))
            .await
            .map_err(|e| DaemonError::RestoreFailed(e.to_string()))?;
        info!(
            restores = report.batches,
            bytes = report.bytes,
            "the safety stash was emptied on request"
        );
        Ok(report.bytes)
    }

    /// The state, how it came about, and what the person has been told.
    async fn get_health(&self) -> Result<HealthReport> {
        Ok(self.shared.health_report())
    }

    /// The configured repository's recovery key, exactly as `borg key export`
    /// writes it, so that the file a person saves is one `borg key import`
    /// reads back without editing.
    async fn export_recovery_key(&self) -> Result<String> {
        let key = self.shared.engine()?.key_export().await?;
        info!("recovery key exported");
        Ok(key)
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
    async fn import_repo(
        &self,
        path: &str,
        passphrase: &str,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<()> {
        let previous = self.shared.config().storage.repository;
        let old = self.shared.spool_passphrase(previous.as_deref()).await;
        self.shared.secrets.set(path, passphrase).await?;
        let engine = BorgCli::new(
            path.to_string(),
            path.to_string(),
            self.shared.secrets.clone(),
        )
        .await?;
        let info = engine.repo_info().await?;

        let mut config = self.shared.config();
        config.storage.repository = Some(path.to_string());
        self.shared.store_config(config)?;
        self.shared.set_engine(Arc::new(engine));
        self.shared.start_protection_clock();
        self.shared.clear_blocking_failure();
        info!(path, archives = info.archive_count, "repository imported");
        self.shared.rekey_spool(old, passphrase).await;
        crate::background::apply(connection, self.shared.wants_background()).await;

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

    /// The overall health state changed. `reason` is what it is about, as a
    /// `Reason` token, and empty when healthy.
    #[zbus(signal)]
    pub async fn status_changed(
        emitter: &SignalEmitter<'_>,
        state: &str,
        reason: &str,
    ) -> zbus::Result<()>;

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

/// How often health is looked at again when nothing has happened. Two things
/// change it with no event at all to hang a look off: a pause running out,
/// and a day passing without a backup.
const REASSESS_EVERY: Duration = Duration::from_secs(60);

/// Turn the job registry's updates into D-Bus signals.
///
/// Runs for the daemon's lifetime. `StatusChanged` is emitted only when the
/// computed state actually differs from the last one announced — a signal per
/// job event would make "the state changed" meaningless.
pub async fn fan_out_signals(shared: Arc<Shared>, emitter: SignalEmitter<'static>) {
    let mut updates = shared.jobs.subscribe();
    let health_changed = shared.health_waker();
    let mut tick = tokio::time::interval(REASSESS_EVERY);
    // The first answer is announced whatever it is: a client that connected
    // while the daemon was starting is waiting to hear it.
    let first = shared
        .reassess(SystemTime::now())
        .unwrap_or_else(|| shared.health());
    announce(&emitter, first).await;

    loop {
        // Three sources, one destination. Jobs are the usual reason health
        // moves, but not the only one: the destination coming or going changes
        // the answer with no job involved at all, and so does the clock.
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
            _ = tick.tick() => None,
        };

        let Some(update) = update else {
            if let Some(changed) = shared.reassess(SystemTime::now()) {
                announce(&emitter, changed).await;
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
                    shared.job_ended(id, kind, &state);
                    // Counted off this loop: the count takes the catalogue's
                    // lock, which an ingest can hold for minutes, and every
                    // signal would wait behind it.
                    let counting = Arc::clone(&shared);
                    tokio::spawn(async move {
                        counting.refresh_pending().await;
                        counting.health_waker().notify_one();
                    });
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
                if let Some(changed) = shared.reassess(SystemTime::now()) {
                    announce(&emitter, changed).await;
                }
            }
        }
    }
}

/// Tell every client the state and its reason.
async fn announce(emitter: &SignalEmitter<'_>, health: Health) {
    let _ = Daemon1::status_changed(emitter, health.state.as_str(), health.reason_str()).await;
}

/// Fold a finished job into the facts health is computed from.
fn update_health(shared: &Shared, kind: JobKind, state: &JobState) {
    match state {
        // A local snapshot counts, and health.md says so: the last success
        // is "the last backup that succeeded anywhere — network, spool, or
        // snapshot". A laptop that has been away for a week and protecting
        // itself hourly is not at risk, and must not be told it is.
        JobState::Done(Outcome::Completed) if kind == JobKind::Backup => {
            shared.record_backup_success(kind);
        }
        JobState::Done(Outcome::Completed) if kind == JobKind::Offline => {
            if shared.record_offline_outcome() {
                shared.record_backup_success(kind);
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
        JobState::Failed(e) => {
            shared.record_error(kind, e);
            if let Some(failure) = e.health_failure() {
                shared.record_blocking_failure(failure);
            }
            if kind == JobKind::Backup
                && matches!(e, backtrack_core::engine::EngineError::BorgFailed { .. })
            {
                *shared.unexplained_failures.lock().unwrap() += 1;
            }
        }
        // The catalogue has been read again, as far as the repository goes.
        // Anything that would not catalogue is left pending, and is the
        // "not yet browsable" state's to report rather than this one's.
        JobState::Done(Outcome::Completed) if kind == JobKind::Index => {
            *shared.catalogue_rebuilding.lock().unwrap() = false;
        }
        // A check that found nothing wrong is the proof a damaged repository
        // is not damaged any more, whether a repair made it so or the damage
        // was never in the repository at all.
        JobState::Done(Outcome::Completed) if kind == JobKind::Check => {
            shared.resolved(&[HealthFailure::RepoCorrupt]);
        }
        _ => {}
    }
}

/// Which part of the daemon a job belongs to, for the last-error record.
fn subsystem(kind: JobKind) -> &'static str {
    match kind {
        JobKind::Backup => "backup",
        JobKind::Offline => "local",
        JobKind::Index => "catalogue",
        JobKind::Check => "check",
        JobKind::Prune | JobKind::Compact => "maintenance",
        JobKind::Restore | JobKind::RestoreEverything => "restore",
    }
}

/// One passphrase, held for one question and never stored: how a passphrase
/// someone has just typed is tried before it is kept anywhere.
struct Trial(String);

#[async_trait::async_trait]
impl SecretStore for Trial {
    async fn get(&self, _repo_id: &str) -> backtrack_core::engine::Result<String> {
        Ok(self.0.clone())
    }

    async fn set(&self, _repo_id: &str, _passphrase: &str) -> backtrack_core::engine::Result<()> {
        Err(backtrack_core::engine::EngineError::Local(
            "a passphrase being tried is not stored".into(),
        ))
    }

    async fn delete(&self, _repo_id: &str) -> backtrack_core::engine::Result<()> {
        Err(backtrack_core::engine::EngineError::Local(
            "a passphrase being tried is not stored".into(),
        ))
    }
}

/// Rename a damaged repository beside where it was, and return its new name.
///
/// A rename in the same folder, so nothing is copied and nothing can be lost
/// on the way: the files are the same files under a new name. The name says
/// what it is and when it was put there, and never replaces anything already
/// by that name.
fn set_aside(repository: &std::path::Path, now: SystemTime) -> Result<PathBuf> {
    let name = repository
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| DaemonError::InvalidArgument("the repository has no name".into()))?;
    let stamp = pipeline::iso8601_basic_at(now);
    let aside = (1..)
        .map(|n| {
            let suffix = if n == 1 {
                String::new()
            } else {
                format!("-{n}")
            };
            repository.with_file_name(format!("{name}.damaged-{stamp}{suffix}"))
        })
        .find(|candidate| !candidate.exists())
        .expect("an unbounded search finds a free name");
    std::fs::rename(repository, &aside).map_err(|e| {
        DaemonError::RestoreFailed(format!(
            "{} could not be put aside: {e}",
            repository.display()
        ))
    })?;
    Ok(aside)
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
        upload_limit_kib: config.advanced.upload_limit_mbps.map(megabytes_to_kib),
        one_file_system: true,
    }
}

/// Preferences' upload limit, in the KiB/s Borg counts in.
fn megabytes_to_kib(megabytes: u32) -> u32 {
    (u64::from(megabytes) * 1_000_000 / 1024).min(u64::from(u32::MAX)) as u32
}

/// The size and free space of the filesystem holding `repository`, or zeros
/// where it is not on this computer.
fn filesystem_space(repository: &str) -> (u64, u64) {
    let path = std::path::Path::new(repository);
    if !path.is_absolute() {
        return (0, 0);
    }
    let Some(existing) = path.ancestors().find(|p| p.exists()) else {
        return (0, 0);
    };
    match rustix::fs::statvfs(existing) {
        Ok(stat) => (
            stat.f_blocks.saturating_mul(stat.f_frsize),
            stat.f_bavail.saturating_mul(stat.f_frsize),
        ),
        Err(_) => (0, 0),
    }
}

/// The retention policy in force. "Automatic" means the recommended ladder,
/// whatever counts are stored beside it: those are kept only so that turning
/// automatic off again brings back what the person had set.
fn prune_policy(config: &Config) -> PrunePolicy {
    let r = if config.storage.retention.automatic {
        backtrack_core::config::Retention::default()
    } else {
        config.storage.retention
    };
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
        config.storage.retention.automatic = false;
        config.storage.retention.keep_daily = 14;
        let policy = prune_policy(&config);
        assert_eq!(policy.keep_hourly, 24);
        assert_eq!(policy.keep_daily, 14);
        assert_eq!(policy.keep_monthly, 6);
    }

    #[test]
    fn automatic_retention_is_the_recommended_ladder_whatever_is_stored() {
        // The custom counts survive turning automatic on, so that turning it
        // off again gives them back, but they are not what prune applies.
        let mut config = Config::default();
        config.storage.retention.keep_daily = 14;
        assert!(config.storage.retention.automatic);
        assert_eq!(prune_policy(&config).keep_daily, 7);
    }

    #[test]
    fn the_upload_limit_reaches_borg_in_its_own_units() {
        let mut config = Config::default();
        assert_eq!(create_spec(&config).upload_limit_kib, None);
        config.advanced.upload_limit_mbps = Some(10);
        assert_eq!(create_spec(&config).upload_limit_kib, Some(9_765));
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

    fn failed(error: backtrack_core::engine::EngineError) -> JobState {
        JobState::Failed(error)
    }

    #[test]
    fn a_failure_outside_the_catalogue_changes_nothing() {
        // The 14:00 failure of health.md's principle 2: a lock, a network
        // blip, Borg saying something nobody classified. None of it is news.
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        shared.record_backup_success(JobKind::Backup);
        for error in [
            backtrack_core::engine::EngineError::LockedByOther,
            backtrack_core::engine::EngineError::RepoUnreachable,
            backtrack_core::engine::EngineError::BorgFailed {
                code: 2,
                stderr: "something".into(),
            },
        ] {
            update_health(&shared, JobKind::Backup, &failed(error));
            assert_eq!(shared.health(), Health::HEALTHY);
        }
    }

    #[test]
    fn a_catalogue_failure_is_broken_with_its_row_until_a_backup_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        update_health(
            &shared,
            JobKind::Backup,
            &failed(backtrack_core::engine::EngineError::DestinationFull),
        );
        let health = shared.health();
        assert_eq!(health.state, HealthState::Broken);
        assert_eq!(health.reason, Some(Reason::DestinationFull));

        update_health(
            &shared,
            JobKind::Backup,
            &JobState::Done(Outcome::Completed),
        );
        assert_eq!(shared.health(), Health::HEALTHY);
    }

    #[test]
    fn a_local_backup_clears_only_the_failures_it_went_through() {
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());

        shared.record_blocking_failure(HealthFailure::DestinationFull);
        shared.record_backup_success(JobKind::Offline);
        assert_eq!(
            shared.health().reason,
            Some(Reason::DestinationFull),
            "a snapshot on this computer says nothing about the destination"
        );

        shared.record_blocking_failure(HealthFailure::PassphraseMissing);
        shared.record_backup_success(JobKind::Offline);
        assert_eq!(
            shared.health(),
            Health::HEALTHY,
            "the snapshot needed the passphrase, so it is back"
        );
    }

    /// Make the local safety net look `bytes` big without using the disk.
    fn spool_of(dir: &std::path::Path, bytes: u64) {
        std::fs::create_dir_all(dir.join("spool/data/0")).unwrap();
        std::fs::File::create(dir.join("spool/data/0/1"))
            .unwrap()
            .set_len(bytes)
            .unwrap();
    }

    #[test]
    fn a_safety_net_over_its_limit_is_degraded_for_as_long_as_it_is_over() {
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        shared.record_backup_success(JobKind::Backup);
        shared.config.lock().unwrap().storage.offline.space_limit_gb = 1;
        spool_of(dir.path(), 1_200 * 1024 * 1024);

        // Found in the Stage 10 drill: a run with nothing to save used to
        // clear this, a minute after it was set.
        for _ in 0..2 {
            let health = shared.health();
            assert_eq!(health.state, HealthState::Degraded);
            assert_eq!(health.reason, Some(Reason::LocalDiskFull));
            update_health(
                &shared,
                JobKind::Offline,
                &JobState::Done(Outcome::Completed),
            );
        }

        shared.config.lock().unwrap().storage.offline.space_limit_gb = 5;
        assert_eq!(
            shared.health(),
            Health::HEALTHY,
            "raising the limit is the fix"
        );
    }

    #[test]
    fn away_from_the_destination_an_over_limit_safety_net_is_still_the_quiet_line() {
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        shared.record_backup_success(JobKind::Backup);
        shared.config.lock().unwrap().storage.offline.space_limit_gb = 1;
        spool_of(dir.path(), 1_200 * 1024 * 1024);
        *shared.destination_reachable.lock().unwrap() = false;
        assert_eq!(shared.health().state, HealthState::ProtectedLocally);
    }

    #[test]
    fn a_change_the_safety_net_held_back_is_cleared_by_reaching_the_destination() {
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        *shared.spool_held.lock().unwrap() = true;
        assert_eq!(shared.health().reason, Some(Reason::LocalDiskFull));
        assert_eq!(shared.health().state, HealthState::Broken);

        shared.record_backup_success(JobKind::Offline);
        assert_eq!(
            shared.health().state,
            HealthState::Broken,
            "a local snapshot of something else does not protect it"
        );
        shared.record_backup_success(JobKind::Backup);
        assert_eq!(shared.health(), Health::HEALTHY);
    }

    #[test]
    fn a_disk_too_full_to_back_up_is_broken_and_a_low_one_degraded() {
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        shared.record_backup_success(JobKind::Backup);

        *shared.local_disk_low.lock().unwrap() = true;
        let health = shared.health();
        assert_eq!(health.state, HealthState::Degraded);
        assert_eq!(health.reason, Some(Reason::LocalDiskFull));

        *shared.local_disk_full.lock().unwrap() = true;
        let health = shared.health();
        assert_eq!(health.state, HealthState::Broken);
        assert_eq!(health.reason, Some(Reason::LocalDiskFull));

        // Measured, not latched: the next look that finds room clears it.
        *shared.local_disk_full.lock().unwrap() = false;
        *shared.local_disk_low.lock().unwrap() = false;
        assert_eq!(shared.health(), Health::HEALTHY);
    }

    #[test]
    fn a_backup_not_yet_catalogued_is_degraded_only_once_nothing_is_cataloguing() {
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        shared.record_backup_success(JobKind::Backup);
        *shared.pending.lock().unwrap() = 1;
        assert_eq!(shared.health().reason, Some(Reason::NotYetBrowsable));

        // The same count while a backup is on its way through is just the
        // pipeline doing its work.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let job = rt.block_on(shared.submit_backup()).unwrap();
        assert_eq!(shared.health(), Health::HEALTHY);
        shared.jobs.cancel(job).unwrap();
    }

    #[test]
    fn nothing_to_back_up_is_never_late() {
        // What an import leaves a new computer with: a destination full of
        // somebody's history, and no folders of its own chosen yet.
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        shared.config.lock().unwrap().backup.include.clear();
        *shared.last_backup.lock().unwrap() =
            Some(SystemTime::now() - Duration::from_secs(30 * 86_400));
        assert_eq!(shared.health(), Health::HEALTHY);
    }

    #[test]
    fn transitions_are_recorded_once_and_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        shared.record_backup_success(JobKind::Backup);
        let now = SystemTime::now();
        assert_eq!(shared.reassess(now), Some(Health::HEALTHY));
        assert_eq!(shared.reassess(now), None, "no change, nothing to announce");

        shared.record_blocking_failure(HealthFailure::RepoCorrupt);
        let broken = shared.reassess(now + Duration::from_secs(60)).unwrap();
        assert_eq!(broken.reason, Some(Reason::RepoCorrupt));
        let since = to_epoch(Some(now + Duration::from_secs(60)));
        assert_eq!(
            shared.state_since(broken, now + Duration::from_secs(600)),
            since
        );

        let restarted = configured(dir.path());
        restarted.restore_persisted_state();
        restarted.record_blocking_failure(HealthFailure::RepoCorrupt);
        restarted.reassess(now + Duration::from_secs(120));
        let history = restarted.persisted.lock().unwrap().health_history.clone();
        let states: Vec<(&str, &str)> = history
            .iter()
            .map(|t| (t.state.as_str(), t.reason.as_str()))
            .collect();
        assert_eq!(
            states,
            vec![("HEALTHY", ""), ("BROKEN", "repo-corrupt")],
            "a restart into the same state is not a transition"
        );
        assert_eq!(
            restarted.state_since(broken, now + Duration::from_secs(600)),
            since,
            "the state began before the restart, not at it"
        );
    }

    #[test]
    fn a_restart_does_not_repeat_what_has_already_been_said() {
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        let now = SystemTime::now();
        shared.record_blocking_failure(HealthFailure::PassphraseMissing);
        shared.reassess(now);
        let told = shared.persisted.lock().unwrap().notified.clone();
        assert_eq!(told.broken_reason.as_deref(), Some("passphrase-missing"));

        let restarted = configured(dir.path());
        restarted.restore_persisted_state();
        restarted.record_blocking_failure(HealthFailure::PassphraseMissing);
        restarted.reassess(now + Duration::from_secs(3_600));
        assert_eq!(
            restarted.persisted.lock().unwrap().notified,
            told,
            "an hour later, after a restart, is still inside the day"
        );
    }

    #[tokio::test]
    async fn a_first_folder_starts_the_clock_that_lateness_is_measured_on() {
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        shared.config.lock().unwrap().backup.include.clear();
        assert_eq!(shared.persisted.lock().unwrap().protected_since, None);

        let daemon = Daemon1::new(Arc::clone(&shared));
        let (_server, client) = p2p().await;
        daemon
            .set_config(
                "backup.include",
                &format!("[{:?}]", dir.path().join("src").display().to_string()),
                &client,
            )
            .await
            .unwrap();
        assert!(shared.persisted.lock().unwrap().protected_since.is_some());
    }

    /// A notification sink that remembers what it was given.
    #[derive(Default)]
    struct Recorder {
        shown: Mutex<Vec<crate::notify::Note>>,
        withdrawn: Mutex<Vec<crate::notify::Topic>>,
    }

    impl crate::notify::NotificationSink for Recorder {
        fn show(&self, note: crate::notify::Note) {
            self.shown.lock().unwrap().push(note);
        }
        fn withdraw(&self, topic: crate::notify::Topic) {
            self.withdrawn.lock().unwrap().push(topic);
        }
    }

    impl Recorder {
        fn titles(&self) -> Vec<String> {
            self.shown
                .lock()
                .unwrap()
                .iter()
                .map(|note| note.title.clone())
                .collect()
        }
    }

    fn recorded(dir: &std::path::Path) -> (Arc<Shared>, Arc<Recorder>) {
        let shared = configured(dir);
        let recorder = Arc::new(Recorder::default());
        shared.set_notifier(Arc::clone(&recorder) as Arc<dyn crate::notify::NotificationSink>);
        (shared, recorder)
    }

    #[test]
    fn the_notifications_a_person_gets_are_the_ones_they_chose() {
        use backtrack_core::config::Notifications;
        let done = JobState::Done(Outcome::Completed);
        for (policy, expected) in [
            (
                Notifications::AttentionOnly,
                vec!["Your first backup is complete", "The backup drive is full."],
            ),
            (
                Notifications::All,
                vec![
                    "Your first backup is complete",
                    "Backup complete",
                    "The backup drive is full.",
                ],
            ),
            (Notifications::None, vec![]),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (shared, recorder) = recorded(dir.path());
            shared.config.lock().unwrap().general.notifications = policy;

            // The first backup, then an ordinary one, then a real problem.
            *shared.first_backup.lock().unwrap() = Some(1);
            shared.job_ended(1, JobKind::Backup, &done);
            shared.job_ended(2, JobKind::Backup, &done);
            shared.job_ended(
                3,
                JobKind::Backup,
                &failed(backtrack_core::engine::EngineError::DestinationFull),
            );
            shared.reassess(SystemTime::now());

            assert_eq!(recorder.titles(), expected, "{policy:?}");
        }
    }

    #[test]
    fn a_local_snapshot_and_a_failed_backup_get_no_success_note() {
        let dir = tempfile::tempdir().unwrap();
        let (shared, recorder) = recorded(dir.path());
        shared.config.lock().unwrap().general.notifications =
            backtrack_core::config::Notifications::All;
        shared.job_ended(1, JobKind::Offline, &JobState::Done(Outcome::Completed));
        shared.job_ended(
            2,
            JobKind::Backup,
            &failed(backtrack_core::engine::EngineError::LockedByOther),
        );
        assert!(recorder.titles().is_empty(), "{:?}", recorder.titles());
    }

    #[test]
    fn a_notice_opens_the_fix_for_what_it_is_about() {
        let dir = tempfile::tempdir().unwrap();
        let (shared, recorder) = recorded(dir.path());
        shared.record_blocking_failure(HealthFailure::PassphraseWrong);
        shared.reassess(SystemTime::now());
        let shown = recorder.shown.lock().unwrap().clone();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].fix, Some(Reason::PassphraseWrong));
        assert_eq!(shown[0].topic, crate::notify::Topic::Health);
    }

    #[test]
    fn a_fixed_problem_takes_its_notification_down() {
        let dir = tempfile::tempdir().unwrap();
        let (shared, recorder) = recorded(dir.path());
        let now = SystemTime::now();
        shared.record_blocking_failure(HealthFailure::AuthExpired);
        shared.reassess(now);
        assert!(recorder.withdrawn.lock().unwrap().is_empty());

        shared.record_backup_success(JobKind::Backup);
        shared.reassess(now + Duration::from_secs(60));
        assert_eq!(
            *recorder.withdrawn.lock().unwrap(),
            vec![crate::notify::Topic::Health]
        );
    }

    #[test]
    fn the_sign_in_notice_names_the_destination() {
        let dir = tempfile::tempdir().unwrap();
        let (shared, recorder) = recorded(dir.path());
        shared.config.lock().unwrap().storage.repository =
            Some("ssh://keith@nas.local/./backups".into());
        shared.record_blocking_failure(HealthFailure::AuthExpired);
        shared.reassess(SystemTime::now());
        assert_eq!(
            recorder.titles(),
            vec!["Backtrack can't sign in to nas.local."]
        );
    }

    #[tokio::test]
    async fn local_storage_says_where_the_room_went_and_the_reductions_free_it() {
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        let daemon = Daemon1::new(Arc::clone(&shared));

        std::fs::create_dir_all(dir.path().join("spool")).unwrap();
        std::fs::write(dir.path().join("spool/segment"), vec![0u8; 1000]).unwrap();
        let now = to_epoch(Some(SystemTime::now()));
        let old = dir.path().join(format!("replaced/{}/home/k", now - 86_400));
        let recent = dir.path().join(format!("replaced/{}/home/k", now - 60));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&recent).unwrap();
        std::fs::write(old.join("report.odt"), vec![0u8; 500]).unwrap();
        std::fs::write(recent.join("notes.txt"), vec![0u8; 300]).unwrap();
        shared.preview.ensure_dir().unwrap();
        std::fs::write(shared.preview.entry_path("a1", "photo.jpg"), vec![0u8; 200]).unwrap();

        let storage = daemon.get_local_storage().await.unwrap();
        assert_eq!(storage.snapshots, 1000);
        assert_eq!(storage.snapshot_limit, 10 * 1024 * 1024 * 1024);
        assert_eq!(storage.stash, 800);
        assert_eq!(storage.cache, 200);
        assert!(storage.free > 0, "a real filesystem has room to report");

        assert_eq!(daemon.clear_preview_cache().await.unwrap(), 200);
        assert_eq!(
            daemon.empty_stash().await.unwrap(),
            500,
            "the last hour's restore is kept: its Undo may still be on screen"
        );
        let storage = daemon.get_local_storage().await.unwrap();
        assert_eq!((storage.stash, storage.cache), (300, 0));
        assert!(recent.join("notes.txt").exists());
    }

    #[tokio::test]
    async fn a_forced_state_is_shown_and_announced_until_it_is_lifted() {
        let dir = tempfile::tempdir().unwrap();
        let (shared, recorder) = recorded(dir.path());
        shared.record_backup_success(JobKind::Backup);
        shared.reassess(SystemTime::now());
        let dev = Dev::new(Arc::clone(&shared));

        dev.force_health("BROKEN", "auth-failed").await.unwrap();
        let health = shared.health();
        assert_eq!(health.state, HealthState::Broken);
        assert_eq!(health.reason, Some(Reason::AuthFailed));
        assert!(shared.reassess(SystemTime::now()).is_some());
        assert_eq!(
            recorder.shown.lock().unwrap().len(),
            1,
            "with its notification"
        );

        assert!(dev.force_health("SIDEWAYS", "").await.is_err());
        assert!(dev.force_health("AT_RISK", "no-such-reason").await.is_err());

        dev.force_health("", "").await.unwrap();
        assert_eq!(shared.health(), Health::HEALTHY);
    }

    #[test]
    fn the_health_report_has_the_history_the_errors_and_what_was_said() {
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        let now = SystemTime::now();
        shared.record_backup_success(JobKind::Backup);
        shared.reassess(now);
        update_health(
            &shared,
            JobKind::Backup,
            &failed(backtrack_core::engine::EngineError::DestinationFull),
        );
        shared.reassess(now + Duration::from_secs(60));

        let report = shared.health_report();
        assert_eq!(report.state, "BROKEN");
        assert_eq!(report.reason, "destination-full");
        let states: Vec<&str> = report.history.iter().map(|(_, s, _)| s.as_str()).collect();
        assert_eq!(states, vec!["HEALTHY", "BROKEN"]);
        assert_eq!(report.errors.len(), 1);
        let (subsystem, _, reason, message) = &report.errors[0];
        assert_eq!(
            (subsystem.as_str(), reason.as_str(), message.as_str()),
            (
                "backup",
                "destination-full",
                "the backup destination is full"
            )
        );
        assert_eq!(report.broken_notified_reason, "destination-full");
        assert!(report.broken_notified > 0);
    }

    #[test]
    fn a_repository_is_put_aside_beside_itself_and_never_over_anything() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join("data")).unwrap();
        std::fs::write(repo.join("config"), b"the repository").unwrap();
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000);
        let stamp = pipeline::iso8601_basic_at(at);
        std::fs::create_dir(dir.path().join(format!("repo.damaged-{stamp}"))).unwrap();

        let aside = set_aside(&repo, at).unwrap();
        assert_eq!(aside, dir.path().join(format!("repo.damaged-{stamp}-2")));
        assert!(!repo.exists());
        assert_eq!(
            std::fs::read(aside.join("config")).unwrap(),
            b"the repository"
        );
        assert!(
            dir.path().join(format!("repo.damaged-{stamp}")).exists(),
            "what already had the first name is left alone"
        );
    }

    fn check_jobs(shared: &Shared) -> usize {
        shared
            .jobs
            .list()
            .iter()
            .filter(|job| job.kind == JobKind::Check)
            .count()
    }

    #[test]
    fn the_routine_check_comes_round_monthly_on_a_mock_clock() {
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        let day = Duration::from_secs(86_400);
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000);

        assert_eq!(
            shared.maybe_check(start),
            None,
            "the first look starts the clock"
        );
        assert_eq!(
            shared.persisted.lock().unwrap().last_check,
            to_epoch(Some(start)).into()
        );
        assert_eq!(shared.maybe_check(start + 29 * day), None);
        assert_eq!(check_jobs(&shared), 0);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let job = shared
            .maybe_check(start + 30 * day)
            .expect("a month on, it runs");
        assert_eq!(check_jobs(&shared), 1);
        assert_eq!(
            shared.persisted.lock().unwrap().last_check,
            to_epoch(Some(start + 30 * day)).into(),
            "and the next is a month from now"
        );
        let _ = shared.jobs.cancel(job);
    }

    #[test]
    fn backups_failing_for_no_known_reason_bring_the_check_forward() {
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        let now = SystemTime::now();
        shared.update_persisted(|state| state.last_check = to_epoch(Some(now)).into());
        let unexplained = backtrack_core::engine::EngineError::BorgFailed {
            code: 2,
            stderr: "something nobody classified".into(),
        };
        for _ in 0..UNEXPLAINED_BEFORE_CHECK - 1 {
            update_health(&shared, JobKind::Backup, &failed(unexplained.clone()));
        }
        assert_eq!(shared.maybe_check(now), None, "two is not yet a pattern");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        update_health(&shared, JobKind::Backup, &failed(unexplained.clone()));
        let job = shared.maybe_check(now).expect("the third is");
        let _ = shared.jobs.cancel(job);
        assert_eq!(*shared.unexplained_failures.lock().unwrap(), 0);

        // A success in between starts the count again.
        update_health(&shared, JobKind::Backup, &failed(unexplained.clone()));
        update_health(&shared, JobKind::Backup, &failed(unexplained.clone()));
        shared.record_backup_success(JobKind::Backup);
        update_health(&shared, JobKind::Backup, &failed(unexplained));
        assert_eq!(shared.maybe_check(now), None);
    }

    #[test]
    fn a_destination_that_is_away_is_not_checked() {
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        let now = SystemTime::now();
        shared.update_persisted(|state| {
            state.last_check = to_epoch(Some(now - Duration::from_secs(60 * 86_400))).into()
        });
        *shared.destination_reachable.lock().unwrap() = false;
        assert_eq!(shared.maybe_check(now), None);
    }

    #[test]
    fn a_catalogue_being_rebuilt_is_degraded_until_the_catalogue_job_ends() {
        let dir = tempfile::tempdir().unwrap();
        let shared = configured(dir.path());
        shared.record_backup_success(JobKind::Backup);
        shared.start_catalogue_rebuild();
        let health = shared.health();
        assert_eq!(health.state, HealthState::Degraded);
        assert_eq!(health.reason, Some(Reason::CatalogueRebuilding));

        update_health(&shared, JobKind::Index, &JobState::Done(Outcome::Completed));
        assert_eq!(shared.health(), Health::HEALTHY);
    }

    /// A connection to hand methods that want one. Nothing is served on it.
    async fn p2p() -> (zbus::Connection, zbus::Connection) {
        let guid = zbus::Guid::generate();
        let (a, b) = tokio::net::UnixStream::pair().unwrap();
        let server = zbus::connection::Builder::unix_stream(a)
            .server(guid)
            .unwrap()
            .p2p()
            .build();
        let client = zbus::connection::Builder::unix_stream(b).p2p().build();
        let (server, client) = futures::join!(server, client);
        (server.unwrap(), client.unwrap())
    }

    #[test]
    fn index_kinds_have_wire_names() {
        assert_eq!(kind_name(Kind::File), "file");
        assert_eq!(kind_name(Kind::Dir), "dir");
        assert_eq!(kind_name(Kind::Symlink), "symlink");
    }
}
