// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Whether a catalogued path is still on this computer.
//!
//! Every status the timeline shows is otherwise measured inside the catalogue:
//! "deleted after this" means absent from the newest catalogued archive, and
//! "changed since then" means a newer version opens after the one being looked
//! at. Both are false by construction at the newest backup, which leaves the
//! one question a person actually arrives with — *is the thing I lost still
//! gone?* — unanswered at the only point in time where it can be asked.
//!
//! The daemon answers it rather than the application, because the application
//! cannot be assumed to reach the user's files: it reads the catalogue, which
//! lives in the data directory, and receives archived file contents as
//! descriptors precisely so a sandboxed build never needs a path of its own.
//! The daemon is not sandboxed and always can.
//!
//! The answer is three-valued. A path whose folder cannot be read — no
//! permission, or an archive taken on another machine whose directories do not
//! exist here — is **unknown**, and an unknown says nothing at all. Claiming a
//! file was deleted when the truth is that nobody looked is worse than leaving
//! the column blank, because a person acts on it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Whether a path is on this computer now.
///
/// The numbers are the wire encoding, so they are part of the interface and do
/// not get reordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OnDisk {
    /// Nobody could look: the containing folder could not be read.
    Unknown = 0,
    /// The folder was read and the name was not in it.
    Absent = 1,
    /// The folder was read and the name was in it.
    Present = 2,
}

impl OnDisk {
    pub fn as_byte(self) -> u8 {
        self as u8
    }
}

/// Resolve each archive-relative path to whether it is on this computer.
///
/// Answers come back in the order they were asked for. Paths are grouped by
/// their containing folder and each folder is read once, because a listing
/// answers for every name in it at the cost of one directory read — a folder
/// pane full of files is one syscall's worth of work rather than one per row.
pub fn resolve(paths: &[String]) -> Vec<OnDisk> {
    let mut folders: HashMap<PathBuf, Option<std::collections::HashSet<std::ffi::OsString>>> =
        HashMap::new();

    paths
        .iter()
        .map(|path| {
            let full = to_local_path(path);
            let (Some(parent), Some(name)) = (full.parent(), full.file_name()) else {
                // A path with no parent is the filesystem root, which is not
                // something the catalogue lists and not something to guess at.
                return OnDisk::Unknown;
            };
            let listing = folders
                .entry(parent.to_path_buf())
                .or_insert_with(|| read_names(parent));
            match listing {
                None => OnDisk::Unknown,
                Some(names) if names.contains(name) => OnDisk::Present,
                Some(_) => OnDisk::Absent,
            }
        })
        .collect()
}

/// The names directly inside `folder`, or `None` if it could not be read.
///
/// A folder that does not exist reads as `None` rather than as an empty
/// listing. It usually means the archive came from somewhere this machine's
/// filesystem does not mirror, and answering "everything in it was deleted" to
/// that is a confident way to be wrong.
fn read_names(folder: &Path) -> Option<std::collections::HashSet<std::ffi::OsString>> {
    let entries = std::fs::read_dir(folder).ok()?;
    Some(entries.flatten().map(|entry| entry.file_name()).collect())
}

/// An archive-relative member path as a path on this computer.
///
/// Borg stores a member by its absolute path with the leading `/` removed, so
/// putting it back is the whole conversion.
fn to_local_path(member: &str) -> PathBuf {
    PathBuf::from("/").join(member.trim_start_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three answers, against a real directory.
    #[test]
    fn a_folder_that_can_be_read_answers_for_every_name_in_it() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("pictures");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("here.png"), b"x").unwrap();

        let member = |name: &str| {
            dir.join(name)
                .to_string_lossy()
                .trim_start_matches('/')
                .to_string()
        };
        let asked = vec![
            member("here.png"),
            member("gone.png"),
            tmp.path()
                .join("no-such-folder/anything.png")
                .to_string_lossy()
                .trim_start_matches('/')
                .to_string(),
        ];

        assert_eq!(
            resolve(&asked),
            vec![OnDisk::Present, OnDisk::Absent, OnDisk::Unknown],
        );
    }

    /// The answers line up with the questions, which is the whole contract of
    /// returning a bare list.
    #[test]
    fn the_answers_come_back_in_the_order_they_were_asked() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        for name in ["a", "c"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let member = |name: &str| {
            dir.join(name)
                .to_string_lossy()
                .trim_start_matches('/')
                .to_string()
        };

        let asked: Vec<String> = ["a", "b", "c", "d"].iter().map(|n| member(n)).collect();
        assert_eq!(
            resolve(&asked),
            vec![
                OnDisk::Present,
                OnDisk::Absent,
                OnDisk::Present,
                OnDisk::Absent
            ],
        );
    }

    /// A folder is read once however many of its files are asked about.
    #[test]
    fn asking_about_a_whole_folder_reads_it_once() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("kept"), b"x").unwrap();
        let member = |name: &str| {
            dir.join(name)
                .to_string_lossy()
                .trim_start_matches('/')
                .to_string()
        };

        let asked: Vec<String> = (0..50)
            .map(|i| member(&format!("file-{i}")))
            .chain(std::iter::once(member("kept")))
            .collect();
        let answers = resolve(&asked);

        assert_eq!(answers.len(), 51);
        assert!(answers[..50].iter().all(|a| *a == OnDisk::Absent));
        assert_eq!(answers[50], OnDisk::Present);
    }

    /// Borg strips the leading slash; putting it back is the whole mapping.
    #[test]
    fn a_member_path_is_an_absolute_path_with_its_slash_taken_off() {
        assert_eq!(
            to_local_path("home/keith/Documents/report.odt"),
            PathBuf::from("/home/keith/Documents/report.odt"),
        );
        // Tolerant of a leading slash that should not be there, because an
        // imported repository's spelling is not this code's to trust.
        assert_eq!(
            to_local_path("/home/keith/report.odt"),
            PathBuf::from("/home/keith/report.odt"),
        );
    }

    /// The encoding is the interface, so it is pinned by a test rather than by
    /// the order the variants happen to be written in.
    #[test]
    fn the_wire_encoding_is_fixed() {
        assert_eq!(OnDisk::Unknown.as_byte(), 0);
        assert_eq!(OnDisk::Absent.as_byte(), 1);
        assert_eq!(OnDisk::Present.as_byte(), 2);
    }
}
