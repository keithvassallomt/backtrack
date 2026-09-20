// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Restores, as the daemon runs them.
//!
//! A restore is two jobs rather than one, and the split is the whole design.
//! **Preparing** extracts the wanted paths into a staging directory and
//! compares them against what is on disk; it touches nothing. **Executing**
//! applies the answers the user gave to what preparing found. Between them the
//! user sees a summary of a restore that has already been worked out, and
//! "Cancel" costs nothing because nothing has happened.
//!
//! The prepared plan is held here, keyed by the job that computed it, so the
//! client can ask for it, answer it, and — afterwards — undo it, all with the
//! one id it was given when it asked.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use backtrack_core::dbus::{RestoreEntry, RestorePreview};
use backtrack_core::engine::{
    ArchiveId, BackupEngine, EngineError, JobEvent, JobStream, JobSummary,
};
use backtrack_core::restore::{self, Class, Decisions, Kind, MoveLog, RestorePlan};
use tracing::{info, warn};

use crate::jobs::JobId;
use crate::pipeline::drive;

/// Events buffered between the work and the registry. Matches the pipeline's.
const EVENT_BUFFER: usize = 64;

/// Phase names carried on progress events. They reach the user interface, so
/// they are the words the user reads.
pub const PHASE_FETCHING: &str = "fetching";
pub const PHASE_COMPARING: &str = "comparing";
pub const PHASE_RESTORING: &str = "restoring";

/// A restore that has been worked out, and possibly carried out.
pub struct Prepared {
    pub plan: RestorePlan,
    /// What the execution did. `None` until it has run; this is what Undo
    /// walks backwards.
    pub log: Option<MoveLog>,
}

/// Every restore the daemon is holding, by the job that prepared it.
#[derive(Default)]
pub struct Restores {
    held: Mutex<BTreeMap<JobId, Prepared>>,
}

impl Restores {
    /// File a finished plan under the job that computed it.
    pub fn keep(&self, job: JobId, plan: RestorePlan) {
        self.held
            .lock()
            .unwrap()
            .insert(job, Prepared { plan, log: None });
    }

    /// The plan for `job`, if it is still being held.
    pub fn plan(&self, job: JobId) -> Option<RestorePlan> {
        self.held.lock().unwrap().get(&job).map(|p| p.plan.clone())
    }

    /// Record what an execution did, so it can be undone.
    pub fn record(&self, job: JobId, log: MoveLog) {
        if let Some(prepared) = self.held.lock().unwrap().get_mut(&job) {
            prepared.log = Some(log);
        }
    }

    /// What `job`'s execution did, if it has run.
    pub fn log(&self, job: JobId) -> Option<MoveLog> {
        self.held
            .lock()
            .unwrap()
            .get(&job)
            .and_then(|p| p.log.clone())
    }

    /// Remove the extracted copy, keeping the plan and its move log.
    ///
    /// Called once an execution has finished: staging holds a whole second copy
    /// of everything being restored, and leaving it behind would fill the disk
    /// one restore at a time. The log stays, because Undo still needs it.
    pub fn clear_staging(&self, job: JobId) {
        let staging = self
            .held
            .lock()
            .unwrap()
            .get(&job)
            .map(|p| p.plan.staging.clone());
        if let Some(staging) = staging {
            remove_staging(&staging);
        }
    }

    /// Forget a restore entirely — the user cancelled, or the window closed.
    pub fn discard(&self, job: JobId) {
        let Some(prepared) = self.held.lock().unwrap().remove(&job) else {
            return;
        };
        remove_staging(&prepared.plan.staging);
    }

    /// Whether anything is being held for `job`.
    pub fn holds(&self, job: JobId) -> bool {
        self.held.lock().unwrap().contains_key(&job)
    }
}

/// Begin an extraction from an empty directory, whatever was there before.
///
/// Staging is named after the job id, and job ids start again from the
/// beginning every time the daemon does. So a staging directory outlives the
/// daemon that made it — killed, crashed, or the machine losing power between
/// preparing a restore and answering its dialog — and the next daemon's job
/// with the same number would extract straight into what was left.
///
/// That is not a tidiness problem. The leftovers sit outside the path this
/// restore asked for, so nothing on disk is walked to compare them against:
/// they classify as "only in the backup", which needs no decision and is
/// written out without asking. A restore of one folder would quietly deliver
/// a different folder from a restore somebody abandoned a week ago.
///
/// `create_dir_all` alone cannot catch this — it succeeds, silently, on a
/// directory that already exists with contents.
fn empty_staging(staging: &std::path::Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(staging) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    std::fs::create_dir_all(staging)
}

