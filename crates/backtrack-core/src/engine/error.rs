// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>

//! Typed engine errors and their mapping to the health.md failure catalogue.
//!
//! Each **engine-relevant** row of health.md maps to exactly one [`HealthFailure`];
//! rows that are not engine failures (index corruption — SQLite; interrupted
//! backup — checkpoint; snapshot-taken-but-indexing-failed — ingest) are
//! deliberately absent, because the engine cannot raise them.

/// A convenience result alias for the engine layer.
pub type Result<T> = std::result::Result<T, EngineError>;

/// Everything the Borg adapter can fail with. Maps 1:1 onto the engine-relevant
/// rows of the health.md failure catalogue via [`EngineError::health_failure`].
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
    BorgMissing { needed: String, found: Option<String> },
    #[error("borg exited with code {code}: {stderr}")]
    BorgFailed { code: i32, stderr: String },
}

/// The engine-relevant rows of health.md's failure catalogue. Used to prove the
/// error taxonomy is exhaustive over failures the engine can actually detect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HealthFailure {
    PassphraseMissing,
    PassphraseWrong,
    AuthExpired,
    DestinationFull,
    LocalDiskFull,
    RepoCorrupt,
    BorgMissing,
    RepoUnreachable,
    LockedByOther,
    UncategorisedBorgFailure,
}

impl HealthFailure {
    /// Every catalogue row, for the exhaustiveness test.
    pub const ALL: &'static [HealthFailure] = &[
        HealthFailure::PassphraseMissing,
        HealthFailure::PassphraseWrong,
        HealthFailure::AuthExpired,
        HealthFailure::DestinationFull,
        HealthFailure::LocalDiskFull,
        HealthFailure::RepoCorrupt,
        HealthFailure::BorgMissing,
        HealthFailure::RepoUnreachable,
        HealthFailure::LockedByOther,
        HealthFailure::UncategorisedBorgFailure,
    ];
}

impl EngineError {
    /// Which health.md catalogue row this error surfaces as. The `match` is total,
    /// so the compiler guarantees every error variant is classified.
    pub fn health_failure(&self) -> HealthFailure {
        match self {
            EngineError::RepoUnreachable => HealthFailure::RepoUnreachable,
            EngineError::PassphraseMissing => HealthFailure::PassphraseMissing,
            EngineError::PassphraseWrong => HealthFailure::PassphraseWrong,
            EngineError::AuthFailed => HealthFailure::AuthExpired,
            EngineError::DestinationFull => HealthFailure::DestinationFull,
            EngineError::LocalDiskFull => HealthFailure::LocalDiskFull,
            EngineError::RepoCorrupt => HealthFailure::RepoCorrupt,
            EngineError::LockedByOther => HealthFailure::LockedByOther,
            EngineError::BorgMissing { .. } => HealthFailure::BorgMissing,
            EngineError::BorgFailed { .. } => HealthFailure::UncategorisedBorgFailure,
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
            EngineError::BorgMissing { needed: ">=1.2".into(), found: None },
            EngineError::BorgFailed { code: 2, stderr: "boom".into() },
        ]
    }

    #[test]
    fn every_health_row_is_covered_by_some_error() {
        let produced: HashSet<HealthFailure> =
            one_of_each().iter().map(|e| e.health_failure()).collect();
        let expected: HashSet<HealthFailure> = HealthFailure::ALL.iter().copied().collect();
        assert_eq!(
            produced, expected,
            "every engine-relevant health.md row must map to at least one EngineError"
        );
    }
}
