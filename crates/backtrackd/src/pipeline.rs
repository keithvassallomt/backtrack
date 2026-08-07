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
use backtrack_core::index::{ArchiveMeta, BorgItem, IndexWriter, Repo};
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
    if let Some(policy) = &plan.prune {
        drive(plan.engine.prune(policy).await?, sink, PHASE_PRUNING).await?;
        // The repository is the authority on what exists. Reconciling after the
        // prune rather than parsing its output means the catalogue is correct
        // even if Borg removed something we did not predict — and it repairs
        // any earlier disagreement for free.
        match reconcile(plan).await {
            Ok(removed) if removed > 0 => info!(removed, "pruned archives left the catalogue"),
            Ok(_) => {}
            Err(e) => warn!("could not reconcile the catalogue after pruning: {e}"),
        }
    }

    Ok(summary)
}

/// Forward one engine job's events under `phase`, and return its summary.
///
/// The engine's own terminal event is consumed here rather than forwarded: this
/// is one phase of a longer job, and a `Finished` in the middle of a stream
/// would tell the registry the whole backup was over.
async fn drive(
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
    let ts = plan
        .spec
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
    let index = Arc::clone(&plan.index);
    let seq = tokio::task::spawn_blocking(move || {
        index.lock().unwrap().append_archive(&meta, Repo::Primary)
    })
    .await
    .map_err(joined)?
    .map_err(indexing)?;

    let (tx, rx) = tokio::sync::mpsc::channel::<Vec<BorgItem>>(2);
    let index = Arc::clone(&plan.index);
    let writer = tokio::task::spawn_blocking(move || {
        let mut index = index.lock().unwrap();
        index.ingest_pending(seq, BatchedItems::new(rx))
    });

    let mut listing = plan
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
            if tx.send(std::mem::take(&mut batch)).await.is_err() {
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
    if failure.is_none() && !sink.is_cancelled() {
        let _ = tx.send(batch).await;
    }
    drop(tx);

    let stats = writer.await.map_err(joined)?.map_err(indexing)?;
    if let Some(e) = failure {
        return Err(e);
    }
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
    rx: tokio::sync::mpsc::Receiver<Vec<BorgItem>>,
    current: std::vec::IntoIter<BorgItem>,
}

impl BatchedItems {
    fn new(rx: tokio::sync::mpsc::Receiver<Vec<BorgItem>>) -> BatchedItems {
        BatchedItems {
            rx,
            current: Vec::new().into_iter(),
        }
    }
}

impl Iterator for BatchedItems {
    type Item = BorgItem;

    fn next(&mut self) -> Option<BorgItem> {
        loop {
            if let Some(item) = self.current.next() {
                return Some(item);
            }
            self.current = self.rx.blocking_recv()?.into_iter();
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

/// This machine's hostname, or `unknown`.
pub fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
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
