// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The published D-Bus contract: the names that identify Backtrack on the
//! session bus, and the payload types its methods return.
//!
//! The names live here — the daemon implements the interface and the
//! clients call it, but both have to agree on what to call, so the strings sit
//! in the one crate they both depend on. There is no zbus dependency in this
//! module.
//!
//! The *interface* and *object path* are fixed: they are the published API from
//! `stack.md` §2. The *bus name* is not, quite — under `BACKTRACK_DEV` it gains
//! a `.Dev` suffix, so a development daemon and an installed one can run side by
//! side without either serving the other's clients. That matters more than it
//! sounds: dev mode already redirects the data directory, so a dev daemon
//! answering the real GUI would show an empty timeline for a machine that is in
//! fact fully backed up.
//!
//! The payload types live here for the same reason: the daemon marshals them
//! and every client unmarshals them, so a single definition is the only way the
//! two cannot drift apart.

use serde::{Deserialize, Serialize};
use zvariant::Type;

/// What `GetStatus` answers with.
///
/// A struct rather than a loose dictionary: the shape is part of the published
/// interface, so it belongs in the introspection output where a client can see
/// it and a snapshot test can pin it.
///
/// Times are seconds since the epoch, with `0` meaning "never" or "not
/// applicable" — D-Bus has no null, and inventing one per field would be worse
/// than one convention applied everywhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Status {
    /// One of health.md's states, as its documented spelling.
    pub state: String,
    /// When the last backup succeeded.
    pub last_backup: u64,
    /// When the next scheduled backup is due; 0 when manual or paused.
    pub next_backup: u64,
    /// Whether the destination answered the last time it was probed. `false`
    /// is what "offline" means everywhere in the interface — there is no
    /// separate flag, because two fields that must agree are two fields that
    /// can disagree.
    pub destination_reachable: bool,
    /// Bytes the local safety net is holding.
    ///
    /// Meaningful for the spool, which is a repository with a size. Filesystem
    /// snapshots share their extents with the live files, so what they "use" is
    /// only what has diverged since — a figure that costs a full tree scan to
    /// compute, and is near zero when it matters. Reported as 0 in that mode;
    /// [`Status::local_snapshots`] is the number to show.
    pub spool_bytes: u64,
    /// How changes are protected while the destination is away: `spool`,
    /// `fs-snapshot`, or `off`.
    pub offline_mode: String,
    /// Snapshots currently held on this computer, of either kind. What the
    /// Storage preferences page counts, and what the timeline badges as "on
    /// this computer".
    pub local_snapshots: u32,
    /// How many of those the destination has caught up with, and which will
    /// therefore be discarded in due course.
    pub expirable_snapshots: u32,
    /// The job currently running, or 0 when idle.
    pub active_job: u64,
    /// When the current pause lifts; 0 when not paused.
    pub paused_until: u64,
    /// Whether a repository has been configured at all.
    pub configured: bool,
    /// Why `state` is what it is, as a [`Reason`] token; empty when healthy.
    ///
    /// The state says how worried to be and the reason says about what. A
    /// banner needs both: "BROKEN" alone cannot choose between asking for a
    /// passphrase and offering to free up space.
    pub reason: String,
    /// When the current state began.
    pub since: u64,
}

/// Why the health state is what it is.
///
/// A row of health.md's failure catalogue where the state has one, and
/// otherwise the plain reason the state exists at all. Shared by the daemon,
/// which decides it, and every client, which chooses its copy and its fix from
/// it, so the two ends cannot spell a row differently.
///
/// The same token is what a notification's click hands the application to say
/// which fix to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reason {
    /// Catalogue: "Passphrase missing (keyring reset/locked)".
    PassphraseMissing,
    /// Catalogue: "Wrong passphrase (repo key changed)".
    PassphraseWrong,
    /// Catalogue: "Destination credentials expired (SMB/SSH auth)", which
    /// also covers a destination that is there and refuses to be written to.
    AuthFailed,
    /// Catalogue: "Destination full".
    DestinationFull,
    /// Catalogue: "Local disk full (spool/staging)", DEGRADED while backups
    /// still run and BROKEN once they cannot.
    LocalDiskFull,
    /// Catalogue: "Repo corruption".
    RepoCorrupt,
    /// Catalogue: "Borg missing / wrong version".
    BorgMissing,
    /// Catalogue: "Index corruption", while the catalogue is rebuilt.
    CatalogueRebuilding,
    /// Catalogue: "Snapshot taken but indexing failed".
    NotYetBrowsable,
    /// AT_RISK: nothing has been protected for too long.
    NoRecentBackup,
    /// PROTECTED_LOCALLY: the destination is away and this computer is
    /// keeping the changes.
    DestinationAway,
    /// PAUSED: the person asked for it.
    Paused,
}

