// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Disaster recovery, as the daemon runs it: everything in one backup brought
//! back to a new computer, a step at a time.
//!
//! A step is one folder of the old home folder, or the hidden folders and
//! loose files together (see [`backtrack_core::recovery`]). Each goes through
//! the same staging pipeline as any restore: fetched into a working folder,
//! compared with what is on this computer, and moved into place. The steps run
//! one after another, because they all read the same repository and because a
//! recovery that is in one place at a time can say exactly where it is.
//!
//! Where it is lives in a manifest on disk, written after every change, so a
//! recovery outlives the daemon running it. Started again, it carries on from
//! the first step that is not done, and inside that step from where it
//! stopped: files that were fetched whole are kept, checked against the
//! catalogue by size and modification time, and only the rest is fetched
//! again. Borg sets a file's modification time after its contents, so a file
//! that was being written when the daemon stopped never passes for finished.
//! Pausing is the same stop, asked for: it finishes the file being moved,
//! stops Borg, and keeps everything.
//!
//! Nothing on this computer is overwritten. A file that is here already and
//! differs from the backup is left alone, and the backup's copy is set aside;
//! when every step is done, all of them are asked about at once, with the
//! summary every folder restore uses. On a new computer there are usually
//! none, and then nothing is asked at all.

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use backtrack_core::dbus::{RecoveryStatus, RecoveryStep};
use backtrack_core::engine::{
    ArchiveId, BackupEngine, EngineError, JobEvent, JobSink, JobStream, JobSummary,
};
use backtrack_core::index::{IndexReader, Kind, MTIME_TOLERANCE_MICROS};
use backtrack_core::recovery::{self as layout, relative};
use backtrack_core::restore::{self, Decision, Decisions, Move};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::jobs::JobId;

/// Events buffered between the work and the registry. Matches the pipeline's.
const EVENT_BUFFER: usize = 64;

/// The phase name on progress events.
pub const PHASE: &str = "restoring";

/// How often progress is measured while a step is being fetched.
const TICK: Duration = Duration::from_millis(500);

/// The window the rate behind the time left is measured over: long enough to
/// ride out a folder of small files followed by one large one, short enough to
/// notice a network that has slowed.
const RATE_WINDOW: Duration = Duration::from_secs(60);

/// How long a rate has to have been measured before it is worth saying.
const RATE_SETTLES: Duration = Duration::from_secs(10);

/// How often the estimate is written to the log, so a run can be checked
/// against it afterwards.
const LOG_EVERY: Duration = Duration::from_secs(30);

/// The manifest format this build writes.
const VERSION: u32 = 1;

/// Where a recovery is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    /// Steps still to do. The only stage that holds the schedule back.
    Restoring,
    /// Every step done; files that clash are waiting for the summary.
    Review,
    /// Every step done and nothing waiting.
    Finished,
}

/// Where one step is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Pending,
    /// Being extracted into the working folder. Whatever is there already may
    /// be kept, once checked.
    Fetching,
    /// Extracted whole; being moved into place.
    Placing,
    Done,
}

/// One step, and how far it has got.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepState {
    #[serde(flatten)]
    pub step: layout::Step,
    pub status: Status,
    /// Files moved into place.
    #[serde(default)]
    pub restored: u64,
    /// Files set aside because this computer has its own copy.
    #[serde(default)]
    pub held: u64,
    /// Files that could not be written.
    #[serde(default)]
    pub failed: u64,
}

/// The record of a recovery: what it is restoring, where to, and how far it
/// has got. Written whole after every change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    /// The backup being restored, by name, and when it was taken.
    pub archive: String,
    pub taken: i64,
    /// The archive path that stands for the home folder.
    pub source: String,
    /// This computer's home folder.
    pub dest: PathBuf,
    /// What to do about files that clash: `ask`, or one answer for all of
    /// them.
    pub policy: String,
    /// Archive paths never restored.
    pub never: Vec<String>,
    pub steps: Vec<StepState>,
    pub stage: Stage,
    /// The person paused it, so a restart should not simply carry on.
    #[serde(default)]
    pub paused: bool,
    pub started: i64,
    /// What to back up afterwards, if nothing has been chosen by then.
    #[serde(default)]
    pub backups_after: Vec<PathBuf>,
    /// The schedule, the backup list and the notification have been seen to.
    #[serde(default)]
    pub handed_over: bool,
}

impl Manifest {
    /// A new recovery of `layout` from `archive` into `dest`.
    pub fn new(
        archive: &str,
        taken: i64,
        layout: layout::Layout,
        dest: PathBuf,
        policy: &str,
        backups_after: Vec<PathBuf>,
    ) -> Manifest {
        Manifest {
            version: VERSION,
            archive: archive.to_string(),
            taken,
            source: layout.source,
            dest,
            policy: policy.to_string(),
            never: layout.never,
            steps: layout
                .steps
                .into_iter()
                .map(|step| StepState {
                    step,
                    status: Status::Pending,
                    restored: 0,
                    held: 0,
                    failed: 0,
                })
                .collect(),
            stage: Stage::Restoring,
            paused: false,
            started: seconds(SystemTime::now()),
            backups_after,
            handed_over: false,
        }
    }

    /// Bytes across every step.
    pub fn total(&self) -> u64 {
        self.steps.iter().map(|s| s.step.bytes).sum()
    }

    /// Bytes of the steps that are done.
    pub fn done(&self) -> u64 {
        self.steps
            .iter()
            .filter(|s| s.status == Status::Done)
            .map(|s| s.step.bytes)
            .sum()
    }

    /// The step under way, or next.
    pub fn current(&self) -> Option<usize> {
        self.steps.iter().position(|s| s.status != Status::Done)
    }

    fn restored(&self) -> u64 {
        self.steps.iter().map(|s| s.restored).sum()
    }
}