/// Keep the stash inside the promises the dialogs make about it: thirty days,
/// and five gigabytes. Runs now, and once a day for as long as the daemon does.
///
/// A daily pass is enough because neither bound is a cliff. Being a few hours
/// over thirty days costs nothing, and the size cap is there to stop a safety
/// net filling a disk over weeks — not to hold a line to the megabyte.
pub async fn keep_the_stash_bounded(root: std::path::PathBuf) {
    const DAILY: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
    loop {
        let root = root.clone();
        let swept = tokio::task::spawn_blocking(move || {
            backtrack_core::restore::expire_stash(&root, seconds_now(), MAX_STASH_BYTES)
        })
        .await;
        match swept {
            Ok(report) if report.batches > 0 => {
                // Files given up early were promised thirty days and did not
                // get them. That is the right trade against filling a disk,
                // but it is not a thing to do silently.
                if report.given_up_early > 0 {
                    warn!(
                        restores = report.given_up_early,
                        bytes = report.bytes,
                        "the safety stash is over its size limit; the oldest restores were given up early"
                    );
                } else {
                    info!(
                        restores = report.batches,
                        bytes = report.bytes,
                        "safety copies past thirty days were given up"
                    );
                }
            }
            Ok(_) => {}
            Err(error) => warn!(%error, "the stash could not be checked"),
        }
        tokio::time::sleep(DAILY).await;
    }
}

/// The size the stash is allowed to reach. A constant here rather than a
/// setting: it is written into the dialogs, and a promise with a knob on it is
/// not a promise.
const MAX_STASH_BYTES: u64 = backtrack_core::restore::MAX_BYTES;

fn seconds_now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Clear every staging directory at start-up.
///
/// Nothing is holding them: the restores that owned them lived in a previous
/// process's memory, and a prepared restore does not survive the daemon that
/// prepared it. Leaving them costs a whole second copy of somebody's folder
/// per abandoned restore, for as long as the machine lasts.
pub fn sweep_staging(root: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        info!(path = %entry.path().display(), "clearing an abandoned restore");
        remove_staging(&entry.path());
    }
}

fn remove_staging(staging: &std::path::Path) {
    if let Err(error) = std::fs::remove_dir_all(staging) {
        if error.kind() != std::io::ErrorKind::NotFound {
            warn!(
                staging = %staging.display(),
                %error,
                "the staging directory could not be cleared"
            );
        }
    }
}

/// Turn a plan into the summary the client reads.
///
/// Only the paths needing an answer cross the bus: a folder restore may touch
/// tens of thousands of files and the review list shows the handful that
/// conflict.
/// The wire spelling of a kind. Matches the `SearchFiles` vocabulary, so a
/// client that already knows what "symlink" means here does not have to learn
/// a second set of words for the same three things.
fn kind_name(kind: Kind) -> String {
    match kind {
        Kind::File => "file",
        Kind::Dir => "dir",
        Kind::Symlink => "symlink",
    }
    .to_string()
}

pub fn preview(plan: &RestorePlan) -> RestorePreview {
    let counts = plan.counts();
    let entries = plan
        .decisions_needed()
        .map(|entry| RestoreEntry {
            path: entry.path.to_string_lossy().to_string(),
            class: match entry.class {
                Class::TypeChanged => "type-changed".to_string(),
                _ => "conflict".to_string(),
            },
            disk_newer: matches!(entry.class, Class::Conflict { disk_newer: true }),
            backup_size: entry.backup.map_or(0, |f| f.size),
            backup_mtime: entry.backup.map_or(0, |f| f.mtime),
            disk_size: entry.disk.map_or(0, |f| f.size),
            disk_mtime: entry.disk.map_or(0, |f| f.mtime),
            backup_kind: entry.backup.map(|f| kind_name(f.kind)).unwrap_or_default(),
            disk_kind: entry.disk.map(|f| kind_name(f.kind)).unwrap_or_default(),
        })
        .collect();

    RestorePreview {
        archive: plan.archive.clone(),
        dest: plan.dest.to_string_lossy().to_string(),
        identical: counts.identical as u32,
        conflicts: counts.conflicts as u32,
        disk_newer: counts.disk_newer as u32,
        only_in_backup: counts.only_in_backup as u32,
        only_on_disk: counts.only_on_disk as u32,
        type_changed: counts.type_changed as u32,
        entries,
        refused: plan
            .refused
            .iter()
            .map(|(path, why)| (path.to_string_lossy().to_string(), why.clone()))
            .collect(),
        missing: plan
            .missing
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect(),
    }
}

