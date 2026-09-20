// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The compare pass: everything the restore is going to do, worked out before
//! it does any of it.
//!
//! This is what lets a folder restore be one summary screen instead of fifty
//! pop-ups. The counts on that screen are not an estimate — they are this plan,
//! counted. And because the plan exists as a value before anything is touched,
//! "Cancel" is free: nothing has happened yet.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::classify::{classify, Class, FileFacts, Kind};
use super::{io_error, Result};

/// One path, and what the restore found for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Relative to the destination.
    pub path: PathBuf,
    pub class: Class,
    /// The version in the backup, if there is one.
    pub backup: Option<FileFacts>,
    /// The version on disk, if there is one.
    pub disk: Option<FileFacts>,
}

impl Entry {
    /// Whether this path needs an answer from the user. Everything else the
    /// restore decides for itself, and says so on the summary.
    pub fn needs_a_decision(&self) -> bool {
        matches!(self.class, Class::Conflict { .. } | Class::TypeChanged)
    }
}

/// The whole restore, as a value.
#[derive(Debug, Clone)]
pub struct RestorePlan {
    /// The archive being restored from, by name.
    pub archive: String,
    /// Where the files are going.
    pub dest: PathBuf,
    /// Where they were extracted to.
    pub staging: PathBuf,
    /// Every path on either side, in a stable order.
    pub entries: Vec<Entry>,
    /// Paths the archive offered that will not be restored, and why. Reported
    /// rather than dropped: a file silently missing from a restore is worse
    /// than one that is explained.
    pub refused: Vec<(PathBuf, String)>,
}

/// The numbers the summary screen is made of.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub identical: usize,
    pub conflicts: usize,
    /// How many of the conflicts have a newer copy on disk — the risky ones,
    /// which the summary calls out separately.
    pub disk_newer: usize,
    pub only_in_backup: usize,
    pub only_on_disk: usize,
    pub type_changed: usize,
}

/// What one conflicting path should have done to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Decision {
    /// Put the backup version in place; the file on disk goes to the stash.
    Replace,
    /// The backup version takes the name; the file on disk is renamed aside.
    KeepBoth,
    /// Leave the file on disk alone.
    #[default]
    Skip,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Replace => "replace",
            Decision::KeepBoth => "keep-both",
            Decision::Skip => "skip",
        }
    }

    /// Parse the wire spelling.
    pub fn parse(value: &str) -> Option<Decision> {
        match value {
            "replace" => Some(Decision::Replace),
            "keep-both" => Some(Decision::KeepBoth),
            "skip" => Some(Decision::Skip),
            _ => None,
        }
    }
}

/// What to do with each conflicting path: one answer for all of them, and any
/// number of exceptions — which is exactly the shape of the summary screen and
/// its review list.
#[derive(Debug, Clone, Default)]
pub struct Decisions {
    blanket: Decision,
    exceptions: BTreeMap<PathBuf, Decision>,
}

impl Decisions {
    /// The same answer for every conflict.
    pub fn all(decision: Decision) -> Decisions {
        Decisions {
            blanket: decision,
            exceptions: BTreeMap::new(),
        }
    }

    /// Override one path — a row unchecked in the review list.
    pub fn except(mut self, path: impl Into<PathBuf>, decision: Decision) -> Decisions {
        self.exceptions.insert(path.into(), decision);
        self
    }

    /// The answer for one path.
    pub fn for_path(&self, path: &Path) -> Decision {
        self.exceptions.get(path).copied().unwrap_or(self.blanket)
    }

    /// The answer for one path *only if it was named*, with no blanket
    /// fallback. A change of type is resolved through this: turning a directory
    /// into a file is not something to infer from a click on "Replace
    /// Changed Files", so it needs its own tick in the review list.
    pub fn named(&self, path: &Path) -> Option<Decision> {
        self.exceptions.get(path).copied()
    }
}

