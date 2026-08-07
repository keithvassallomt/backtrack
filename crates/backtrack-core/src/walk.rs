// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Walking the backup sources as they are *now*, so the offline spool can work
//! out what changed.
//!
//! Only the offline path needs this. An ordinary backup hands Borg the sources
//! and lets it do its own walking; the spool cannot, because it has to know the
//! delta before it asks Borg for anything.
//!
//! What the walk emits is shaped by what it is for:
//!
//! - **Archive-relative paths**, leading `/` removed, because that is how Borg
//!   stores member paths and therefore how the catalogue holds them. Comparing
//!   anything else against the index would report every file as new.
//! - **Microsecond mtimes, rounded the way Borg rounds them.** The kernel keeps
//!   nanoseconds; Borg records microseconds. This is not a detail — change
//!   detection is an equality test, so being one microsecond out systematically
//!   makes every file compare as modified and the first offline tick tries to
//!   spool the entire home directory.
//! - **Files and symlinks only.** Directories are descended but not emitted.
//!   Handing Borg a directory would make it recurse, and a directory's mtime
//!   changes whenever any child is added or removed — so a single new file in
//!   `Documents` would drag every unchanged document in it into the spool. The
//!   cost is that a folder *created* during an offline window is not itself an
//!   entry in the spool snapshot; its files are catalogued and restorable, and
//!   the next backup to the real destination records the folder properly.
//! - **Pruned at exclusions**, using the same patterns Borg is given. See
//!   [`crate::pattern`] for why that matching has to happen here at all.

use std::path::{Path, PathBuf};

use crate::index::{BorgItem, Kind, LiveEntry};
use crate::pattern::{is_within, ExcludeSet};

/// What to walk, and what to leave alone.
pub struct WalkSpec {
    /// The configured backup sources, as absolute paths.
    pub sources: Vec<PathBuf>,
    /// Borg's exclusion patterns, already compiled.
    pub excludes: ExcludeSet,
    /// Stay on the filesystem each source starts on.
    pub one_file_system: bool,
    /// Directories never descended into, whatever the patterns say. Backtrack's
    /// own data directory goes here: the spool repository lives in it, and a
    /// walk that picked it up would be feeding the backup its own output.
    pub never: Vec<PathBuf>,
    /// Emit directory entries as well as descending into them.
    ///
    /// Off for the spool, which is producing a *delta* to hand to Borg: a
    /// directory in that list would make Borg recurse, and a directory's mtime
    /// changes whenever any child does, so one new file in `Documents` would
    /// drag every unchanged document into the archive.
    ///
    /// On for a filesystem snapshot, which is a *full* listing: the catalogue
    /// needs directory rows or the timeline cannot list a folder inside its
    /// parent.
    pub include_dirs: bool,
}

/// What a walk found.
#[derive(Debug, Default)]
pub struct Walked {
    /// Everything in scope, in the same shape a Borg listing would arrive in,
    /// so both consumers — the delta diff and a full snapshot ingest — read the
    /// same walk.
    pub items: Vec<BorgItem>,
    /// Paths that could not be read. Not an error — a directory the user cannot
    /// enter is a fact about the machine, not a failure of the backup — but
    /// worth counting so a walk that skipped most of the disk does not look
    /// like a walk that found nothing to do.
    pub unreadable: usize,
}

impl Walked {
    /// The walk as change-detection input.
    pub fn live_entries(self) -> Vec<LiveEntry> {
        self.items
            .into_iter()
            .map(|item| LiveEntry {
                path: PathBuf::from(item.path),
                size: item.size,
                mtime: item.mtime,
            })
            .collect()
    }
}

