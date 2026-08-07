// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The live job registry: submission, queuing, execution, cancellation.
//!
//! Submitting a job returns a [`JobId`] immediately and never blocks — every
//! D-Bus method that starts long work answers straight away and the caller
//! follows the [`JobUpdate`] broadcast. Whether the job *starts* immediately is
//! the [`RepoGuard`]'s decision.
//!
//! Jobs are submitted as a factory rather than a running stream, for two
//! reasons: a queued job must not touch Borg until it is admitted, and a paused
//! job needs to be startable a second time.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use backtrack_core::engine::{EngineError, JobEvent, JobStream, LogLevel};
use futures::future::BoxFuture;
use futures::StreamExt;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::{JobError, JobId, JobKind, JobState, Outcome, ProgressThrottle, RepoGuard};

/// How many updates the broadcast buffers before a slow subscriber starts
/// missing them. A lagging subscriber loses intermediate progress, never the
/// state changes it needs to stay correct — those are re-derivable from
/// [`JobRegistry::snapshot`].
const UPDATE_BUFFER: usize = 256;

/// Starts the underlying engine work. Called once per run attempt, so a paused
/// job can be started again on resume.
pub type JobFactory =
    Arc<dyn Fn() -> BoxFuture<'static, backtrack_core::engine::Result<JobStream>> + Send + Sync>;

/// What subscribers hear. The D-Bus layer (S03-T3) turns these into
/// `BackupProgress` / `RestoreProgress` / `IndexingProgress` and
/// `StatusChanged`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobUpdate {
    /// The job moved to a new state.
    State {
        id: JobId,
        kind: JobKind,
        state: JobState,
    },
    /// Progress within the current state, already rate-limited to 4 Hz.
    Progress {
        id: JobId,
        kind: JobKind,
        current: u64,
        total: Option<u64>,
        phase: String,
    },
}

/// A job as seen from outside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobSnapshot {
    pub id: JobId,
    pub kind: JobKind,
    pub state: JobState,
}

struct Job {
    kind: JobKind,
    state: JobState,
    factory: JobFactory,
    /// Trips to stop the current run. Replaced on each start, so a cancel
    /// aimed at a previous run cannot stop the next one.
    cancel: CancellationToken,
}

struct Inner {
    next_id: JobId,
    jobs: BTreeMap<JobId, Job>,
    queue: VecDeque<JobId>,
}

/// The daemon's one job registry.
pub struct JobRegistry {
    inner: Mutex<Inner>,
    updates: broadcast::Sender<JobUpdate>,
}

impl JobRegistry {
    pub fn new() -> Arc<JobRegistry> {
        let (updates, _) = broadcast::channel(UPDATE_BUFFER);
        Arc::new(JobRegistry {
            inner: Mutex::new(Inner {
                next_id: 1,
                jobs: BTreeMap::new(),
                queue: VecDeque::new(),
            }),
            updates,
        })
    }

    /// Listen to every job's state changes and progress.
    pub fn subscribe(&self) -> broadcast::Receiver<JobUpdate> {
        self.updates.subscribe()
    }

    /// Queue a job and try to start it. Returns its id straight away.
    pub fn submit(self: &Arc<Self>, kind: JobKind, factory: JobFactory) -> JobId {
        let id = {
            let mut inner = self.inner.lock().unwrap();
            let id = inner.next_id;
            inner.next_id += 1;
            inner.jobs.insert(
                id,
                Job {
                    kind,
                    state: JobState::Queued,
                    factory,
                    cancel: CancellationToken::new(),
                },
            );
            inner.queue.push_back(id);
            id
        };
        info!(job = id, kind = kind.as_str(), "job queued");
        self.publish(JobUpdate::State {
            id,
            kind,
            state: JobState::Queued,
        });
        self.pump();
        id
    }

    /// A job's current state, or [`JobError::NotFound`].
    pub fn snapshot(&self, id: JobId) -> Result<JobSnapshot, JobError> {
        let inner = self.inner.lock().unwrap();
        let job = inner.jobs.get(&id).ok_or(JobError::NotFound(id))?;
        Ok(JobSnapshot {
            id,
            kind: job.kind,
            state: job.state.clone(),
        })
    }

