// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The job model: what long-running work the daemon does, what state it is in,
//! and what is allowed to happen next.
//!
//! Every operation that can outlive a D-Bus method call becomes a job with a
//! [`JobId`], so the caller gets an identity back immediately and follows
//! progress through signals. The state machine lives here, admission control in
//! [`guard`], signal rate limiting in [`throttle`], and the live registry that
//! ties them together in [`registry`].

// The registry is complete and tested, but its callers are the D-Bus methods
// that arrive in S03-T3 — until then most of this surface has no caller in the
// binary. Remove both allows once the service is wired up.
#![allow(dead_code, unused_imports)]

mod guard;
mod registry;
mod throttle;

pub use guard::{RepoAccess, RepoGuard};
pub use registry::{JobFactory, JobRegistry, JobSnapshot, JobUpdate};
pub use throttle::ProgressThrottle;

use backtrack_core::engine::EngineError;

/// A job's identity. Monotonic per daemon run and never reused, so a stale
/// client holding an old id gets [`JobError::NotFound`] rather than progress
/// belonging to somebody else's job.
pub type JobId = u64;

/// What a job is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JobKind {
    Backup,
    /// Protecting changes locally because the destination is away. A backup in
    /// every way that matters to the user, and a distinct kind for one reason:
    /// only a backup that reached the *real* destination lets the local
    /// snapshots start expiring, and telling the two apart from bookkeeping
    /// rather than from the job itself is how that goes wrong.
    Offline,
    Restore,
    RestoreEverything,
    Index,
    Prune,
    Compact,
    Check,
}

impl JobKind {
    /// Every kind, for exhaustiveness tests.
    pub const ALL: &'static [JobKind] = &[
        JobKind::Backup,
        JobKind::Offline,
        JobKind::Restore,
        JobKind::RestoreEverything,
        JobKind::Index,
        JobKind::Prune,
        JobKind::Compact,
        JobKind::Check,
    ];

    /// How this kind uses the repository, which decides what may run beside it.
    ///
    /// The split mirrors Borg's own locking: `create`, `prune`, `compact` and
    /// `check` take an exclusive repository lock, while `extract` and `list`
    /// take a shared one. Modelling it here means the daemon queues jobs that
    /// would collide rather than letting Borg fail them on a lock it could not
    /// take.
    pub fn access(self) -> RepoAccess {
        match self {
            JobKind::Backup
            | JobKind::Offline
            | JobKind::Prune
            | JobKind::Compact
            | JobKind::Check => RepoAccess::Exclusive,
            JobKind::Restore | JobKind::RestoreEverything | JobKind::Index => RepoAccess::Shared,
        }
    }

    /// Whether [`JobRegistry::pause`] is meaningful for this kind.
    ///
    /// Only guided disaster recovery is pausable, and only because it is built
    /// as a sequence of per-folder extracts with a recorded position — pausing
    /// it means stopping between folders, which loses nothing.
    ///
    /// Pausing is emphatically *not* implemented by stopping the Borg process.
    /// A `SIGSTOP`ped Borg keeps its repository lock and its network sockets
    /// open indefinitely: every other job would block on a lock whose holder is
    /// never going to run again, and a remote destination would sit on a
    /// half-finished transaction until it timed out. A backup is better
    /// cancelled and re-run — Borg's deduplication means the re-run is cheap.
    pub fn is_pausable(self) -> bool {
        matches!(self, JobKind::RestoreEverything)
    }

    /// A stable identifier for logs and D-Bus payloads.
    pub fn as_str(self) -> &'static str {
        match self {
            JobKind::Backup => "backup",
            JobKind::Offline => "offline",
            JobKind::Restore => "restore",
            JobKind::RestoreEverything => "restore-everything",
            JobKind::Index => "index",
            JobKind::Prune => "prune",
            JobKind::Compact => "compact",
            JobKind::Check => "check",
        }
    }
}

/// How a job that reached [`JobState::Done`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Ran to completion.
    Completed,
    /// Stopped because someone asked it to. A cancelled job is *done*, not
    /// failed: nothing went wrong, and the health model must not raise a banner
    /// for it.
    Cancelled,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Completed => "completed",
            Outcome::Cancelled => "cancelled",
        }
    }
}