impl Reason {
    /// Every reason, for exhaustiveness tests on both sides of the bus.
    pub const ALL: [Reason; 12] = [
        Reason::PassphraseMissing,
        Reason::PassphraseWrong,
        Reason::AuthFailed,
        Reason::DestinationFull,
        Reason::LocalDiskFull,
        Reason::RepoCorrupt,
        Reason::BorgMissing,
        Reason::CatalogueRebuilding,
        Reason::NotYetBrowsable,
        Reason::NoRecentBackup,
        Reason::DestinationAway,
        Reason::Paused,
    ];

    /// The wire form.
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::PassphraseMissing => "passphrase-missing",
            Reason::PassphraseWrong => "passphrase-wrong",
            Reason::AuthFailed => "auth-failed",
            Reason::DestinationFull => "destination-full",
            Reason::LocalDiskFull => "local-disk-full",
            Reason::RepoCorrupt => "repo-corrupt",
            Reason::BorgMissing => "borg-missing",
            Reason::CatalogueRebuilding => "catalogue-rebuilding",
            Reason::NotYetBrowsable => "not-yet-browsable",
            Reason::NoRecentBackup => "no-recent-backup",
            Reason::DestinationAway => "destination-away",
            Reason::Paused => "paused",
        }
    }

    /// Read a wire token back. `None` for anything this build does not know,
    /// which a client treats as "no reason it can act on".
    pub fn parse(token: &str) -> Option<Reason> {
        Reason::ALL
            .into_iter()
            .find(|reason| reason.as_str() == token)
    }

    /// The catalogue row that stops backups, for the reasons that are one.
    pub fn failure(self) -> Option<crate::engine::HealthFailure> {
        crate::engine::HealthFailure::ALL
            .iter()
            .copied()
            .find(|failure| Reason::from(*failure) == self)
    }
}

impl From<crate::engine::HealthFailure> for Reason {
    fn from(failure: crate::engine::HealthFailure) -> Reason {
        use crate::engine::HealthFailure;
        match failure {
            HealthFailure::PassphraseMissing => Reason::PassphraseMissing,
            HealthFailure::PassphraseWrong => Reason::PassphraseWrong,
            HealthFailure::AuthExpired => Reason::AuthFailed,
            HealthFailure::DestinationFull => Reason::DestinationFull,
            HealthFailure::LocalDiskFull => Reason::LocalDiskFull,
            HealthFailure::RepoCorrupt => Reason::RepoCorrupt,
            HealthFailure::BorgMissing => Reason::BorgMissing,
        }
    }
}

/// One result row from `SearchFiles`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SearchResult {
    /// Archive-relative path.
    pub path: String,
    pub name: String,
    /// "file", "dir", "symlink", or "other".
    pub kind: String,
    /// First and last archive sequence the path appears in.
    pub first_seq: i64,
    pub last_seq: i64,
    /// Timestamps of those archives, seconds since the epoch.
    pub first_ts: i64,
    pub last_ts: i64,
    /// How many distinct versions the path has had.
    pub versions: u32,
    /// The size of the newest version, in bytes.
    pub size: i64,
    /// Whether this path is known to be gone from the computer now.
    ///
    /// True only for a *known* absence. A path the daemon could not check —
    /// an unreadable folder, or an archive taken on another machine — reads as
    /// false, because the tag this drives ("no longer on your disk") is a
    /// claim, and a claim nobody verified should not be made.
    ///
    /// Deliberately not "is it in the newest archive", which is what this
    /// field used to hold under the name `exists_today`. The newest backup can
    /// be an hour old, and the hour since is exactly when the thing a person
    /// is searching for went missing.
    pub gone_from_disk: bool,
}

