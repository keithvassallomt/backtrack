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
//!
//! Something restored out of the Trash is logged as a kind of move of its own,
//! because Undo does not delete it: it goes back into the Trash, with its
//! `.trashinfo` as it was.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tracing::{debug, warn};

use super::classify::{Class, Kind};
use super::plan::{Decision, Decisions, FromTrash, RestorePlan};
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
    /// Something was taken out of the Trash to `path`, from `copy` there.
    /// What stood at `path` went as `displaced` says, if anything did.
    FromTrash {
        path: PathBuf,
        copy: PathBuf,
        displaced: Option<Displaced>,
    },
    /// Nothing but folders was left of `copy` in the Trash once the files had
    /// gone, so the folders went too, and, for a whole trashed item, its
    /// `.trashinfo`, which said `text`.
    LeftTrash {
        copy: PathBuf,
        /// The folders that went, relative to `copy`, shallowest first. Empty
        /// for a file.
        folders: Vec<PathBuf>,
        dir: bool,
        info: Option<(PathBuf, String)>,
    },
}

/// What happened to the file that stood where something from the Trash went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Displaced {
    /// Replaced: it is in the stash.
    Stashed(PathBuf),
    /// Kept Both: it was renamed aside.
    Renamed(PathBuf),
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
            .filter(|m| !matches!(m, Move::MadeDir { .. } | Move::LeftTrash { .. }))
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
    /// The execution was asked to stop and did, between two files, before
    /// reaching the end of the plan.
    pub stopped: bool,
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
    execute_watched(plan, decisions, stash, now, &|| false, &mut |_| {})
}

/// [`execute`], looking at `stop` before each path and telling `done` about
/// each move as it is made.
///
/// What a restore that has to be pausable needs: it stops between two files,
/// never in the middle of one, and what it had done by then is already known
/// to whoever was told, so nothing it did is lost to the stop.
pub fn execute_watched(
    plan: &RestorePlan,
    decisions: &Decisions,
    stash: &Path,
    now: SystemTime,
    stop: &dyn Fn() -> bool,
    done: &mut dyn FnMut(&Move),
) -> Result<Report> {
    check_space(plan, decisions, stash)?;

    let stash_dir = stash.join(stamp(now).to_string());
    let mut report = Report::default();

    // Entries are sorted by path, so a directory is always created before the
    // files that go in it.
    for entry in &plan.entries {
        if stop() {
            report.stopped = true;
            break;
        }
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
            Ok(Some(moved)) => {
                done(&moved);
                report.restored += 1;
                report.log.moves.push(moved);
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
    for trashed in &plan.trash {
        match leave_trash(trashed) {
            Ok(Some(left)) => {
                done(&left);
                report.log.moves.push(left);
            }
            Ok(None) => {}
            Err(error) => {
                warn!(copy = %trashed.copy.display(), %error, "the Trash could not be tidied after a restore");
            }
        }
    }
    Ok(report)
}

/// Once everything has been taken out of `trashed`, take the folders left
/// behind as well, and the `.trashinfo` of a whole item, so the Trash no
/// longer lists it. Anything still in it, skipped or left alone, keeps it in
/// the Trash as it is.
fn leave_trash(trashed: &FromTrash) -> Result<Option<Move>> {
    let copy = &trashed.copy;
    let mut folders = Vec::new();
    let dir = match std::fs::symlink_metadata(copy) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(io_error(copy, error)),
        Ok(facts) if facts.is_dir() => {
            if !only_folders(copy, Path::new(""), &mut folders)? {
                return Ok(None);
            }
            // Deepest first, and each only if it is still empty.
            for folder in folders.iter().rev() {
                let path = copy.join(folder);
                std::fs::remove_dir(&path).map_err(|e| io_error(&path, e))?;
            }
            std::fs::remove_dir(copy).map_err(|e| io_error(copy, e))?;
            true
        }
        // Something is still there: Skip left it, or it was never restored.
        Ok(_) => return Ok(None),
    };
    let info = match &trashed.info {
        None => None,
        Some(info) => {
            let text = std::fs::read_to_string(info).map_err(|e| io_error(info, e))?;
            std::fs::remove_file(info).map_err(|e| io_error(info, e))?;
            Some((info.clone(), text))
        }
    };
    Ok(Some(Move::LeftTrash {
        copy: copy.clone(),
        folders,
        dir,
        info,
    }))
}

/// Whether `root` holds nothing but folders, collecting them, shallowest
/// first, relative to it.
fn only_folders(root: &Path, relative: &Path, folders: &mut Vec<PathBuf>) -> Result<bool> {
    let here = root.join(relative);
    let listing = std::fs::read_dir(&here).map_err(|e| io_error(&here, e))?;
    for item in listing {
        let item = item.map_err(|e| io_error(&here, e))?;
        let child = relative.join(item.file_name());
        let facts = item
            .file_type()
            .map_err(|e| io_error(root.join(&child), e))?;
        if !facts.is_dir() {
            return Ok(false);
        }
        folders.push(child.clone());
        if !only_folders(root, &child, folders)? {
            return Ok(false);
        }
    }
    Ok(true)
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
    let source = plan.source_of(relative);
    let from_trash = plan.from_trash(relative);
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
            Some(Displaced::Stashed(kept))
        }
        (true, Decision::KeepBoth) => {
            let aside = keep_both_name(&target, |candidate| candidate.exists());
            move_path(&target, &aside)?;
            Some(Displaced::Renamed(aside))
        }
        (true, Decision::Skip) => return Ok(None),
    };

    move_path(&source, &target)?;
    Ok(Some(match (from_trash, displaced) {
        (true, displaced) => Move::FromTrash {
            path: target,
            copy: source,
            displaced,
        },
        (false, None) => Move::Added { path: target },
        (false, Some(Displaced::Stashed(stashed))) => Move::Replaced {
            path: target,
            stashed,
        },
        (false, Some(Displaced::Renamed(renamed))) => Move::KeptBoth {
            path: target,
            renamed,
        },
    }))
}

