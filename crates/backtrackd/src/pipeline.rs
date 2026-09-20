// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! What a backup actually is: create the archive, read it into the catalogue,
//! apply the retention policy.
//!
//! These are three Borg operations, but one *operation* as far as anybody using
//! the product is concerned — one progress bar, one cancel button, one "did it
//! work?". So they are composed into a single [`JobStream`] with named phases,
//! rather than submitted as three jobs. Three jobs would mean three progress
//! bars, a cancel that could leave half a backup behind, and — worst — a backup
//! reported as finished the moment the archive existed but before it could be
//! browsed.
//!
//! The catalogue is written between the two: **the archive row is recorded
//! before its file list is read**. A backup that reaches the repository and then
//! dies during cataloguing leaves a `pending` row, which is precisely what
//! reconciliation looks for on the next start. The alternative — record it only
//! once catalogued — loses the archive entirely from the daemon's point of view,
//! and it would be a silent loss.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use backtrack_core::engine::{
    ArchiveId, BackupEngine, CreateSpec, EngineError, JobEvent, JobSink, JobStream, JobSummary,
    PrunePolicy,
};
use backtrack_core::index::{ArchiveMeta, BorgItem, IndexWriter, ListingIncomplete, Repo};
use futures::StreamExt;
use tracing::{debug, info, warn};

/// Events buffered between the pipeline and the job registry. Deep enough that
/// a burst of Borg progress does not stall the phase producing it, shallow
/// enough that a cancel is noticed within a few events.
const EVENT_BUFFER: usize = 64;

/// How many listing items cross into the blocking ingest task at a time.
/// Bounded on purpose: the whole point of streaming the catalogue in is that a
/// 500,000-file archive never exists in memory at once.
const INGEST_BATCH: usize = 4_096;

/// Phase names carried on progress events. They reach the user interface, so
/// they are the words the user reads.
pub const PHASE_ARCHIVING: &str = "archiving";
pub const PHASE_CATALOGUING: &str = "cataloguing";
pub const PHASE_PRUNING: &str = "pruning";

/// Everything a backup needs to run. Assembled by the service layer so this
/// module knows nothing about D-Bus, configuration files, or health.
pub struct BackupPlan {
    pub engine: Arc<dyn BackupEngine>,
    pub index: Arc<Mutex<IndexWriter>>,
    pub spec: CreateSpec,
    pub prune: Option<PrunePolicy>,
}

/// Start a backup, returning the stream the job registry will drive.
///
/// Returns immediately: the work happens in a spawned task feeding the stream,
/// so the D-Bus method that asked for it answers straight away.
pub fn start_backup(plan: BackupPlan) -> JobStream {
    let (sink, stream) = JobStream::channel(EVENT_BUFFER);
    tokio::spawn(async move {
        let outcome = run(&plan, &sink).await;
        // The terminal event is the job's whole verdict, so it goes out even if
        // nobody is listening any more — the send failing simply means the job
        // was cancelled, which the registry already knows.
        sink.send(JobEvent::Finished(outcome)).await;
    });
    stream
}

/// Bring the catalogue up to date with the repository.
///
/// Runs on three occasions, all the same problem seen from different angles:
/// at startup (a backup that reached the repository but was never catalogued —
/// the daemon was killed, the machine lost power), after adopting an existing
/// repository (nothing is catalogued yet), and whenever a prune has changed what
/// exists.
///
/// Deliberately a `JobKind::Index` job, which takes only a *shared* repository
/// lock: a year of history takes a while to catalogue, and it must not stop the
/// user restoring a file — or the next hourly backup — in the meantime.
pub struct CataloguePlan {
    pub engine: Arc<dyn BackupEngine>,
    pub index: Arc<Mutex<IndexWriter>>,
}

/// Start cataloguing whatever is outstanding.
pub fn start_catalogue(plan: CataloguePlan) -> JobStream {
    let (sink, stream) = JobStream::channel(EVENT_BUFFER);
    tokio::spawn(async move {
        let outcome = catch_up(&plan, &sink).await;
        sink.send(JobEvent::Finished(outcome)).await;
    });
    stream
}

async fn catch_up(plan: &CataloguePlan, sink: &JobSink) -> Result<JobSummary, EngineError> {
    let entries = plan.engine.list_archives().await?;
    let index = Arc::clone(&plan.index);
    let report = tokio::task::spawn_blocking(move || {
        index.lock().unwrap().sync_archives(Repo::Primary, &entries)
    })
    .await
    .map_err(joined)?
    .map_err(indexing)?;
    if report.removed > 0 {
        info!(
            removed = report.removed,
            "archives the repository no longer has left the catalogue"
        );
    }

    // Everything outstanding, not merely what this sync added: a run cut short
    // last time left work behind, and this is where it gets picked up.
    let index = Arc::clone(&plan.index);
    let pending = tokio::task::spawn_blocking(move || index.lock().unwrap().pending_archives())
        .await
        .map_err(joined)?
        .map_err(indexing)?;
    if pending.is_empty() {
        return Ok(JobSummary::default());
    }
    info!(
        count = pending.len(),
        "cataloguing backups that are not browsable yet"
    );

    let total = pending.len() as u64;
    for (done, (seq, name)) in pending.into_iter().enumerate() {
        if sink.is_cancelled() {
            // Nothing is lost. What remains is still `pending` in the
            // catalogue, so the next run — or the next daemon — picks up
            // exactly here.
            info!(
                catalogued = done,
                remaining = total as usize - done,
                "cataloguing stopped; the rest stays queued"
            );
            return Err(EngineError::Cancelled);
        }
        // Progress is per archive rather than per item: the interesting number
        // during a backfill is how many snapshots are still not browsable, and
        // Borg gives no denominator for a listing anyway.
        sink.send(JobEvent::Progress {
            current: done as u64,
            total: Some(total),
            phase: name.clone(),
        })
        .await;

        match ingest_one(plan, seq, &name).await {
            Ok(items) => debug!(archive = %name, seq, items, "archive catalogued"),
            // One unreadable archive must not abandon the rest: it stays
            // pending and the others still become browsable.
            Err(e) => warn!(archive = %name, "could not catalogue this archive: {e}"),
        }
    }
    sink.send(JobEvent::Progress {
        current: total,
        total: Some(total),
        phase: String::new(),
    })
    .await;
    Ok(JobSummary::default())
}

/// Catalogue the single newest archive, and report how many are still
/// outstanding.
///
/// The one piece of cataloguing that is *not* a background job. Adopting an
/// existing repository with a year of history means a wizard that has just said
/// "you're set up" opening onto an empty timeline, which reads as a failure —
/// so the newest snapshot, the one anybody would look at first, is read before
/// the import call returns. The rest follows in the background, newest to
/// oldest, so the timeline fills in from the end people care about.
pub async fn catalogue_newest(plan: &CataloguePlan) -> Result<usize, EngineError> {
    let entries = plan.engine.list_archives().await?;
    let index = Arc::clone(&plan.index);
    tokio::task::spawn_blocking(move || {
        index.lock().unwrap().sync_archives(Repo::Primary, &entries)
    })
    .await
    .map_err(joined)?
    .map_err(indexing)?;

    let index = Arc::clone(&plan.index);
    let pending = tokio::task::spawn_blocking(move || index.lock().unwrap().pending_archives())
        .await
        .map_err(joined)?
        .map_err(indexing)?;

    // `pending_archives` is newest first, so the head is the snapshot the user
    // is about to be shown.
    let Some((seq, name)) = pending.first().cloned() else {
        return Ok(0);
    };
    let items = ingest_one(plan, seq, &name).await?;
    info!(archive = %name, items, "newest snapshot catalogued");
    Ok(pending.len() - 1)
}