/// What the window is shown while a step is fetched.
#[derive(Debug, Default)]
struct Live {
    /// Bytes restored, overall.
    done: u64,
    /// The file being restored, relative to the home folder.
    current: String,
    /// Recent measurements of `done`, oldest first.
    samples: VecDeque<(Instant, u64)>,
    /// Why the last run stopped, if it failed.
    error: Option<String>,
}

impl Live {
    fn measure(&mut self, at: Instant, done: u64) {
        self.done = done;
        self.samples.push_back((at, done));
        while self
            .samples
            .front()
            .is_some_and(|(then, _)| at.duration_since(*then) > RATE_WINDOW)
        {
            self.samples.pop_front();
        }
    }

    /// Bytes a second over the last minute, once there is enough of a minute.
    fn rate(&self) -> Option<f64> {
        let (first_at, first) = *self.samples.front()?;
        let (last_at, last) = *self.samples.back()?;
        let span = last_at.duration_since(first_at);
        if span < RATE_SETTLES || last <= first {
            return None;
        }
        Some((last - first) as f64 / span.as_secs_f64())
    }
}

/// Seconds left at `rate` bytes a second, or nothing when it cannot be said.
pub fn eta(done: u64, total: u64, rate: Option<f64>) -> Option<u64> {
    let rate = rate.filter(|r| *r > 0.0)?;
    Some((total.saturating_sub(done) as f64 / rate).ceil() as u64)
}

/// The daemon's one disaster recovery.
pub struct Recovery {
    manifest: PathBuf,
    root: PathBuf,
    /// One run at a time. A paused run winds down while the person may
    /// already be pressing Resume; the next waits for it here rather than
    /// both working in the same folder.
    one_at_a_time: tokio::sync::Mutex<()>,
    /// The stage on disk, so the scheduler can ask every tick without
    /// reading a file.
    stage: Mutex<Option<Stage>>,
    job: Mutex<Option<JobId>>,
    /// Set when the person pauses, so the run that stops writes down that it
    /// was a pause rather than a daemon going away.
    pausing: AtomicBool,
    live: Mutex<Live>,
}

impl Recovery {
    pub fn new(manifest: PathBuf, root: PathBuf) -> Recovery {
        let recovery = Recovery {
            manifest,
            root,
            one_at_a_time: tokio::sync::Mutex::new(()),
            stage: Mutex::new(None),
            job: Mutex::new(None),
            pausing: AtomicBool::new(false),
            live: Mutex::new(Live::default()),
        };
        let stage = recovery.load().map(|m| m.stage);
        *recovery.stage.lock().unwrap() = stage;
        recovery
    }

    /// Where the step being fetched is extracted.
    pub fn staging(&self) -> PathBuf {
        self.root.join("staging")
    }

    /// Where the backup's copies of clashing files wait for the summary.
    pub fn held(&self) -> PathBuf {
        self.root.join("held")
    }

    fn added(&self) -> PathBuf {
        self.root.join("added")
    }

    fn patterns_file(&self) -> PathBuf {
        self.root.join("wanted.patterns")
    }

    /// The manifest, if there is a recovery. One that cannot be read is
    /// reported and treated as none: guessing at where a recovery had got to
    /// risks restoring into a home folder twice.
    pub fn load(&self) -> Option<Manifest> {
        let text = match std::fs::read_to_string(&self.manifest) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => {
                warn!(%error, path = %self.manifest.display(), "the recovery record could not be read");
                return None;
            }
        };
        match serde_json::from_str::<Manifest>(&text) {
            Ok(manifest) if manifest.version == VERSION => Some(manifest),
            Ok(manifest) => {
                warn!(
                    version = manifest.version,
                    "the recovery record is from another version"
                );
                None
            }
            Err(error) => {
                warn!(%error, "the recovery record is damaged");
                None
            }
        }
    }

    /// Write the manifest, whole or not at all.
    pub fn save(&self, manifest: &Manifest) -> std::io::Result<()> {
        if let Some(parent) = self.manifest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(manifest).map_err(std::io::Error::other)?;
        let partial = self.manifest.with_extension("json.partial");
        {
            let mut file = std::fs::File::create(&partial)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
        }
        std::fs::rename(&partial, &self.manifest)?;
        *self.stage.lock().unwrap() = Some(manifest.stage);
        Ok(())
    }

    /// Forget the recovery and everything it was working with.
    pub fn clear(&self) {
        remove(&self.manifest);
        remove(&self.root);
        *self.stage.lock().unwrap() = None;
        *self.job.lock().unwrap() = None;
        *self.live.lock().unwrap() = Live::default();
    }

    /// Once every step is done, only the files waiting for the summary are
    /// worth keeping: the working folder is empty, and the record of what was
    /// added only ever served Discard, which is offered while it runs.
    pub fn keep_only_what_waits(&self) {
        remove(&self.staging());
        remove(&self.added());
        remove(&self.patterns_file());
        self.tidy();
    }

    /// Whether a recovery has steps left, which is what holds back the
    /// schedule.
    pub fn restoring(&self) -> bool {
        *self.stage.lock().unwrap() == Some(Stage::Restoring)
    }

    /// The job running the recovery, if it has one.
    pub fn job(&self) -> Option<JobId> {
        *self.job.lock().unwrap()
    }

    pub fn set_job(&self, job: JobId) {
        *self.job.lock().unwrap() = Some(job);
    }

    /// The person paused it, or resumed it.
    pub fn set_pausing(&self, pausing: bool) {
        self.pausing.store(pausing, Ordering::SeqCst);
    }

    fn failed(&self, error: &EngineError) {
        self.live.lock().unwrap().error = Some(error.to_string());
    }

    /// Wait for whatever run is winding down to stop.
    pub async fn quiet(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.one_at_a_time.lock().await
    }

    /// A finished recovery whose summary has been answered or put away has
    /// nothing left: forget it.
    pub fn tidy(&self) {
        let Some(manifest) = self.load() else {
            *self.stage.lock().unwrap() = None;
            return;
        };
        if manifest.stage == Stage::Review && manifest.handed_over && !has_anything(&self.held()) {
            info!("the recovery's last question has been answered");
            self.clear();
        }
    }

    /// Everything the progress window shows, given the state of the job.
    pub fn status(&self, job_state: Option<&str>) -> RecoveryStatus {
        let Some(manifest) = self.load() else {
            return RecoveryStatus {
                state: "none".into(),
                job: 0,
                archive: String::new(),
                taken: 0,
                done: 0,
                total: 0,
                eta: -1,
                current: String::new(),
                steps: Vec::new(),
                restored: 0,
                conflicts: 0,
                error: String::new(),
            };
        };
        let live = self.live.lock().unwrap();
        let state = match (manifest.stage, job_state) {
            (Stage::Review, _) => "review",
            (Stage::Finished, _) => "finished",
            (Stage::Restoring, Some("running" | "queued")) => "running",
            (Stage::Restoring, Some("paused")) => "paused",
            (Stage::Restoring, _) => "stopped",
        };
        let total = manifest.total();
        // The live count while a run is measuring, and the record otherwise:
        // after a restart the live count starts from nothing.
        let done = if state == "running" {
            live.done.max(manifest.done())
        } else {
            manifest.done()
        };
        let current = manifest.current();
        let steps = manifest
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| RecoveryStep {
                key: s.step.key.clone(),
                bytes: s.step.bytes,
                status: if s.status == Status::Done {
                    "done"
                } else if Some(i) == current && manifest.stage == Stage::Restoring {
                    "current"
                } else {
                    "pending"
                }
                .to_string(),
            })
            .collect();
        let eta = if state == "running" {
            eta(done, total, live.rate()).map_or(-1, |s| s as i64)
        } else {
            -1
        };
        RecoveryStatus {
            state: state.into(),
            job: self.job().unwrap_or(0),
            archive: manifest.archive.clone(),
            taken: manifest.taken,
            done,
            total,
            eta,
            current: if state == "running" {
                live.current.clone()
            } else {
                String::new()
            },
            steps,
            restored: manifest.restored(),
            conflicts: count_files(&self.held()),
            error: if state == "stopped" {
                live.error.clone().unwrap_or_default()
            } else {
                String::new()
            },
        }
    }
}