/// Where a job is in its life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    /// Submitted, waiting for the repository guard to admit it.
    Queued,
    /// Executing.
    Running,
    /// Stopped between units of work, resumable. Only ever reached by kinds
    /// where [`JobKind::is_pausable`] holds.
    Paused,
    /// A cancel has been requested and the engine is being torn down.
    /// Transient, and always resolves to `Done` — never to `Failed`.
    Cancelling,
    /// Finished, one way or the other.
    Done(Outcome),
    /// Finished badly.
    Failed(EngineError),
}

impl JobState {
    /// Whether this state admits no further transitions.
    pub fn is_terminal(&self) -> bool {
        matches!(self, JobState::Done(_) | JobState::Failed(_))
    }

    /// How the job ended, as `JobFinished` reports it, or `None` while it is
    /// still going. A failure is one word here on purpose: the reason belongs
    /// in the health state and the log, not in a signal every client has to
    /// parse.
    pub fn outcome_token(&self) -> Option<&'static str> {
        match self {
            JobState::Done(outcome) => Some(outcome.as_str()),
            JobState::Failed(_) => Some("failed"),
            _ => None,
        }
    }

    /// Whether the job is occupying the repository right now. `Cancelling`
    /// counts: Borg is still winding down and still holding its lock, so
    /// admitting the next job here would hand it a repository that is not free
    /// yet.
    pub fn holds_repo(&self) -> bool {
        matches!(self, JobState::Running | JobState::Cancelling)
    }

    /// Whether `next` is a legal successor of this state.
    ///
    /// The table is deliberately explicit rather than permissive — an illegal
    /// transition is a bug in the daemon, and the registry logs and refuses it
    /// instead of quietly corrupting a job's history.
    pub fn can_transition_to(&self, next: &JobState) -> bool {
        use JobState::*;
        match (self, next) {
            // Admission, or cancelled/failed before it ever ran.
            (Queued, Running) => true,
            (Queued, Done(Outcome::Cancelled)) => true,
            (Queued, Failed(_)) => true,

            // The ordinary working life of a job.
            (Running, Paused) => true,
            (Running, Cancelling) => true,
            (Running, Done(_)) => true,
            (Running, Failed(_)) => true,

            // A paused job holds nothing, so it goes back to the queue to be
            // re-admitted rather than straight to Running.
            (Paused, Queued) => true,
            (Paused, Cancelling) => true,
            (Paused, Done(Outcome::Cancelled)) => true,

            // Cancelling resolves to Done, either way. Usually `Cancelled`, but
            // a job that ran to completion in the moment between the request
            // and the teardown really did complete, and telling someone their
            // backup was cancelled when the archive exists is a lie they would
            // act on. What Cancelling may *not* become is `Failed`: the fallout
            // of an intentional stop is not a health banner.
            (Cancelling, Done(_)) => true,

            _ => false,
        }
    }
}

/// Why a job request could not be honoured.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JobError {
    #[error("no job with id {0}")]
    NotFound(JobId),

    #[error("a {kind} job cannot be paused")]
    NotPausable { kind: &'static str },

    #[error("job {id} has already finished")]
    AlreadyFinished { id: JobId },

    #[error("job {id} is not paused")]
    NotPaused { id: JobId },

    #[error("job {id} cannot go from {from} to {to}")]
    IllegalTransition {
        id: JobId,
        from: &'static str,
        to: &'static str,
    },
}

impl JobState {
    /// A short name for logs, errors, and D-Bus payloads.
    pub fn as_str(&self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Running => "running",
            JobState::Paused => "paused",
            JobState::Cancelling => "cancelling",
            JobState::Done(Outcome::Completed) => "completed",
            JobState::Done(Outcome::Cancelled) => "cancelled",
            JobState::Failed(_) => "failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_state() -> Vec<JobState> {
        vec![
            JobState::Queued,
            JobState::Running,
            JobState::Paused,
            JobState::Cancelling,
            JobState::Done(Outcome::Completed),
            JobState::Done(Outcome::Cancelled),
            JobState::Failed(EngineError::RepoCorrupt),
        ]
    }