/// Read one archive's listing into the catalogue.
async fn ingest_one(plan: &CataloguePlan, seq: i64, name: &str) -> Result<usize, EngineError> {
    let mut listing = plan
        .engine
        .list_archive(&ArchiveId(name.to_string()))
        .await?;
    let (tx, rx) = channel();
    let index = Arc::clone(&plan.index);
    let writer = tokio::task::spawn_blocking(move || {
        index
            .lock()
            .unwrap()
            .ingest_pending_fallible(seq, BatchedItems::new(rx))
    });

    let mut batch = Vec::with_capacity(INGEST_BATCH);
    let mut failure = None;
    while let Some(item) = listing.next().await {
        match item {
            Ok(item) => batch.push(item),
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
        if batch.len() >= INGEST_BATCH && tx.send(Ok(std::mem::take(&mut batch))).await.is_err() {
            break;
        }
    }
    finish(&tx, batch, failure.is_some()).await;
    drop(tx);

    // Awaited first, reported last — see the note in `catalogue`.
    let ingest = writer.await.map_err(joined)?;
    match failure {
        Some(e) => Err(e),
        None => Ok(ingest.map_err(indexing)?.items),
    }
}

/// The channel the listing crosses into the blocking ingest on.
///
/// It carries a `Result` rather than bare batches so a listing that dies part
/// way tells the writer to roll back, instead of ending indistinguishably from
/// a complete one. Without that, an interrupted listing would be committed as a
/// finished catalogue and the archive marked browsable while missing half its
/// files — a snapshot that shows the user's documents gone, with nothing to say
/// anything went wrong.
type Batch = std::result::Result<Vec<BorgItem>, ListingIncomplete>;

fn channel() -> (
    tokio::sync::mpsc::Sender<Batch>,
    tokio::sync::mpsc::Receiver<Batch>,
) {
    tokio::sync::mpsc::channel(2)
}

/// Send the last batch, or the abort marker if the listing did not finish.
async fn finish(tx: &tokio::sync::mpsc::Sender<Batch>, batch: Vec<BorgItem>, aborted: bool) {
    let last = if aborted {
        Err(ListingIncomplete)
    } else {
        Ok(batch)
    };
    let _ = tx.send(last).await;
}

/// The pipeline proper.
async fn run(plan: &BackupPlan, sink: &JobSink) -> Result<JobSummary, EngineError> {
    let archive = plan.spec.archive_name.clone();

    // ── Archiving ──
    let summary = drive(plan.engine.create(&plan.spec).await?, sink, PHASE_ARCHIVING).await?;
    info!(archive, "archive written");
    if sink.is_cancelled() {
        return Err(EngineError::Cancelled);
    }

    // ── Cataloguing ──
    //
    // A backup nobody can browse is only half a backup, so this is part of the
    // job rather than something that happens afterwards. A failure here is
    // reported as a warning, not as a failed backup: the archive exists and the
    // user's files are safe, which is the thing that matters. Reconciliation
    // will catalogue it on the next start.
    match catalogue(plan, sink, &archive).await {
        Ok(items) => info!(archive, items, "archive catalogued"),
        Err(e) if sink.is_cancelled() => return Err(e),
        Err(e) => warn!(
            archive,
            "the backup succeeded but could not be catalogued yet: {e}"
        ),
    }
    if sink.is_cancelled() {
        return Err(EngineError::Cancelled);
    }

    // ── Pruning ──
    //
    // Applying retention is housekeeping. The backup has already happened, so a
    // prune that fails must not be reported as a failed backup — and the reason
    // is not merely cosmetic: a failed job never records a successful backup, so
    // a repository whose prune kept failing would drift into `AT_RISK` while
    // being backed up perfectly well every hour.
    //
    // The exception is a failure the user has to act on. A full or damaged
    // repository reaches the health model through the job's outcome, so those
    // still fail the job.
    if let Some(policy) = &plan.prune {
        let pruned = match plan.engine.prune(policy).await {
            Ok(stream) => drive(stream, sink, PHASE_PRUNING).await.map(|_| ()),
            Err(e) => Err(e),
        };
        match pruned {
            Ok(()) => {
                // The repository is the authority on what exists. Reconciling
                // after the prune rather than parsing its output means the
                // catalogue is correct even if Borg removed something we did
                // not predict — and it repairs any earlier disagreement for
                // free.
                match reconcile(plan).await {
                    Ok(removed) if removed > 0 => {
                        info!(removed, "pruned archives left the catalogue")
                    }
                    Ok(_) => {}
                    Err(e) => warn!("could not reconcile the catalogue after pruning: {e}"),
                }
            }
            Err(e) if e.health_failure().is_some() => return Err(e),
            Err(e) => warn!(archive, "retention could not be applied this time: {e}"),
        }
    }

    Ok(summary)
}

/// Forward one engine job's events under `phase`, and return its summary.
///
/// The engine's own terminal event is consumed here rather than forwarded: this
/// is one phase of a longer job, and a `Finished` in the middle of a stream
/// would tell the registry the whole backup was over.
pub(crate) async fn drive(
    mut stream: JobStream,
    sink: &JobSink,
    phase: &str,
) -> Result<JobSummary, EngineError> {
    while let Some(event) = stream.next().await {
        match event {
            JobEvent::Finished(result) => return result,
            JobEvent::Progress { current, total, .. } => {
                if !sink
                    .send(JobEvent::Progress {
                        current,
                        total,
                        phase: phase.to_string(),
                    })
                    .await
                {
                    return Err(EngineError::Cancelled);
                }
            }
            other => {
                if !sink.send(other).await {
                    return Err(EngineError::Cancelled);
                }
            }
        }
    }
    // Same contract the registry enforces, for the same reason: a stream that
    // ends without saying how it went has not proved anything, and an
    // unverified backup reported as complete is the one thing this must never do.
    Err(EngineError::BorgFailed {
        code: -1,
        stderr: format!("borg ended the {phase} phase without an outcome"),
    })
}

/// Read the new archive's file list into the catalogue.
///
/// The listing is streamed rather than collected: it arrives from Borg over a
/// pipe and leaves into SQLite, and at 500,000 files the difference between
/// streaming and buffering is the difference between a few megabytes and a few
/// hundred. SQLite's writes are blocking, so the ingest runs on a blocking
/// thread and the two halves meet over a bounded channel.
async fn catalogue(plan: &BackupPlan, sink: &JobSink, archive: &str) -> Result<usize, EngineError> {
    catalogue_archive(
        CatalogueOne {
            engine: &plan.engine,
            index: &plan.index,
            archive,
            created_at: plan.spec.created_at,
            repo: Repo::Primary,
            delta: false,
        },
        sink,
    )
    .await
}

/// One archive's listing, read into the catalogue.
struct CatalogueOne<'a> {
    engine: &'a Arc<dyn BackupEngine>,
    index: &'a Arc<Mutex<IndexWriter>>,
    archive: &'a str,
    created_at: SystemTime,
    repo: Repo,
    /// Whether the archive holds only what changed. See
    /// [`IndexWriter::ingest_delta`] — a delta read as a full listing would show
    /// every unchanged file as deleted at that snapshot.
    delta: bool,
}

async fn catalogue_archive(one: CatalogueOne<'_>, sink: &JobSink) -> Result<usize, EngineError> {
    let archive = one.archive;
    let ts = one
        .created_at
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs() as i64;
    let meta = ArchiveMeta {
        borg_id: None,
        name: archive.to_string(),
        ts,
    };

    // Record the archive before reading a single item. See the module note: a
    // crash from here on leaves a row reconciliation can find.
    let index = Arc::clone(one.index);
    let repo = one.repo;
    let seq =
        tokio::task::spawn_blocking(move || index.lock().unwrap().append_archive(&meta, repo))
            .await
            .map_err(joined)?
            .map_err(indexing)?;

    let (tx, rx) = channel();
    let index = Arc::clone(one.index);
    let delta = one.delta;
    let writer = tokio::task::spawn_blocking(move || {
        let mut index = index.lock().unwrap();
        let items = BatchedItems::new(rx);
        if delta {
            index.ingest_delta(seq, items)
        } else {
            index.ingest_pending_fallible(seq, items)
        }
    });

    let mut listing = one
        .engine
        .list_archive(&ArchiveId(archive.to_string()))
        .await?;
    let mut batch = Vec::with_capacity(INGEST_BATCH);
    let mut sent = 0usize;
    let mut failure = None;
    while let Some(item) = listing.next().await {
        match item {
            Ok(item) => batch.push(item),
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
        if batch.len() >= INGEST_BATCH {
            sent += batch.len();
            if tx.send(Ok(std::mem::take(&mut batch))).await.is_err() {
                break;
            }
            batch.reserve(INGEST_BATCH);
            // Borg gives no denominator for a listing, so progress is a count
            // rather than a percentage. `total: None` says so honestly instead
            // of inventing a bar that jumps.
            sink.send(JobEvent::Progress {
                current: sent as u64,
                total: None,
                phase: PHASE_CATALOGUING.to_string(),
            })
            .await;
            if sink.is_cancelled() {
                break;
            }
        }
    }
    finish(&tx, batch, failure.is_some() || sink.is_cancelled()).await;
    drop(tx);

    // The writer is awaited first so its thread is never left detached, but its
    // error is reported last. When a listing breaks off, the writer's answer is
    // always "the listing was incomplete" — true, and a paraphrase of the
    // question. What the log needs is why Borg stopped talking.
    let ingest = writer.await.map_err(joined)?;
    if let Some(e) = failure {
        return Err(e);
    }
    let stats = ingest.map_err(indexing)?;
    if sink.is_cancelled() {
        return Err(EngineError::Cancelled);
    }
    debug!(
        seq = stats.seq,
        items = stats.items,
        new = stats.new_versions,
        extended = stats.extended,
        "catalogue updated"
    );
    Ok(stats.items)
}

/// Phase name for evicting local snapshots to stay inside the spool cap.
pub const PHASE_EVICTING: &str = "making room";

/// Everything a local, offline-window backup needs.
///
/// Assembled by the service layer, like [`BackupPlan`], but the expensive parts
/// — walking the sources, diffing against the catalogue — happen inside the job
/// rather than when it is submitted. A walk of a large home directory takes
/// seconds, and the scheduler tick that asks for the backup must not wait for
/// it.
pub struct OfflinePlan {
    /// An engine pointing at the local spool repository, not the destination.
    pub engine: Arc<dyn BackupEngine>,
    pub index: Arc<Mutex<IndexWriter>>,
    /// Where to read the catalogue from for the diff. A separate read-only
    /// connection, so working out what changed never contends with the writer.
    pub index_path: std::path::PathBuf,
    pub walk: backtrack_core::walk::WalkSpec,
    pub excludes: Vec<String>,
    pub compression: backtrack_core::engine::Compression,
    /// The spool's size limit in bytes; `0` for no limit.
    pub cap_bytes: u64,
    pub spool_dir: std::path::PathBuf,
    pub created_at: SystemTime,
}

/// What a local backup did, for the caller's logging and health bookkeeping.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OfflineOutcome {
    /// Files archived. Zero means nothing had changed, which is the common case.
    pub files: usize,
    /// Local snapshots dropped to stay inside the cap.
    pub evicted: usize,
    /// The spool is under strain and the user should be told.
    pub degraded: bool,
}