/// Everything preparing a restore needs.
pub struct PreparePlan {
    pub engine: Arc<dyn BackupEngine>,
    pub archive: ArchiveId,
    /// Archive-relative member paths to fetch.
    pub paths: Vec<String>,
    pub dest: PathBuf,
    pub staging: PathBuf,
    pub restores: Arc<Restores>,
    /// The job this is, so the finished plan can be filed under it.
    pub job: JobId,
    /// Whether what was asked for lands *inside* `dest` rather than at the
    /// absolute paths it came from.
    ///
    /// An archive stores a member by its whole path, so a restore rooted at
    /// the staging directory reproduces that path under the destination:
    /// choosing a folder to restore into would give you eight empty
    /// directories and your files at the bottom of them. Restoring in place
    /// wants exactly that reproduction; restoring somewhere else wants the
    /// opposite, and this is which.
    pub into_dest: bool,
}

/// Fetch the wanted paths and work out what restoring them would do.
pub fn start_prepare(plan: PreparePlan) -> JobStream {
    let (sink, stream) = JobStream::channel(EVENT_BUFFER);
    tokio::spawn(async move {
        let outcome = prepare(&plan, &sink).await;
        if outcome.is_err() {
            // Nothing was promised and nothing should be left behind.
            remove_staging(&plan.staging);
        }
        sink.send(JobEvent::Finished(outcome.map(|_| JobSummary::default())))
            .await;
    });
    stream
}

/// Where to plan from when the answer goes somewhere the user chose.
///
/// A folder's *contents* land in the destination, and a file lands in it
/// beside nothing — both of which come out of "open the folder and your things
/// are there". So a directory is planned from itself and a file from the
/// directory holding it, scoped to the one name.
///
/// This is decided after the extraction rather than from the archive listing,
/// because the staging tree is the thing being planned against: whatever borg
/// actually produced is the truth, and asking it twice invites the two answers
/// to differ.
fn rooted_at_what_was_asked_for(staging: &Path, paths: &[String]) -> (PathBuf, Vec<PathBuf>) {
    let Some(asked) = paths.first() else {
        return (staging.to_path_buf(), Vec::new());
    };
    let extracted = staging.join(asked);
    if extracted.is_dir() {
        let inside = top_level(&extracted);
        return (extracted, inside);
    }
    match (extracted.parent(), extracted.file_name()) {
        (Some(parent), Some(name)) => (parent.to_path_buf(), vec![PathBuf::from(name)]),
        _ => (staging.to_path_buf(), Vec::new()),
    }
}

/// The names directly inside a directory. What bounds the comparison when the
/// destination is a folder made for this restore: everything about to be
/// written, and nothing else.
fn top_level(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| PathBuf::from(entry.file_name()))
        .collect()
}

async fn prepare(
    plan: &PreparePlan,
    sink: &backtrack_core::engine::JobSink,
) -> Result<RestorePlan, EngineError> {
    empty_staging(&plan.staging).map_err(local)?;

    // ── Fetching ──
    let stream = plan
        .engine
        .extract(&plan.archive, &plan.paths, &plan.staging)
        .await?;
    drive(stream, sink, PHASE_FETCHING).await?;
    if sink.is_cancelled() {
        return Err(EngineError::Cancelled);
    }

    // ── Comparing ──
    //
    // A whole-tree walk with content reads in it, so it does not belong on an
    // async worker that other jobs are sharing.
    sink.send(JobEvent::Progress {
        current: 0,
        total: None,
        phase: PHASE_COMPARING.to_string(),
    })
    .await;

    let archive = plan.archive.0.clone();
    let dest = plan.dest.clone();
    // What was asked for bounds the comparison. Without it, restoring one file
    // would walk every sibling of every directory above it.
    let (staging, asked_for) = if plan.into_dest {
        rooted_at_what_was_asked_for(&plan.staging, &plan.paths)
    } else {
        (
            plan.staging.clone(),
            plan.paths.iter().map(PathBuf::from).collect(),
        )
    };
    let computed =
        tokio::task::spawn_blocking(move || restore::plan(&archive, &staging, &dest, &asked_for))
            .await
            .map_err(|e| EngineError::Local(e.to_string()))?
            .map_err(|e| EngineError::Local(e.to_string()))?;

    let counts = computed.counts();
    info!(
        archive = plan.archive.0,
        identical = counts.identical,
        conflicts = counts.conflicts,
        adding = counts.only_in_backup,
        keeping = counts.only_on_disk,
        "restore prepared"
    );
    plan.restores.keep(plan.job, computed.clone());
    Ok(computed)
}