/// Walk `spec`'s sources.
///
/// Blocking: this is a lot of `lstat`, and it belongs on a blocking thread.
///
/// Symbolic links are recorded but never followed. Following them would let a
/// link into `/` turn an offline tick into a walk of the whole machine, and
/// would archive the same files under two names.
pub fn walk(spec: &WalkSpec) -> Walked {
    let mut out = Walked::default();
    for source in &spec.sources {
        let root_device = match std::fs::symlink_metadata(source) {
            Ok(meta) => device_of(&meta),
            Err(_) => {
                out.unreadable += 1;
                continue;
            }
        };
        walk_from(source, root_device, spec, &mut out);
    }
    out.items.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Depth-first, iteratively. Recursion would put the directory tree's depth on
/// the stack, and a deep enough tree — or a pathological one — would take the
/// daemon down.
fn walk_from(root: &Path, root_device: u64, spec: &WalkSpec, out: &mut Walked) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let listing = match std::fs::read_dir(&dir) {
            Ok(listing) => listing,
            Err(_) => {
                out.unreadable += 1;
                continue;
            }
        };
        for entry in listing {
            let Ok(entry) = entry else {
                out.unreadable += 1;
                continue;
            };
            let path = entry.path();
            if spec.never.iter().any(|skip| is_within(&path, skip)) {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                // Vanished between the listing and the stat. Nothing to do: it
                // is not there to protect.
                out.unreadable += 1;
                continue;
            };
            if spec.one_file_system && device_of(&meta) != root_device {
                continue;
            }
            let relative = archive_path(&path);
            if spec.excludes.excludes(&relative) {
                // Matching a directory prunes the subtree, exactly as it does
                // for Borg — and it is what keeps a cache directory from
                // costing a walk of every file in it.
                continue;
            }
            let is_dir = meta.is_dir();
            if is_dir {
                stack.push(path);
                if !spec.include_dirs {
                    continue;
                }
            }
            out.items.push(BorgItem {
                path: relative,
                kind: kind_of(&meta),
                size: if is_dir { 0 } else { meta.len() as i64 },
                mtime: mtime_micros(&meta),
                mode: mode_of(&meta),
                chunk_hash: None,
            });
        }
    }
}

/// A path in the form Borg stores it and the index holds it: absolute, with the
/// leading `/` removed.
pub fn archive_path(path: &Path) -> String {
    path.to_string_lossy().trim_start_matches('/').to_string()
}

/// Modification time in epoch microseconds, **rounded** from the kernel's
/// nanoseconds.
///
/// Rounded, not truncated, because that is what Borg does on the way in: it
/// renders the timestamp through Python's microsecond-resolution `datetime`,
/// which rounds. Measured rather than assumed — across 300 files written by
/// borg 1.4.5, rounding reproduced every stored timestamp exactly and
/// truncation was one microsecond low on every one of them.
///
/// Getting this backwards is not a rounding error, it is a broken product:
/// change detection compares this against the indexed value for equality, so a
/// systematic one-microsecond difference makes every file on the machine
/// compare as modified, and the first offline tick tries to spool the entire
/// home directory.
fn mtime_micros(meta: &std::fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    let nanos = i64::from(meta.mtime_nsec() as i32);
    meta.mtime()
        .saturating_mul(1_000_000)
        .saturating_add((nanos + 500) / 1_000)
}

/// The object type, in the vocabulary the catalogue stores.
fn kind_of(meta: &std::fs::Metadata) -> Kind {
    let t = meta.file_type();
    if t.is_dir() {
        Kind::Dir
    } else if t.is_symlink() {
        Kind::Symlink
    } else if t.is_file() {
        Kind::File
    } else {
        Kind::Other
    }
}

/// Permission and special bits only — the type lives in [`Kind`], exactly as it
/// does when the same row arrives from a Borg listing.
fn mode_of(meta: &std::fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    i64::from(meta.mode() & 0o7777)
}

