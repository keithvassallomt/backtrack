// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Admission control for the repository.
//!
//! Borg takes an exclusive lock for `create`, `prune`, `compact` and `check`,
//! and a shared one for `extract` and `list`. The guard models the same rule a
//! step earlier, so two jobs that would collide are *queued* rather than
//! started and failed: a user who hits "Restore" during the hourly backup gets
//! a restore that starts a minute later, not an error about a lock they have
//! never heard of.
//!
//! It is a plain reader-writer admission rule with no waiting inside it —
//! [`RepoGuard::admits`] answers a question about a set of running jobs and the
//! registry does the queuing. That keeps it a pure function, which is what
//! makes the interleaving testable without spawning anything.

/// How a job uses the repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoAccess {
    /// Rewrites the repository; nothing else may run.
    Exclusive,
    /// Reads the repository; may run beside other readers.
    Shared,
}

/// What is currently holding the repository.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RepoGuard {
    exclusive: bool,
    shared: usize,
}

impl RepoGuard {
    /// A guard holding nothing.
    pub fn new() -> RepoGuard {
        RepoGuard::default()
    }

    /// Rebuild the guard from the access modes of everything running. The
    /// registry derives this from its job table rather than tracking it
    /// separately, so the two can never disagree about who holds what.
    pub fn from_running(running: impl IntoIterator<Item = RepoAccess>) -> RepoGuard {
        let mut guard = RepoGuard::new();
        for access in running {
            guard.acquire(access);
        }
        guard
    }

    /// Whether a job wanting `access` may start now.
    pub fn admits(&self, access: RepoAccess) -> bool {
        match access {
            // A writer needs the repository to itself.
            RepoAccess::Exclusive => !self.exclusive && self.shared == 0,
            // Readers coexist, but never with a writer.
            RepoAccess::Shared => !self.exclusive,
        }
    }

    /// Record that a job holding `access` has started.
    pub fn acquire(&mut self, access: RepoAccess) {
        match access {
            RepoAccess::Exclusive => self.exclusive = true,
            RepoAccess::Shared => self.shared += 1,
        }
    }

    /// Whether anything at all holds the repository.
    pub fn is_idle(&self) -> bool {
        !self.exclusive && self.shared == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::JobKind;

    #[test]
    fn an_idle_repository_admits_anything() {
        let guard = RepoGuard::new();
        assert!(guard.is_idle());
        assert!(guard.admits(RepoAccess::Exclusive));
        assert!(guard.admits(RepoAccess::Shared));
    }

    #[test]
    fn a_backup_excludes_everything() {
        let guard = RepoGuard::from_running([JobKind::Backup.access()]);
        assert!(
            !guard.admits(RepoAccess::Exclusive),
            "one backup-class job at a time"
        );
        assert!(
            !guard.admits(RepoAccess::Shared),
            "restores must not run alongside a backup"
        );
    }

    #[test]
    fn restores_run_alongside_indexing() {
        // The rule the stage calls out by name.
        let guard = RepoGuard::from_running([JobKind::Index.access()]);
        assert!(guard.admits(JobKind::Restore.access()));

        let both = RepoGuard::from_running([JobKind::Index.access(), JobKind::Restore.access()]);
        assert!(
            both.admits(JobKind::Restore.access()),
            "readers keep admitting readers"
        );
        assert!(
            !both.admits(JobKind::Backup.access()),
            "but a backup waits for the readers to drain"
        );
    }

    #[test]
    fn maintenance_jobs_exclude_each_other() {
        for holder in [JobKind::Prune, JobKind::Compact, JobKind::Check] {
            let guard = RepoGuard::from_running([holder.access()]);
            assert!(
                !guard.admits(JobKind::Backup.access()),
                "{holder:?} must exclude a backup"
            );
            assert!(
                !guard.admits(JobKind::Restore.access()),
                "{holder:?} must exclude a restore"
            );
        }
    }

    #[test]
    fn readers_release_independently() {
        let mut guard = RepoGuard::new();
        guard.acquire(RepoAccess::Shared);
        guard.acquire(RepoAccess::Shared);
        assert!(!guard.is_idle());
        assert!(!guard.admits(RepoAccess::Exclusive));

        // Rebuilt with one reader gone — the registry recomputes rather than
        // decrementing, so a dropped release cannot leak a permit.
        let guard = RepoGuard::from_running([RepoAccess::Shared]);
        assert!(!guard.admits(RepoAccess::Exclusive));

        let guard = RepoGuard::from_running([]);
        assert!(guard.admits(RepoAccess::Exclusive));
    }
}