/// Move the backup's copy of every path that needs a decision out of staging
/// and into `aside`, at the same place relative to it, leaving the files on
/// disk untouched.
///
/// For a restore that cannot ask as it goes. Everything that needs no answer
/// is restored; what does is kept aside until it can be asked about, all at
/// once, and the staging directory is free to be cleared.
///
/// A path inside one that was already set aside went with it.
pub fn set_aside(plan: &RestorePlan, aside: &Path) -> Report {
    let mut report = Report::default();
    let mut moved: Vec<&Path> = Vec::new();
    for entry in plan.decisions_needed() {
        if moved.iter().any(|done| entry.path.starts_with(done)) {
            continue;
        }
        let source = plan.source_of(&entry.path);
        if std::fs::symlink_metadata(&source).is_err() {
            continue;
        }
        match move_path(&source, &aside.join(&entry.path)) {
            Ok(()) => {
                report.restored += 1;
                moved.push(entry.path.as_path());
            }
            Err(error) => report
                .failures
                .push((entry.path.clone(), error.to_string())),
        }
    }
    report
}

/// Put everything a restore did back the way it was, newest move first.
///
/// Walked backwards because the moves are not independent: a Keep Both renamed
/// a file aside before another took its name, and undoing them in the order
/// they happened would put the first one back on top of the second.
///
/// Something that came out of the Trash goes back into it, never away: a
/// trashed item's `.trashinfo` is written again before its files return, under
/// a free name if something else has been given its own in the meantime, so
/// the files follow it there.
pub fn undo(log: &MoveLog) -> Report {
    let mut report = Report::default();
    // A trashed item that went back under another name: where its copy was,
    // and where it is now.
    let mut renamed: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
    for done in log.moves.iter().rev() {
        let result = match done {
            Move::Added { path } => remove(path),
            Move::Replaced { path, stashed } => remove(path).and_then(|_| move_path(stashed, path)),
            Move::KeptBoth { path, renamed } => remove(path).and_then(|_| move_path(renamed, path)),
            // Only if it is still empty: a directory that was created may
            // since have had other things put in it.
            Move::MadeDir { path } => {
                let _ = std::fs::remove_dir(path);
                Ok(())
            }
            Move::FromTrash {
                path,
                copy,
                displaced,
            } => back_to_trash(path, &moved(copy, &renamed)).and_then(|_| match displaced {
                None => Ok(()),
                Some(Displaced::Stashed(from) | Displaced::Renamed(from)) => move_path(from, path),
            }),
            Move::LeftTrash {
                copy,
                folders,
                dir,
                info,
            } => into_trash(copy, folders, *dir, info.as_ref()).map(|now| {
                if &now != copy {
                    renamed.insert(copy.clone(), now);
                }
            }),
        };
        match result {
            Ok(()) => {
                if !matches!(done, Move::LeftTrash { .. }) {
                    report.restored += 1;
                }
            }
            Err(error) => {
                let path = match done {
                    Move::Added { path }
                    | Move::Replaced { path, .. }
                    | Move::KeptBoth { path, .. }
                    | Move::MadeDir { path }
                    | Move::FromTrash { path, .. } => path.clone(),
                    Move::LeftTrash { copy, .. } => copy.clone(),
                };
                warn!(path = %path.display(), %error, "a file could not be put back");
                report.failures.push((path, error.to_string()));
            }
        }
    }
    report
}

/// Where `copy` is now, if the item it is in went back to the Trash under
/// another name.
fn moved(copy: &Path, renamed: &BTreeMap<PathBuf, PathBuf>) -> PathBuf {
    for (was, now) in renamed {
        if let Ok(rest) = copy.strip_prefix(was) {
            return if rest.as_os_str().is_empty() {
                now.clone()
            } else {
                now.join(rest)
            };
        }
    }
    copy.to_path_buf()
}