    /// Every job this daemon run has seen, oldest first.
    pub fn list(&self) -> Vec<JobSnapshot> {
        let inner = self.inner.lock().unwrap();
        inner
            .jobs
            .iter()
            .map(|(id, job)| JobSnapshot {
                id: *id,
                kind: job.kind,
                state: job.state.clone(),
            })
            .collect()
    }

    /// Stop a job. A queued job is cancelled outright; a running one enters
    /// `Cancelling` while the engine is torn down.
    pub fn cancel(self: &Arc<Self>, id: JobId) -> Result<(), JobError> {
        let (next, token) = {
            let mut inner = self.inner.lock().unwrap();
            let job = inner.jobs.get_mut(&id).ok_or(JobError::NotFound(id))?;
            match &job.state {
                s if s.is_terminal() => return Err(JobError::AlreadyFinished { id }),
                // Never started: nothing to tear down.
                JobState::Queued | JobState::Paused => (JobState::Done(Outcome::Cancelled), None),
                _ => (JobState::Cancelling, Some(job.cancel.clone())),
            }
        };
        self.transition(id, next)?;
        if let Some(token) = token {
            // The run task is selecting on this; it drops the JobStream, whose
            // own Drop signals Borg.
            token.cancel();
        } else {
            self.remove_from_queue(id);
            self.pump();
        }
        Ok(())
    }

    /// Pause a job between units of work. Only guided disaster recovery
    /// qualifies — see [`JobKind::is_pausable`] for why stopping a Borg process
    /// is not an option.
    pub fn pause(self: &Arc<Self>, id: JobId) -> Result<(), JobError> {
        let token = {
            let inner = self.inner.lock().unwrap();
            let job = inner.jobs.get(&id).ok_or(JobError::NotFound(id))?;
            if !job.kind.is_pausable() {
                return Err(JobError::NotPausable {
                    kind: job.kind.as_str(),
                });
            }
            if job.state.is_terminal() {
                return Err(JobError::AlreadyFinished { id });
            }
            job.cancel.clone()
        };
        self.transition(id, JobState::Paused)?;
        // Stopping the current unit of work *is* the checkpoint: the job's
        // recorded position survives in the index, and resume starts the next
        // unit from there.
        token.cancel();
        self.pump();
        Ok(())
    }

    /// Put a paused job back in the queue.
    pub fn resume(self: &Arc<Self>, id: JobId) -> Result<(), JobError> {
        {
            let inner = self.inner.lock().unwrap();
            let job = inner.jobs.get(&id).ok_or(JobError::NotFound(id))?;
            if job.state != JobState::Paused {
                return Err(JobError::NotPaused { id });
            }
        }
        self.transition(id, JobState::Queued)?;
        {
            let mut inner = self.inner.lock().unwrap();
            inner.queue.push_back(id);
        }
        self.pump();
        Ok(())
    }

    /// Admit whatever the repository guard now allows.
    ///
    /// The queue is *scanned*, not just peeked: a reader may pass a waiting
    /// writer. That is a deliberate choice against strict fairness — a user who
    /// clicks Restore while an hourly backup is queued behind a long index
    /// backfill should not wait for both. The writer it passes is a scheduled
    /// backup that will be retried on the next tick, and the readers ahead of it
    /// are finite work, so it cannot be starved indefinitely.
    fn pump(self: &Arc<Self>) {
        loop {
            let started = {
                let mut inner = self.inner.lock().unwrap();
                let guard = RepoGuard::from_running(
                    inner
                        .jobs
                        .values()
                        .filter(|j| j.state.holds_repo())
                        .map(|j| j.kind.access()),
                );
                let next = inner.queue.iter().position(|id| {
                    inner
                        .jobs
                        .get(id)
                        .is_some_and(|j| guard.admits(j.kind.access()))
                });
                match next {
                    None => None,
                    Some(pos) => {
                        let id = inner.queue.remove(pos).expect("position just found");
                        let job = inner.jobs.get_mut(&id).expect("queued job exists");
                        job.state = JobState::Running;
                        job.cancel = CancellationToken::new();
                        Some((id, job.kind, job.factory.clone(), job.cancel.clone()))
                    }
                }
            };

            let Some((id, kind, factory, cancel)) = started else {
                return;
            };
            info!(job = id, kind = kind.as_str(), "job started");
            self.publish(JobUpdate::State {
                id,
                kind,
                state: JobState::Running,
            });
            let registry = Arc::clone(self);
            tokio::spawn(async move { registry.run(id, kind, factory, cancel).await });
        }
    }