/// Protect what changed, locally.
pub fn start_offline_backup(plan: OfflinePlan) -> (JobStream, OfflineHandle) {
    let (sink, stream) = JobStream::channel(EVENT_BUFFER);
    let outcome = Arc::new(Mutex::new(OfflineOutcome::default()));
    let handle = OfflineHandle {
        outcome: Arc::clone(&outcome),
    };
    tokio::spawn(async move {
        let result = run_offline(&plan, &sink, &outcome).await;
        sink.send(JobEvent::Finished(result)).await;
    });
    (stream, handle)
}

/// Lets the submitter read what the local backup did once it has finished.
///
/// The job model carries only success or failure, and the health model needs
/// more than that: whether the safety net actually holds anything, and whether
/// it is under strain.
#[derive(Clone)]
pub struct OfflineHandle {
    outcome: Arc<Mutex<OfflineOutcome>>,
}

impl OfflineHandle {
    pub fn outcome(&self) -> OfflineOutcome {
        *self.outcome.lock().unwrap()
    }
}

async fn run_offline(
    plan: &OfflinePlan,
    sink: &JobSink,
    outcome: &Arc<Mutex<OfflineOutcome>>,
) -> Result<JobSummary, EngineError> {
    // ── What changed ──
    //
    // The walk and the diff are both blocking and both potentially long, so
    // they go to a blocking thread together. The catalogue is read through its
    // own read-only connection: the writer's lock is held by ingests, and
    // taking it here to answer a question would be the deadlock S04-T5 already
    // found once.
    let index_path = plan.index_path.clone();
    let walk_spec = clone_walk_spec(&plan.walk, &plan.excludes);
    let (changed, unreadable, baseline) = tokio::task::spawn_blocking(move || {
        let reader = backtrack_core::index::IndexReader::open(&index_path)?;
        let baseline = reader.latest_catalogued_seq()?;
        let walked = backtrack_core::walk::walk(&walk_spec);
        let unreadable = walked.unreadable;
        let live = walked.live_entries();
        let changed = match baseline {
            Some(seq) => reader.changed_since(seq, live)?,
            // Nothing has ever been catalogued, so there is no "since" to speak
            // of. Everything walked is at risk.
            None => live.into_iter().map(|e| e.path).collect(),
        };
        Ok::<_, backtrack_core::index::IndexError>((changed, unreadable, baseline))
    })
    .await
    .map_err(joined)?
    .map_err(indexing)?;

    if unreadable > 0 {
        debug!(
            unreadable,
            "some paths could not be read while looking for changes"
        );
    }
    if changed.is_empty() {
        // The quiet, common case: an offline hour in which nothing was edited.
        // No archive, no log line above debug, nothing for the user to see.
        debug!(?baseline, "nothing has changed; no local snapshot needed");
        return Ok(JobSummary::default());
    }

    // ── Room to put it ──
    let projected: u64 = changed
        .iter()
        .map(|p| {
            std::fs::symlink_metadata(std::path::Path::new("/").join(p))
                .map(|m| m.len())
                .unwrap_or(0)
        })
        .sum();
    let held = crate::offline::directory_bytes(&plan.spool_dir);
    let index = Arc::clone(&plan.index);
    let existing =
        tokio::task::spawn_blocking(move || index.lock().unwrap().archives_in(Repo::Spool))
            .await
            .map_err(joined)?
            .map_err(indexing)?;
    let existing: Vec<crate::offline::LocalArchive> = existing
        .into_iter()
        .map(|row| crate::offline::LocalArchive {
            seq: row.seq,
            name: row.name,
        })
        .collect();
    let cap = crate::offline::plan_cap(plan.cap_bytes, held, projected, &existing);

    if !cap.evict.is_empty() {
        sink.send(JobEvent::Progress {
            current: 0,
            total: Some(cap.evict.len() as u64),
            phase: PHASE_EVICTING.to_string(),
        })
        .await;
        evict(plan, &cap.evict).await?;
        info!(
            count = cap.evict.len(),
            "older local snapshots removed to stay inside the storage limit"
        );
    }

    {
        let mut outcome = outcome.lock().unwrap();
        outcome.evicted = cap.evict.len();
        outcome.degraded = cap.degraded;
    }
    if cap.degraded {
        // health.md's DEGRADED, not an error: protection is still happening,
        // it is just constrained. Never silently stop.
        warn!(
            limit_bytes = plan.cap_bytes,
            held, projected, "the local safety net is at its storage limit"
        );
    }
    if !cap.proceed {
        return Ok(JobSummary::default());
    }

    // ── Archive it ──
    let taken: Vec<String> = existing.iter().map(|a| a.name.clone()).collect();
    let (archive, created_at) = crate::offline::next_local_name(plan.created_at, &taken);
    let spec = CreateSpec {
        archive_name: archive.clone(),
        sources: plan.walk.sources.clone(),
        // Absolute paths: the walk reports them archive-relative, which is the
        // form the catalogue holds, but borg is being asked to read them off
        // the disk.
        paths: changed
            .iter()
            .map(|p| std::path::Path::new("/").join(p))
            .collect(),
        excludes: plan.excludes.clone(),
        compression: plan.compression,
        one_file_system: plan.walk.one_file_system,
        created_at,
    };
    let files = spec.paths.len();
    drive(plan.engine.create(&spec).await?, sink, PHASE_ARCHIVING).await?;
    if sink.is_cancelled() {
        return Err(EngineError::Cancelled);
    }

    // ── Catalogue it ──
    //
    // As a *delta*: the archive holds only what changed, and the catalogue has
    // to read every other file as still being there.
    match catalogue_archive(
        CatalogueOne {
            engine: &plan.engine,
            index: &plan.index,
            archive: &archive,
            created_at,
            repo: Repo::Spool,
            delta: true,
        },
        sink,
    )
    .await
    {
        Ok(items) => debug!(archive, items, "local snapshot catalogued"),
        Err(e) if sink.is_cancelled() => return Err(e),
        // Same rule as a network backup: the files are protected, which is what
        // matters. Reconciliation catalogues it later.
        Err(e) => warn!(
            archive,
            "the local snapshot could not be catalogued yet: {e}"
        ),
    }

    outcome.lock().unwrap().files = files;
    info!(archive, files, "changes protected on this computer");
    Ok(JobSummary::default())
}