/// One path a restore needs an answer about, as `GetRestorePreview` reports it.
///
/// Only the paths that need a decision cross the bus. A folder restore may
/// touch tens of thousands of files, and the review list shows the handful
/// that conflict; sending the rest would be a large message nobody reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RestoreEntry {
    /// Relative to the destination.
    pub path: String,
    /// `conflict` or `type-changed`.
    pub class: String,
    /// Whether the copy on disk is the newer of the two — the risky direction,
    /// which the dialog says louder.
    pub disk_newer: bool,
    /// Size and modification time of each side. `0` where that side has no
    /// version, which for these two classes does not happen.
    pub backup_size: u64,
    pub backup_mtime: i64,
    pub disk_size: u64,
    pub disk_mtime: i64,
    /// What each side is: `file`, `dir` or `symlink`, and empty where that
    /// side has no version.
    ///
    /// "Changed type" is not a complete sentence. Which way round it is
    /// decides whether the person is about to lose a folder or a file, and a
    /// dialog that shows a size of `—` and leaves them to infer the rest has
    /// told them the least useful half of it.
    pub backup_kind: String,
    pub disk_kind: String,
}

/// What a prepared restore would do, computed before anything is touched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RestorePreview {
    /// The archive being restored from, by name.
    pub archive: String,
    /// Where the files are going.
    pub dest: String,
    /// The summary counts, in the order the summary screen lists them.
    pub identical: u32,
    pub conflicts: u32,
    /// How many of the conflicts are newer on disk.
    pub disk_newer: u32,
    pub only_in_backup: u32,
    /// Always kept. A restore merges and never deletes.
    pub only_on_disk: u32,
    pub type_changed: u32,
    /// Only the paths needing an answer.
    pub entries: Vec<RestoreEntry>,
    /// Paths that will not be restored, and why — an archive member that would
    /// escape the destination, most often. Reported rather than dropped.
    pub refused: Vec<(String, String)>,
    /// What was asked for and is not in this backup.
    ///
    /// Without this an empty plan is ambiguous, and the two things it can mean
    /// are opposites: everything is already up to date, or the backup does not
    /// have the file. Saying the first when the second is true is a backup
    /// tool reassuring somebody about a file it could not find.
    pub missing: Vec<String>,
}

/// One file the safety stash is keeping, as `ListReplaced` reports it.
///
/// The stash has no database: a replaced file is filed under the path it had,
/// and this is that path read back. So there is nothing to fall out of step
/// with the files, and a person with a file manager can find theirs without
/// Backtrack's help.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ReplacedFile {
    /// Where the file was when it was replaced, absolute.
    pub original: String,
    /// Where the safety copy is now. Also the handle: `PutBackReplaced` takes
    /// it, because it is the one thing about an entry that is unique.
    pub stashed: String,
    pub size: u64,
    /// When the restore that displaced it ran. Shared by every file that
    /// restore replaced, which is what groups them on screen.
    pub replaced_at: i64,
    /// The file's own modification time, which the stash preserves.
    pub mtime: i64,
}

/// What `GetStorageInfo` answers with: the figures Preferences shows about
/// where the backups are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct StorageInfo {
    /// Borg's encryption mode, as it names it (`repokey-blake2`, `none`, ...).
    pub encryption: String,
    /// What the backups occupy, after compression and deduplication.
    pub stored: u64,
    /// What they add up to before either.
    pub original: u64,
    /// The size of the filesystem holding them, and how much of it is free.
    /// `0` where that cannot be found out from here, as for an SSH server.
    pub capacity: u64,
    pub free: u64,
}

/// What `GetLocalStorage` answers with: where the room on this computer has
/// gone, for the "not enough space on this computer" fix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct LocalStorage {
    /// Free space where Backtrack keeps its data; 0 when it could not be read.
    pub free: u64,
    /// What the local safety net holds while the destination is away, and the
    /// limit it is kept within.
    pub snapshots: u64,
    pub snapshot_limit: u64,
    /// Files restores replaced, kept so that a restore can be undone.
    pub stash: u64,
    /// Copies of backed-up files extracted for previews and comparisons.
    pub cache: u64,
}