/// Everything a run needs.
pub struct Driver {
    pub engine: Arc<dyn BackupEngine>,
    pub recovery: Arc<Recovery>,
    /// The catalogue, read to check what an interrupted fetch left behind.
    pub index: PathBuf,
    /// Where files replaced by a summary answer of "replace" are kept.
    pub stash: PathBuf,
}

/// Run the recovery on disk from where it is to the end, or to the next stop.
pub fn start(driver: Driver) -> JobStream {
    let (sink, stream) = JobStream::channel(EVENT_BUFFER);
    tokio::spawn(async move {
        let outcome = driver.run(&sink).await;
        match &outcome {
            Ok(_) => info!("the recovery has finished"),
            Err(EngineError::Cancelled) => info!("the recovery has stopped where it was"),
            Err(error) => {
                warn!(%error, "the recovery stopped");
                driver.recovery.failed(error);
            }
        }
        sink.send(JobEvent::Finished(outcome)).await;
    });
    stream
}

impl Driver {
    async fn run(&self, sink: &JobSink) -> Result<JobSummary, EngineError> {
        let _turn = self.recovery.quiet().await;
        let Some(mut manifest) = self.recovery.load() else {
            return Ok(JobSummary::default());
        };
        if manifest.stage != Stage::Restoring {
            return Ok(JobSummary::default());
        }
        self.recovery.set_pausing(false);
        manifest.paused = false;
        self.save(&manifest)?;
        self.recovery.live.lock().unwrap().error = None;

        let outcome = self.steps(&mut manifest, sink).await;
        if matches!(outcome, Err(EngineError::Cancelled)) {
            manifest.paused = self.recovery.pausing.load(Ordering::SeqCst);
            self.save(&manifest)?;
        }
        outcome?;
        self.finish(&mut manifest).await?;
        Ok(JobSummary::default())
    }

    fn save(&self, manifest: &Manifest) -> Result<(), EngineError> {
        self.recovery.save(manifest).map_err(|e| {
            EngineError::Local(format!("the recovery record could not be written: {e}"))
        })
    }

    async fn steps(&self, manifest: &mut Manifest, sink: &JobSink) -> Result<(), EngineError> {
        let total = manifest.total();
        for index in 0..manifest.steps.len() {
            if manifest.steps[index].status == Status::Done {
                continue;
            }
            if sink.is_cancelled() {
                return Err(EngineError::Cancelled);
            }
            let mut meter = Meter::new(Arc::clone(&self.recovery), manifest.done(), total);
            let key = manifest.steps[index].step.key.clone();
            info!(
                step = key,
                bytes = manifest.steps[index].step.bytes,
                "restoring a step"
            );

            if manifest.steps[index].status == Status::Pending {
                manifest.steps[index].status = Status::Fetching;
                self.save(manifest)?;
            }
            if manifest.steps[index].status == Status::Fetching {
                self.fetch(manifest, index, sink, &mut meter).await?;
                manifest.steps[index].status = Status::Placing;
                self.save(manifest)?;
            }
            meter.fetched(manifest.steps[index].step.bytes);
            meter.report(sink).await?;
            self.place(manifest, index, sink).await?;
            manifest.steps[index].status = Status::Done;
            self.save(manifest)?;
            remove(&self.recovery.staging());
            info!(
                step = key,
                restored = manifest.steps[index].restored,
                set_aside = manifest.steps[index].held,
                failed = manifest.steps[index].failed,
                "step restored"
            );
        }
        Ok(())
    }

