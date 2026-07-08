// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>

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

/// What `repo_info` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    pub repository_id: String,
    pub archive_count: usize,
}

/// A Borg archive name or hex id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveId(pub String);
