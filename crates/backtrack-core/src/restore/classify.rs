// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Deciding what a single path needs, which is the whole of the restore
//! engine's judgement in one function.
//!
//! It is kept free of I/O so the matrix of cases can be tested as a matrix
//! rather than as a directory tree per row. The one question it cannot answer
//! from metadata — "are these two files actually the same?" — is passed in as a
//! closure, which the walker fills with a content comparison and a test fills
//! with an answer.

/// What a path is, as far as a restore cares. Everything that is not a
/// directory or a symlink is treated as a file: a fifo or a socket in a backup
/// is vanishingly rare and is restored by the same move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
}

/// What is known about one side of a comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileFacts {
    pub kind: Kind,
    pub size: u64,
    /// Modification time, epoch **microseconds**.
    pub mtime: i64,
}

/// How far apart two modification times may be and still count as the same
/// instant, in microseconds.
///
/// Borg stores modification times to microsecond resolution, so a file
/// extracted from an archive carries a truncated copy of the time the original
/// still has in full. Comparing at nanosecond resolution would therefore report
/// every untouched file as modified, and the restore would offer to replace a
/// folder with itself.
pub const MTIME_TOLERANCE_MICROS: i64 = 1;

/// What the restore has found for one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Same on both sides. Left alone, and never shown in a dialog — "already
    /// up to date" is not a decision anybody wants to be asked to make.
    Identical,
    /// In the backup, not on disk. Added.
    OnlyInBackup,
    /// On disk, not in the backup. **Always kept**, with no decision attached:
    /// a restore merges and never deletes.
    OnlyOnDisk,
    /// On both sides and different. The only case that needs an answer.
    Conflict {
        /// Whether the copy on disk is the newer of the two. Restoring over
        /// newer work is the risky direction, and the dialog says so louder.
        disk_newer: bool,
    },
    /// A file where the backup has a directory, or either where the other has a
    /// symlink. Never resolved silently — replacing a directory with a file is
    /// not something to infer from a click on "Replace".
    TypeChanged,
}