    /// Fetch step `index` into the working folder: all of it, or what an
    /// earlier run did not finish.
    async fn fetch(
        &self,
        manifest: &Manifest,
        index: usize,
        sink: &JobSink,
        meter: &mut Meter,
    ) -> Result<(), EngineError> {
        let step = &manifest.steps[index].step;
        let staging = self.recovery.staging();
        let archive = ArchiveId(manifest.archive.clone());

        let resumed = if staging.exists() {
            self.unfinished(manifest, index).await
        } else {
            None
        };
        // Whatever is already whole takes no more room. Checked before
        // anything is fetched, because finding out part-way through a folder
        // of photographs is finding out after filling the disk.
        let have = resumed.as_ref().map_or(0, |left| left.have);
        enough_room(&staging, &step.key, step.bytes.saturating_sub(have))?;

        let patterns = match resumed {
            Some(left) => {
                meter.already(left.have);
                if left.missing.is_empty() {
                    info!(step = step.key, "everything was fetched before the stop");
                    return Ok(());
                }
                info!(
                    step = step.key,
                    kept = left.kept,
                    missing = left.missing.len(),
                    "carrying on from where the fetch stopped"
                );
                layout::patterns(&manifest.never, &[], &left.missing)
            }
            None => None,
        };
        let patterns = match patterns {
            Some(text) => text,
            None => {
                fresh(&staging).map_err(local)?;
                meter.already(0);
                match layout::patterns(&manifest.never, &step.members, &[]) {
                    Some(text) => text,
                    // A folder whose very name a patterns file cannot hold:
                    // fetched by name instead, and the parts that must never
                    // come back removed before anything is compared.
                    None => {
                        let stream = self
                            .engine
                            .extract(&archive, &step.members, &staging)
                            .await?;
                        return meter.watch(stream, sink, &staging, &manifest.source).await;
                    }
                }
            }
        };
        let patterns_file = self.recovery.patterns_file();
        std::fs::write(&patterns_file, patterns).map_err(local)?;
        let stream = self
            .engine
            .extract_patterns(&archive, &patterns_file, &staging)
            .await?;
        meter.watch(stream, sink, &staging, &manifest.source).await
    }

    /// What an interrupted fetch of step `index` left: the files that are
    /// whole, which are kept, and the members still to fetch. `None` when it
    /// cannot be told, and the step is fetched again from the start.
    async fn unfinished(&self, manifest: &Manifest, index: usize) -> Option<Unfinished> {
        let index_path = self.index.clone();
        let staging = self.recovery.staging();
        let archive = manifest.archive.clone();
        let members = manifest.steps[index].step.members.clone();
        let never = manifest.never.clone();
        tokio::task::spawn_blocking(move || {
            let reader = IndexReader::open(&index_path)
                .map_err(|error| warn!(%error, "the catalogue could not be read to resume"))
                .ok()?;
            let seq = reader.seq_of(&archive).ok()??;
            let mut left = Unfinished::default();
            for member in &members {
                reader
                    .each_member(member, seq, |entry| {
                        if never.iter().any(|n| layout::within(&entry.path, n)) {
                            return;
                        }
                        let staged = staging.join(&entry.path);
                        if whole(&staged, &entry) {
                            left.kept += 1;
                            if entry.kind == Kind::File {
                                left.have += entry.size.max(0) as u64;
                            }
                        } else {
                            left.missing.push(entry.path);
                        }
                    })
                    .map_err(|error| warn!(%error, "the catalogue could not be read to resume"))
                    .ok()?;
            }
            Some(left)
        })
        .await
        .ok()
        .flatten()
    }

    /// Move step `index` into place, and set aside what clashes.
    async fn place(
        &self,
        manifest: &mut Manifest,
        index: usize,
        sink: &JobSink,
    ) -> Result<(), EngineError> {
        let staging = self.recovery.staging();
        let root = if manifest.source.is_empty() {
            staging.clone()
        } else {
            staging.join(&manifest.source)
        };
        let asked: Vec<PathBuf> = manifest.steps[index]
            .step
            .members
            .iter()
            .filter_map(|member| relative(member, &manifest.source))
            .filter(|inside| !inside.is_empty())
            .map(PathBuf::from)
            .collect();
        let never: Vec<PathBuf> = manifest.never.iter().map(|n| staging.join(n)).collect();
        let archive = manifest.archive.clone();
        let dest = manifest.dest.clone();
        let stash = self.stash.clone();
        let held = self.recovery.held();
        let added = self.recovery.added();
        let stop = sink.token();

        let placed = tokio::task::spawn_blocking(move || -> Result<Placed, EngineError> {
            for path in &never {
                remove(path);
            }
            let plan = restore::plan(&archive, &root, &dest, &asked)
                .map_err(|e| local_text(e.to_string()))?;
            let mut log = AddedLog::open(&added).map_err(local)?;
            let report = restore::execute_watched(
                &plan,
                &Decisions::all(Decision::Skip),
                &stash,
                SystemTime::now(),
                &|| stop.is_cancelled(),
                &mut |moved| log.record(moved),
            )
            .map_err(|error| match error {
                restore::RestoreError::NotEnoughSpace { .. } => EngineError::LocalDiskFull,
                other => local_text(other.to_string()),
            })?;
            for (path, why) in &report.failures {
                warn!(path = %path.display(), why, "a file could not be restored");
            }
            if report.stopped {
                return Ok(Placed::Stopped);
            }
            let aside = restore::set_aside(&plan, &held);
            for (path, why) in &aside.failures {
                warn!(path = %path.display(), why, "a clashing file could not be set aside");
            }
            Ok(Placed::Done {
                restored: report.log.restored() as u64,
                held: aside.restored as u64,
                failed: (report.failures.len() + aside.failures.len()) as u64,
            })
        })
        .await
        .map_err(|e| local_text(e.to_string()))??;

        match placed {
            Placed::Stopped => Err(EngineError::Cancelled),
            Placed::Done {
                restored,
                held,
                failed,
            } => {
                let step = &mut manifest.steps[index];
                step.restored += restored;
                step.held += held;
                step.failed += failed;
                Ok(())
            }
        }
    }