/// Everything carrying out a prepared restore needs.
pub struct ExecutePlan {
    pub restores: Arc<Restores>,
    /// The job that prepared it, which is also where the move log is filed.
    pub prepared: JobId,
    pub decisions: Decisions,
    pub stash: PathBuf,
}

/// Apply the answers to a prepared restore.
pub fn start_execute(plan: ExecutePlan) -> JobStream {
    let (sink, stream) = JobStream::channel(EVENT_BUFFER);
    tokio::spawn(async move {
        sink.send(JobEvent::Progress {
            current: 0,
            total: None,
            phase: PHASE_RESTORING.to_string(),
        })
        .await;
        let outcome = apply(&plan).await;
        sink.send(JobEvent::Finished(outcome)).await;
    });
    stream
}

async fn apply(plan: &ExecutePlan) -> Result<JobSummary, EngineError> {
    let Some(prepared) = plan.restores.plan(plan.prepared) else {
        return Err(EngineError::Local(
            "that restore is no longer prepared; work it out again".to_string(),
        ));
    };
    let outcome = carry_out(plan, prepared).await;
    // Whether it worked or not, the extracted copy has served its purpose.
    plan.restores.clear_staging(plan.prepared);
    outcome
}

async fn carry_out(plan: &ExecutePlan, prepared: RestorePlan) -> Result<JobSummary, EngineError> {
    let decisions = plan.decisions.clone();
    let stash = plan.stash.clone();
    let report = tokio::task::spawn_blocking(move || {
        restore::execute(&prepared, &decisions, &stash, SystemTime::now())
    })
    .await
    .map_err(|e| EngineError::Local(e.to_string()))?
    .map_err(restore_failure)?;

    info!(
        restored = report.restored,
        skipped = report.skipped,
        failed = report.failures.len(),
        "restore carried out"
    );
    for (path, why) in &report.failures {
        warn!(path = %path.display(), why, "a file could not be restored");
    }
    plan.restores.record(plan.prepared, report.log.clone());

    // A restore that could not write a single thing it was asked to is a failed
    // restore, however politely each individual failure was collected.
    if report.restored == 0 && !report.failures.is_empty() {
        return Err(EngineError::Local(format!(
            "nothing could be restored: {}",
            report.failures[0].1
        )));
    }
    Ok(JobSummary::default())
}

/// Put back what a restore did.
pub struct UndoPlan {
    pub restores: Arc<Restores>,
    pub prepared: JobId,
}

/// Reverse a restore, using the log of what it moved.
pub fn start_undo(plan: UndoPlan) -> JobStream {
    let (sink, stream) = JobStream::channel(EVENT_BUFFER);
    tokio::spawn(async move {
        let outcome = match plan.restores.log(plan.prepared) {
            None => Err(EngineError::Local(
                "there is nothing recorded to undo".to_string(),
            )),
            Some(log) => tokio::task::spawn_blocking(move || restore::undo(&log))
                .await
                .map_err(|e| EngineError::Local(e.to_string()))
                .and_then(|report| {
                    info!(
                        reverted = report.restored,
                        failed = report.failures.len(),
                        "restore undone"
                    );
                    if report.failures.is_empty() {
                        Ok(JobSummary::default())
                    } else {
                        Err(EngineError::Local(format!(
                            "{} of the restored files could not be put back",
                            report.failures.len()
                        )))
                    }
                }),
        };
        sink.send(JobEvent::Finished(outcome)).await;
    });
    stream
}

/// Local I/O, reported as what it is rather than as a Borg failure.
fn local(error: std::io::Error) -> EngineError {
    EngineError::Local(error.to_string())
}

/// A restore that could not start, mapped to the engine's vocabulary where one
/// fits and to the local catch-all where it does not.
fn restore_failure(error: restore::RestoreError) -> EngineError {
    match error {
        restore::RestoreError::NotEnoughSpace { .. } => EngineError::LocalDiskFull,
        other => EngineError::Local(other.to_string()),
    }
}