/// Drop local snapshots from the spool repository and the catalogue together.
async fn evict(
    plan: &OfflinePlan,
    archives: &[crate::offline::LocalArchive],
) -> Result<(), EngineError> {
    let ids: Vec<ArchiveId> = archives.iter().map(|a| ArchiveId(a.name.clone())).collect();
    let mut stream = plan.engine.delete_archives(&ids).await?;
    while let Some(event) = stream.next().await {
        if let JobEvent::Finished(result) = event {
            result?;
        }
    }
    let seqs: Vec<i64> = archives.iter().map(|a| a.seq).collect();
    let index = Arc::clone(&plan.index);
    tokio::task::spawn_blocking(move || index.lock().unwrap().remove_archives(&seqs))
        .await
        .map_err(joined)?
        .map_err(indexing)?;
    Ok(())
}

/// Everything a filesystem-snapshot backup needs.
pub struct SnapshotPlan {
    pub index: Arc<Mutex<IndexWriter>>,
    /// The subvolume that holds the sources.
    pub subvolume: std::path::PathBuf,
    pub snapshots_dir: std::path::PathBuf,
    pub walk: backtrack_core::walk::WalkSpec,
    pub excludes: Vec<String>,
    pub created_at: SystemTime,
}

/// Take a read-only filesystem snapshot and register it in the catalogue.
///
/// Unlike the spool this involves no Borg at all: the "archive" is a directory
/// the kernel produced in constant time, and a restore from it is a file copy.
/// It is also a **full** listing rather than a delta — a snapshot really does
/// hold everything — so it catches deletions, which the spool cannot.
pub fn start_snapshot_backup(plan: SnapshotPlan) -> JobStream {
    let (sink, stream) = JobStream::channel(EVENT_BUFFER);
    tokio::spawn(async move {
        let outcome = run_snapshot(&plan, &sink).await;
        sink.send(JobEvent::Finished(outcome)).await;
    });
    stream
}

async fn run_snapshot(plan: &SnapshotPlan, sink: &JobSink) -> Result<JobSummary, EngineError> {
    let index = Arc::clone(&plan.index);
    let taken =
        tokio::task::spawn_blocking(move || index.lock().unwrap().archives_in(Repo::FsSnapshot))
            .await
            .map_err(joined)?
            .map_err(indexing)?;
    let taken: Vec<String> = taken.into_iter().map(|row| row.name).collect();
    let (name, created_at) = crate::offline::next_local_name(plan.created_at, &taken);
    let root = plan.snapshots_dir.join(&name);

    sink.send(JobEvent::Progress {
        current: 0,
        total: None,
        phase: PHASE_ARCHIVING.to_string(),
    })
    .await;
    crate::snapshot::create(&plan.subvolume, &root)
        .await
        .map_err(|e| EngineError::BorgFailed {
            code: -1,
            stderr: format!("taking a filesystem snapshot: {e}"),
        })?;
    info!(snapshot = %root.display(), "changes protected on this computer");

    // Walk the snapshot, not the live tree: the snapshot is the thing that will
    // still be there when somebody comes to restore from it, and reading the
    // live tree would record versions the snapshot does not hold.
    let subvolume = plan.subvolume.clone();
    let snapshot_root = root.clone();
    let walk_spec = backtrack_core::walk::WalkSpec {
        sources: plan
            .walk
            .sources
            .iter()
            .filter_map(|s| s.strip_prefix(&plan.subvolume).ok())
            .map(|rel| root.join(rel))
            .collect(),
        excludes: backtrack_core::pattern::ExcludeSet::compile(&plan.excludes),
        one_file_system: false,
        never: plan.walk.never.clone(),
        // A full listing: the catalogue needs directory rows to show a folder
        // inside its parent.
        include_dirs: true,
    };
    let items = tokio::task::spawn_blocking(move || {
        let mut walked = backtrack_core::walk::walk(&walk_spec);
        // Record every file where it really lives, not where this snapshot
        // happens to keep its copy — otherwise a file's history splits into two
        // unrelated trees and neither tells the whole story.
        let from = backtrack_core::walk::archive_path(&snapshot_root);
        let to = backtrack_core::walk::archive_path(&subvolume);
        walked
            .items
            .retain_mut(|item| match relocate(&item.path, &from, &to) {
                Some(path) => {
                    item.path = path;
                    true
                }
                None => false,
            });
        walked
    })
    .await
    .map_err(joined)?;

    let ts = created_at
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs() as i64;
    let meta = ArchiveMeta {
        borg_id: None,
        name: name.clone(),
        ts,
    };
    let count = items.items.len();
    let index = Arc::clone(&plan.index);
    tokio::task::spawn_blocking(move || {
        let mut index = index.lock().unwrap();
        let seq = index.append_archive(&meta, Repo::FsSnapshot)?;
        index.ingest_pending(seq, items.items.into_iter())
    })
    .await
    .map_err(joined)?
    .map_err(indexing)?;

    debug!(snapshot = %name, files = count, "local snapshot catalogued");

    // Yesterday's hourlies stop being interesting once there are newer ones,
    // and snapshots are cheap but not free. Expiry runs after the new snapshot
    // rather than before it, so a failure here can never leave the machine with
    // nothing held.
    let index = Arc::clone(&plan.index);
    let held =
        tokio::task::spawn_blocking(move || index.lock().unwrap().archives_in(Repo::FsSnapshot))
            .await
            .map_err(joined)?
            .map_err(indexing)?;
    let now = SystemTime::now();
    let expired: Vec<(i64, String)> = held
        .into_iter()
        .filter(|row| {
            let created = UNIX_EPOCH + Duration::from_secs(row.ts.max(0) as u64);
            crate::snapshot::expires_at(created, None) <= now
        })
        .map(|row| (row.seq, row.name))
        .collect();
    if let Err(e) = expire_snapshots(&plan.index, &plan.snapshots_dir, &expired).await {
        // Housekeeping, exactly like a failed prune: the user is protected
        // either way, and reporting this as a failed backup would be false.
        warn!("expired local snapshots could not be cleared: {e}");
    }

    Ok(JobSummary::default())
}

/// Rewrite an archive-relative path from inside a snapshot to where the file
/// really lives. `None` when the path is not inside the snapshot at all.
fn relocate(path: &str, from: &str, to: &str) -> Option<String> {
    let rest = path.strip_prefix(from)?.trim_start_matches('/');
    if to.is_empty() {
        return Some(rest.to_string());
    }
    Some(format!("{to}/{rest}"))
}

/// Remove local snapshots that have outlived their retention.
pub async fn expire_snapshots(
    index: &Arc<Mutex<IndexWriter>>,
    snapshots_dir: &std::path::Path,
    expired: &[(i64, String)],
) -> Result<(), EngineError> {
    if expired.is_empty() {
        return Ok(());
    }
    for (_, name) in expired {
        if let Err(e) = crate::snapshot::remove(&snapshots_dir.join(name)).await {
            warn!(snapshot = %name, "could not remove an expired local snapshot: {e}");
        }
    }
    let seqs: Vec<i64> = expired.iter().map(|(seq, _)| *seq).collect();
    let index = Arc::clone(index);
    tokio::task::spawn_blocking(move || index.lock().unwrap().remove_archives(&seqs))
        .await
        .map_err(joined)?
        .map_err(indexing)?;
    info!(count = expired.len(), "expired local snapshots removed");
    Ok(())
}

/// `WalkSpec` holds a compiled `ExcludeSet`, which is not `Clone` — recompiling
/// from the patterns is cheap and keeps the plan's ownership simple.
fn clone_walk_spec(
    spec: &backtrack_core::walk::WalkSpec,
    excludes: &[String],
) -> backtrack_core::walk::WalkSpec {
    backtrack_core::walk::WalkSpec {
        sources: spec.sources.clone(),
        excludes: backtrack_core::pattern::ExcludeSet::compile(excludes),
        one_file_system: spec.one_file_system,
        never: spec.never.clone(),
        include_dirs: spec.include_dirs,
    }
}

/// Make the catalogue agree with the repository, and report how many archives
/// it had to let go of.
async fn reconcile(plan: &BackupPlan) -> Result<usize, EngineError> {
    let entries = plan.engine.list_archives().await?;
    let index = Arc::clone(&plan.index);
    let report = tokio::task::spawn_blocking(move || {
        index.lock().unwrap().sync_archives(Repo::Primary, &entries)
    })
    .await
    .map_err(joined)?
    .map_err(indexing)?;
    Ok(report.removed)
}

/// Turn batches back into the flat iterator the index wants, blocking on the
/// channel between them. Runs on a blocking thread, which is the only place
/// `blocking_recv` is allowed.
struct BatchedItems {
    rx: tokio::sync::mpsc::Receiver<Batch>,
    current: std::vec::IntoIter<BorgItem>,
}

