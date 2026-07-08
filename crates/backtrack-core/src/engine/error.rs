// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>

//! Typed engine errors and their mapping to the health.md failure catalogue.
//!
//! The engine can fail in more ways than health.md surfaces as *banner
//! failures*. [`EngineError::health_failure`] returns `Some(row)` only for the
//! engine-relevant rows of health.md's failure catalogue, and `None` for errors
//! that are not banner-failures:
//! - `RepoUnreachable` is the `PROTECTED_LOCALLY` health *state*, not a failure
//!   (health.md: an unreachable destination is "the product working as designed",
//!   no notification);
//! - `LockedByOther` is transient (borg retries);
//! - `BorgFailed` is an uncategorised catch-all with no dedicated row;
//! - `Cancelled` is a user-initiated stop, not a failure.
//!
//! Rows of the catalogue that are not engine failures at all — index corruption
//! (SQLite), interrupted backup (checkpoint), snapshot-taken-but-indexing-failed
//! (ingest) — have no [`HealthFailure`] entry, because the engine cannot raise
//! them.

/// A convenience result alias for the engine layer.
pub type Result<T> = std::result::Result<T, EngineError>;

/// Everything the Borg adapter can fail with. The engine-relevant rows of
/// health.md's failure catalogue are reachable via [`EngineError::health_failure`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    #[error("the backup destination is unreachable")]
    RepoUnreachable,
    #[error("no passphrase is stored for this repository")]
    PassphraseMissing,
    #[error("the stored passphrase no longer matches the repository")]
    PassphraseWrong,
    #[error("authentication to the destination failed")]
    AuthFailed,
    #[error("the backup destination is full")]
    DestinationFull,
    #[error("not enough space on this computer")]
    LocalDiskFull,
    #[error("the repository is corrupt and needs repair")]
    RepoCorrupt,
    #[error("the repository is locked by another process")]
    LockedByOther,
    #[error("borg {needed} is required (found {found:?})")]
    BorgMissing {
        needed: String,
        found: Option<String>,
    },
    #[error("borg exited with code {code}: {stderr}")]
    BorgFailed { code: i32, stderr: String },
    #[error("the job was cancelled")]
    Cancelled,
}

/// The engine-relevant rows of health.md's failure catalogue — the failures the
/// engine can detect that health.md surfaces as banner states with a resolution
/// flow. Used to prove the taxonomy covers every such row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HealthFailure {
    /// "Passphrase missing (keyring reset/locked)"
    PassphraseMissing,
    /// "Wrong passphrase (repo key changed)"
    PassphraseWrong,
    /// "Destination credentials expired (SMB/SSH auth)"
    AuthExpired,
    /// "Destination full"
    DestinationFull,
    /// "Local disk full (spool/staging)"
    LocalDiskFull,
    /// "Repo corruption"
    RepoCorrupt,
    /// "Borg missing / wrong version"
    BorgMissing,
}

impl HealthFailure {
    /// Every engine-relevant catalogue row, for the coverage test.
    pub const ALL: &'static [HealthFailure] = &[
        HealthFailure::PassphraseMissing,
        HealthFailure::PassphraseWrong,
        HealthFailure::AuthExpired,
        HealthFailure::DestinationFull,
        HealthFailure::LocalDiskFull,
        HealthFailure::RepoCorrupt,
        HealthFailure::BorgMissing,
    ];
}

impl EngineError {
    /// Which health.md failure-catalogue row this error surfaces as, if any.
    /// `None` for errors that are not banner-failures (see the module docs). The
    /// `match` is total, so the compiler forces every variant to be classified.
    pub fn health_failure(&self) -> Option<HealthFailure> {
        match self {
            EngineError::PassphraseMissing => Some(HealthFailure::PassphraseMissing),
            EngineError::PassphraseWrong => Some(HealthFailure::PassphraseWrong),
            EngineError::AuthFailed => Some(HealthFailure::AuthExpired),
            EngineError::DestinationFull => Some(HealthFailure::DestinationFull),
            EngineError::LocalDiskFull => Some(HealthFailure::LocalDiskFull),
            EngineError::RepoCorrupt => Some(HealthFailure::RepoCorrupt),
            EngineError::BorgMissing { .. } => Some(HealthFailure::BorgMissing),
            EngineError::RepoUnreachable => None,
            EngineError::LockedByOther => None,
            EngineError::BorgFailed { .. } => None,
            EngineError::Cancelled => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// One representative EngineError per variant.
    fn one_of_each() -> Vec<EngineError> {
        vec![
            EngineError::RepoUnreachable,
            EngineError::PassphraseMissing,
            EngineError::PassphraseWrong,
            EngineError::AuthFailed,
            EngineError::DestinationFull,
            EngineError::LocalDiskFull,
            EngineError::RepoCorrupt,
            EngineError::LockedByOther,
            EngineError::BorgMissing {
                needed: ">=1.2".into(),
                found: None,
            },
            EngineError::BorgFailed {
                code: 2,
                stderr: "boom".into(),
            },
            EngineError::Cancelled,
        ]
    }

    #[test]
    fn every_health_catalogue_row_has_an_error() {
        let covered: HashSet<HealthFailure> = one_of_each()
            .iter()
            .filter_map(|e| e.health_failure())
            .collect();
        let expected: HashSet<HealthFailure> = HealthFailure::ALL.iter().copied().collect();
        assert_eq!(
            covered, expected,
            "every engine-relevant health.md catalogue row must have a corresponding EngineError"
        );
    }

    #[test]
    fn non_banner_failures_map_to_no_catalogue_row() {
        // health.md: an unreachable destination is PROTECTED_LOCALLY (not a
        // failure); lock contention is transient; BorgFailed is an uncategorised
        // catch-all. None of these is a banner-failure row.
        assert_eq!(EngineError::RepoUnreachable.health_failure(), None);
        assert_eq!(EngineError::LockedByOther.health_failure(), None);
        assert_eq!(
            EngineError::BorgFailed {
                code: 2,
                stderr: String::new()
            }
            .health_failure(),
            None
        );
        assert_eq!(EngineError::Cancelled.health_failure(), None);
    }
}