    /// Every step is done: settle what clashed, and say where that leaves the
    /// recovery.
    async fn finish(&self, manifest: &mut Manifest) -> Result<(), EngineError> {
        remove(&self.recovery.staging());
        remove(&self.recovery.patterns_file());
        let held = self.recovery.held();
        let decision = Decision::parse(&manifest.policy);
        if has_anything(&held) {
            if let Some(decision) = decision {
                // Somebody without a window asked for one answer to every
                // clash. Given now, through the same stash as ever.
                let archive = manifest.archive.clone();
                let dest = manifest.dest.clone();
                let stash = self.stash.clone();
                let aside = held.clone();
                tokio::task::spawn_blocking(move || -> Result<(), EngineError> {
                    let asked = top_level(&aside);
                    let plan = restore::plan(&archive, &aside, &dest, &asked)
                        .map_err(|e| local_text(e.to_string()))?;
                    let report = restore::execute(
                        &plan,
                        &Decisions::all(decision),
                        &stash,
                        SystemTime::now(),
                    )
                    .map_err(|e| local_text(e.to_string()))?;
                    info!(
                        answer = decision.as_str(),
                        restored = report.restored,
                        "the clashing files were settled"
                    );
                    Ok(())
                })
                .await
                .map_err(|e| local_text(e.to_string()))??;
                remove(&held);
            }
        }
        manifest.stage = if has_anything(&held) {
            Stage::Review
        } else {
            Stage::Finished
        };
        self.save(manifest)
    }
}

/// Room to spare on this computer when a step needs `bytes` more.
const MARGIN: u64 = 64 * 1024 * 1024;

/// Refuse a step there is no room for, saying how much is needed.
///
/// Reported as a failure of the restore, not as the "local disk full" health
/// row: that row is about backups that cannot protect changes, and its words
/// and its fix are for that. The recovery's own window says what this is.
fn enough_room(staging: &Path, step: &str, bytes: u64) -> Result<(), EngineError> {
    let Ok(free) = restore::free_space(staging) else {
        return Ok(());
    };
    if free >= bytes.saturating_add(MARGIN) {
        return Ok(());
    }
    let name = if step == layout::THE_REST {
        "the remaining files".to_string()
    } else {
        step.to_string()
    };
    Err(EngineError::Local(format!(
        "there is not enough space on this computer for {name}: {} needed and {} free",
        gigabytes(bytes.saturating_add(MARGIN)),
        gigabytes(free)
    )))
}

fn gigabytes(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

#[derive(Debug, Default)]
struct Unfinished {
    /// Members still to fetch.
    missing: Vec<String>,
    /// Members already whole.
    kept: u64,
    /// Bytes of the files already whole.
    have: u64,
}

enum Placed {
    Stopped,
    Done {
        restored: u64,
        held: u64,
        failed: u64,
    },
}

/// Whether `staged` is the whole of `entry`, as the catalogue knows it.
///
/// A file must match in size and modification time. Borg writes the contents
/// first and the time last, so a file it was writing when it was stopped has
/// the time it was stopped at, not the one in the backup.
fn whole(staged: &Path, entry: &backtrack_core::index::Member) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(facts) = std::fs::symlink_metadata(staged) else {
        return false;
    };
    let kind = facts.file_type();
    match entry.kind {
        Kind::Dir => kind.is_dir(),
        Kind::Symlink => kind.is_symlink(),
        Kind::Other => true,
        Kind::File => {
            let mtime = facts.mtime() * 1_000_000 + facts.mtime_nsec() / 1_000;
            let whole = kind.is_file()
                && facts.len() == entry.size.max(0) as u64
                && (mtime - entry.mtime).abs() <= MTIME_TOLERANCE_MICROS;
            if !whole {
                // Not worth keeping, and in the way of the copy that will be.
                let _ = std::fs::remove_file(staged);
            }
            whole
        }
    }
}

/// Bytes restored while a step is fetched: what was there already, what has
/// been fetched since, and how far into the file being written Borg is.
struct Meter {
    recovery: Arc<Recovery>,
    /// Bytes of the steps before this one.
    before: u64,
    total: u64,
    /// Bytes of this step already whole when this fetch began.
    have: u64,
    /// Bytes of the files fetched whole since.
    fetched: u64,
    /// This step's size, once it is known to be fetched entirely.
    step: Option<u64>,
    /// The file Borg is writing.
    writing: Option<PathBuf>,
    source: String,
    /// When progress was last written to the log; never, for a step just
    /// begun, so every step says where it starts from.
    last_log: Option<Instant>,
}

impl Meter {
    fn new(recovery: Arc<Recovery>, before: u64, total: u64) -> Meter {
        // A new step starts a new measurement: a resumed one arrives with
        // bytes already whole, and counting them as moved in the last minute
        // would promise a rate nothing is achieving.
        recovery.live.lock().unwrap().samples.clear();
        Meter {
            recovery,
            before,
            total,
            have: 0,
            fetched: 0,
            step: None,
            writing: None,
            source: String::new(),
            last_log: None,
        }
    }

    fn already(&mut self, bytes: u64) {
        self.have = bytes;
        self.fetched = 0;
    }

    fn fetched(&mut self, bytes: u64) {
        self.step = Some(bytes);
        self.writing = None;
    }

    /// Borg has reached `path`: whatever it was writing before is whole.
    fn reached(&mut self, path: PathBuf) {
        self.settle_writing();
        self.writing = Some(path);
    }