    /// Drive one job's engine stream to a terminal state.
    async fn run(
        self: Arc<Self>,
        id: JobId,
        kind: JobKind,
        factory: JobFactory,
        cancel: CancellationToken,
    ) {
        // Cancelling before the engine has even been asked to start is common:
        // the job was admitted a moment before the user changed their mind.
        let started = tokio::select! {
            _ = cancel.cancelled() => {
                self.settle(id, kind, JobState::Done(Outcome::Cancelled));
                return;
            }
            started = factory() => started,
        };

        let mut stream = match started {
            Ok(stream) => stream,
            Err(e) => {
                self.settle(id, kind, JobState::Failed(e));
                return;
            }
        };

        let mut throttle = ProgressThrottle::default();
        loop {
            let event = tokio::select! {
                _ = cancel.cancelled() => {
                    // Dropping the stream trips its token, which is what signals
                    // the Borg child. Do it before settling so the repository is
                    // actually free by the time the next job is admitted.
                    drop(stream);
                    self.settle(id, kind, self.cancellation_outcome(id));
                    return;
                }
                event = stream.next() => event,
            };

            match event {
                Some(JobEvent::Progress {
                    current,
                    total,
                    phase,
                }) => {
                    if throttle.should_emit(Instant::now(), &phase) {
                        self.publish(JobUpdate::Progress {
                            id,
                            kind,
                            current,
                            total,
                            phase,
                        });
                    }
                }
                Some(JobEvent::Log { level, msg }) => match level {
                    LogLevel::Error => error!(job = id, "{msg}"),
                    LogLevel::Warning => warn!(job = id, "{msg}"),
                    _ => debug!(job = id, "{msg}"),
                },
                Some(JobEvent::ItemDone { path }) => {
                    tracing::trace!(job = id, path, "item done")
                }
                Some(JobEvent::Finished(Ok(summary))) => {
                    // The last progress tick must land or the bar stops short.
                    throttle.release();
                    debug!(job = id, archive = ?summary.archive_id, "job finished");
                    self.settle(id, kind, JobState::Done(Outcome::Completed));
                    return;
                }
                Some(JobEvent::Finished(Err(EngineError::Cancelled))) => {
                    self.settle(id, kind, JobState::Done(Outcome::Cancelled));
                    return;
                }
                Some(JobEvent::Finished(Err(e))) => {
                    self.settle(id, kind, self.failure_outcome(id, e));
                    return;
                }
                None => {
                    // The engine contract puts a `Finished` before the end of
                    // the stream. Reaching here means it broke that contract, so
                    // we must not claim success: an unverified backup reported
                    // as complete is the one failure mode this product cannot
                    // have.
                    let outcome = self.failure_outcome(
                        id,
                        EngineError::BorgFailed {
                            code: -1,
                            stderr: "engine stream ended without a terminal event".into(),
                        },
                    );
                    self.settle(id, kind, outcome);
                    return;
                }
            }
        }
    }

    /// What a job that stopped mid-cancel should end as. Normally `Cancelled`,
    /// but a job that had already reached a terminal state keeps it.
    fn cancellation_outcome(&self, id: JobId) -> JobState {
        let inner = self.inner.lock().unwrap();
        match inner.jobs.get(&id) {
            Some(job) if job.state.is_terminal() => job.state.clone(),
            _ => JobState::Done(Outcome::Cancelled),
        }
    }

    /// What a failing job should end as. A failure raised *while cancelling* is
    /// reported as a cancellation: the user asked the job to stop, and raising a
    /// health banner for the wreckage of an intentional stop would be noise.
    fn failure_outcome(&self, id: JobId, error: EngineError) -> JobState {
        let inner = self.inner.lock().unwrap();
        match inner.jobs.get(&id) {
            Some(job) if job.state == JobState::Cancelling => {
                warn!(job = id, %error, "engine failed while cancelling; reporting as cancelled");
                JobState::Done(Outcome::Cancelled)
            }
            _ => JobState::Failed(error),
        }
    }

    /// Move a job to its terminal state and let the queue move on.
    fn settle(self: &Arc<Self>, id: JobId, kind: JobKind, state: JobState) {
        match &state {
            JobState::Failed(e) => error!(job = id, kind = kind.as_str(), %e, "job failed"),
            s => info!(
                job = id,
                kind = kind.as_str(),
                state = s.as_str(),
                "job settled"
            ),
        }
        if let Err(e) = self.transition(id, state) {
            error!(job = id, %e, "could not settle job");
        }
        self.pump();
    }

