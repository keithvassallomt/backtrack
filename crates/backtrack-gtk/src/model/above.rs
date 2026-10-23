// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The folders above the backed-up ones (S06-T11).
//!
//! A backup holds the folders it was told to back up and everything inside
//! them, and nothing of the folders they sit in. With only `~/Smoke/Documents`
//! backed up, the catalogue lists Documents inside Smoke, because Documents is
//! in the backup, and nothing at all inside the home folder, because Smoke is
//! not. Left at that, the home folder is a dead end: the breadcrumb only leads
//! up, and the pane has nothing to step into.
//!
//! So a folder outside the backups that holds backed-up folders shows the
//! folders that lead down to them, and says that only some of it is backed up.
//! Which folders those are follows the backup being viewed: one taken before
//! Documents was added leads nowhere.
//!
//! Paths here are archive paths (no leading slash; the empty string is the
//! filesystem root), and `roots` are a backup's top folders, as
//! `IndexReader::backed_up_roots` gives them.

use crate::path;

/// The badge on a folder that leads down to backed-up folders without being
/// backed up itself.
pub const BADGE: &str = "Some items backed up";

/// Whether `folder` is outside every one of `roots` and holds at least one of
/// them: a folder the pane says is only partly backed up.
pub fn partly(folder: &str, roots: &[String]) -> bool {
    !roots.iter().any(|root| within(folder, root))
        && roots
            .iter()
            .any(|root| root != folder && within(root, folder))
}

/// The folders directly inside `folder` that lead down to one of `roots`
/// further below, by name, sorted and each once.
///
/// A root directly inside `folder` is not among them: it is in the backup
/// itself, so the catalogue lists it already, with its size and its date.
pub fn leading_down(folder: &str, roots: &[String]) -> Vec<String> {
    let mut names: Vec<String> = roots
        .iter()
        .filter(|root| *root != folder && within(root, folder))
        .filter_map(|root| {
            let rest = if folder.is_empty() {
                root.as_str()
            } else {
                &root[folder.len() + 1..]
            };
            // Only a root two or more levels down needs a way to it.
            let (first, _) = rest.split_once('/')?;
            Some(first.to_string())
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The line above a partly backed-up folder's list. `label` is what the
/// breadcrumb calls the folder ("Home" for the home folder), or `None` for
/// the filesystem root, which has no crumb.
pub fn notice(label: Option<&str>) -> String {
    match label {
        Some(label) => format!("Some items in {label} are backed up."),
        None => "Some items here are backed up.".to_string(),
    }
}

/// Whether `path` is `folder` or inside it; the filesystem root holds all.
fn within(path: &str, folder: &str) -> bool {
    folder.is_empty() || path::is_within(path, folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn the_home_folder_leads_down_to_a_folder_backed_up_deep_inside_it() {
        let backed_up = roots(&["home/smoke/Smoke/Documents"]);
        assert!(partly("home/smoke", &backed_up));
        assert_eq!(leading_down("home/smoke", &backed_up), ["Smoke"]);
        // One level up from the backed-up folder, the catalogue lists it
        // itself: nothing to add, and still only partly backed up.
        assert!(partly("home/smoke/Smoke", &backed_up));
        assert!(leading_down("home/smoke/Smoke", &backed_up).is_empty());
        // From the filesystem root, the way starts at "home".
        assert!(partly("", &backed_up));
        assert_eq!(leading_down("", &backed_up), ["home"]);
    }

    #[test]
    fn inside_a_backed_up_folder_nothing_is_added() {
        let backed_up = roots(&["home/smoke/Smoke/Documents"]);
        for folder in [
            "home/smoke/Smoke/Documents",
            "home/smoke/Smoke/Documents/Reports",
        ] {
            assert!(!partly(folder, &backed_up), "{folder}");
            assert!(leading_down(folder, &backed_up).is_empty(), "{folder}");
        }
    }

    #[test]
    fn a_folder_inside_the_backups_is_never_partly_backed_up() {
        // A backup's top folders do not overlap, but a list that did, with
        // the home folder and a folder deep inside it, still leaves the home
        // folder whole: everything in it is backed up.
        let overlapping = roots(&["home/smoke", "home/smoke/Smoke/Documents"]);
        assert!(!partly("home/smoke", &overlapping));
        assert!(!partly("home/smoke/Smoke", &overlapping));
    }

    #[test]
    fn a_folder_beside_the_backups_is_not_partly_backed_up() {
        let backed_up = roots(&["home/smoke/Smoke/Documents"]);
        assert!(!partly("home/smoke/Downloads", &backed_up));
        assert!(leading_down("home/smoke/Downloads", &backed_up).is_empty());
        // A name that merely starts the same is another folder.
        assert!(!partly("home/smoke/Smo", &backed_up));
    }

    #[test]
    fn folders_backed_up_in_several_places_each_get_a_way_down() {
        let backed_up = roots(&[
            "home/smoke/Smoke/Documents",
            "home/smoke/Smoke/Pictures",
            "home/smoke/Work/Projects/2026",
            "srv/data",
        ]);
        assert_eq!(leading_down("home/smoke", &backed_up), ["Smoke", "Work"]);
        assert_eq!(leading_down("", &backed_up), ["home", "srv"]);
        assert_eq!(leading_down("home/smoke/Work", &backed_up), ["Projects"]);
    }

    #[test]
    fn a_backup_of_the_whole_computer_has_nothing_above_it() {
        let backed_up = roots(&[""]);
        for folder in ["", "home", "home/smoke"] {
            assert!(!partly(folder, &backed_up), "{folder:?}");
            assert!(leading_down(folder, &backed_up).is_empty(), "{folder:?}");
        }
    }

    #[test]
    fn a_backup_from_before_the_folder_was_added_leads_nowhere() {
        // The older backup held Pictures only; Documents came later.
        let older = roots(&["home/smoke/Pictures"]);
        assert!(leading_down("home/smoke/Smoke", &older).is_empty());
        assert!(!partly("home/smoke/Smoke", &older));
    }

    #[test]
    fn the_notice_names_the_folder_as_the_breadcrumb_does() {
        assert_eq!(notice(Some("Home")), "Some items in Home are backed up.");
        assert_eq!(notice(Some("Smoke")), "Some items in Smoke are backed up.");
        assert_eq!(notice(None), "Some items here are backed up.");
    }
}