    fn settle_writing(&mut self) {
        if let Some(previous) = self.writing.take() {
            if let Ok(facts) = std::fs::symlink_metadata(&previous) {
                if facts.is_file() {
                    self.fetched += facts.len();
                }
            }
        }
    }

    fn done(&self) -> u64 {
        let step = match self.step {
            Some(bytes) => bytes,
            None => {
                let writing = self
                    .writing
                    .as_ref()
                    .and_then(|p| std::fs::symlink_metadata(p).ok())
                    .filter(|f| f.is_file())
                    .map_or(0, |f| f.len());
                self.have + self.fetched + writing
            }
        };
        (self.before + step).min(self.total)
    }

    /// Measure, tell the registry, and now and then the log.
    async fn report(&mut self, sink: &JobSink) -> Result<(), EngineError> {
        let done = self.done();
        let now = Instant::now();
        let current = self
            .writing
            .as_ref()
            .and_then(|path| path.to_str())
            .and_then(|path| {
                let staging = self.recovery.staging();
                let inside = path
                    .strip_prefix(staging.to_str()?)?
                    .trim_start_matches('/');
                relative(inside, &self.source).map(str::to_string)
            })
            .unwrap_or_default();
        let rate = {
            let mut live = self.recovery.live.lock().unwrap();
            live.measure(now, done);
            live.current = current;
            live.rate()
        };
        if self
            .last_log
            .is_none_or(|then| now.duration_since(then) >= LOG_EVERY)
        {
            self.last_log = Some(now);
            info!(
                done,
                total = self.total,
                bytes_per_second = rate.map(|r| r as u64),
                eta_seconds = eta(done, self.total, rate),
                "recovery progress"
            );
        }
        if sink
            .send(JobEvent::Progress {
                current: done,
                total: Some(self.total),
                phase: PHASE.to_string(),
            })
            .await
        {
            Ok(())
        } else {
            Err(EngineError::Cancelled)
        }
    }

    /// Follow an extraction to its end, measuring as it goes.
    async fn watch(
        &mut self,
        mut stream: JobStream,
        sink: &JobSink,
        staging: &Path,
        source: &str,
    ) -> Result<(), EngineError> {
        self.source = source.to_string();
        let stop = sink.token();
        let mut tick = tokio::time::interval(TICK);
        loop {
            tokio::select! {
                biased;
                _ = stop.cancelled() => {
                    // Dropping the stream stops Borg. What it had finished
                    // stays in the working folder for the next run.
                    drop(stream);
                    return Err(EngineError::Cancelled);
                }
                _ = tick.tick() => self.report(sink).await?,
                event = stream.next() => match event {
                    Some(JobEvent::ItemDone { path }) => self.reached(staging.join(path)),
                    Some(JobEvent::Log { level, msg }) => {
                        if !sink.send(JobEvent::Log { level, msg }).await {
                            return Err(EngineError::Cancelled);
                        }
                    }
                    Some(JobEvent::Progress { .. }) => {}
                    Some(JobEvent::Finished(outcome)) => {
                        self.settle_writing();
                        outcome?;
                        return Ok(());
                    }
                    None => {
                        return Err(EngineError::BorgFailed {
                            code: -1,
                            stderr: "borg ended a recovery step without an outcome".into(),
                        })
                    }
                },
            }
        }
    }
}

/// The record of what a recovery put on this computer, so Discard can take
/// exactly that away again and nothing else.
///
/// One record per file or folder made, ending in a NUL so that no name can
/// break the format: a kind, the size and modification time the file had when
/// it arrived, and its path.
struct AddedLog {
    file: std::fs::File,
}

impl AddedLog {
    fn open(path: &Path) -> std::io::Result<AddedLog> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(AddedLog {
            file: std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?,
        })
    }

    fn record(&mut self, moved: &Move) {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::MetadataExt;
        let (kind, path) = match moved {
            Move::Added { path } => ('F', path),
            Move::MadeDir { path } => ('D', path),
            // A recovery answers every clash with "leave it alone", so it
            // never replaces or renames anything.
            Move::Replaced { .. } | Move::KeptBoth { .. } => return,
        };
        let (size, mtime) = std::fs::symlink_metadata(path)
            .map(|f| (f.len(), f.mtime() * 1_000_000 + f.mtime_nsec() / 1_000))
            .unwrap_or((0, 0));
        let mut record = format!("{kind}\t{size}\t{mtime}\t").into_bytes();
        record.extend_from_slice(path.as_os_str().as_bytes());
        record.push(0);
        // Unbuffered, a record per write: a daemon killed mid-step loses
        // nothing it had already moved.
        if let Err(error) = self.file.write_all(&record) {
            warn!(%error, "what was restored could not be written down");
        }
    }
}

/// What Discard did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Discarded {
    pub files: u64,
    /// Files changed since they were restored, and so kept.
    pub kept: u64,
    pub folders: u64,
}

/// Take away what a recovery put on this computer: every file it added that
/// is still as it arrived, then every folder it made that is now empty.
///
/// A file somebody has changed since is theirs now, and stays; so does a
/// folder with anything else in it. Nothing the recovery did not add is
/// touched, because nothing else is in the record.
pub fn discard(record: &Path) -> Discarded {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    let mut done = Discarded::default();
    let Ok(bytes) = std::fs::read(record) else {
        return done;
    };
    let mut folders = Vec::new();
    for entry in bytes.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let mut fields = entry.splitn(4, |b| *b == b'\t');
        let (Some(kind), Some(size), Some(mtime), Some(path)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(path));
        let number =
            |field: &[u8]| -> Option<i64> { std::str::from_utf8(field).ok()?.parse().ok() };
        match kind {
            b"F" => {
                let Ok(facts) = std::fs::symlink_metadata(&path) else {
                    continue;
                };
                let now = (
                    facts.len() as i64,
                    facts.mtime() * 1_000_000 + facts.mtime_nsec() / 1_000,
                );
                if Some(now) == number(size).zip(number(mtime)) && !facts.is_dir() {
                    if std::fs::remove_file(&path).is_ok() {
                        done.files += 1;
                    }
                } else {
                    done.kept += 1;
                }
            }
            b"D" => folders.push(path),
            _ => {}
        }
    }
    // Deepest first, so a folder made inside another is gone before the
    // outer one is looked at.
    folders.sort_by_key(|f| std::cmp::Reverse(f.components().count()));
    for folder in folders {
        if std::fs::remove_dir(&folder).is_ok() {
            done.folders += 1;
        }
    }
    done
}