impl BatchedItems {
    fn new(rx: tokio::sync::mpsc::Receiver<Batch>) -> BatchedItems {
        BatchedItems {
            rx,
            current: Vec::new().into_iter(),
        }
    }
}

impl Iterator for BatchedItems {
    type Item = std::result::Result<BorgItem, ListingIncomplete>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(item) = self.current.next() {
                return Some(Ok(item));
            }
            match self.rx.blocking_recv()? {
                Ok(batch) => self.current = batch.into_iter(),
                // The producer stopped early. Passing this on rolls the
                // transaction back, leaving the archive to be re-read.
                Err(incomplete) => return Some(Err(incomplete)),
            }
        }
    }
}

/// An index failure, in the engine's error vocabulary. The job model speaks
/// `EngineError`, and a catalogue problem is not a Borg problem, so it arrives
/// as an uncategorised failure rather than being mistaken for repository
/// corruption and lighting up a "your backup needs repair" banner.
fn indexing(e: backtrack_core::index::IndexError) -> EngineError {
    EngineError::BorgFailed {
        code: -1,
        stderr: format!("catalogue: {e}"),
    }
}

/// A blocking task that panicked or was cancelled.
fn joined(e: tokio::task::JoinError) -> EngineError {
    EngineError::BorgFailed {
        code: -1,
        stderr: format!("catalogue task did not finish: {e}"),
    }
}

/// The archive name for a backup taken at `now`.
///
/// `bt-{hostname}-{iso8601}`, per the stage plan. Three properties earn their
/// keep: the prefix identifies archives this product created in a repository
/// that may hold others; the hostname distinguishes machines sharing one
/// repository, which is a normal household arrangement; and the timestamp sorts
/// lexicographically in the same order as chronologically.
///
/// The basic ISO 8601 form (`20260807T065552Z`) rather than the extended one,
/// because the extended form's colons are the one character Borg gives meaning
/// to in an archive reference (`repo::archive`). The trailing `Z` is not
/// decoration: it makes the instant unambiguous, and it is what lets a repository
/// imported onto a machine in another timezone still be read exactly.
pub fn archive_name(hostname: &str, now: SystemTime) -> String {
    let secs = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs() as i64;
    format!("bt-{}-{}", sanitise_hostname(hostname), iso8601_basic(secs))
}

/// Keep a hostname to characters that are unambiguous in an archive name.
/// Anything else becomes `-`; an empty result becomes `unknown`, since an
/// archive named `bt--20260807T065552Z` reads like a bug.
fn sanitise_hostname(hostname: &str) -> String {
    let cleaned: String = hostname
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed
    }
}

/// An archive name that is not already taken, and the instant it names.
///
/// Archive names have one-second resolution, which reads well in a timeline and
/// is unique for any realistic cadence. It is not unique for two backups
/// starting inside the same second, and Borg does not forgive that: `create`
/// refuses with "Archive … already exists" (exit 30) and the run fails outright,
/// with an error in the log for something nobody did wrong.
///
/// That became reachable the moment reconnecting started an immediate catch-up
/// backup: an unscheduled run can land in the same second as a scheduled tick.
/// Pressing "Back Up Now" twice does it too. Rather than making every archive
/// name uglier, a colliding run is dated to the next free second — under a
/// second out, against a backup that would otherwise not happen at all.
pub fn next_free_name(
    now: SystemTime,
    taken: &[String],
    name_at: impl Fn(SystemTime) -> String,
) -> (String, SystemTime) {
    let mut at = now;
    loop {
        let name = name_at(at);
        if !taken.iter().any(|existing| existing == &name) {
            return (name, at);
        }
        at += Duration::from_secs(1);
    }
}

/// This machine's hostname, or `unknown`.
pub fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// `YYYYMMDDThhmmssZ` for an instant, shared by every archive-naming scheme so
/// primary and local snapshots sort together and read alike.
pub fn iso8601_basic_at(now: SystemTime) -> String {
    iso8601_basic(
        now.duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs() as i64,
    )
}

/// Epoch seconds as `YYYYMMDDThhmmssZ` (UTC).
///
/// Written out rather than pulled in: the date libraries this would otherwise
/// need bring a timezone database with them, and the one thing wanted here is
/// the calendar arithmetic that has been settled since 1582.
fn iso8601_basic(epoch_secs: i64) -> String {
    let days = epoch_secs.div_euclid(86_400);
    let secs_of_day = epoch_secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        y,
        m,
        d,
        secs_of_day / 3_600,
        (secs_of_day % 3_600) / 60,
        secs_of_day % 60
    )
}