/// What `GetHealth` answers with: the state, how it came about, and what the
/// person has been told about it. The detail behind a banner, and what
/// `backtrack doctor` puts in a bug report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct HealthReport {
    pub state: String,
    pub reason: String,
    pub since: u64,
    /// Recent transitions, oldest first: when, state, reason.
    pub history: Vec<(u64, String, String)>,
    /// The last failure of each part of the daemon: which part, when, the
    /// reason token when the failure is a catalogue row, and what it said.
    pub errors: Vec<(String, u64, String, String)>,
    /// The at-risk notices so far: how many for the current stretch without
    /// a backup, when the last went, and when that stretch began. Zero when
    /// there have been none.
    pub at_risk_notices: u32,
    pub at_risk_notified: u64,
    pub at_risk_since: u64,
    /// The last broken notice: what it was about, and when.
    pub broken_notified_reason: String,
    pub broken_notified: u64,
}

/// Phase names carried on `BackupProgress`. Defined here because a client
/// reads them to decide what to say, and a phase renamed on one side only
/// would leave a progress page silent.
pub const PHASE_ARCHIVING: &str = "archiving";
pub const PHASE_CATALOGUING: &str = "cataloguing";
pub const PHASE_PRUNING: &str = "pruning";

/// The well-known bus name of an installed daemon.
pub const BUS_NAME: &str = "org.backtrack.Daemon1";

/// The bus name of a development daemon (`BACKTRACK_DEV` set).
pub const BUS_NAME_DEV: &str = "org.backtrack.Daemon1.Dev";

/// The interface every client talks. Fixed in both modes — only the bus name
/// varies, so introspection output is identical either way.
pub const INTERFACE: &str = "org.backtrack.Daemon1";

/// The single object the daemon exports.
pub const OBJECT_PATH: &str = "/org/backtrack/Daemon1";

/// The prefix for the daemon's named D-Bus errors, which mirror the engine's
/// error taxonomy (e.g. `org.backtrack.Error.PassphraseMissing`).
pub const ERROR_PREFIX: &str = "org.backtrack.Error";

/// The bus name to own (daemon) or call (clients), honoring `BACKTRACK_DEV`.
pub fn bus_name() -> &'static str {
    bus_name_for(std::env::var_os("BACKTRACK_DEV").is_some())
}

/// Pure selection, split out so it can be tested without touching the process
/// environment.
fn bus_name_for(dev: bool) -> &'static str {
    if dev {
        BUS_NAME_DEV
    } else {
        BUS_NAME
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_mode_uses_a_distinct_bus_name() {
        assert_eq!(bus_name_for(false), BUS_NAME);
        assert_eq!(bus_name_for(true), BUS_NAME_DEV);
        assert_ne!(BUS_NAME, BUS_NAME_DEV);
    }

    #[test]
    fn reason_tokens_are_unique_and_read_back() {
        let mut seen = std::collections::HashSet::new();
        for reason in Reason::ALL {
            assert!(seen.insert(reason.as_str()), "{reason:?} repeats a token");
            assert_eq!(Reason::parse(reason.as_str()), Some(reason));
        }
        assert_eq!(Reason::parse(""), None, "healthy has no reason");
        assert_eq!(Reason::parse("something-new"), None);
    }

    #[test]
    fn every_catalogue_failure_has_a_reason_of_its_own() {
        let reasons: std::collections::HashSet<Reason> = crate::engine::HealthFailure::ALL
            .iter()
            .map(|failure| Reason::from(*failure))
            .collect();
        assert_eq!(reasons.len(), crate::engine::HealthFailure::ALL.len());
        for failure in crate::engine::HealthFailure::ALL {
            assert_eq!(Reason::from(*failure).failure(), Some(*failure));
        }
        assert_eq!(Reason::NoRecentBackup.failure(), None);
        assert_eq!(Reason::CatalogueRebuilding.failure(), None);
    }

    #[test]
    fn the_published_interface_does_not_vary() {
        // Clients introspect the same interface in both modes; only the name
        // they connect through differs.
        assert_eq!(INTERFACE, "org.backtrack.Daemon1");
        assert_eq!(OBJECT_PATH, "/org/backtrack/Daemon1");
    }
}