/// Discard if asked, then forget the recovery. What `CancelRecovery` runs once
/// the recovery itself has stopped.
pub fn cancel(recovery: &Recovery, discard_it: bool) -> Option<Discarded> {
    let discarded = discard_it.then(|| discard(&recovery.added()));
    if let Some(d) = &discarded {
        info!(
            files = d.files,
            kept = d.kept,
            folders = d.folders,
            "what the recovery restored was taken away"
        );
    }
    recovery.clear();
    discarded
}

/// The names directly inside `dir`.
pub fn top_level(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| PathBuf::from(entry.file_name()))
        .collect()
}

fn has_anything(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
}

/// Files (not folders) at or below `dir`.
fn count_files(dir: &Path) -> u64 {
    let mut count = 0;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(here) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&here) else {
            continue;
        };
        for entry in entries.flatten() {
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => pending.push(entry.path()),
                Ok(_) => count += 1,
                Err(_) => {}
            }
        }
    }
    count
}

/// An empty working folder, whatever was there.
fn fresh(dir: &Path) -> std::io::Result<()> {
    remove(dir);
    std::fs::create_dir_all(dir)
}

fn remove(path: &Path) {
    let outcome = match std::fs::symlink_metadata(path) {
        Ok(facts) if facts.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(_) => return,
    };
    if let Err(error) = outcome {
        warn!(path = %path.display(), %error, "could not be cleared");
    }
}

fn local(error: std::io::Error) -> EngineError {
    EngineError::Local(error.to_string())
}

fn local_text(message: String) -> EngineError {
    EngineError::Local(message)
}

