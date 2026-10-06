// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! What the file pane says when a folder has nothing to show.
//!
//! An empty list has more than one cause, and each wants its own sentence: a
//! folder that was empty at that backup, one that did not exist yet, one that
//! has just been added to the backups and is waiting for the next one, and one
//! that is not backed up at all. The last is where a file manager sends the
//! window for a folder outside the backups: Dolphin's menu cannot know which
//! folders those are, so it offers itself everywhere.
//!
//! Inside a backed-up folder, the window cannot tell an exclusion, or a disk
//! mounted there, from a folder created since the last backup: only the daemon
//! applies those rules. So it says "will be in the next backup" only where
//! that is certain, for a backed-up folder the last backup did not have at
//! all, and otherwise names the possibilities.

use std::path::PathBuf;

/// Why a folder has nothing to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Empty {
    /// It was empty at this backup, or was not there yet, and a newer backup
    /// may say otherwise.
    Then,
    /// The newest backup does not have it, for a reason only the daemon knows.
    Missing,
    /// It is in a folder added to the backups since the newest one was taken.
    Waiting,
    /// It is outside every folder Backtrack backs up.
    Outside,
}

/// Where a folder stands with the folders Backtrack backs up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// The daemon could not be asked what they are.
    Unknown,
    /// Outside all of them.
    Outside,
    /// Inside one of them, which the backup being viewed has or does not.
    Inside { root_in_backup: bool },
}

/// The backed-up folder `folder` (an archive path) is in, if any.
pub fn root_of<'a>(folder: &str, roots: &'a [PathBuf]) -> Option<&'a PathBuf> {
    roots.iter().find(|root| {
        let root = crate::path::to_archive(root);
        // The filesystem root holds everything.
        root.is_empty() || crate::path::is_within(folder, &root)
    })
}

/// Why a folder has nothing to show at the backup being viewed.
///
/// `in_backup` is whether the folder itself is in that backup, which makes it
/// an empty folder rather than a missing one. `newest` is whether that backup
/// is the newest: a folder missing from an older one may be in a newer one.
pub fn why(in_backup: bool, newest: bool, coverage: Coverage) -> Empty {
    if in_backup || !newest {
        return Empty::Then;
    }
    match coverage {
        Coverage::Outside => Empty::Outside,
        Coverage::Inside {
            root_in_backup: false,
        } => Empty::Waiting,
        Coverage::Inside {
            root_in_backup: true,
        }
        | Coverage::Unknown => Empty::Missing,
    }
}

impl Empty {
    pub fn title(self) -> &'static str {
        match self {
            Empty::Then => "Not in this backup",
            Empty::Missing => "Not in the latest backup",
            Empty::Waiting => "Not backed up yet",
            Empty::Outside => "This folder is not backed up",
        }
    }

    /// The sentence under the title, about the folder called `name`.
    pub fn description(self, name: &str) -> String {
        match self {
            Empty::Then => format!(
                "“{name}” has nothing in it at this point in time. Step forward, or pick a more recent backup."
            ),
            Empty::Missing => format!(
                "“{name}” was not in the most recent backup. It may be newer than that backup, or left out of it by an exclusion or because it is on another disk."
            ),
            Empty::Waiting => format!("“{name}” will be in the next backup."),
            Empty::Outside => format!(
                "“{name}” is not one of the folders Backtrack backs up, so there are no earlier versions of it to look through."
            ),
        }
    }

    /// Whether the page offers to add the folder to the backups.
    pub fn offers_to_add(self) -> bool {
        self == Empty::Outside
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots(paths: &[&str]) -> Vec<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn a_folder_in_the_backup_was_simply_empty() {
        assert_eq!(why(true, true, Coverage::Outside), Empty::Then);
        assert_eq!(
            why(
                true,
                true,
                Coverage::Inside {
                    root_in_backup: true
                }
            ),
            Empty::Then
        );
    }

    #[test]
    fn an_older_backup_cannot_speak_for_a_newer_one() {
        // A newer backup may have it, so the page says to step forward.
        for coverage in [
            Coverage::Unknown,
            Coverage::Outside,
            Coverage::Inside {
                root_in_backup: false,
            },
        ] {
            assert_eq!(why(false, false, coverage), Empty::Then, "{coverage:?}");
        }
    }

    #[test]
    fn a_folder_outside_every_root_is_not_backed_up() {
        let why = why(false, true, Coverage::Outside);
        assert_eq!(why, Empty::Outside);
        assert!(why.offers_to_add());
    }

    #[test]
    fn a_folder_in_a_newly_added_root_waits_for_the_next_backup() {
        let why = why(
            false,
            true,
            Coverage::Inside {
                root_in_backup: false,
            },
        );
        assert_eq!(why, Empty::Waiting);
        assert!(!why.offers_to_add());
    }

    #[test]
    fn a_folder_missing_from_a_backed_up_root_is_not_promised_to_the_next() {
        // An exclusion, or a disk mounted inside the root, keeps it out of
        // every backup, and the window cannot tell those from a new folder.
        let missing = why(
            false,
            true,
            Coverage::Inside {
                root_in_backup: true,
            },
        );
        assert_eq!(missing, Empty::Missing);
        assert!(!missing.offers_to_add());
        assert_eq!(why(false, true, Coverage::Unknown), Empty::Missing);
    }

    #[test]
    fn the_root_a_folder_is_in() {
        let roots = roots(&["/home/keith/Documents", "/srv/data"]);
        assert_eq!(
            root_of("home/keith/Documents/2026", &roots),
            Some(&roots[0])
        );
        assert_eq!(root_of("srv/data", &roots), Some(&roots[1]));
        assert_eq!(root_of("home/keith/Downloads", &roots), None);
    }

    #[test]
    fn a_folder_above_a_root_is_outside_it() {
        // Borg records nothing above the folders it was given, so the home
        // folder of a backup of Documents alone has nothing in it.
        let roots = roots(&["/home/keith/Documents"]);
        assert_eq!(root_of("home", &roots), None);
    }

    #[test]
    fn a_root_s_name_is_not_a_prefix_match() {
        let roots = roots(&["/home/keith/Doc"]);
        assert_eq!(root_of("home/keith/Documents", &roots), None);
    }

    #[test]
    fn the_filesystem_root_holds_everything() {
        let roots = roots(&["/"]);
        assert_eq!(root_of("srv/data", &roots), Some(&roots[0]));
    }

    #[test]
    fn every_page_names_the_folder() {
        for why in [Empty::Then, Empty::Missing, Empty::Waiting, Empty::Outside] {
            assert!(why.description("Downloads").contains("“Downloads”"));
        }
    }
}
