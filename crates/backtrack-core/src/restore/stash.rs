// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The safety stash: what "Replace" did with the file it replaced.
//!
//! Every file a restore overwrites is moved here first, under
//! `replaced/<when>/<path>` — a directory per restore, named for the second it
//! ran, holding the displaced files at their original paths with the leading
//! separator removed. The layout is the whole index: what a file was called
//! and where it lived are read back out of the path, so there is no database
//! to fall out of step with the files, and a person with a file manager can
//! find their file without Backtrack's help.
//!
//! It is what makes Replace a decision rather than a commitment. The toast's
//! Undo covers the next ten seconds; this covers the next thirty days, which
//! is the timescale on which people actually notice.
//!
//! Nothing here is free, so the stash is bounded twice: by age, and by size.
//! A safety net that quietly fills a disk has caused a problem of its own.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::execute::{move_path, stamp, strip_root};
use super::{io_error, Result};

/// How long a replaced file is kept. Promised in the dialogs, so it is not a
/// number to change without changing them.
pub const KEEP_DAYS: i64 = 30;

/// How much of the disk the stash may hold before the oldest restores are
/// given up early.
pub const MAX_BYTES: u64 = 5 * 1024 * 1024 * 1024;

const DAY: i64 = 86_400;

/// How recent a restore has to be for the size cap to leave it alone.
///
/// The cap evicts the oldest batch, and the batch a restore is writing into is
/// the newest — so the two only ever meet when a single restore is itself over
/// the limit. Then the eviction would be deleting files out from under the job
/// creating them. An hour's grace makes that impossible and costs nothing: the
/// cap is there to stop a stash growing over weeks, not to hold a line to the
/// megabyte for the next sixty minutes.
const EVICTION_GRACE: i64 = 3_600;

/// One file being kept in case its replacement was a mistake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replaced {
    /// Where the file was when it was replaced, absolute.
    pub original: PathBuf,
    /// Where the safety copy is now.
    pub stashed: PathBuf,
    pub size: u64,
    /// When the restore that displaced it ran — the batch it is filed under.
    /// Shared by every file the same restore replaced, which is what groups
    /// them in the window.
    pub replaced_at: i64,
    /// The file's own modification time, which the stash preserves.
    pub mtime: i64,
}

/// What an expiry pass gave up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Expiry {
    /// Restores whose whole batch was removed.
    pub batches: usize,
    pub bytes: u64,
    /// How many went early because the stash was over its size limit rather
    /// than over its age. Worth logging separately: files promised for thirty
    /// days and kept for four is a thing the person should be able to find out
    /// about after the fact.
    pub given_up_early: usize,
}

/// Every file in the stash, newest restore first, at most `limit` of them.
///
/// Bounded because a restore of a large folder replaces as many files as it
/// touches, and the window that shows this wants the recent ones — not all of
/// them marshalled across a bus to be scrolled past.
pub fn list(root: &Path, limit: usize) -> Vec<Replaced> {
    let mut found = Vec::new();
    for batch in batches(root) {
        if found.len() >= limit {
            break;
        }
        collect(&batch.dir, &batch.dir, batch.at, limit, &mut found);
    }
    found
}

/// Look one entry up by where the stash is keeping it.
///
/// The path arrives from a client, so it is checked rather than trusted: it
/// has to resolve to somewhere inside the stash, under a batch this code could
/// have written. Naming a path is not a licence to have it moved somewhere
/// else, and the thing on the other end of `put_back` is a `rename` onto a
/// path this function decides.
///
/// The *parent* is canonicalised rather than the file, because the stash holds
/// symlinks too — resolving the last component would follow one out of the
/// stash and check the wrong path.
pub fn find(root: &Path, stashed: &Path) -> Option<Replaced> {
    let root = root.canonicalize().ok()?;
    let full = stashed
        .parent()?
        .canonicalize()
        .ok()?
        .join(stashed.file_name()?);

    let relative = full.strip_prefix(&root).ok()?;
    let mut parts = relative.components();
    let at: i64 = parts.next()?.as_os_str().to_string_lossy().parse().ok()?;
    let inside: PathBuf = parts.collect();
    if inside.as_os_str().is_empty() {
        return None;
    }

    let facts = full.symlink_metadata().ok()?;
    if facts.is_dir() {
        return None;
    }
    Some(Replaced {
        original: Path::new("/").join(inside),
        stashed: full,
        size: facts.len(),
        replaced_at: at,
        mtime: seconds(facts.modified().ok()),
    })
}