pub fn seconds(at: SystemTime) -> i64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The name of the person this daemon runs for.
pub fn user_name() -> String {
    std::env::var("USER")
        .ok()
        .filter(|name| !name.is_empty())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .and_then(|home| home.file_name().map(|n| n.to_string_lossy().into_owned()))
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest::new(
            "bt-old-20261004T220000Z",
            1_791_100_000,
            layout::Layout {
                source: "home/old".into(),
                steps: vec![
                    layout::Step {
                        key: "Documents".into(),
                        members: vec!["home/old/Documents".into()],
                        bytes: 300,
                        files: 2,
                    },
                    layout::Step {
                        key: "Pictures".into(),
                        members: vec!["home/old/Pictures".into()],
                        bytes: 700,
                        files: 1,
                    },
                ],
                never: vec!["home/old/.local/share/backtrack".into()],
            },
            PathBuf::from("/home/new"),
            "ask",
            vec![PathBuf::from("/home/new/Documents")],
        )
    }

    fn recovery(dir: &Path) -> Recovery {
        Recovery::new(dir.join("dr-job.json"), dir.join("recovery"))
    }

    #[test]
    fn the_record_survives_being_written_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let r = recovery(dir.path());
        assert_eq!(r.load(), None);
        assert!(!r.restoring());

        let mut m = manifest();
        m.steps[0].status = Status::Done;
        r.save(&m).unwrap();
        assert_eq!(r.load(), Some(m.clone()));
        assert!(r.restoring(), "steps left hold the schedule");

        // A new daemon reads the same answer from disk.
        let again = recovery(dir.path());
        assert!(again.restoring());
        assert_eq!(again.load().unwrap().current(), Some(1));

        m.stage = Stage::Review;
        r.save(&m).unwrap();
        assert!(!r.restoring(), "a summary waiting holds nothing back");
        r.clear();
        assert_eq!(r.load(), None);
    }

    #[test]
    fn a_damaged_record_is_no_recovery_rather_than_a_guess() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("dr-job.json"), b"{ not json").unwrap();
        let r = recovery(dir.path());
        assert_eq!(r.load(), None);
        assert!(!r.restoring());
    }

    #[test]
    fn progress_counts_the_steps_that_are_done() {
        let mut m = manifest();
        assert_eq!((m.done(), m.total()), (0, 1_000));
        m.steps[0].status = Status::Done;
        m.steps[1].status = Status::Placing;
        assert_eq!(m.done(), 300);
        assert_eq!(m.current(), Some(1));
    }

    #[test]
    fn the_time_left_is_measured_over_the_last_minute_and_not_before_it_settles() {
        let mut live = Live::default();
        let start = Instant::now();
        live.measure(start, 0);
        live.measure(start + Duration::from_secs(5), 5_000);
        assert_eq!(live.rate(), None, "five seconds is not a rate");

        live.measure(start + Duration::from_secs(20), 20_000);
        let rate = live.rate().unwrap();
        assert!((rate - 1_000.0).abs() < 1.0, "{rate}");
        assert_eq!(eta(20_000, 80_000, Some(rate)), Some(60));

        // A minute later the slow start has dropped out of the window and only
        // the last minute counts.
        live.measure(start + Duration::from_secs(70), 70_000);
        live.measure(start + Duration::from_secs(90), 170_000);
        let rate = live.rate().unwrap();
        assert!(rate > 3_000.0, "{rate}");

        // Nothing moving is not a rate of zero and an infinite wait.
        let mut stuck = Live::default();
        stuck.measure(start, 100);
        stuck.measure(start + Duration::from_secs(30), 100);
        assert_eq!(stuck.rate(), None);
        assert_eq!(eta(0, 100, None), None);
    }

    #[test]
    fn a_file_is_whole_only_with_the_size_and_time_the_backup_gave_it() {
        use backtrack_core::index::Member;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.jpg");
        std::fs::write(&path, b"12345").unwrap();
        let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(when)
            .unwrap();
        let entry = |size: i64, mtime: i64| Member {
            path: "photo.jpg".into(),
            kind: Kind::File,
            size,
            mtime,
        };
        let micros = 1_700_000_000 * 1_000_000;

        assert!(whole(&path, &entry(5, micros)));
        assert!(
            whole(&path, &entry(5, micros + 1)),
            "within Borg's rounding"
        );
        // Cut short: the wrong size, and taken out of the way.
        assert!(!whole(&path, &entry(9, micros)));
        assert!(!path.exists());

        // Written in full but stopped before its time was set.
        std::fs::write(&path, b"12345").unwrap();
        assert!(!whole(&path, &entry(5, micros)));
        assert!(!whole(&dir.path().join("never-fetched"), &entry(5, micros)));
    }

    #[test]
    fn discard_takes_away_what_was_added_and_nothing_anybody_has_touched() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join("Documents/tax")).unwrap();
        std::fs::write(home.join("Documents/report.odt"), b"restored").unwrap();
        std::fs::write(home.join("Documents/tax/2025.pdf"), b"restored").unwrap();
        std::fs::write(home.join("Documents/notes.txt"), b"restored").unwrap();
        std::fs::write(home.join("mine.txt"), b"was here before").unwrap();

        let record = dir.path().join("added");
        let mut log = AddedLog::open(&record).unwrap();
        log.record(&Move::MadeDir {
            path: home.join("Documents"),
        });
        log.record(&Move::MadeDir {
            path: home.join("Documents/tax"),
        });
        for file in [
            "Documents/report.odt",
            "Documents/tax/2025.pdf",
            "Documents/notes.txt",
        ] {
            log.record(&Move::Added {
                path: home.join(file),
            });
        }
        drop(log);
        // Edited after it was restored: somebody's work now.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(home.join("Documents/notes.txt"), b"edited since").unwrap();

        let done = discard(&record);

        assert_eq!(
            done,
            Discarded {
                files: 2,
                kept: 1,
                folders: 1
            }
        );
        assert!(
            !home.join("Documents/tax").exists(),
            "emptied and made by the restore"
        );
        assert!(!home.join("Documents/report.odt").exists());
        assert_eq!(
            std::fs::read(home.join("Documents/notes.txt")).unwrap(),
            b"edited since"
        );
        assert!(
            home.join("Documents").is_dir(),
            "it still holds the edited file"
        );
        assert_eq!(
            std::fs::read(home.join("mine.txt")).unwrap(),
            b"was here before"
        );
    }

    #[test]
    fn a_step_there_is_no_room_for_is_refused_before_anything_is_fetched() {
        let dir = tempfile::tempdir().unwrap();
        assert!(enough_room(dir.path(), "Documents", 1_000).is_ok());
        let refused = enough_room(dir.path(), "Pictures", u64::MAX / 2).unwrap_err();
        let text = refused.to_string();
        assert!(
            text.contains("not enough space on this computer for Pictures"),
            "{text}"
        );
        assert!(text.contains("GB free"), "{text}");
        assert!(enough_room(dir.path(), layout::THE_REST, u64::MAX / 2)
            .unwrap_err()
            .to_string()
            .contains("the remaining files"));
    }

    #[test]
    fn a_name_with_a_line_break_survives_the_record() {
        let dir = tempfile::tempdir().unwrap();
        let odd = dir.path().join("two\nlines\tand a tab");
        std::fs::write(&odd, b"x").unwrap();
        let record = dir.path().join("added");
        AddedLog::open(&record)
            .unwrap()
            .record(&Move::Added { path: odd.clone() });
        assert_eq!(discard(&record).files, 1);
        assert!(!odd.exists());
    }

    #[test]
    fn the_status_says_where_it_is_and_what_is_next() {
        let dir = tempfile::tempdir().unwrap();
        let r = recovery(dir.path());
        assert_eq!(r.status(None).state, "none");

        let mut m = manifest();
        m.steps[0].status = Status::Done;
        m.steps[0].restored = 2;
        r.save(&m).unwrap();
        r.set_job(7);

        let paused = r.status(Some("paused"));
        assert_eq!(paused.state, "paused");
        assert_eq!(paused.job, 7);
        assert_eq!((paused.done, paused.total), (300, 1_000));
        assert_eq!(paused.eta, -1, "paused has no time left to speak of");
        let steps: Vec<(&str, &str)> = paused
            .steps
            .iter()
            .map(|s| (s.key.as_str(), s.status.as_str()))
            .collect();
        assert_eq!(steps, [("Documents", "done"), ("Pictures", "current")]);
        assert_eq!(paused.restored, 2);

        r.failed(&EngineError::RepoUnreachable);
        let stopped = r.status(Some("failed"));
        assert_eq!(stopped.state, "stopped");
        assert!(!stopped.error.is_empty());
    }

    #[test]
    fn a_finished_summary_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let r = recovery(dir.path());
        let mut m = manifest();
        m.stage = Stage::Review;
        m.handed_over = true;
        r.save(&m).unwrap();
        std::fs::create_dir_all(r.held().join("Documents")).unwrap();
        std::fs::write(r.held().join("Documents/clash.txt"), b"backup").unwrap();

        r.tidy();
        assert_eq!(r.status(None).state, "review");
        assert_eq!(r.status(None).conflicts, 1);

        // The summary was answered, which takes the set-aside copies away.
        std::fs::remove_dir_all(r.held()).unwrap();
        r.tidy();
        assert_eq!(r.load(), None);
        assert!(!dir.path().join("recovery").exists());
    }
}
