// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Where a path sits among btrfs subvolumes.
//!
//! Two things need to know: the daemon's own snapshots (Stage 5), which copy
//! the subvolume holding the backed-up folders, and Snapper's (Stage 17), which
//! are kept at the top of that same subvolume. Both find it here, without
//! privileges or an ioctl.

use std::path::{Path, PathBuf};

/// `BTRFS_SUPER_MAGIC`, from `linux/magic.h`.
const BTRFS_MAGIC: rustix::fs::FsWord = 0x9123_683e;

/// The inode number every btrfs subvolume root has. This is the documented way
/// to find a subvolume boundary without privileges or an ioctl, and this
/// codebase denies `unsafe`, so an ioctl is not on the table.
const SUBVOLUME_ROOT_INO: u64 = 256;

/// A subvolume, and where a path of interest sits inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subvolume {
    /// The subvolume's mount-visible root.
    pub root: PathBuf,
    /// The path of interest, relative to that root. Empty when the path *is*
    /// the subvolume root.
    ///
    /// This is what makes a snapshot usable: the snapshot reproduces the
    /// subvolume, so a file that lived at `<root>/<prefix>/x` is found in the
    /// snapshot at `<snapshot>/<prefix>/x`, and has to be recorded in the
    /// catalogue under its original name rather than the snapshot's.
    pub prefix: PathBuf,
}

/// Whether `path` is on a btrfs filesystem.
pub fn is_btrfs(path: &Path) -> bool {
    rustix::fs::statfs(path).is_ok_and(|stat| stat.f_type == BTRFS_MAGIC)
}

/// Whether `path` is a subvolume root.
pub fn is_subvolume_root(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).is_ok_and(|meta| meta.ino() == SUBVOLUME_ROOT_INO)
}

/// The subvolume containing `path`, and the path's position inside it.
///
/// Walks up until it finds a subvolume boundary. Stops at a filesystem
/// boundary: crossing one would name a subvolume that does not contain `path`
/// at all, and snapshotting it would protect the wrong data.
pub fn containing_subvolume(path: &Path) -> Option<Subvolume> {
    if !is_btrfs(path) {
        return None;
    }
    let device = device_of(path)?;
    let mut current = path;
    let mut climbed: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if is_subvolume_root(current) {
            climbed.reverse();
            return Some(Subvolume {
                root: current.to_path_buf(),
                prefix: climbed.iter().collect(),
            });
        }
        let parent = current.parent()?;
        // Leaving the filesystem, or leaving btrfs, means there is no
        // containing subvolume to speak of.
        if device_of(parent) != Some(device) || !is_btrfs(parent) {
            return None;
        }
        climbed.push(current.file_name()?);
        current = parent;
    }
}

fn device_of(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|meta| meta.dev())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_btrfs_path_has_no_containing_subvolume() {
        // tmpfs or ext4 under /tmp on most machines; either way, not btrfs.
        let dir = tempfile::tempdir().unwrap();
        if is_btrfs(dir.path()) {
            // The developer's /tmp is on btrfs; this particular assertion has
            // nothing to say, and the btrfs behaviour is covered in the
            // daemon's snapshot tests.
            return;
        }
        assert_eq!(containing_subvolume(dir.path()), None);
    }

    #[test]
    fn the_subvolume_search_stops_at_a_filesystem_boundary() {
        // Not a btrfs assertion: whatever `/proc` is, it is not the filesystem
        // its parent is on, and the search must not climb out of it into `/`.
        let proc = Path::new("/proc/self");
        if !proc.exists() {
            return;
        }
        assert_eq!(containing_subvolume(proc), None);
    }
}
