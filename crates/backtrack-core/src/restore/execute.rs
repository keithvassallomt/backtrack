// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Carrying out a plan, and putting it back.
//!
//! Files arrive by `rename()` from the staging directory, so every path holds
//! either the old file or the new one at every instant — an interrupted restore
//! cannot leave a half-written document. When staging turns out to be on a
//! different filesystem than the destination, the copy is made beside the
//! target and renamed into place, which keeps the same guarantee at the cost of
//! the copy.
//!
//! Nothing is overwritten in the ordinary sense. A file about to be replaced is
//! moved into the stash first, and every move is written down, so the whole
//! restore can be reversed afterwards by walking the log backwards. That log is
//! what "Undo" is.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tracing::{debug, warn};

use super::classify::{Class, Kind};
use super::plan::{Decision, Decisions, RestorePlan};
use super::{io_error, RestoreError, Result};

/// One thing the restore did, in enough detail to undo it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Move {
    /// A file was put where there was nothing.
    Added { path: PathBuf },
    /// A file was replaced; the one that was there is in the stash.
    Replaced { path: PathBuf, stashed: PathBuf },
    /// The backup version took the name and the file on disk was renamed.
    KeptBoth { path: PathBuf, renamed: PathBuf },
    /// A directory was created to hold restored files.
    MadeDir { path: PathBuf },
}

/// Everything the restore did, in the order it did it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MoveLog {
    pub moves: Vec<Move>,
}

impl MoveLog {
    /// How many files ended up somewhere new — what the toast counts.
    pub fn restored(&self) -> usize {
        self.moves
            .iter()
            .filter(|m| !matches!(m, Move::MadeDir { .. }))
            .count()
    }
}

/// What happened to one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Restored,
    Skipped,
    Failed(String),
}

/// The result of an execution.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub log: MoveLog,
    pub restored: usize,
    pub skipped: usize,
    /// Paths that could not be written, with why. A restore is not abandoned
    /// because one file in it is read-only: the rest is still wanted, and the
    /// failures are reported at the end.
    pub failures: Vec<(PathBuf, String)>,
}

/// Carry out `plan`.
///
/// `stash` is the directory safety copies go under; `now` names the
/// subdirectory for this restore, so one job's replaced files stay together.
pub fn execute(
    plan: &RestorePlan,
    decisions: &Decisions,
    stash: &Path,
    now: SystemTime,
) -> Result<Report> {
    check_space(plan, decisions, stash)?;

    let stash_dir = stash.join(stamp(now).to_string());
    let mut report = Report::default();

    // Entries are sorted by path, so a directory is always created before the
    // files that go in it.
    for entry in &plan.entries {
        let decision = match entry.class {
            Class::Identical | Class::OnlyOnDisk => {
                report.skipped += 1;
                continue;
            }
            Class::OnlyInBackup => Decision::Replace,
            // Never resolved by a blanket answer: replacing a directory with a
            // file is not something to infer from a click on "Replace".
            Class::TypeChanged => decisions.named(&entry.path).unwrap_or(Decision::Skip),
            Class::Conflict { .. } => decisions.for_path(&entry.path),
        };
        if decision == Decision::Skip {
            report.skipped += 1;
            continue;
        }

        match apply(plan, entry.path.as_path(), entry, decision, &stash_dir) {
            Ok(Some(done)) => {
                report.restored += 1;
                report.log.moves.push(done);
            }
            Ok(None) => report.skipped += 1,
            Err(error) => {
                warn!(path = %entry.path.display(), %error, "a file could not be restored");
                report
                    .failures
                    .push((entry.path.clone(), error.to_string()));
            }
        }
    }
    Ok(report)
}

/// Do one path, returning what was done, or `None` if there was nothing to do.
fn apply(
    plan: &RestorePlan,
    relative: &Path,
    entry: &super::plan::Entry,
    decision: Decision,
    stash_dir: &Path,
) -> Result<Option<Move>> {
    let target = super::safe_join(&plan.dest, relative)?;
    let source = plan.staging.join(relative);
    let kind = entry.backup.map(|f| f.kind);

    // A directory in the backup is made, not moved: its contents arrive as
    // their own entries.
    if kind == Some(Kind::Dir) {
        if target.is_dir() {
            return Ok(None);
        }
        std::fs::create_dir_all(&target).map_err(|e| io_error(&target, e))?;
        return Ok(Some(Move::MadeDir { path: target }));
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
    }

    let existed = entry.disk.is_some();
    let displaced = match (existed, decision) {
        (false, _) => None,
        (true, Decision::Replace) => {
            let kept = stash_dir.join(strip_root(&target));
            move_path(&target, &kept)?;
            Some(Move::Replaced {
                path: target.clone(),
                stashed: kept,
            })
        }
        (true, Decision::KeepBoth) => {
            let aside = keep_both_name(&target, |candidate| candidate.exists());
            move_path(&target, &aside)?;
            Some(Move::KeptBoth {
                path: target.clone(),
                renamed: aside,
            })
        }
        (true, Decision::Skip) => return Ok(None),
    };

    move_path(&source, &target)?;
    Ok(Some(displaced.unwrap_or(Move::Added { path: target })))
}