/// Classify one path from what is known about each side.
///
/// `same_content` is consulted only when metadata cannot settle it: equal sizes
/// with differing timestamps, which is the one case where reading the bytes is
/// both necessary and affordable. Differing sizes are different by definition,
/// and no file is read to prove it.
pub fn classify(
    backup: Option<&FileFacts>,
    disk: Option<&FileFacts>,
    same_content: impl FnOnce() -> bool,
) -> Class {
    match (backup, disk) {
        (Some(_), None) => Class::OnlyInBackup,
        (None, Some(_)) => Class::OnlyOnDisk,
        // Not reachable through the walker, which only yields paths seen on at
        // least one side. Nothing to do is the truthful answer regardless.
        (None, None) => Class::Identical,
        (Some(backup), Some(disk)) => {
            if backup.kind != disk.kind {
                return Class::TypeChanged;
            }
            // A directory present on both sides is nothing to restore. Its
            // contents are separate paths and are classified on their own.
            if backup.kind == Kind::Dir {
                return Class::Identical;
            }
            let conflict = Class::Conflict {
                disk_newer: disk.mtime > backup.mtime,
            };
            // A symlink's size and timestamp say nothing about it. Where it
            // points is the only question, and reading that is a `readlink`
            // rather than a file read, so it is always asked.
            if backup.kind == Kind::Symlink {
                return if same_content() {
                    Class::Identical
                } else {
                    conflict
                };
            }
            if backup.size != disk.size {
                return conflict;
            }
            if (backup.mtime - disk.mtime).abs() <= MTIME_TOLERANCE_MICROS {
                return Class::Identical;
            }
            if same_content() {
                Class::Identical
            } else {
                conflict
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(size: u64, mtime: i64) -> FileFacts {
        FileFacts {
            kind: Kind::File,
            size,
            mtime,
        }
    }

    fn dir() -> FileFacts {
        FileFacts {
            kind: Kind::Dir,
            size: 0,
            mtime: 0,
        }
    }

    fn link(mtime: i64) -> FileFacts {
        FileFacts {
            kind: Kind::Symlink,
            size: 0,
            mtime,
        }
    }

    /// Panics if called: proves the metadata path settled it without reading.
    fn never_read() -> bool {
        panic!("no file should have been read to decide this");
    }

    #[test]
    fn a_path_on_one_side_only_is_an_addition_or_a_keep() {
        assert_eq!(
            classify(Some(&file(10, 100)), None, never_read),
            Class::OnlyInBackup
        );
        assert_eq!(
            classify(None, Some(&file(10, 100)), never_read),
            Class::OnlyOnDisk
        );
    }

    #[test]
    fn matching_size_and_time_is_identical_without_reading_anything() {
        assert_eq!(
            classify(Some(&file(10, 100)), Some(&file(10, 100)), never_read),
            Class::Identical
        );
    }

    #[test]
    fn a_microsecond_of_slack_absorbs_borgs_truncated_timestamps() {
        // The extracted copy carries a truncated version of a time the original
        // still holds in full; without the slack every untouched file would be
        // offered for replacement.
        assert_eq!(
            classify(Some(&file(10, 100)), Some(&file(10, 101)), never_read),
            Class::Identical
        );
        // Two microseconds apart is a real edit, and gets read to be sure.
        assert_eq!(
            classify(Some(&file(10, 100)), Some(&file(10, 102)), || false),
            Class::Conflict { disk_newer: true }
        );
    }

    #[test]
    fn differing_sizes_are_a_conflict_and_no_file_is_read_to_prove_it() {
        assert_eq!(
            classify(Some(&file(10, 100)), Some(&file(11, 100)), never_read),
            Class::Conflict { disk_newer: false }
        );
    }

    #[test]
    fn equal_sizes_with_different_times_are_settled_by_the_contents() {
        // A file touched but not changed is up to date, and must not turn into
        // a dialog asking whether to replace it with itself.
        assert_eq!(
            classify(Some(&file(10, 100)), Some(&file(10, 500)), || true),
            Class::Identical
        );
        assert_eq!(
            classify(Some(&file(10, 100)), Some(&file(10, 500)), || false),
            Class::Conflict { disk_newer: true }
        );
    }

    #[test]
    fn which_side_is_newer_is_recorded_for_the_dialog_to_shout_about() {
        assert_eq!(
            classify(Some(&file(10, 100)), Some(&file(20, 900)), never_read),
            Class::Conflict { disk_newer: true }
        );
        assert_eq!(
            classify(Some(&file(10, 900)), Some(&file(20, 100)), never_read),
            Class::Conflict { disk_newer: false }
        );
    }

    #[test]
    fn a_change_of_type_is_never_resolved_silently() {
        assert_eq!(
            classify(Some(&dir()), Some(&file(10, 100)), never_read),
            Class::TypeChanged
        );
        assert_eq!(
            classify(Some(&file(10, 100)), Some(&dir()), never_read),
            Class::TypeChanged
        );
        assert_eq!(
            classify(Some(&link(100)), Some(&file(10, 100)), never_read),
            Class::TypeChanged
        );
    }

    #[test]
    fn a_directory_on_both_sides_is_nothing_to_restore() {
        assert_eq!(
            classify(Some(&dir()), Some(&dir()), never_read),
            Class::Identical
        );
    }

    #[test]
    fn two_symlinks_are_compared_by_where_they_point() {
        // Size and time say nothing useful about a link, so the target is the
        // only question, and it is the closure's to answer.
        assert_eq!(
            classify(Some(&link(100)), Some(&link(500)), || true),
            Class::Identical
        );
        assert_eq!(
            classify(Some(&link(100)), Some(&link(500)), || false),
            Class::Conflict { disk_newer: true }
        );
    }

    #[test]
    fn matching_metadata_does_not_excuse_a_symlink_from_being_looked_at() {
        // Two links of the same age and size can still point somewhere
        // different, and metadata cannot tell.
        assert_eq!(
            classify(Some(&link(100)), Some(&link(100)), || false),
            Class::Conflict { disk_newer: false }
        );
    }
}
