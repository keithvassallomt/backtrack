// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The restore engine.
//!
//! One structure serves every restore: **extract to `staging/` → compare every
//! path → move into place atomically.** Nothing is touched until the whole
//! comparison is done, which is what makes the folder summary possible — the
//! counts in it are facts about a plan that has already been computed, not a
//! guess about what is coming.
//!
//! Three promises follow from that structure, and they are the reason it is
//! worth the staging directory:
//!
//! - **A restore merges; it never deletes.** A file that exists only on disk is
//!   left exactly as it is, always, with no option anywhere to change that. It
//!   is the first thing people fear about restoring a folder, and the design
//!   answers it by making the fear impossible rather than by promising.
//! - **Replace is never destructive.** The file being overwritten is moved into
//!   a 30-day stash first, so "Replace" can be undone long after the toast has
//!   gone.
//! - **Every move is atomic.** Files arrive by `rename()` from a staging area
//!   on the same filesystem, so an interrupted restore leaves every path
//!   holding either the old file or the new one, never half of either.

mod classify;
mod execute;
mod plan;
mod safety;

#[cfg(test)]
mod tests;

pub use classify::{classify, Class, FileFacts, Kind};
pub use execute::{execute, keep_both_name, undo, Move, MoveLog, Outcome, Report};
pub use plan::{plan, Counts, Decision, Decisions, Entry, RestorePlan};
pub use safety::{free_space, safe_join, SafetyError};

use std::path::PathBuf;

/// What can go wrong before a single file has been touched.
#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    #[error("{0}")]
    Safety(#[from] SafetyError),
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "restoring needs about {needed_mb} MB free and {available_mb} MB is available; \
         a restore briefly holds both the incoming files and safety copies of the ones \
         it replaces"
    )]
    NotEnoughSpace { needed_mb: u64, available_mb: u64 },
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, RestoreError>;

/// Attach a path to an I/O error, which `std::io::Error` does not carry.
pub(crate) fn io_error(path: impl Into<PathBuf>, source: std::io::Error) -> RestoreError {
    RestoreError::Io {
        path: path.into(),
        source,
    }
}