impl RestorePlan {
    /// Count the plan up for the summary.
    pub fn counts(&self) -> Counts {
        let mut counts = Counts::default();
        for entry in &self.entries {
            match entry.class {
                Class::Identical => counts.identical += 1,
                Class::OnlyInBackup => counts.only_in_backup += 1,
                Class::OnlyOnDisk => counts.only_on_disk += 1,
                Class::TypeChanged => counts.type_changed += 1,
                Class::Conflict { disk_newer } => {
                    counts.conflicts += 1;
                    if disk_newer {
                        counts.disk_newer += 1;
                    }
                }
            }
        }
        counts
    }

    /// The paths that need an answer, in the order the review list shows them.
    pub fn decisions_needed(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.needs_a_decision())
    }

    /// Whether anything at all would change. A restore with nothing to do is
    /// worth saying so about rather than running.
    pub fn is_a_no_op(&self) -> bool {
        self.entries
            .iter()
            .all(|e| matches!(e.class, Class::Identical | Class::OnlyOnDisk))
    }

    /// Bytes the execution will need, given `decisions`.
    ///
    /// Two separate filesystems may be involved — the stash lives with
    /// Backtrack's own data and the destination is wherever the user keeps
    /// their files — so they are counted separately rather than summed into a
    /// number that is true of neither.
    pub fn space_needed(&self, decisions: &Decisions) -> SpaceNeeded {
        let mut needed = SpaceNeeded::default();
        for entry in &self.entries {
            let decision = match entry.class {
                Class::OnlyInBackup => Decision::Replace,
                Class::Conflict { .. } | Class::TypeChanged => decisions.for_path(&entry.path),
                _ => continue,
            };
            if decision == Decision::Skip {
                continue;
            }
            needed.dest += entry.backup.map_or(0, |f| f.size);
            if decision == Decision::Replace {
                // The copy that is about to be overwritten is kept, so it has
                // to go somewhere first.
                needed.stash += entry.disk.map_or(0, |f| f.size);
            }
        }
        needed
    }
}

/// Room the execution needs, by destination.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SpaceNeeded {
    /// On the filesystem holding the files being restored.
    pub dest: u64,
    /// On the filesystem holding the safety stash.
    pub stash: u64,
}

/// Compare an extracted `staging` tree against `dest` and work out the restore.
///
/// `asked_for` is what the user actually requested, archive-relative, and it is
/// what bounds the comparison. It cannot be inferred from the staging tree:
/// extracting `home/keith/Documents/notes.txt` recreates every directory above
/// it, so the extracted tree's top-level name is `home` — and taking that as
/// the scope means walking the whole of `/home` to restore one file, then
/// reporting everything in it as "kept". Measured on a real machine before this
/// argument existed: 4,213,710 files, and ninety seconds to count them.
///
/// Both sides are walked without following symlinks: a link in either tree is
/// compared as a link, and is never a door into somewhere else.
pub fn plan(
    archive: &str,
    staging: &Path,
    dest: &Path,
    asked_for: &[PathBuf],
) -> Result<RestorePlan> {
    let from_backup = walk(staging)?;
    // Only the parts of the destination the restore actually covers. A folder
    // restore covers its whole subtree; a single-file restore covers that file
    // and nothing around it.
    let mut from_disk = BTreeMap::new();
    for requested in asked_for {
        let Ok(requested) = super::safety::validate_relative(requested) else {
            continue;
        };
        let under = dest.join(&requested);
        for (path, facts) in walk(&under)? {
            from_disk.insert(requested.join(path), facts);
        }
        if let Some(facts) = facts_of(&under)? {
            from_disk.insert(requested.clone(), facts);
        }
    }

    let mut paths: Vec<PathBuf> = from_backup
        .keys()
        .chain(from_disk.keys())
        .cloned()
        .collect();
    paths.sort();
    paths.dedup();

    let mut entries = Vec::with_capacity(paths.len());
    let mut refused = Vec::new();
    for path in paths {
        if let Err(why) = super::safe_join(dest, &path) {
            refused.push((path, why.to_string()));
            continue;
        }
        // Extracting a file recreates every directory above it. Those are
        // scaffolding, not content: they are made if they are missing, and
        // they have no business in a summary that says how many files are
        // being added. Counting them would tell someone restoring one file
        // that nine were added.
        if is_scaffolding(&path, &from_backup, asked_for) {
            continue;
        }
        let backup = from_backup.get(&path).copied();
        let disk = from_disk.get(&path).copied();
        let class = classify(backup.as_ref(), disk.as_ref(), || {
            // Only reached for same-size, different-time pairs and for
            // symlinks; a read failure means "assume different", which errs
            // towards asking rather than towards silently skipping.
            same_content(&staging.join(&path), &dest.join(&path), backup).unwrap_or(false)
        });
        entries.push(Entry {
            path,
            class,
            backup,
            disk,
        });
    }

    Ok(RestorePlan {
        archive: archive.to_string(),
        dest: dest.to_path_buf(),
        staging: staging.to_path_buf(),
        entries,
        refused,
    })
}