    #[test]
    fn terminal_states_admit_nothing() {
        for terminal in [
            JobState::Done(Outcome::Completed),
            JobState::Done(Outcome::Cancelled),
            JobState::Failed(EngineError::RepoCorrupt),
        ] {
            assert!(terminal.is_terminal());
            for next in every_state() {
                assert!(
                    !terminal.can_transition_to(&next),
                    "{terminal:?} must not transition to {next:?}"
                );
            }
        }
    }

    #[test]
    fn the_happy_path_is_legal() {
        assert!(JobState::Queued.can_transition_to(&JobState::Running));
        assert!(JobState::Running.can_transition_to(&JobState::Done(Outcome::Completed)));
    }

    #[test]
    fn cancelling_resolves_to_done_but_never_to_failed() {
        let c = JobState::Cancelling;
        assert!(c.can_transition_to(&JobState::Done(Outcome::Cancelled)));
        // A job that finished in the gap between the request and the teardown
        // is reported honestly: the archive exists.
        assert!(c.can_transition_to(&JobState::Done(Outcome::Completed)));
        // A failure raised while tearing down is not a failure of the job: the
        // user asked it to stop and it stopped.
        assert!(!c.can_transition_to(&JobState::Failed(EngineError::RepoCorrupt)));
        assert!(!c.can_transition_to(&JobState::Running));
        assert!(!c.can_transition_to(&JobState::Paused));
    }

    #[test]
    fn a_queued_job_cannot_be_paused_or_completed() {
        let q = JobState::Queued;
        assert!(!q.can_transition_to(&JobState::Paused));
        assert!(!q.can_transition_to(&JobState::Done(Outcome::Completed)));
        assert!(!q.can_transition_to(&JobState::Cancelling));
        // Cancelling something that never started needs no teardown.
        assert!(q.can_transition_to(&JobState::Done(Outcome::Cancelled)));
    }

    #[test]
    fn a_running_job_cannot_go_back_to_queued() {
        assert!(!JobState::Running.can_transition_to(&JobState::Queued));
    }

    #[test]
    fn a_paused_job_re_enters_the_queue_rather_than_running_directly() {
        let p = JobState::Paused;
        assert!(p.can_transition_to(&JobState::Queued));
        assert!(
            !p.can_transition_to(&JobState::Running),
            "resuming must re-acquire the repo guard, not bypass it"
        );
    }

    #[test]
    fn only_disaster_recovery_is_pausable() {
        for kind in JobKind::ALL {
            assert_eq!(
                kind.is_pausable(),
                *kind == JobKind::RestoreEverything,
                "{kind:?} pausability"
            );
        }
    }

    #[test]
    fn repo_access_matches_borg_locking() {
        // Exclusive: everything that writes to or rewrites the repository.
        for kind in [
            JobKind::Backup,
            JobKind::Offline,
            JobKind::Prune,
            JobKind::Compact,
            JobKind::Check,
        ] {
            assert_eq!(kind.access(), RepoAccess::Exclusive, "{kind:?}");
        }
        // Shared: readers. This is what lets a restore run while indexing.
        for kind in [JobKind::Restore, JobKind::RestoreEverything, JobKind::Index] {
            assert_eq!(kind.access(), RepoAccess::Shared, "{kind:?}");
        }
    }

    #[test]
    fn cancelling_still_holds_the_repository() {
        assert!(JobState::Running.holds_repo());
        assert!(
            JobState::Cancelling.holds_repo(),
            "borg is still winding down and still holds its lock"
        );
        assert!(!JobState::Queued.holds_repo());
        assert!(!JobState::Paused.holds_repo());
        assert!(!JobState::Done(Outcome::Completed).holds_repo());
    }

    #[test]
    fn state_and_kind_names_are_unique() {
        let states: Vec<_> = every_state().iter().map(|s| s.as_str()).collect();
        let mut sorted = states.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            states.len(),
            "state names collide: {states:?}"
        );

        let kinds: Vec<_> = JobKind::ALL.iter().map(|k| k.as_str()).collect();
        let mut sorted = kinds.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), kinds.len(), "kind names collide: {kinds:?}");
    }
}