/// Put everything a restore did back the way it was, newest move first.
///
/// Walked backwards because the moves are not independent: a Keep Both renamed
/// a file aside before another took its name, and undoing them in the order
/// they happened would put the first one back on top of the second.
pub fn undo(log: &MoveLog) -> Report {
    let mut report = Report::default();
    for done in log.moves.iter().rev() {
        let result = match done {
            Move::Added { path } => remove(path),
            Move::Replaced { path, stashed } => remove(path).and_then(|_| move_path(stashed, path)),
            Move::KeptBoth { path, renamed } => remove(path).and_then(|_| move_path(renamed, path)),
            // Left alone: a directory that was created may since have had
            // other things put in it, and removing it would take them too.
            Move::MadeDir { .. } => Ok(()),
        };
        match result {
            Ok(()) => report.restored += 1,
            Err(error) => {
                let path = match done {
                    Move::Added { path }
                    | Move::Replaced { path, .. }
                    | Move::KeptBoth { path, .. }
                    | Move::MadeDir { path } => path.clone(),
                };
                warn!(path = %path.display(), %error, "a file could not be put back");
                report.failures.push((path, error.to_string()));
            }
        }
    }
    report
}

/// What the file on disk is renamed to under Keep Both.
///
/// The backup version takes the real name, because in a *restore* tool the
/// user's intent is "give me the backup version back" — the inverse of a copy
/// dialog, and the same choice Time Machine makes.
pub fn keep_both_name(path: &Path, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_os_string();
    let extension = path.extension().map(|e| e.to_os_string());
    let build = |suffix: &str| {
        let mut name = OsString::from(&stem);
        name.push(suffix);
        if let Some(extension) = &extension {
            name.push(".");
            name.push(extension);
        }
        path.with_file_name(name)
    };

    let first = build(" (current)");
    if !taken(&first) {
        return first;
    }
    // Two restores of the same file, or a name the user already used.
    for n in 2..1000 {
        let candidate = build(&format!(" (current {n})"));
        if !taken(&candidate) {
            return candidate;
        }
    }
    build(" (current)")
}

/// Refuse before starting if there is not room to finish.
///
/// Discovering this halfway through is the worst time: the stash would hold
/// some of the originals and the destination some of the replacements.
fn check_space(plan: &RestorePlan, decisions: &Decisions, stash: &Path) -> Result<()> {
    let needed = plan.space_needed(decisions);
    for (bytes, where_) in [(needed.dest, plan.dest.as_path()), (needed.stash, stash)] {
        if bytes == 0 {
            continue;
        }
        let available = super::free_space(where_).unwrap_or(u64::MAX);
        // A margin, because a filesystem that is exactly full is a filesystem
        // that fails in surprising ways.
        if available < bytes.saturating_add(16 * 1024 * 1024) {
            return Err(RestoreError::NotEnoughSpace {
                needed_mb: bytes / (1024 * 1024),
                available_mb: available / (1024 * 1024),
            });
        }
    }
    Ok(())
}

/// Move `from` to `to`, atomically where the filesystem allows it.
fn move_path(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
    }
    match std::fs::rename(from, to) {
        Ok(()) => return Ok(()),
        Err(error) if error.raw_os_error() == Some(libc_exdev()) => {
            debug!(
                from = %from.display(),
                to = %to.display(),
                "across filesystems; copying instead of renaming"
            );
        }
        Err(error) => return Err(io_error(to, error)),
    }
    copy_then_rename(from, to)?;
    std::fs::remove_file(from)
        .or_else(|_| std::fs::remove_dir_all(from))
        .ok();
    Ok(())
}

/// `EXDEV`, the error a rename gives when the two paths are on different
/// filesystems. Spelled out rather than pulled from a C binding, because it is
/// the one value needed and it is fixed on Linux.
fn libc_exdev() -> i32 {
    18
}

/// Copy beside the target and rename into place, so the target is never seen
/// half-written even when a plain rename is not available.
fn copy_then_rename(from: &Path, to: &Path) -> Result<()> {
    let facts = std::fs::symlink_metadata(from).map_err(|e| io_error(from, e))?;

    if facts.file_type().is_symlink() {
        let target = std::fs::read_link(from).map_err(|e| io_error(from, e))?;
        let _ = std::fs::remove_file(to);
        std::os::unix::fs::symlink(target, to).map_err(|e| io_error(to, e))?;
        return Ok(());
    }

    let temporary = to.with_extension("backtrack-partial");
    std::fs::copy(from, &temporary).map_err(|e| io_error(&temporary, e))?;
    // Both the contents and the metadata the stash promises to keep.
    let file = std::fs::File::options()
        .write(true)
        .open(&temporary)
        .map_err(|e| io_error(&temporary, e))?;
    let times = std::fs::FileTimes::new()
        .set_modified(facts.modified().unwrap_or(SystemTime::UNIX_EPOCH))
        .set_accessed(facts.accessed().unwrap_or(SystemTime::UNIX_EPOCH));
    let _ = file.set_times(times);
    file.sync_all().map_err(|e| io_error(&temporary, e))?;
    drop(file);
    std::fs::set_permissions(&temporary, facts.permissions())
        .map_err(|e| io_error(&temporary, e))?;
    std::fs::rename(&temporary, to).map_err(|e| io_error(to, e))?;
    Ok(())
}

fn remove(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(path, error)),
        Ok(facts) if facts.is_dir() => std::fs::remove_dir_all(path).map_err(|e| io_error(path, e)),
        Ok(_) => std::fs::remove_file(path).map_err(|e| io_error(path, e)),
    }
}

/// An absolute path in the form the stash stores it under: leading `/` removed,
/// the same convention Borg uses for archive members.
fn strip_root(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().trim_start_matches('/').to_string())
}

/// Seconds since the epoch, which is what a stash directory is named.
fn stamp(now: SystemTime) -> u64 {
    now.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