/// Whether `path` is only there to hold something that was asked for: a
/// directory strictly above one of the requested paths.
fn is_scaffolding(
    path: &Path,
    from_backup: &BTreeMap<PathBuf, FileFacts>,
    asked_for: &[PathBuf],
) -> bool {
    if from_backup.get(path).map(|f| f.kind) != Some(Kind::Dir) {
        return false;
    }
    asked_for
        .iter()
        .filter_map(|requested| super::safety::validate_relative(requested).ok())
        .any(|requested| requested.starts_with(path) && requested != path)
}

/// Every path under `root`, relative to it, without following symlinks.
/// A missing root is an empty walk, not an error: it simply is not there yet.
fn walk(root: &Path) -> Result<BTreeMap<PathBuf, FileFacts>> {
    let mut found = BTreeMap::new();
    if !root.exists() {
        return Ok(found);
    }
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        let here = root.join(&relative);
        let listing = match std::fs::read_dir(&here) {
            Ok(listing) => listing,
            // Unreadable directories are reported by the execution, where the
            // user can be told which one. Planning around one is not possible.
            Err(_) => continue,
        };
        for item in listing {
            let item = item.map_err(|e| io_error(&here, e))?;
            let child = relative.join(item.file_name());
            let Some(facts) = facts_of(&root.join(&child))? else {
                continue;
            };
            if facts.kind == Kind::Dir {
                pending.push(child.clone());
            }
            found.insert(child, facts);
        }
    }
    Ok(found)
}

/// What a path is, without following it. `None` if it is not there.
fn facts_of(path: &Path) -> Result<Option<FileFacts>> {
    let facts = match std::fs::symlink_metadata(path) {
        Ok(facts) => facts,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(path, error)),
    };
    let file_type = facts.file_type();
    let kind = if file_type.is_symlink() {
        Kind::Symlink
    } else if file_type.is_dir() {
        Kind::Dir
    } else {
        Kind::File
    };
    Ok(Some(FileFacts {
        kind,
        size: facts.len(),
        mtime: mtime_micros(&facts),
    }))
}

/// Modification time in epoch microseconds, the unit the index uses.
fn mtime_micros(facts: &std::fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    facts.mtime() * 1_000_000 + i64::from(facts.mtime_nsec() as i32) / 1_000
}

/// Whether two paths hold the same thing. Links are compared by target; files
/// by their bytes, which is only ever asked when the sizes already match.
fn same_content(backup: &Path, disk: &Path, facts: Option<FileFacts>) -> std::io::Result<bool> {
    if facts.is_some_and(|f| f.kind == Kind::Symlink) {
        return Ok(std::fs::read_link(backup)? == std::fs::read_link(disk)?);
    }
    let mut a = std::io::BufReader::new(std::fs::File::open(backup)?);
    let mut b = std::io::BufReader::new(std::fs::File::open(disk)?);
    let mut left = [0u8; 64 * 1024];
    let mut right = [0u8; 64 * 1024];
    loop {
        let read = read_full(&mut a, &mut left)?;
        if read != read_full(&mut b, &mut right)? {
            return Ok(false);
        }
        if read == 0 {
            return Ok(true);
        }
        if left[..read] != right[..read] {
            return Ok(false);
        }
    }
}

/// Fill `buffer` as far as the reader allows, so two readers can be compared
/// chunk for chunk without one short read making them look different.
fn read_full(reader: &mut impl Read, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}
