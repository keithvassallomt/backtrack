// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The errors that cross the bus.
//!
//! Every failure a client can act on gets its own D-Bus error name under
//! `org.backtrack.Error`, mirroring the engine taxonomy one-for-one. The name
//! is the contract: a GUI matches on `org.backtrack.Error.PassphraseMissing` to
//! decide it should open the passphrase dialog, and matching on English message
//! text would break the moment anything is translated.
//!
//! The mapping is total — [`From<EngineError>`] handles every variant — so a new
//! engine error cannot reach the bus as an anonymous failure.

use backtrack_core::config::ConfigError;
use backtrack_core::engine::EngineError;
use backtrack_core::index::IndexError;

use crate::jobs::JobError;

/// Errors as clients see them. The derive turns each variant into
/// `org.backtrack.Error.<Variant>`.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.backtrack.Error")]
#[allow(clippy::enum_variant_names)]
pub enum DaemonError {
    /// zbus requires this variant; it carries anything that is genuinely a
    /// malformed request rather than a Backtrack condition.
    #[zbus(error)]
    ZBus(zbus::Error),

    /// No passphrase is stored for this repository.
    PassphraseMissing(String),
    /// The stored passphrase no longer opens the repository.
    PassphraseWrong(String),
    /// The destination refused our credentials (distinct from unreachable).
    AuthFailed(String),
    /// The destination cannot be reached right now.
    RepoUnreachable(String),
    /// The destination is out of space.
    DestinationFull(String),
    /// This computer is out of space.
    LocalDiskFull(String),
    /// The repository is damaged and needs repair.
    RepoCorrupt(String),
    /// Another process holds the repository lock.
    LockedByOther(String),
    /// Borg is absent or too old.
    BorgMissing(String),
    /// Borg failed in a way we have not classified.
    BorgFailed(String),
    /// The operation was cancelled.
    Cancelled(String),

    /// No job with that id.
    NoSuchJob(String),
    /// The job cannot be paused (see `JobKind::is_pausable`).
    NotPausable(String),
    /// The job has already reached a terminal state.
    JobFinished(String),
    /// The job is not paused, so it cannot be resumed.
    JobNotPaused(String),

    /// The configuration on disk, or an attempted edit to it, is not valid.
    InvalidConfig(String),
    /// A configuration key the schema does not define.
    UnknownConfigKey(String),
    /// The operation needs a configured repository and there is not one yet.
    NotConfigured(String),

    /// The catalogue could not be read or written.
    IndexUnavailable(String),
    /// The requested archive or path is not in the catalogue.
    NotFound(String),
    /// The request itself is malformed (bad policy name, empty path, ...).
    InvalidArgument(String),

    /// A restore could not be carried out on this machine — staging could not
    /// be read, the destination could not be written. Distinct from the Borg
    /// failures above because the repository had no part in it, and a person
    /// reading the message needs to know which side of the operation went
    /// wrong.
    RestoreFailed(String),
}

/// A convenience alias for anything the interface returns.
pub type Result<T> = std::result::Result<T, DaemonError>;

impl From<EngineError> for DaemonError {
    fn from(e: EngineError) -> DaemonError {
        let message = e.to_string();
        match e {
            EngineError::PassphraseMissing => DaemonError::PassphraseMissing(message),
            EngineError::PassphraseWrong => DaemonError::PassphraseWrong(message),
            EngineError::AuthFailed => DaemonError::AuthFailed(message),
            EngineError::RepoUnreachable => DaemonError::RepoUnreachable(message),
            EngineError::DestinationFull => DaemonError::DestinationFull(message),
            EngineError::LocalDiskFull => DaemonError::LocalDiskFull(message),
            EngineError::RepoCorrupt => DaemonError::RepoCorrupt(message),
            EngineError::LockedByOther => DaemonError::LockedByOther(message),
            EngineError::BorgMissing { .. } => DaemonError::BorgMissing(message),
            EngineError::BorgFailed { .. } => DaemonError::BorgFailed(message),
            EngineError::Cancelled => DaemonError::Cancelled(message),
            EngineError::Local(_) => DaemonError::RestoreFailed(message),
        }
    }
}

impl From<JobError> for DaemonError {
    fn from(e: JobError) -> DaemonError {
        let message = e.to_string();
        match e {
            JobError::NotFound(_) => DaemonError::NoSuchJob(message),
            JobError::NotPausable { .. } => DaemonError::NotPausable(message),
            JobError::AlreadyFinished { .. } => DaemonError::JobFinished(message),
            JobError::NotPaused { .. } => DaemonError::JobNotPaused(message),
            JobError::IllegalTransition { .. } => DaemonError::JobFinished(message),
        }
    }
}

impl From<ConfigError> for DaemonError {
    fn from(e: ConfigError) -> DaemonError {
        DaemonError::InvalidConfig(e.to_string())
    }
}

impl From<IndexError> for DaemonError {
    fn from(e: IndexError) -> DaemonError {
        DaemonError::IndexUnavailable(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::DBusError as _;

    /// One representative of every engine error.
    fn every_engine_error() -> Vec<EngineError> {
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
    fn every_engine_error_gets_a_distinct_bus_name() {
        let mut names = Vec::new();
        for e in every_engine_error() {
            let name = DaemonError::from(e.clone()).name().to_string();
            assert!(
                name.starts_with("org.backtrack.Error."),
                "{e:?} produced {name}"
            );
            names.push(name);
        }
        let mut unique = names.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            names.len(),
            "engine errors must not collapse onto one bus name: {names:?}"
        );
    }

    #[test]
    fn the_documented_names_are_exactly_these() {
        // stack.md names PassphraseMissing explicitly; the rest follow the same
        // rule. Pinned so a rename cannot silently break a client's match.
        let name = DaemonError::from(EngineError::PassphraseMissing)
            .name()
            .to_string();
        assert_eq!(name, "org.backtrack.Error.PassphraseMissing");
    }

    #[test]
    fn job_errors_map_onto_their_own_names() {
        let cases = [
            (JobError::NotFound(1), "org.backtrack.Error.NoSuchJob"),
            (
                JobError::NotPausable { kind: "backup" },
                "org.backtrack.Error.NotPausable",
            ),
            (
                JobError::AlreadyFinished { id: 1 },
                "org.backtrack.Error.JobFinished",
            ),
            (
                JobError::NotPaused { id: 1 },
                "org.backtrack.Error.JobNotPaused",
            ),
        ];
        for (error, expected) in cases {
            let name = DaemonError::from(error).name().to_string();
            assert_eq!(name, expected);
        }
    }

    #[test]
    fn the_message_survives_the_crossing() {
        // The name drives client behaviour, but the message is what ends up in
        // a bug report, so it must not be discarded.
        let e = DaemonError::from(EngineError::DestinationFull);
        assert!(e.to_string().contains("full"), "expected the cause in: {e}");
    }
}