/// A restore with nobody to ask: prepare and carry out under one job id, with
/// a blanket answer for every conflict.
///
/// This is what the command line and any non-interactive caller get. It is the
/// same pipeline — the same staging, the same stash, the same atomic moves —
/// and only the asking is missing.
pub fn start_direct(prepare_plan: PreparePlan, decisions: Decisions, stash: PathBuf) -> JobStream {
    let (sink, stream) = JobStream::channel(EVENT_BUFFER);
    tokio::spawn(async move {
        let outcome = match prepare(&prepare_plan, &sink).await {
            Err(error) => {
                remove_staging(&prepare_plan.staging);
                Err(error)
            }
            Ok(_) => {
                sink.send(JobEvent::Progress {
                    current: 0,
                    total: None,
                    phase: PHASE_RESTORING.to_string(),
                })
                .await;
                let execute = ExecutePlan {
                    restores: Arc::clone(&prepare_plan.restores),
                    prepared: prepare_plan.job,
                    decisions,
                    stash,
                };
                apply(&execute).await
            }
        };
        sink.send(JobEvent::Finished(outcome)).await;
    });
    stream
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scenario this guards: the daemon is killed holding a prepared
    /// restore, comes back, and hands job 3 to somebody restoring something
    /// else entirely. Without the clear, the old extraction is merged into the
    /// new plan — and merged as "only in the backup", which is the class that
    /// is written to disk without a dialog.
    /// A staging tree as an extraction leaves it: the archive member at its
    /// whole path, with every directory above it recreated.
    fn extracted(root: &std::path::Path, member: &str, files: &[&str]) {
        for file in files {
            let path = root.join(member).join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        }
    }

    #[test]
    fn a_folder_restored_elsewhere_arrives_as_its_contents() {
        // Not as the eight empty directories its archive path is made of, with
        // the files at the bottom — which is what restoring in place wants and
        // what choosing a destination emphatically does not.
        let root = tempfile::tempdir().unwrap();
        let member = "home/keith/Projects/website";
        extracted(root.path(), member, &["README.md", "css/main.css"]);

        let (staging, asked_for) = rooted_at_what_was_asked_for(root.path(), &[member.to_string()]);

        assert_eq!(staging, root.path().join(member));
        let mut names: Vec<String> = asked_for
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        names.sort();
        assert_eq!(names, ["README.md", "css"]);
    }

    #[test]
    fn a_single_file_restored_elsewhere_lands_beside_nothing() {
        let root = tempfile::tempdir().unwrap();
        extracted(root.path(), "home/keith/Documents", &["report.odt"]);
        let member = "home/keith/Documents/report.odt";

        let (staging, asked_for) = rooted_at_what_was_asked_for(root.path(), &[member.to_string()]);

        assert_eq!(staging, root.path().join("home/keith/Documents"));
        assert_eq!(asked_for, [PathBuf::from("report.odt")]);
    }

    #[test]
    fn a_reused_job_number_does_not_inherit_the_last_ones_extraction() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("3");
        std::fs::create_dir_all(staging.join("home/Documents")).unwrap();
        std::fs::write(staging.join("home/Documents/report.odt"), b"last week").unwrap();

        empty_staging(&staging).unwrap();

        assert!(
            staging.is_dir(),
            "the extraction still needs somewhere to go"
        );
        assert_eq!(
            std::fs::read_dir(&staging).unwrap().count(),
            0,
            "an extraction must not start on top of an abandoned one"
        );
    }

    #[test]
    fn a_first_restore_is_not_an_error_for_want_of_something_to_clear() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("1");
        empty_staging(&staging).unwrap();
        assert!(staging.is_dir());
    }

    #[test]
    fn nothing_survives_the_sweep_and_the_root_does() {
        let root = tempfile::tempdir().unwrap();
        for job in ["1", "7"] {
            std::fs::create_dir_all(root.path().join(job).join("deep")).unwrap();
            std::fs::write(root.path().join(job).join("deep/file"), b"x").unwrap();
        }
        sweep_staging(root.path());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        assert!(root.path().is_dir(), "the staging root itself is kept");
    }

    #[test]
    fn a_sweep_of_a_directory_that_was_never_made_is_not_a_failure() {
        // First run on a new machine: nothing has staged anything yet.
        let root = tempfile::tempdir().unwrap();
        sweep_staging(&root.path().join("never-created"));
    }
}
