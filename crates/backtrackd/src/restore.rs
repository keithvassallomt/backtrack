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
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use backtrack_core::dbus::{RestoreEntry, RestorePreview};
use backtrack_core::engine::{
    ArchiveId, BackupEngine, EngineError, JobEvent, JobStream, JobSummary,
};
use backtrack_core::restore::{self, Class, Decisions, MoveLog, RestorePlan};
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

async fn prepare(
    plan: &PreparePlan,
    sink: &backtrack_core::engine::JobSink,
) -> Result<RestorePlan, EngineError> {
    std::fs::create_dir_all(&plan.staging).map_err(local)?;

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
    let staging = plan.staging.clone();
    let dest = plan.dest.clone();
    // What was asked for bounds the comparison. Without it, restoring one file
    // would walk every sibling of every directory above it.
    let asked_for: Vec<PathBuf> = plan.paths.iter().map(PathBuf::from).collect();
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