/// Put one file back where it came from.
///
/// Whatever is in its place now is not thrown away — it goes into the stash in
/// turn. Putting a file back is a restore like any other, and the same promise
/// has to hold: the thing being overwritten survives the overwriting.
///
/// Returns where the displaced file went, if there was one.
pub fn put_back(entry: &Replaced, root: &Path, now: SystemTime) -> Result<Option<PathBuf>> {
    let displaced = match entry.original.symlink_metadata() {
        Ok(_) => {
            let kept = root
                .join(stamp(now).to_string())
                .join(strip_root(&entry.original));
            move_path(&entry.original, &kept)?;
            Some(kept)
        }
        Err(_) => {
            if let Some(parent) = entry.original.parent() {
                std::fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
            }
            None
        }
    };
    move_path(&entry.stashed, &entry.original)?;
    prune_upwards(root, &entry.stashed);
    Ok(displaced)
}

/// Give up what the stash is no longer entitled to keep.
///
/// Age first, then size: a file inside its thirty days is given up only
/// because the stash as a whole is too large, and then the oldest restore goes
/// first. Whole batches, never part of one — half a restore in the stash is a
/// worse answer than none of it, because the half that is missing is the half
/// somebody goes looking for.
pub fn expire(root: &Path, now: i64, max_bytes: u64) -> Expiry {
    let mut kept: Vec<Batch> = Vec::new();
    let mut report = Expiry::default();

    for batch in batches(root) {
        if now - batch.at > KEEP_DAYS * DAY {
            report.bytes += discard(&batch);
            report.batches += 1;
        } else {
            kept.push(batch);
        }
    }

    // Newest first, so taking from the end takes the oldest.
    let mut total: u64 = kept.iter().map(|b| b.bytes).sum();
    while total > max_bytes {
        let Some(oldest) = kept.pop() else { break };
        if now - oldest.at < EVICTION_GRACE {
            // Everything left is inside the grace period, and so is everything
            // already passed over — the list is in age order.
            break;
        }
        total -= oldest.bytes;
        report.bytes += discard(&oldest);
        report.batches += 1;
        report.given_up_early += 1;
    }
    report
}

/// One restore's worth of replaced files.
struct Batch {
    dir: PathBuf,
    /// The directory's name: the second the restore ran.
    at: i64,
    bytes: u64,
}

/// The stash's batches, newest first.
///
/// A directory whose name is not a number was not written by us. It is left
/// alone rather than removed: this code deletes things, and the one rule that
/// makes that safe is that it only deletes what it can prove it created.
fn batches(root: &Path) -> Vec<Batch> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut batches: Vec<Batch> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let at = e.file_name().to_string_lossy().parse::<i64>().ok()?;
            Some(Batch {
                bytes: weigh(&e.path()),
                dir: e.path(),
                at,
            })
        })
        .collect();
    batches.sort_by_key(|batch| std::cmp::Reverse(batch.at));
    batches
}

/// Walk one batch, turning every file in it back into where it came from.
fn collect(batch: &Path, dir: &Path, at: i64, limit: usize, found: &mut Vec<Replaced>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if found.len() >= limit {
            return;
        }
        let path = entry.path();
        let Ok(facts) = path.symlink_metadata() else {
            continue;
        };
        if facts.is_dir() {
            collect(batch, &path, at, limit, found);
            continue;
        }
        let Ok(relative) = path.strip_prefix(batch) else {
            continue;
        };
        found.push(Replaced {
            original: Path::new("/").join(relative),
            stashed: path.clone(),
            size: facts.len(),
            replaced_at: at,
            mtime: seconds(facts.modified().ok()),
        });
    }
}

/// What a batch is costing.
fn weigh(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.path().symlink_metadata() {
            Ok(facts) if facts.is_dir() => weigh(&entry.path()),
            Ok(facts) => facts.len(),
            Err(_) => 0,
        })
        .sum()
}

fn discard(batch: &Batch) -> u64 {
    match std::fs::remove_dir_all(&batch.dir) {
        Ok(()) => batch.bytes,
        Err(error) => {
            tracing::warn!(
                dir = %batch.dir.display(),
                %error,
                "a stashed restore could not be given up"
            );
            0
        }
    }
}

/// Take away the directories a put-back has just emptied, up to the batch.
///
/// Without this the stash keeps the shape of every restore it ever held, as a
/// tree of empty directories that `list` has to walk past for ever.
fn prune_upwards(root: &Path, from: &Path) {
    let mut dir = from.parent();
    while let Some(candidate) = dir {
        if candidate == root || !candidate.starts_with(root) {
            return;
        }
        if std::fs::remove_dir(candidate).is_err() {
            return;
        }
        dir = candidate.parent();
    }
}

fn seconds(time: Option<SystemTime>) -> i64 {
    time.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