fn device_of(meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.dev()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(root: &Path, excludes: &[&str]) -> WalkSpec {
        WalkSpec {
            sources: vec![root.to_path_buf()],
            excludes: ExcludeSet::compile(
                &excludes.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            ),
            one_file_system: true,
            never: Vec::new(),
            include_dirs: false,
        }
    }

    fn names(walked: &Walked, root: &Path) -> Vec<String> {
        let prefix = archive_path(root);
        let mut names: Vec<String> = walked
            .items
            .iter()
            .map(|e| {
                e.path
                    .strip_prefix(&prefix)
                    .unwrap_or_default()
                    .trim_start_matches('/')
                    .to_string()
            })
            .collect();
        names.sort();
        names
    }

    /// A small tree with the shapes that matter: nesting, a cache to prune, a
    /// symlink, and an empty directory.
    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("docs/deep")).unwrap();
        std::fs::create_dir_all(root.join(".cache/inner")).unwrap();
        std::fs::create_dir_all(root.join("empty")).unwrap();
        std::fs::write(root.join("top.txt"), b"top").unwrap();
        std::fs::write(root.join("docs/a.txt"), b"aaa").unwrap();
        std::fs::write(root.join("docs/deep/b.txt"), b"bb").unwrap();
        std::fs::write(root.join(".cache/junk"), b"junk").unwrap();
        std::fs::write(root.join(".cache/inner/more"), b"more").unwrap();
        std::os::unix::fs::symlink("docs/a.txt", root.join("link")).unwrap();
        dir
    }

    #[test]
    fn every_file_and_symlink_is_found_and_directories_are_not_emitted() {
        let dir = tree();
        let walked = walk(&spec(dir.path(), &[]));
        assert_eq!(
            names(&walked, dir.path()),
            vec![
                ".cache/inner/more",
                ".cache/junk",
                "docs/a.txt",
                "docs/deep/b.txt",
                "link",
                "top.txt",
            ]
        );
    }

    #[test]
    fn an_excluded_directory_prunes_its_whole_subtree() {
        // Not merely filtered out of the results — never descended into, which
        // is what stops a cache directory costing a walk of every file in it.
        let dir = tree();
        let walked = walk(&spec(dir.path(), &["**/.cache"]));
        assert_eq!(
            names(&walked, dir.path()),
            vec!["docs/a.txt", "docs/deep/b.txt", "link", "top.txt"]
        );
    }

    #[test]
    fn paths_come_back_in_the_form_the_catalogue_holds_them() {
        // Archive-relative, no leading slash. Comparing anything else against
        // the index would report every file on the machine as new.
        let dir = tree();
        let walked = walk(&spec(dir.path(), &[]));
        for item in &walked.items {
            assert!(
                !item.path.starts_with('/'),
                "{} kept its leading slash",
                item.path
            );
        }
    }

    #[test]
    fn sizes_and_times_are_recorded_at_borgs_resolution() {
        let dir = tree();
        let walked = walk(&spec(dir.path(), &[]));
        let top = walked
            .items
            .iter()
            .find(|e| e.path.ends_with("top.txt"))
            .expect("top.txt was walked");
        assert_eq!(top.size, 3);

        let meta = std::fs::metadata(dir.path().join("top.txt")).unwrap();
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            top.mtime,
            meta.mtime() * 1_000_000 + (i64::from(meta.mtime_nsec() as i32) + 500) / 1_000,
            "microseconds, rounded — the resolution borg stores, and how it rounds"
        );
    }

    #[test]
    fn a_symlink_is_recorded_without_being_followed() {
        // Following would archive the target's contents a second time, and a
        // link pointing at `/` would turn an hourly delta into a walk of the
        // whole machine.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::fs::write(dir.path().join("real/inside.txt"), b"x").unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("loop")).unwrap();

        let walked = walk(&spec(dir.path(), &[]));
        let found = names(&walked, dir.path());
        assert!(
            found.contains(&"loop".to_string()),
            "the link itself: {found:?}"
        );
        assert!(
            !found.contains(&"loop/inside.txt".to_string()),
            "the link was followed: {found:?}"
        );
    }

    #[test]
    fn a_symlink_cycle_terminates() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(dir.path(), dir.path().join("self")).unwrap();
        let walked = walk(&spec(dir.path(), &[]));
        assert_eq!(names(&walked, dir.path()), vec!["self"]);
    }

    #[test]
    fn the_never_list_is_not_descended_into() {
        // Backtrack's own data directory. Without this the spool repository
        // would be walked, found to have changed, and archived into itself.
        let dir = tree();
        let mut spec = spec(dir.path(), &[]);
        spec.never = vec![dir.path().join("docs")];
        let found = names(&walk(&spec), dir.path());
        assert_eq!(
            found,
            vec![".cache/inner/more", ".cache/junk", "link", "top.txt"]
        );
    }

    #[test]
    fn an_unreadable_directory_is_counted_rather_than_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::write(locked.join("secret"), b"x").unwrap();
        std::fs::write(dir.path().join("readable.txt"), b"y").unwrap();
        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o000);
        std::fs::set_permissions(&locked, perms).unwrap();

        let walked = walk(&spec(dir.path(), &[]));

        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o700);
        std::fs::set_permissions(&locked, perms).unwrap();

        assert_eq!(names(&walked, dir.path()), vec!["readable.txt"]);
        assert_eq!(walked.unreadable, 1, "and it is counted, not swallowed");
    }

    #[test]
    fn an_absent_source_is_counted_rather_than_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let walked = walk(&WalkSpec {
            sources: vec![dir.path().join("not-there")],
            excludes: ExcludeSet::default(),
            one_file_system: true,
            never: Vec::new(),
            include_dirs: false,
        });
        assert!(walked.items.is_empty());
        assert_eq!(walked.unreadable, 1);
    }

    #[test]
    fn a_full_listing_includes_directories_and_their_kinds() {
        // What a filesystem snapshot needs: the catalogue cannot list a folder
        // inside its parent without a row for the folder itself.
        let dir = tree();
        let mut spec = spec(dir.path(), &[]);
        spec.include_dirs = true;
        let walked = walk(&spec);

        let found = names(&walked, dir.path());
        assert!(found.contains(&"docs".to_string()), "got {found:?}");
        assert!(found.contains(&"docs/deep".to_string()));
        assert!(found.contains(&"empty".to_string()), "even an empty one");

        let kinds: Vec<(&str, Kind)> = walked
            .items
            .iter()
            .map(|i| (i.path.as_str(), i.kind))
            .collect();
        assert!(kinds
            .iter()
            .any(|(p, k)| p.ends_with("docs") && *k == Kind::Dir));
        assert!(kinds
            .iter()
            .any(|(p, k)| p.ends_with("top.txt") && *k == Kind::File));
        assert!(kinds
            .iter()
            .any(|(p, k)| p.ends_with("link") && *k == Kind::Symlink));
    }

    #[test]
    fn modes_carry_permissions_without_the_type_bits() {
        // The same convention a Borg listing arrives in: `Kind` holds the type,
        // `mode` holds permissions. Mixing them would make an ingested snapshot
        // disagree with an ingested archive for the same file.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, b"x").unwrap();
        let mut perms = std::fs::metadata(&file).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o640);
        std::fs::set_permissions(&file, perms).unwrap();

        let walked = walk(&spec(dir.path(), &[]));
        assert_eq!(walked.items[0].mode, 0o640);
        assert_eq!(walked.items[0].kind, Kind::File);
    }

    #[test]
    fn results_are_ordered_so_two_walks_agree() {
        let dir = tree();
        let first = walk(&spec(dir.path(), &[]));
        let second = walk(&spec(dir.path(), &[]));
        assert_eq!(
            first.items.iter().map(|e| &e.path).collect::<Vec<_>>(),
            second.items.iter().map(|e| &e.path).collect::<Vec<_>>()
        );
    }
}
