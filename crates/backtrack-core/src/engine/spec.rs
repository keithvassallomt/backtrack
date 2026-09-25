// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Inputs to the engine operations. Minimal-but-honest for Stage 2: only what a
//! real backup and the integration tests exercise. Retention detail, exclude
//! files, checkpoint interval and chunker params arrive with the Stage 4
//! pipeline that consumes them.

use std::path::PathBuf;

/// A `borg create` request.
#[derive(Debug, Clone)]
pub struct CreateSpec {
    pub archive_name: String,
    pub sources: Vec<PathBuf>,
    pub excludes: Vec<String>,
    pub compression: Compression,
    pub one_file_system: bool,
    /// Archive exactly these paths instead of walking [`CreateSpec::sources`].
    /// Empty for an ordinary backup.
    ///
    /// This is how the offline spool archives a delta: it has already worked
    /// out which files changed, and asking Borg to walk the tree again to
    /// rediscover them would cost more than the archive itself. The paths are
    /// absolute and are stored the same way any other member is, so a spool
    /// archive's members line up with the primary repository's.
    ///
    /// The exclusions still apply — Borg evaluates them against an explicit
    /// list as it does against a walked one — so a file that should never be
    /// archived cannot get in this way even if the caller offers it.
    pub paths: Vec<PathBuf>,
    /// When the backup was started. Carried so the catalogue can date the
    /// archive from the same instant its name was built from, rather than from
    /// whenever the ingest happened to run — on a large first backup those are
    /// hours apart, and the timeline would show the wrong one.
    pub created_at: std::time::SystemTime,
}

/// Compression algorithm passed to `--compression`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compression {
    #[default]
    Zstd,
    Lz4,
    None,
}

impl Compression {
    pub fn as_borg_arg(self) -> &'static str {
        match self {
            Compression::Zstd => "zstd",
            Compression::Lz4 => "lz4",
            Compression::None => "none",
        }
    }
}

/// Retention counts for `borg prune`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrunePolicy {
    pub keep_hourly: u32,
    pub keep_daily: u32,
    pub keep_weekly: u32,
    pub keep_monthly: u32,
}

/// Depth of a `borg check`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckLevel {
    /// `--repository-only`
    Repository,
    /// `--archives-only`
    Archives,
    /// Full check (default).
    Full,
}

/// A repository to create.
#[derive(Debug, Clone)]
pub struct RepoSpec {
    /// Local path, `ssh://…`, or a mounted-share path.
    pub path: String,
    pub encryption: Encryption,
}

/// Encryption mode for `borg init`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encryption {
    #[default]
    RepokeyBlake2,
}

impl Encryption {
    pub fn as_borg_arg(self) -> &'static str {
        match self {
            Encryption::RepokeyBlake2 => "repokey-blake2",
        }
    }
}

/// What is at a destination before anything is created there.
///
/// Three answers rather than a yes or no, because the wizard does something
/// different for each: an empty place gets a new repository, an existing
/// repository is offered for import, and a folder holding something else is
/// refused rather than written into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// Nothing there yet, or an empty folder: a repository can be created.
    Empty,
    /// A Borg repository is already there.
    Existing,
    /// Something that is not a repository is there.
    Occupied,
    /// Nothing is there, and nothing can be put there: a folder on this
    /// computer that the user is not allowed to write into.
    Unwritable,
}

/// What `repo_info` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    pub repository_id: String,
    pub archive_count: usize,
}

/// A Borg archive name or hex id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveId(pub String);
