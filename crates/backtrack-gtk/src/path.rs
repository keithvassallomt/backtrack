// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Translating between the paths on this machine and the paths in an archive.
//!
//! Borg records an absolute path with its leading `/` removed, so
//! `/home/keith/Documents` is stored as `home/keith/Documents`. Every path the
//! index holds is in that form, and every path the user hands us — on the
//! command line, from a file manager — is in the other. One conversion, in one
//! place, rather than a `strip_prefix` scattered through the widgets.

use std::path::{Path, PathBuf};

/// The archive-relative form of an absolute filesystem path.
///
/// A relative path is returned unchanged: it is already in the archive's form,
/// which is what makes this safe to apply twice.
pub fn to_archive(path: &Path) -> String {
    let text = path.to_string_lossy();
    let trimmed = text.trim_end_matches('/');
    trimmed.trim_start_matches('/').to_string()
}

/// Where an archive path would live on this machine.
pub fn to_filesystem(archive_path: &str) -> PathBuf {
    PathBuf::from(format!("/{archive_path}"))
}

/// The folder containing `archive_path`, or `None` at the root of the archive.
pub fn parent(archive_path: &str) -> Option<String> {
    let (parent, _) = archive_path.rsplit_once('/')?;
    Some(parent.to_string())
}

/// The last component of `archive_path`.
pub fn name(archive_path: &str) -> &str {
    archive_path
        .rsplit_once('/')
        .map_or(archive_path, |(_, n)| n)
}

/// A child of `folder`.
pub fn join(folder: &str, child: &str) -> String {
    if folder.is_empty() {
        child.to_string()
    } else {
        format!("{folder}/{child}")
    }
}

/// Whether `path` is `folder` itself or inside it.
pub fn is_within(path: &str, folder: &str) -> bool {
    path == folder || path.starts_with(&format!("{folder}/"))
}

/// One step of the breadcrumb.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crumb {
    /// What the button reads.
    pub label: String,
    /// The folder it navigates to.
    pub path: String,
    /// Whether it stands for the user's home directory, which gets an icon
    /// instead of the account name — the same shorthand every file manager uses.
    pub is_home: bool,
}

/// The trail from the top of the tree down to `folder`.
///
/// When `folder` is inside `home`, the trail starts at a single Home crumb
/// rather than spelling out `home ▸ keith`: those two components are noise in
/// every file manager, and they are noise here.
pub fn breadcrumbs(folder: &str, home: Option<&str>) -> Vec<Crumb> {
    if folder.is_empty() {
        return Vec::new();
    }
    if let Some(home) = home.filter(|h| !h.is_empty() && is_within(folder, h)) {
        let mut crumbs = vec![Crumb {
            label: "Home".to_string(),
            path: home.to_string(),
            is_home: true,
        }];
        let rest = folder[home.len()..].trim_start_matches('/');
        crumbs.extend(trail(home, rest));
        return crumbs;
    }
    trail("", folder)
}

/// Crumbs for each component of `rest`, rooted at `base`.
fn trail(base: &str, rest: &str) -> Vec<Crumb> {
    let mut path = base.to_string();
    rest.split('/')
        .filter(|c| !c.is_empty())
        .map(|component| {
            path = join(&path, component);
            Crumb {
                label: component.to_string(),
                path: path.clone(),
                is_home: false,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absolute_path_loses_its_leading_slash() {
        assert_eq!(
            to_archive(Path::new("/home/keith/Documents")),
            "home/keith/Documents"
        );
        assert_eq!(to_archive(Path::new("/")), "");
    }

    #[test]
    fn converting_an_archive_path_twice_does_not_eat_a_component() {
        // A file manager may hand over either form; applying the conversion to
        // its own output has to be harmless.
        let once = to_archive(Path::new("/home/keith"));
        assert_eq!(to_archive(Path::new(&once)), once);
    }

    #[test]
    fn a_trailing_slash_does_not_make_a_different_folder() {
        assert_eq!(to_archive(Path::new("/home/keith/")), "home/keith");
    }

    #[test]
    fn the_round_trip_back_to_the_filesystem_is_absolute() {
        assert_eq!(
            to_filesystem("home/keith/Documents"),
            PathBuf::from("/home/keith/Documents")
        );
    }

    #[test]
    fn walking_up_runs_out_at_the_archive_root() {
        assert_eq!(
            parent("home/keith/Documents"),
            Some("home/keith".to_string())
        );
        assert_eq!(parent("home"), None);
    }

    #[test]
    fn the_home_folder_is_one_crumb_rather_than_two() {
        let crumbs = breadcrumbs("home/keith/Documents/Projects", Some("home/keith"));
        let labels: Vec<_> = crumbs.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Home", "Documents", "Projects"]);
        assert!(crumbs[0].is_home);
        assert_eq!(crumbs[1].path, "home/keith/Documents");
        assert_eq!(crumbs[2].path, "home/keith/Documents/Projects");
    }

    #[test]
    fn home_itself_is_a_trail_of_one() {
        let crumbs = breadcrumbs("home/keith", Some("home/keith"));
        assert_eq!(crumbs.len(), 1);
        assert_eq!(crumbs[0].path, "home/keith");
    }

    #[test]
    fn a_folder_outside_home_is_spelled_out_from_the_root() {
        let crumbs = breadcrumbs("srv/data/archive", Some("home/keith"));
        let labels: Vec<_> = crumbs.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["srv", "data", "archive"]);
        assert_eq!(crumbs[0].path, "srv");
        assert!(!crumbs[0].is_home);
    }

    #[test]
    fn a_folder_whose_name_merely_starts_like_home_is_not_inside_it() {
        assert!(!is_within("home/keithvassallo", "home/keith"));
        let crumbs = breadcrumbs("home/keithvassallo", Some("home/keith"));
        assert_eq!(crumbs[0].label, "home");
    }

    #[test]
    fn the_root_of_the_archive_has_no_trail() {
        assert!(breadcrumbs("", Some("home/keith")).is_empty());
    }
}