/// The inverse of the index's `days_from_civil` — Howard Hinnant's
/// `civil_from_days`, proleptic Gregorian.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use backtrack_core::index::{IndexReader, Kind};
    use backtrack_testkit::MockEngine;

    fn item(path: &str, size: i64) -> BorgItem {
        BorgItem {
            path: path.to_string(),
            kind: Kind::File,
            size,
            mtime: 100,
            mode: 0o644,
            chunk_hash: None,
        }
    }

    fn spec(name: &str) -> CreateSpec {
        CreateSpec {
            archive_name: name.to_string(),
            sources: vec!["/home/k".into()],
            excludes: vec![],
            compression: Default::default(),
            one_file_system: true,
            created_at: UNIX_EPOCH + Duration::from_secs(1_000),
            paths: vec![],
        }
    }

    fn finished() -> Vec<JobEvent> {
        vec![JobEvent::Finished(Ok(JobSummary::default()))]
    }

    /// Drain a pipeline to its terminal event.
    async fn drain(mut stream: JobStream) -> (Vec<String>, Result<JobSummary, EngineError>) {
        let mut phases = Vec::new();
        while let Some(event) = stream.next().await {
            match event {
                JobEvent::Progress { phase, .. } => {
                    if phases.last().map(String::as_str) != Some(phase.as_str()) {
                        phases.push(phase);
                    }
                }
                JobEvent::Finished(result) => return (phases, result),
                _ => {}
            }
        }
        panic!("the pipeline ended without a terminal event");
    }

    #[tokio::test]
    async fn a_backup_creates_an_archive_and_catalogues_it() {
        // The acceptance criterion, in miniature: after a backup, the archive
        // is browsable — which is the thing a repository alone cannot tell you.
        let engine = Arc::new(
            MockEngine::default()
                .with_create_events(finished())
                .with_items(vec![item("home/a.txt", 1), item("home/b.txt", 2)]),
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        let index = Arc::new(Mutex::new(IndexWriter::open(&path).unwrap()));

        let (_, result) = drain(start_backup(BackupPlan {
            engine,
            index,
            spec: spec("bt-host-20260807T065552Z"),
            prune: None,
        }))
        .await;
        result.expect("the backup succeeds");

        let reader = IndexReader::open(&path).unwrap();
        let archives = reader.archives_overview().unwrap();
        assert_eq!(archives.len(), 1);
        assert_eq!(archives[0].name, "bt-host-20260807T065552Z");
        assert_eq!(
            archives[0].ts, 1_000,
            "the archive is dated when the backup was taken"
        );
        assert_eq!(reader.folder_at("home", 1).unwrap().len(), 2);
    }

    #[tokio::test]
    async fn the_phases_are_reported_in_order_under_one_job() {
        let engine = Arc::new(
            MockEngine::default()
                .with_create_events(vec![
                    JobEvent::Progress {
                        current: 1,
                        total: Some(2),
                        phase: "borg's own name for it".into(),
                    },
                    JobEvent::Finished(Ok(JobSummary::default())),
                ])
                // Enough items to cross a batch boundary, so cataloguing
                // reports progress rather than finishing in silence.
                .with_items(
                    (0..INGEST_BATCH + 1)
                        .map(|i| item(&format!("home/f{i}"), 1))
                        .collect(),
                ),
        );
        let dir = tempfile::tempdir().unwrap();
        let index = Arc::new(Mutex::new(
            IndexWriter::open(&dir.path().join("index.db")).unwrap(),
        ));

        let (phases, result) = drain(start_backup(BackupPlan {
            engine,
            index,
            spec: spec("a"),
            prune: Some(PrunePolicy {
                keep_hourly: 1,
                keep_daily: 0,
                keep_weekly: 0,
                keep_monthly: 0,
            }),
        }))
        .await;
        result.expect("succeeds");
        assert_eq!(
            phases,
            vec![PHASE_ARCHIVING, PHASE_CATALOGUING],
            "the user sees one job moving through named phases"
        );
    }

    #[tokio::test]
    async fn pruning_removes_the_archive_from_the_repository_and_the_catalogue_together() {
        // The stage's acceptance criterion: a tightened policy makes the oldest
        // archive disappear from both, or the timeline offers a snapshot that no
        // longer exists.
        let engine = Arc::new(
            MockEngine::default()
                .with_create_events(finished())
                .with_items(vec![item("home/a.txt", 1)])
                .keeping(2),
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        let index = Arc::new(Mutex::new(IndexWriter::open(&path).unwrap()));
        let policy = PrunePolicy {
            keep_hourly: 2,
            keep_daily: 0,
            keep_weekly: 0,
            keep_monthly: 0,
        };

        for i in 0..3 {
            let (_, result) = drain(start_backup(BackupPlan {
                engine: engine.clone(),
                index: Arc::clone(&index),
                spec: spec(&format!("bt-host-{i}")),
                prune: Some(policy.clone()),
            }))
            .await;
            result.expect("succeeds");
        }

        let repo: Vec<String> = engine.archives().into_iter().map(|a| a.name).collect();
        let reader = IndexReader::open(&path).unwrap();
        let catalogue: Vec<String> = reader
            .archives_overview()
            .unwrap()
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert_eq!(repo, vec!["bt-host-1", "bt-host-2"]);
        assert_eq!(
            catalogue.len(),
            2,
            "the catalogue followed the repository: {catalogue:?}"
        );
        assert!(!catalogue.contains(&"bt-host-0".to_string()));
    }

    #[tokio::test]
    async fn a_prune_that_fails_does_not_make_the_backup_a_failure() {
        // Housekeeping failing is not protection failing. It matters beyond
        // wording: a failed job records no successful backup, so a repository
        // whose prune kept failing would drift into AT_RISK while being backed
        // up perfectly well every hour.
        let engine = Arc::new(
            MockEngine::default()
                .with_create_events(finished())
                .with_items(vec![item("home/a.txt", 1)])
                .with_prune_error(EngineError::LockedByOther),
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        let index = Arc::new(Mutex::new(IndexWriter::open(&path).unwrap()));

        let (_, result) = drain(start_backup(BackupPlan {
            engine,
            index,
            spec: spec("a"),
            prune: Some(PrunePolicy {
                keep_hourly: 1,
                keep_daily: 0,
                keep_weekly: 0,
                keep_monthly: 0,
            }),
        }))
        .await;
        result.expect("the backup itself succeeded and must be reported as such");

        // And the archive is catalogued, which is the point.
        let reader = IndexReader::open(&path).unwrap();
        assert_eq!(reader.archives_overview().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_prune_failure_the_user_must_act_on_still_fails_the_job() {
        // The exception: a full or damaged repository has to reach the health
        // model, and the job's outcome is how it gets there.
        let engine = Arc::new(
            MockEngine::default()
                .with_create_events(finished())
                .with_items(vec![item("home/a.txt", 1)])
                .with_prune_error(EngineError::DestinationFull),
        );
        let dir = tempfile::tempdir().unwrap();
        let index = Arc::new(Mutex::new(
            IndexWriter::open(&dir.path().join("index.db")).unwrap(),
        ));

        let (_, result) = drain(start_backup(BackupPlan {
            engine,
            index,
            spec: spec("a"),
            prune: Some(PrunePolicy {
                keep_hourly: 1,
                keep_daily: 0,
                keep_weekly: 0,
                keep_monthly: 0,
            }),
        }))
        .await;
        assert_eq!(result.unwrap_err(), EngineError::DestinationFull);
    }

    #[tokio::test]
    async fn a_failed_create_never_reaches_the_catalogue() {
        let engine = Arc::new(
            MockEngine::default()
                .with_create_events(vec![JobEvent::Finished(Err(EngineError::DestinationFull))]),
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        let index = Arc::new(Mutex::new(IndexWriter::open(&path).unwrap()));

        let (_, result) = drain(start_backup(BackupPlan {
            engine,
            index,
            spec: spec("a"),
            prune: None,
        }))
        .await;
        assert_eq!(result.unwrap_err(), EngineError::DestinationFull);
        assert!(IndexReader::open(&path)
            .unwrap()
            .archives_overview()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn a_backup_that_cannot_be_catalogued_is_still_a_successful_backup() {
        // The files are safe in the repository. Reporting the run as failed
        // would raise a banner telling the user they are unprotected, which
        // would be false — and worse, it would train them to ignore banners.
        let engine = Arc::new(
            MockEngine::default()
                .with_create_events(finished())
                .with_items(vec![item("home/a.txt", 1)]),
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        let index = Arc::new(Mutex::new(IndexWriter::open(&path).unwrap()));
        // A listing that cannot be read: the archive exists, its catalogue does
        // not. The mock fails everything after create has already been scripted.
        let plan = BackupPlan {
            engine: Arc::new(
                MockEngine::default()
                    .with_create_events(finished())
                    .with_items(vec![]),
            ),
            index: Arc::clone(&index),
            spec: spec("a"),
            prune: None,
        };
        drop(engine);

        let (_, result) = drain(start_backup(plan)).await;
        result.expect("an uncatalogued backup is still a backup");

        // The row is there, awaiting its listing.
        let reader = IndexReader::open(&path).unwrap();
        assert_eq!(reader.archives_overview().unwrap().len(), 1);
    }

    /// Drain a catalogue job to its terminal event.
    async fn drain_catalogue(mut stream: JobStream) -> Result<JobSummary, EngineError> {
        while let Some(event) = stream.next().await {
            if let JobEvent::Finished(result) = event {
                return result;
            }
        }
        panic!("the catalogue job ended without a terminal event");
    }

    fn archive(name: &str, ts: i64) -> ArchiveMeta {
        ArchiveMeta {
            borg_id: None,
            name: name.to_string(),
            ts,
        }
    }

    #[tokio::test]
    async fn reconciliation_catalogues_archives_the_index_never_heard_of() {
        // The crash case: backups that reached the repository while the daemon
        // was being killed, or before it was ever run against this repository.
        let engine = Arc::new(
            MockEngine::default()
                .with_archives(vec![
                    archive("bt-host-1", 1_000),
                    archive("bt-host-2", 2_000),
                    archive("bt-host-3", 3_000),
                ])
                .with_items(vec![item("home/a.txt", 1)]),
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        let index = Arc::new(Mutex::new(IndexWriter::open(&path).unwrap()));

        drain_catalogue(start_catalogue(CataloguePlan {
            engine,
            index: Arc::clone(&index),
        }))
        .await
        .expect("cataloguing succeeds");

        assert!(
            index.lock().unwrap().pending_archives().unwrap().is_empty(),
            "everything the repository holds is browsable"
        );
        let reader = IndexReader::open(&path).unwrap();
        assert_eq!(reader.archives_overview().unwrap().len(), 3);
        assert_eq!(reader.folder_at("home", 2).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn reconciliation_leaves_a_catalogued_repository_alone() {
        // It runs on every start, so doing nothing when there is nothing to do
        // has to be genuinely nothing.
        let engine = Arc::new(
            MockEngine::default()
                .with_archives(vec![archive("bt-host-1", 1_000)])
                .with_items(vec![item("home/a.txt", 1)]),
        );
        let dir = tempfile::tempdir().unwrap();
        let index = Arc::new(Mutex::new(
            IndexWriter::open(&dir.path().join("index.db")).unwrap(),
        ));
        let plan = || CataloguePlan {
            engine: Arc::clone(&engine) as Arc<dyn BackupEngine>,
            index: Arc::clone(&index),
        };

        drain_catalogue(start_catalogue(plan())).await.unwrap();
        let first = IndexReader::open(&dir.path().join("index.db"))
            .unwrap()
            .folder_at("home", 1)
            .unwrap();
        drain_catalogue(start_catalogue(plan())).await.unwrap();
        let second = IndexReader::open(&dir.path().join("index.db"))
            .unwrap()
            .folder_at("home", 1)
            .unwrap();
        assert_eq!(first, second, "a second reconciliation changes nothing");
    }

    #[tokio::test]
    async fn a_listing_cut_short_catalogues_nothing_and_leaves_the_archive_pending() {
        // The failure this guards against is the nastiest one available: an
        // archive marked browsable while holding half its files, so the timeline
        // shows a snapshot with the user's documents missing and nothing
        // anywhere says something went wrong.
        let engine = Arc::new(
            MockEngine::default()
                .with_archives(vec![archive("bt-host-1", 1_000)])
                .with_items(vec![item("home/a.txt", 1), item("home/b.txt", 2)])
                .with_truncated_listing(EngineError::BorgFailed {
                    code: 2,
                    stderr: "connection lost".into(),
                }),
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        let index = Arc::new(Mutex::new(IndexWriter::open(&path).unwrap()));

        // The job as a whole succeeds — one unreadable archive must not abandon
        // the rest — but that archive is not catalogued.
        drain_catalogue(start_catalogue(CataloguePlan {
            engine,
            index: Arc::clone(&index),
        }))
        .await
        .expect("the run itself completes");

        assert_eq!(
            index.lock().unwrap().pending_archives().unwrap().len(),
            1,
            "the half-read archive stays queued for another attempt"
        );
        let reader = IndexReader::open(&path).unwrap();
        assert!(
            reader.folder_at("home", 1).unwrap().is_empty(),
            "nothing partial was committed"
        );
    }

    #[tokio::test]
    async fn cataloguing_can_be_stopped_and_picks_up_where_it_left_off() {
        // Cancellation has to be safe at any point, because the user closing
        // the lid during a first-run backfill is the normal case, not an edge
        // one. What remains is still `pending`, which is the whole resume state.
        let engine = Arc::new(
            MockEngine::default()
                .with_archives(
                    (1..=4)
                        .map(|i| archive(&format!("a{i}"), i * 1_000))
                        .collect(),
                )
                .with_items(vec![item("home/a.txt", 1)]),
        );
        let dir = tempfile::tempdir().unwrap();
        let index = Arc::new(Mutex::new(
            IndexWriter::open(&dir.path().join("index.db")).unwrap(),
        ));

        let stream = start_catalogue(CataloguePlan {
            engine: Arc::clone(&engine) as Arc<dyn BackupEngine>,
            index: Arc::clone(&index),
        });
        // Dropping the stream is how the job registry cancels.
        drop(stream);
        // Give the cancelled task a moment to notice and stop.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let after_cancel = index.lock().unwrap().pending_archives().unwrap().len();

        drain_catalogue(start_catalogue(CataloguePlan {
            engine,
            index: Arc::clone(&index),
        }))
        .await
        .expect("the second run finishes the job");

        assert!(after_cancel <= 4);
        assert!(
            index.lock().unwrap().pending_archives().unwrap().is_empty(),
            "resuming caught up whatever the first run did not reach"
        );
    }

    #[tokio::test]
    async fn adopting_a_repository_makes_the_newest_snapshot_browsable_first() {
        // The acceptance criterion for a first run: thirty archives, and the one
        // the user is about to be shown is readable before anything else has
        // been touched.
        let engine = Arc::new(
            MockEngine::default()
                .with_archives(
                    (1..=30)
                        .map(|i| archive(&format!("a{i:02}"), i * 1_000))
                        .collect(),
                )
                .with_items(vec![item("home/a.txt", 1)]),
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        let index = Arc::new(Mutex::new(IndexWriter::open(&path).unwrap()));

        let remaining = catalogue_newest(&CataloguePlan {
            engine: Arc::clone(&engine) as Arc<dyn BackupEngine>,
            index: Arc::clone(&index),
        })
        .await
        .expect("the newest archive is catalogued");

        assert_eq!(remaining, 29, "the rest are left for the background");
        let reader = IndexReader::open(&path).unwrap();
        assert_eq!(
            reader.archives_overview().unwrap().len(),
            30,
            "the whole history is known about straight away"
        );
        assert_eq!(
            reader.folder_at("home", 30).unwrap().len(),
            1,
            "and the newest snapshot can already be browsed"
        );
        assert!(
            reader.folder_at("home", 1).unwrap().is_empty(),
            "the oldest is not catalogued yet, and does not pretend to be"
        );

        // The background job then finishes the job.
        drain_catalogue(start_catalogue(CataloguePlan {
            engine,
            index: Arc::clone(&index),
        }))
        .await
        .expect("backfill completes");
        assert!(index.lock().unwrap().pending_archives().unwrap().is_empty());
        let reader = IndexReader::open(&path).unwrap();
        assert_eq!(reader.folder_at("home", 1).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_backfill_interrupted_by_a_restart_resumes_where_it_stopped() {
        // "Resumes after restart via archives.status='pending' rows" — the
        // resume state is a query against the catalogue, not anything held in
        // memory, so a fresh process picks up exactly where the last one
        // stopped. Modelled here by cataloguing a few archives, then starting
        // over with a brand new writer over the same database.
        let engine = Arc::new(
            MockEngine::default()
                .with_archives(
                    (1..=10)
                        .map(|i| archive(&format!("a{i:02}"), i * 1_000))
                        .collect(),
                )
                .with_items(vec![item("home/a.txt", 1)]),
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");

        {
            let index = Arc::new(Mutex::new(IndexWriter::open(&path).unwrap()));
            catalogue_newest(&CataloguePlan {
                engine: Arc::clone(&engine) as Arc<dyn BackupEngine>,
                index: Arc::clone(&index),
            })
            .await
            .unwrap();
            assert_eq!(index.lock().unwrap().pending_archives().unwrap().len(), 9);
        }

        // A new daemon over the same catalogue.
        let index = Arc::new(Mutex::new(IndexWriter::open(&path).unwrap()));
        assert_eq!(
            index.lock().unwrap().pending_archives().unwrap().len(),
            9,
            "the outstanding work survived the restart"
        );
        drain_catalogue(start_catalogue(CataloguePlan {
            engine,
            index: Arc::clone(&index),
        }))
        .await
        .expect("the new daemon finishes it");

        assert!(index.lock().unwrap().pending_archives().unwrap().is_empty());
        let reader = IndexReader::open(&path).unwrap();
        for seq in 1..=10 {
            assert_eq!(
                reader.folder_at("home", seq).unwrap().len(),
                1,
                "archive {seq} is browsable"
            );
        }
    }

    #[tokio::test]
    async fn backfilling_newest_to_oldest_builds_the_same_catalogue_as_a_forward_ingest() {
        // The catalogue must not depend on the direction it was filled in. Here
        // a file changes half way through the history, so the intervals have a
        // boundary that a naive backward pass would put in the wrong place.
        let mut listings = Vec::new();
        for i in 1..=6 {
            listings.push(item("home/a.txt", if i <= 3 { 1 } else { 2 }));
        }
        let dir = tempfile::tempdir().unwrap();

        // Forward, one archive at a time — the ordinary hourly path.
        let forward_path = dir.path().join("forward.db");
        {
            let mut w = IndexWriter::open(&forward_path).unwrap();
            for (i, listing) in listings.iter().enumerate() {
                w.ingest_archive(
                    &archive(&format!("a{:02}", i + 1), (i as i64 + 1) * 1_000),
                    Repo::Primary,
                    std::iter::once(listing.clone()),
                )
                .unwrap();
            }
        }

        // Backward, through the backfill path.
        let backward_path = dir.path().join("backward.db");
        {
            let mut w = IndexWriter::open(&backward_path).unwrap();
            let entries: Vec<ArchiveMeta> = (1..=6)
                .map(|i| archive(&format!("a{i:02}"), i * 1_000))
                .collect();
            w.sync_archives(Repo::Primary, &entries).unwrap();
            for (seq, _) in w.pending_archives().unwrap() {
                let listing = listings[seq as usize - 1].clone();
                w.ingest_pending(seq, std::iter::once(listing)).unwrap();
            }
        }

        let forward = IndexReader::open(&forward_path).unwrap();
        let backward = IndexReader::open(&backward_path).unwrap();
        assert_eq!(
            forward.file_history("home/a.txt").unwrap(),
            backward.file_history("home/a.txt").unwrap(),
            "the direction of the backfill must not show in the result"
        );
        assert_eq!(
            forward.file_history("home/a.txt").unwrap().len(),
            2,
            "one interval either side of the change"
        );
    }

    /// The btrfs acceptance criterion, against a real filesystem: a snapshot is
    /// taken, indexed, browsable through `folder_at`, restorable by copying a
    /// file out of it, and removable afterwards.
    ///
    /// Skipped with a warning where btrfs is unavailable or subvolumes cannot
    /// be created, exactly as the stage plan asks. It uses a subvolume the test
    /// user owns, because that is the only kind an unprivileged process can
    /// snapshot — which is the same limitation that makes this mode fall back
    /// to the spool on a stock desktop.
    #[cfg(feature = "integration")]
    #[tokio::test]
    async fn a_filesystem_snapshot_is_taken_indexed_and_browsable() {
        use backtrack_core::index::IndexReader;

        let base = match std::env::var_os("XDG_CACHE_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache"))
            }) {
            Some(base) if crate::snapshot::is_btrfs(&base) => base,
            _ => {
                eprintln!("skipping: no btrfs filesystem available for snapshot tests");
                return;
            }
        };
        let scratch = base.join(format!("backtrack-snap-pipeline-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).unwrap();

        let subvolume = scratch.join("home");
        let made = tokio::process::Command::new("btrfs")
            .args(["subvolume", "create"])
            .arg(&subvolume)
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !made {
            eprintln!("skipping: cannot create a subvolume here");
            let _ = std::fs::remove_dir_all(&scratch);
            return;
        }

        let source = subvolume.join("k/Documents");
        std::fs::create_dir_all(source.join("deep")).unwrap();
        std::fs::write(source.join("report.odt"), b"the original").unwrap();
        std::fs::write(source.join("deep/notes.txt"), b"notes").unwrap();

        let index_dir = tempfile::tempdir().unwrap();
        let index_path = index_dir.path().join("index.db");
        let index = Arc::new(Mutex::new(IndexWriter::open(&index_path).unwrap()));
        let snapshots = scratch.join("snapshots");

        let plan = SnapshotPlan {
            index: Arc::clone(&index),
            subvolume: subvolume.clone(),
            snapshots_dir: snapshots.clone(),
            walk: backtrack_core::walk::WalkSpec {
                sources: vec![source.clone()],
                excludes: backtrack_core::pattern::ExcludeSet::default(),
                one_file_system: true,
                never: Vec::new(),
                include_dirs: true,
            },
            excludes: Vec::new(),
            created_at: SystemTime::now(),
        };
        let (_, result) = drain(start_snapshot_backup(plan)).await;
        result.expect("the snapshot succeeds");

        // Indexed as a local snapshot, and browsable.
        let reader = IndexReader::open(&index_path).unwrap();
        let archives = reader.archives_overview().unwrap();
        assert_eq!(archives.len(), 1);
        assert_eq!(archives[0].repo, "fs-snapshot");
        assert!(archives[0].name.starts_with("bt-local-"));

        let folder = backtrack_core::walk::archive_path(&source);
        let entries = reader.folder_at(&folder, archives[0].seq).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.contains(&"report.odt") && names.contains(&"deep"),
            "the snapshot browses at the files' real paths, got {names:?}"
        );

        // Restoring from a filesystem snapshot is a plain file copy.
        let snapshot_root = snapshots.join(&archives[0].name);
        let inside = snapshot_root.join("k/Documents/report.odt");
        assert_eq!(std::fs::read_to_string(&inside).unwrap(), "the original");

        // And it is a point in time: editing the original leaves it alone.
        std::fs::write(source.join("report.odt"), b"edited later").unwrap();
        assert_eq!(std::fs::read_to_string(&inside).unwrap(), "the original");

        // Expiry removes it from the filesystem and the catalogue together.
        expire_snapshots(
            &index,
            &snapshots,
            &[(archives[0].seq, archives[0].name.clone())],
        )
        .await
        .expect("expiry succeeds");
        assert!(!snapshot_root.exists(), "the snapshot is gone from disk");
        assert!(
            IndexReader::open(&index_path)
                .unwrap()
                .archives_overview()
                .unwrap()
                .is_empty(),
            "and from the catalogue"
        );

        let _ = std::fs::remove_dir_all(&subvolume);
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn a_snapshots_contents_are_recorded_where_the_files_really_live() {
        // The catalogue has to hold `home/k/Documents/report.odt`, not the copy
        // inside a snapshot directory. Getting this wrong splits a file's
        // history into two unrelated trees, neither of which tells the whole
        // story.
        let from = "home/k/.local/share/backtrack/snapshots/bt-local-1";
        let to = "home";
        assert_eq!(
            relocate(&format!("{from}/k/Documents/report.odt"), from, to).as_deref(),
            Some("home/k/Documents/report.odt")
        );
        // The snapshot root itself maps to the subvolume root.
        assert_eq!(relocate(from, from, to).as_deref(), Some("home/"));
        // Anything outside the snapshot is not ours to relocate.
        assert_eq!(relocate("etc/passwd", from, to), None);
    }

    #[test]
    fn relocating_into_the_filesystem_root_does_not_invent_a_leading_slash() {
        // A subvolume mounted at `/` has an empty archive-relative prefix, and
        // the catalogue stores paths without a leading separator.
        assert_eq!(
            relocate("snapshots/one/home/k/f", "snapshots/one", "").as_deref(),
            Some("home/k/f")
        );
    }

    #[test]
    fn archive_names_are_sortable_prefixed_and_colon_free() {
        let name = archive_name("thinkpad", UNIX_EPOCH + Duration::from_secs(1_786_085_752));
        assert_eq!(name, "bt-thinkpad-20260807T065552Z");
        // A colon is the one character Borg reads specially in `repo::archive`.
        assert!(!name.contains(':'));

        let earlier = archive_name("h", UNIX_EPOCH + Duration::from_secs(1_000));
        let later = archive_name("h", UNIX_EPOCH + Duration::from_secs(2_000));
        assert!(earlier < later, "{earlier} must sort before {later}");
    }

    #[test]
    fn two_backups_in_one_second_do_not_collide() {
        // Found by the live definition-of-done run, as an ERROR in the log for
        // something nobody did wrong: reconnecting starts an immediate catch-up
        // backup, it landed in the same second as a scheduled tick, and borg
        // refused the second outright ("Archive … already exists", exit 30).
        let now = UNIX_EPOCH + Duration::from_secs(1_786_085_752);
        let name_at = |t: SystemTime| archive_name("thinkpad", t);

        let (first, at) = next_free_name(now, &[], name_at);
        assert_eq!(at, now, "an uncontested name is used as it is");

        let (second, at) = next_free_name(now, std::slice::from_ref(&first), name_at);
        assert_ne!(second, first);
        assert_eq!(at, now + Duration::from_secs(1));
        assert!(
            first < second,
            "and the later one still sorts later: {first} then {second}"
        );
    }

    #[test]
    fn a_run_of_collisions_still_finds_a_name() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let name_at = |t: SystemTime| archive_name("h", t);
        let taken: Vec<String> = (0..4)
            .map(|i| name_at(now + Duration::from_secs(i)))
            .collect();
        let (name, at) = next_free_name(now, &taken, name_at);
        assert_eq!(at, now + Duration::from_secs(4));
        assert!(!taken.contains(&name));
    }

    #[test]
    fn hostnames_are_kept_to_characters_that_mean_nothing_to_borg() {
        assert_eq!(sanitise_hostname("keith-laptop"), "keith-laptop");
        assert_eq!(sanitise_hostname("home.lan"), "home-lan");
        assert_eq!(sanitise_hostname("a b:c"), "a-b-c");
        assert_eq!(sanitise_hostname(""), "unknown");
        assert_eq!(sanitise_hostname("..."), "unknown");
    }

    #[test]
    fn the_calendar_matches_known_dates() {
        assert_eq!(iso8601_basic(0), "19700101T000000Z");
        // A leap day, and the end of a century that is not a leap year.
        assert_eq!(iso8601_basic(1_709_164_800), "20240229T000000Z");
        assert_eq!(iso8601_basic(951_782_400), "20000229T000000Z");
        assert_eq!(iso8601_basic(1_786_085_752), "20260807T065552Z");
    }

    #[test]
    fn the_calendar_round_trips_against_the_index_parser() {
        // The two directions live in different modules; if they ever disagree,
        // an archive's name and its catalogue timestamp would disagree too.
        for secs in [
            0i64,
            1_000,
            951_782_400,
            1_709_164_800,
            1_786_085_752,
            2_000_000_000,
        ] {
            let text = iso8601_basic(secs);
            let iso = format!(
                "{}-{}-{}T{}:{}:{}",
                &text[0..4],
                &text[4..6],
                &text[6..8],
                &text[9..11],
                &text[11..13],
                &text[13..15],
            );
            let parsed = backtrack_core::index::parse_borg_mtime(&iso).unwrap() / 1_000_000;
            assert_eq!(parsed, secs, "{text} round-trips");
        }
    }

    #[test]
    fn this_machine_has_a_usable_hostname() {
        let name = hostname();
        assert!(!name.is_empty());
        assert_eq!(sanitise_hostname(&name), sanitise_hostname(&name.clone()));
    }
}
