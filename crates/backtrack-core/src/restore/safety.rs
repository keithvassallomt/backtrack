// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Keeping a restore inside the folder it was aimed at.
//!
//! An archive is not trustworthy input. Its member paths were whatever was on
//! somebody's disk when it was written, and a repository can be handed to
//! Backtrack by whoever wrote it — the import path in Stage 2 exists precisely
//! so that a repository from elsewhere can be restored from. So a restore must
//! not be able to write outside its destination no matter what the archive
//! says, and "the paths looked fine" is not a defence:
//!
//! - A member path of `../../etc/cron.d/evil` escapes by arithmetic.
//! - A member path of `/etc/cron.d/evil` escapes by being absolute.
//! - A symlink `data -> /etc` followed by a member `data/passwd` escapes
//!   without either — the path is relative, contains no `..`, and still lands
//!   in `/etc`. This is the one that gets missed, and it is why the check walks
//!   the destination rather than only reading the string.

use std::path::{Component, Path, PathBuf};

/// Why a path was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SafetyError {
    #[error("refusing an absolute path from the archive: {0}")]
    Absolute(PathBuf),
    #[error("refusing a path from the archive that climbs out of the destination: {0}")]
    Traversal(PathBuf),
    #[error("refusing to restore through the symbolic link at {0}")]
    SymlinkInPath(PathBuf),
    #[error("refusing an empty path from the archive")]
    Empty,
}

/// The lexical half of the check: is this a plain relative path?
///
/// Returns the path with any `.` components dropped, which is the form the
/// destination walk expects.
pub fn validate_relative(relative: &Path) -> Result<PathBuf, SafetyError> {
    let mut clean = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir => return Err(SafetyError::Traversal(relative.to_path_buf())),
            Component::RootDir | Component::Prefix(_) => {
                return Err(SafetyError::Absolute(relative.to_path_buf()))
            }
        }
    }
    if clean.as_os_str().is_empty() {
        return Err(SafetyError::Empty);
    }
    Ok(clean)
}

/// Where `relative` should be written under `dest`, or why it must not be.
///
/// Every directory between `dest` and the target is checked as it is walked: if
/// one of them is a symbolic link, the restore stops there rather than
/// following it. The final component is not checked — a symlink *being*
/// restored over is an ordinary conflict, and replacing it is a decision the
/// user gets to make.
pub fn safe_join(dest: &Path, relative: &Path) -> Result<PathBuf, SafetyError> {
    let clean = validate_relative(relative)?;
    let mut here = dest.to_path_buf();
    let components: Vec<_> = clean.components().collect();
    for component in &components[..components.len() - 1] {
        here.push(component);
        // `symlink_metadata` does not follow, which is the entire point.
        if let Ok(facts) = std::fs::symlink_metadata(&here) {
            if facts.file_type().is_symlink() {
                return Err(SafetyError::SymlinkInPath(here));
            }
        }
    }
    Ok(dest.join(clean))
}

/// Bytes available to an unprivileged user on the filesystem holding `path`.
///
/// Walks up to the nearest existing ancestor, because the destination of a
/// restore may not exist yet.
pub fn free_space(path: &Path) -> std::io::Result<u64> {
    let mut probe = path;
    loop {
        match rustix::fs::statvfs(probe) {
            Ok(stats) => return Ok(stats.f_bavail.saturating_mul(stats.f_frsize)),
            Err(_) => match probe.parent() {
                Some(parent) => probe = parent,
                None => {
                    return Err(std::io::Error::other(format!(
                        "no filesystem found for {}",
                        path.display()
                    )))
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_relative_path_is_fine() {
        assert_eq!(
            validate_relative(Path::new("Documents/report.odt")).unwrap(),
            PathBuf::from("Documents/report.odt")
        );
        assert_eq!(
            validate_relative(Path::new("./Documents/./report.odt")).unwrap(),
            PathBuf::from("Documents/report.odt")
        );
    }

    #[test]
    fn climbing_out_is_refused() {
        for path in ["../etc/passwd", "a/../../etc/passwd", ".."] {
            assert!(
                matches!(
                    validate_relative(Path::new(path)),
                    Err(SafetyError::Traversal(_))
                ),
                "{path} should have been refused"
            );
        }
    }

    #[test]
    fn an_absolute_path_is_refused() {
        assert!(matches!(
            validate_relative(Path::new("/etc/passwd")),
            Err(SafetyError::Absolute(_))
        ));
    }

    #[test]
    fn an_empty_path_is_refused() {
        assert_eq!(validate_relative(Path::new("")), Err(SafetyError::Empty));
        assert_eq!(validate_relative(Path::new(".")), Err(SafetyError::Empty));
    }

    #[test]
    fn a_symlinked_directory_in_the_destination_is_not_followed() {
        // The attack the lexical check cannot see: the path is relative and has
        // no `..` in it, and it still lands outside the destination.
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest");
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, dest.join("data")).unwrap();

        let refused = safe_join(&dest, Path::new("data/passwd"));
        assert!(
            matches!(refused, Err(SafetyError::SymlinkInPath(_))),
            "got {refused:?}"
        );
        // And nothing was written through it.
        assert!(!elsewhere.join("passwd").exists());
    }

    #[test]
    fn a_real_directory_of_the_same_name_is_allowed_through() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest");
        std::fs::create_dir_all(dest.join("data")).unwrap();
        assert_eq!(
            safe_join(&dest, Path::new("data/passwd")).unwrap(),
            dest.join("data/passwd")
        );
    }

    #[test]
    fn a_symlink_at_the_target_itself_is_a_conflict_not_a_refusal() {
        // Restoring over a symlink is a decision the user is allowed to make;
        // restoring *through* one is not.
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", dest.join("link")).unwrap();
        assert_eq!(
            safe_join(&dest, Path::new("link")).unwrap(),
            dest.join("link")
        );
    }

    #[test]
    fn free_space_answers_for_a_destination_that_does_not_exist_yet() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("not/created/yet");
        assert!(free_space(&missing).unwrap() > 0);
    }
}