    /// Apply a state change, refusing any the machine does not allow.
    fn transition(&self, id: JobId, next: JobState) -> Result<(), JobError> {
        let kind = {
            let mut inner = self.inner.lock().unwrap();
            let job = inner.jobs.get_mut(&id).ok_or(JobError::NotFound(id))?;
            if !job.state.can_transition_to(&next) {
                return Err(JobError::IllegalTransition {
                    id,
                    from: job.state.as_str(),
                    to: next.as_str(),
                });
            }
            job.state = next.clone();
            job.kind
        };
        self.publish(JobUpdate::State {
            id,
            kind,
            state: next,
        });
        Ok(())
    }

    fn remove_from_queue(&self, id: JobId) {
        let mut inner = self.inner.lock().unwrap();
        inner.queue.retain(|queued| *queued != id);
    }

    /// Send an update. A send with no subscribers is not an error — the daemon
    /// runs perfectly well with no GUI attached.
    fn publish(&self, update: JobUpdate) {
        let _ = self.updates.send(update);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use backtrack_core::engine::{BackupEngine, CreateSpec, JobSummary};
    use backtrack_testkit::MockEngine;
    use std::time::Duration;

    fn spec() -> CreateSpec {
        CreateSpec {
            archive_name: "a".into(),
            sources: vec!["/tmp".into()],
            excludes: vec![],
            compression: Default::default(),
            one_file_system: true,
            created_at: std::time::SystemTime::UNIX_EPOCH,
            paths: vec![],
        }
    }

    /// A factory that runs `engine.create`.
    fn backup_factory(engine: Arc<MockEngine>) -> JobFactory {
        Arc::new(move || {
            let engine = Arc::clone(&engine);
            Box::pin(async move { engine.create(&spec()).await })
        })
    }

    /// A factory whose job never ends on its own.
    fn pending_factory() -> JobFactory {
        backup_factory(Arc::new(MockEngine::default().with_create_pending()))
    }

    /// Poll until `predicate` holds, or fail. Bounded so a broken registry fails
    /// the test rather than hanging it.
    async fn wait_until(
        registry: &Arc<JobRegistry>,
        id: JobId,
        label: &str,
        predicate: impl Fn(&JobState) -> bool,
    ) -> JobState {
        for _ in 0..500 {
            let state = registry.snapshot(id).expect("job exists").state;
            if predicate(&state) {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let state = registry.snapshot(id).expect("job exists").state;
        panic!("job {id} never reached {label}; stuck in {state:?}");
    }

    #[tokio::test]
    async fn a_completed_backup_ends_in_done_completed() {
        let engine = Arc::new(MockEngine::default().with_create_events(vec![
            JobEvent::Progress {
                current: 1,
                total: Some(1),
                phase: "archiving".into(),
            },
            JobEvent::Finished(Ok(JobSummary {
                archive_id: Some("mock-1".into()),
            })),
        ]));
        let registry = JobRegistry::new();
        let id = registry.submit(JobKind::Backup, backup_factory(engine));

        let state = wait_until(&registry, id, "terminal", |s| s.is_terminal()).await;
        assert_eq!(state, JobState::Done(Outcome::Completed));
    }

    #[tokio::test]
    async fn cancelling_a_running_backup_lands_in_done_cancelled() {
        // The stage's acceptance criterion.
        let registry = JobRegistry::new();
        let id = registry.submit(JobKind::Backup, pending_factory());

        wait_until(&registry, id, "running", |s| *s == JobState::Running).await;
        registry.cancel(id).expect("cancellable");

        let state = wait_until(&registry, id, "terminal", |s| s.is_terminal()).await;
        assert_eq!(
            state,
            JobState::Done(Outcome::Cancelled),
            "a cancelled job is done, not failed"
        );
    }

    #[tokio::test]
    async fn cancelling_a_queued_job_never_starts_it() {
        let registry = JobRegistry::new();
        let blocker = registry.submit(JobKind::Backup, pending_factory());
        wait_until(&registry, blocker, "running", |s| *s == JobState::Running).await;

        // Queued behind the exclusive backup.
        let queued = registry.submit(JobKind::Restore, pending_factory());
        assert_eq!(
            registry.snapshot(queued).unwrap().state,
            JobState::Queued,
            "a restore must not start alongside a backup"
        );

        registry.cancel(queued).expect("cancellable");
        assert_eq!(
            registry.snapshot(queued).unwrap().state,
            JobState::Done(Outcome::Cancelled)
        );

        registry.cancel(blocker).expect("cancellable");
        wait_until(&registry, blocker, "terminal", |s| s.is_terminal()).await;
    }

    #[tokio::test]
    async fn a_queued_job_starts_when_the_repository_frees_up() {
        let registry = JobRegistry::new();
        let backup = registry.submit(JobKind::Backup, pending_factory());
        wait_until(&registry, backup, "running", |s| *s == JobState::Running).await;

        let restore = registry.submit(JobKind::Restore, pending_factory());
        assert_eq!(registry.snapshot(restore).unwrap().state, JobState::Queued);

        registry.cancel(backup).expect("cancellable");
        wait_until(&registry, restore, "running", |s| *s == JobState::Running).await;

        registry.cancel(restore).expect("cancellable");
    }

    #[tokio::test]
    async fn a_restore_runs_alongside_indexing() {
        let registry = JobRegistry::new();
        let index = registry.submit(JobKind::Index, pending_factory());
        wait_until(&registry, index, "running", |s| *s == JobState::Running).await;

        let restore = registry.submit(JobKind::Restore, pending_factory());
        wait_until(&registry, restore, "running", |s| *s == JobState::Running).await;

        // And a backup waits for both.
        let backup = registry.submit(JobKind::Backup, pending_factory());
        assert_eq!(registry.snapshot(backup).unwrap().state, JobState::Queued);

        registry.cancel(index).unwrap();
        registry.cancel(restore).unwrap();
        wait_until(&registry, backup, "running", |s| *s == JobState::Running).await;
        registry.cancel(backup).unwrap();
    }

    #[tokio::test]
    async fn a_failing_engine_lands_in_failed() {
        let engine = Arc::new(
            MockEngine::default()
                .with_create_events(vec![JobEvent::Finished(Err(EngineError::DestinationFull))]),
        );
        let registry = JobRegistry::new();
        let id = registry.submit(JobKind::Backup, backup_factory(engine));

        let state = wait_until(&registry, id, "terminal", |s| s.is_terminal()).await;
        assert_eq!(state, JobState::Failed(EngineError::DestinationFull));
    }

    #[tokio::test]
    async fn a_stream_that_ends_without_finishing_is_not_reported_as_success() {
        // No Finished event: the engine broke its contract. Claiming the backup
        // completed would be the worst possible answer.
        let engine = Arc::new(
            MockEngine::default().with_create_events(vec![JobEvent::ItemDone {
                path: "home/a".into(),
            }]),
        );
        let registry = JobRegistry::new();
        let id = registry.submit(JobKind::Backup, backup_factory(engine));

        let state = wait_until(&registry, id, "terminal", |s| s.is_terminal()).await;
        assert!(
            matches!(state, JobState::Failed(_)),
            "expected a failure, got {state:?}"
        );
    }

    #[tokio::test]
    async fn only_disaster_recovery_accepts_a_pause() {
        let registry = JobRegistry::new();
        let backup = registry.submit(JobKind::Backup, pending_factory());
        wait_until(&registry, backup, "running", |s| *s == JobState::Running).await;

        assert_eq!(
            registry.pause(backup),
            Err(JobError::NotPausable { kind: "backup" }),
            "pausing a backup must be refused, not faked with SIGSTOP"
        );
        registry.cancel(backup).unwrap();
    }

    #[tokio::test]
    async fn pausing_disaster_recovery_frees_the_repository_and_resume_requeues() {
        let registry = JobRegistry::new();
        let dr = registry.submit(JobKind::RestoreEverything, pending_factory());
        wait_until(&registry, dr, "running", |s| *s == JobState::Running).await;

        registry.pause(dr).expect("DR is pausable");
        assert_eq!(registry.snapshot(dr).unwrap().state, JobState::Paused);

        // Paused work holds nothing, so an exclusive job can now run.
        let backup = registry.submit(JobKind::Backup, pending_factory());
        wait_until(&registry, backup, "running", |s| *s == JobState::Running).await;
        registry.cancel(backup).unwrap();

        registry.resume(dr).expect("resumable");
        wait_until(&registry, dr, "running", |s| *s == JobState::Running).await;
        registry.cancel(dr).unwrap();
    }

    #[tokio::test]
    async fn resuming_a_job_that_is_not_paused_is_refused() {
        let registry = JobRegistry::new();
        let id = registry.submit(JobKind::RestoreEverything, pending_factory());
        wait_until(&registry, id, "running", |s| *s == JobState::Running).await;
        assert_eq!(registry.resume(id), Err(JobError::NotPaused { id }));
        registry.cancel(id).unwrap();
    }

    #[tokio::test]
    async fn a_finished_job_cannot_be_cancelled_again() {
        let engine = Arc::new(
            MockEngine::default()
                .with_create_events(vec![JobEvent::Finished(Ok(JobSummary::default()))]),
        );
        let registry = JobRegistry::new();
        let id = registry.submit(JobKind::Backup, backup_factory(engine));
        wait_until(&registry, id, "terminal", |s| s.is_terminal()).await;

        assert_eq!(registry.cancel(id), Err(JobError::AlreadyFinished { id }));
    }

    #[tokio::test]
    async fn unknown_ids_are_rejected_everywhere() {
        let registry = JobRegistry::new();
        assert_eq!(registry.snapshot(99).unwrap_err(), JobError::NotFound(99));
        assert_eq!(registry.cancel(99), Err(JobError::NotFound(99)));
        assert_eq!(registry.pause(99), Err(JobError::NotFound(99)));
        assert_eq!(registry.resume(99), Err(JobError::NotFound(99)));
    }

    #[tokio::test]
    async fn ids_are_monotonic_and_never_reused() {
        let registry = JobRegistry::new();
        let engine = Arc::new(
            MockEngine::default()
                .with_create_events(vec![JobEvent::Finished(Ok(JobSummary::default()))]),
        );
        let a = registry.submit(JobKind::Backup, backup_factory(Arc::clone(&engine)));
        wait_until(&registry, a, "terminal", |s| s.is_terminal()).await;
        let b = registry.submit(JobKind::Backup, backup_factory(engine));
        wait_until(&registry, b, "terminal", |s| s.is_terminal()).await;

        assert!(b > a, "ids must not go backwards");
        assert_eq!(registry.list().len(), 2, "finished jobs stay queryable");
    }

    #[tokio::test]
    async fn subscribers_see_the_state_changes() {
        let registry = JobRegistry::new();
        let mut updates = registry.subscribe();
        let engine = Arc::new(
            MockEngine::default()
                .with_create_events(vec![JobEvent::Finished(Ok(JobSummary::default()))]),
        );
        let id = registry.submit(JobKind::Backup, backup_factory(engine));

        let mut seen = Vec::new();
        while let Ok(update) = tokio::time::timeout(Duration::from_secs(2), updates.recv()).await {
            if let Ok(JobUpdate::State { state, .. }) = update {
                let terminal = state.is_terminal();
                seen.push(state);
                if terminal {
                    break;
                }
            }
        }
        assert_eq!(
            seen,
            vec![
                JobState::Queued,
                JobState::Running,
                JobState::Done(Outcome::Completed)
            ],
            "job {id} update sequence"
        );
    }

    #[tokio::test]
    async fn progress_reaches_subscribers_rate_limited() {
        let mut events: Vec<JobEvent> = (0..200)
            .map(|i| JobEvent::Progress {
                current: i,
                total: Some(200),
                phase: "archiving".into(),
            })
            .collect();
        events.push(JobEvent::Finished(Ok(JobSummary::default())));

        let registry = JobRegistry::new();
        let mut updates = registry.subscribe();
        let engine = Arc::new(MockEngine::default().with_create_events(events));
        registry.submit(JobKind::Backup, backup_factory(engine));

        let mut progress = 0;
        while let Ok(Ok(update)) =
            tokio::time::timeout(Duration::from_secs(2), updates.recv()).await
        {
            match update {
                JobUpdate::Progress { .. } => progress += 1,
                JobUpdate::State { state, .. } if state.is_terminal() => break,
                _ => {}
            }
        }
        assert!(
            progress >= 1,
            "at least one progress update must reach the UI"
        );
        assert!(
            progress < 200,
            "200 events in well under a second must be throttled, got {progress}"
        );
    }
}