/// Move `path` back to `copy` in the Trash, beside whatever has been put there
/// since rather than over it.
fn back_to_trash(path: &Path, copy: &Path) -> Result<()> {
    let taken = |n: &u32| {
        let candidate = copy.with_file_name(trash_name(copy.file_name().unwrap_or_default(), *n));
        std::fs::symlink_metadata(candidate).is_ok()
    };
    let free = (1..1000).find(|n| !taken(n)).unwrap_or(1);
    move_path(
        path,
        &copy.with_file_name(trash_name(copy.file_name().unwrap_or_default(), free)),
    )
}

/// Make the place in the Trash that a trashed item's files go back to: its
/// `.trashinfo` written again as it was, and the folders it had. Returns where
/// its copy is, which is under another name if its own has been taken.
fn into_trash(
    copy: &Path,
    folders: &[PathBuf],
    dir: bool,
    info: Option<&(PathBuf, String)>,
) -> Result<PathBuf> {
    let copy = match info {
        None => copy.to_path_buf(),
        Some((info, text)) => restore_info(copy, info, text)?,
    };
    if dir {
        std::fs::create_dir_all(&copy).map_err(|e| io_error(&copy, e))?;
        for folder in folders {
            let path = copy.join(folder);
            std::fs::create_dir_all(&path).map_err(|e| io_error(&path, e))?;
        }
    }
    Ok(copy)
}

/// Write a `.trashinfo` back, under the name it had if nothing else holds that
/// name now, and otherwise under the first free one, as the Trash
/// specification has it: the `.trashinfo` is made first, and only if it did
/// not exist, so that two programs cannot both take the same name.
fn restore_info(copy: &Path, info: &Path, text: &str) -> Result<PathBuf> {
    use std::io::Write;
    let (Some(files), Some(infos), Some(name)) = (copy.parent(), info.parent(), copy.file_name())
    else {
        return Err(io_error(
            copy,
            std::io::Error::other("not a place in a Trash"),
        ));
    };
    for n in 1..1000 {
        let name = trash_name(name, n);
        let copy = files.join(&name);
        if std::fs::symlink_metadata(&copy).is_ok() {
            continue;
        }
        let mut info_name = name.clone();
        info_name.push(".trashinfo");
        let info = infos.join(info_name);
        std::fs::create_dir_all(infos).map_err(|e| io_error(infos, e))?;
        let mut file = match std::fs::File::options()
            .write(true)
            .create_new(true)
            .open(&info)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error(&info, error)),
        };
        file.write_all(text.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|e| io_error(&info, e))?;
        std::fs::create_dir_all(files).map_err(|e| io_error(files, e))?;
        return Ok(copy);
    }
    Err(io_error(
        copy,
        std::io::Error::other("no free name is left in the Trash"),
    ))
}

/// The `n`th name an item can take in the Trash: its own, then `name.2.ext`,
/// `name.3.ext`, as GLib names a second item of the same name.
fn trash_name(name: &std::ffi::OsStr, n: u32) -> OsString {
    if n == 1 {
        return name.to_os_string();
    }
    let text = name.to_string_lossy();
    // The first dot after the first character: a hidden file's own dot does
    // not begin an extension.
    match text.char_indices().skip(1).find(|&(_, c)| c == '.') {
        Some((dot, _)) => OsString::from(format!("{}.{n}{}", &text[..dot], &text[dot..])),
        None => OsString::from(format!("{text}.{n}")),
    }
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
///
/// Only a copy needs room. A file that arrives by rename from a staging
/// directory on the same filesystem is already taking the space it will take,
/// and asking for that much again refused a large folder on any disk with less
/// than twice its size free; the same goes for a replaced file renamed into a
/// stash beside it.
fn check_space(plan: &RestorePlan, decisions: &Decisions, stash: &Path) -> Result<()> {
    let needed = plan.space_needed(decisions);
    let staged = if super::same_filesystem(&plan.staging, &plan.dest) {
        0
    } else {
        needed.dest - needed.from_trash
    };
    // A Trash on another drive than the destination, as for Restore To onto
    // another drive, is copied from like a staging directory there.
    let trashed = if plan
        .trash
        .iter()
        .all(|t| super::same_filesystem(&t.copy, &plan.dest))
    {
        0
    } else {
        needed.from_trash
    };
    let dest = staged + trashed;
    let stashed = if super::same_filesystem(&plan.dest, stash) {
        0
    } else {
        needed.stash
    };
    for (bytes, where_) in [(dest, plan.dest.as_path()), (stashed, stash)] {
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
pub(super) fn move_path(from: &Path, to: &Path) -> Result<()> {
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
pub(super) fn strip_root(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().trim_start_matches('/').to_string())
}

/// Seconds since the epoch, which is what a stash directory is named.
pub(super) fn stamp(now: SystemTime) -> u64 {
    now.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
