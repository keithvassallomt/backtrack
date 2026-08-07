// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The btrfs half of offline protection: copy-on-write snapshots instead of a
//! spool repository.
//!
//! Where it works, this is the better safety net and it is the one Time Machine
//! uses. A snapshot is near-free to take, costs only what diverges afterwards,
//! and restoring from it is a file copy rather than an extraction.
//!
//! ## Whether it works is a question the machine has to be asked
//!
//! Backtrack's daemon is a **user** service, and that turns out to decide the
//! answer on most systems. Measured on a standard btrfs layout (Arch, and the
//! same shape as Fedora's and openSUSE's defaults) with btrfs-progs 7.1:
//!
//! - `$HOME` is usually **not** a subvolume boundary. `/home/user` is an
//!   ordinary directory inside a subvolume — `@home` here — so the thing that
//!   would have to be snapshotted is that subvolume, and the paths inside it
//!   sit below a prefix that has to be recorded and stripped back off.
//! - That subvolume is **owned by root**, and an unprivileged process cannot
//!   snapshot it: `Operation not permitted`. A user *can* snapshot a subvolume
//!   they own, which is what makes this testable at all, but nobody's home
//!   directory is one by default.
//! - `btrfs subvolume delete` is refused for an unprivileged owner unless the
//!   filesystem was mounted `user_subvol_rm_allowed`, which is not a default.
//!   Removal is possible by clearing the read-only property and unlinking the
//!   tree, which is what [`remove`] does.
//!
//! So on a stock desktop this detects btrfs, tries, fails, says so once, and
//! the spool carries the load. That is why the spool is the path that got the
//! deeper testing — it is the one that will actually run.
//!
//! ## The probe creates *and removes*
//!
//! [`probe`] is not a capability check, it is a rehearsal: it takes a real
//! snapshot and then removes it, and only reports success if both worked. A
//! snapshot that can be created but not removed is worse than no snapshot at
//! all — hourly snapshots that nothing can expire would fill the user's disk
//! and there would be no way out of it from inside the application.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tracing::{debug, warn};

/// `BTRFS_SUPER_MAGIC`, from `linux/magic.h`.
const BTRFS_MAGIC: rustix::fs::FsWord = 0x9123_683e;

/// The inode number every btrfs subvolume root has. This is the documented way
/// to find a subvolume boundary without privileges or an ioctl — and this
/// codebase denies `unsafe`, so an ioctl is not on the table.
const SUBVOLUME_ROOT_INO: u64 = 256;

/// How long hourly local snapshots are kept.
pub const SNAPSHOT_RETENTION: Duration = Duration::from_secs(24 * 3_600);

/// How long a snapshot may take to be created or removed before we give up and
/// treat btrfs as unavailable. A wedged `btrfs` process must not hold a backup.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

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

/// Take a read-only snapshot of `subvolume` at `dest`.
pub async fn create(subvolume: &Path, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    btrfs(&[
        "subvolume".as_ref(),
        "snapshot".as_ref(),
        "-r".as_ref(),
        subvolume.as_os_str(),
        dest.as_os_str(),
    ])
    .await
}

/// Remove a snapshot taken by [`create`].
///
/// Not `btrfs subvolume delete`: that ioctl is refused for an unprivileged
/// owner unless the filesystem was mounted `user_subvol_rm_allowed`, which is
/// not a default on any distribution Backtrack targets. Clearing the read-only
/// property and unlinking the tree achieves the same thing with the permissions
/// a user service actually has — verified against btrfs-progs 7.1, where
/// `subvolume delete` returns `Operation not permitted` and this succeeds.
///
/// It is not free: unlinking is proportional to the number of files, where the
/// ioctl is constant time. Nothing is *copied* — the snapshot shares its extents
/// — so the cost is metadata, paid by an expiry job that nothing waits on.
pub async fn remove(dest: &Path) -> Result<(), String> {
    // Best effort: a snapshot that is already writable is fine to unlink, and a
    // failure here shows up as the removal failing, which is the error worth
    // reporting.
    let _ = btrfs(&[
        "property".as_ref(),
        "set".as_ref(),
        dest.as_os_str(),
        "ro".as_ref(),
        "false".as_ref(),
    ])
    .await;
    let target = dest.to_path_buf();
    tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&target))
        .await
        .map_err(|e| format!("removing {}: {e}", dest.display()))?
        .map_err(|e| format!("removing {}: {e}", dest.display()))
}

/// Whether this machine can actually use snapshots for `subvolume`.
///
/// Takes a real snapshot and removes it again. Both halves have to work: see
/// the module note on why a snapshot that cannot be removed is worse than none.
/// The probe cleans up after itself even when the caller never asks again.
pub async fn probe(subvolume: &Path, snapshots_dir: &Path) -> bool {
    let dest = snapshots_dir.join("bt-probe");
    // A probe left behind by a previous run — a daemon killed mid-probe — must
    // not make every future probe fail on "destination already exists".
    if dest.exists() {
        let _ = remove(&dest).await;
    }
    if let Err(e) = create(subvolume, &dest).await {
        debug!(
            subvolume = %subvolume.display(),
            "filesystem snapshots are not available here: {e}"
        );
        return false;
    }
    if let Err(e) = remove(&dest).await {
        // The dangerous half. Creating worked, so without this check the
        // product would happily take hourly snapshots it could never delete.
        warn!(
            subvolume = %subvolume.display(),
            "filesystem snapshots can be created here but not removed, so they \
             will not be used: {e}"
        );
        // Leave nothing behind that we could not clean up.
        let _ = std::fs::remove_dir_all(&dest);
        return false;
    }
    true
}

/// When a local snapshot should be removed.
///
/// The earlier of two rules, per the stage plan: hourly snapshots are kept for
/// a day, and anything the destination has already caught up with expires on
/// its own clock. In practice the day almost always wins, which is the intent —
/// snapshots are cheap but not free, and yesterday's hourlies stop being
/// interesting once the real repository holds them.
pub fn expires_at(created: SystemTime, expirable_at: Option<SystemTime>) -> SystemTime {
    let by_age = created + SNAPSHOT_RETENTION;
    match expirable_at {
        Some(marked) => by_age.min(marked),
        None => by_age,
    }
}

/// Run `btrfs` with the given arguments.
async fn btrfs(args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let mut cmd = tokio::process::Command::new("btrfs");
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let run = cmd.output();
    let out = match tokio::time::timeout(COMMAND_TIMEOUT, run).await {
        Ok(Ok(out)) => out,
        // btrfs-progs absent is the common case on ext4 machines, and is not
        // worth more than a debug line.
        Ok(Err(e)) => return Err(format!("running btrfs: {e}")),
        Err(_) => return Err("btrfs did not finish in time".to_string()),
    };
    if out.status.success() {
        return Ok(());
    }
    Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn hourly_snapshots_are_kept_for_a_day() {
        assert_eq!(expires_at(at(1_000), None), at(1_000) + SNAPSHOT_RETENTION);
    }

    #[test]
    fn an_expiry_the_destination_set_wins_when_it_comes_first() {
        let created = at(1_000);
        let soon = created + Duration::from_secs(3_600);
        assert_eq!(expires_at(created, Some(soon)), soon);
    }

    #[test]
    fn a_later_expiry_does_not_extend_the_daily_retention() {
        // "whichever first": a 30-day marker must not keep yesterday's hourlies
        // alive for a month.
        let created = at(1_000);
        let far = created + Duration::from_secs(30 * 24 * 3_600);
        assert_eq!(expires_at(created, Some(far)), created + SNAPSHOT_RETENTION);
    }

    #[test]
    fn a_non_btrfs_path_has_no_containing_subvolume() {
        // tmpfs or ext4 under /tmp on most machines; either way, not btrfs.
        let dir = tempfile::tempdir().unwrap();
        if is_btrfs(dir.path()) {
            // The developer's /tmp is on btrfs; this particular assertion has
            // nothing to say, and the btrfs behaviour is covered below.
            return;
        }
        assert_eq!(containing_subvolume(dir.path()), None);
    }

    /// A scratch directory on a btrfs filesystem, or `None`.
    ///
    /// Deliberately under the user's own cache directory rather than `/tmp`:
    /// this needs a real btrfs filesystem, and `/tmp` is usually tmpfs. Skipped
    /// with a warning where btrfs is not available, exactly as the stage plan
    /// asks — building a loopback image would need root, which the daemon this
    /// is testing does not have either.
    #[cfg(feature = "integration")]
    fn btrfs_scratch(name: &str) -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
        if !is_btrfs(&base) {
            eprintln!(
                "skipping: {} is not on btrfs, so snapshot behaviour cannot be exercised",
                base.display()
            );
            return None;
        }
        // Named per test: these run concurrently, and a shared scratch
        // directory would have them deleting each other's subvolumes.
        let dir = base.join(format!(
            "backtrack-btrfs-test-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok()?;
        Some(dir)
    }

    /// Create a subvolume the test user owns, which is the one kind an
    /// unprivileged process can snapshot.
    #[cfg(feature = "integration")]
    async fn make_subvolume(at: &Path) -> bool {
        btrfs(&["subvolume".as_ref(), "create".as_ref(), at.as_os_str()])
            .await
            .is_ok()
    }

    #[cfg(feature = "integration")]
    #[tokio::test]
    async fn a_snapshot_can_be_taken_read_and_removed() {
        // The whole btrfs path against a real filesystem: take a snapshot of a
        // subvolume, read a file out of it (which is what makes restoring from
        // one a plain copy), and remove it again.
        let Some(scratch) = btrfs_scratch("roundtrip") else {
            return;
        };
        let subvolume = scratch.join("sub");
        if !make_subvolume(&subvolume).await {
            eprintln!("skipping: cannot create a subvolume here");
            let _ = std::fs::remove_dir_all(&scratch);
            return;
        }
        std::fs::create_dir_all(subvolume.join("docs")).unwrap();
        std::fs::write(subvolume.join("docs/report.odt"), b"the original").unwrap();

        assert!(
            is_subvolume_root(&subvolume),
            "a freshly created subvolume is a boundary"
        );
        let found = containing_subvolume(&subvolume.join("docs")).expect("inside a subvolume");
        assert_eq!(found.root, subvolume);
        assert_eq!(
            found.prefix,
            PathBuf::from("docs"),
            "the position inside the subvolume is what a snapshot has to be read through"
        );

        let snapshots = scratch.join("snapshots");
        let dest = snapshots.join("bt-local-1");
        create(&subvolume, &dest).await.expect("snapshot taken");

        // A restore from a filesystem snapshot is a file copy, and this is it.
        assert_eq!(
            std::fs::read_to_string(dest.join("docs/report.odt")).unwrap(),
            "the original"
        );

        // The snapshot is a point in time: editing the original does not
        // disturb it, which is the entire reason it is worth taking.
        std::fs::write(subvolume.join("docs/report.odt"), b"edited afterwards").unwrap();
        assert_eq!(
            std::fs::read_to_string(dest.join("docs/report.odt")).unwrap(),
            "the original",
            "the snapshot holds the version it was taken from"
        );

        remove(&dest).await.expect("snapshot removed");
        assert!(!dest.exists(), "and it is really gone");

        let _ = std::fs::remove_dir_all(&subvolume);
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[cfg(feature = "integration")]
    #[tokio::test]
    async fn the_probe_agrees_with_what_actually_happens() {
        // The probe is a rehearsal, so it must not report success where a real
        // snapshot would fail — nor failure where one would work.
        let Some(scratch) = btrfs_scratch("probe") else {
            return;
        };
        let subvolume = scratch.join("sub");
        if !make_subvolume(&subvolume).await {
            let _ = std::fs::remove_dir_all(&scratch);
            return;
        }
        std::fs::write(subvolume.join("f.txt"), b"x").unwrap();
        let snapshots = scratch.join("snapshots");

        assert!(
            probe(&subvolume, &snapshots).await,
            "a subvolume this user owns can be snapshotted and cleaned up"
        );
        assert!(
            !snapshots.join("bt-probe").exists(),
            "and the probe leaves nothing behind"
        );

        // An ordinary directory is not a subvolume and cannot be snapshotted;
        // the probe has to say so rather than leaving a half-made snapshot.
        let plain = scratch.join("not-a-subvolume");
        std::fs::create_dir_all(&plain).unwrap();
        assert!(!probe(&plain, &snapshots).await);
        assert!(!snapshots.join("bt-probe").exists());

        let _ = std::fs::remove_dir_all(&subvolume);
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[cfg(feature = "integration")]
    #[tokio::test]
    async fn a_root_owned_subvolume_is_refused_rather_than_half_used() {
        // The case that decides this on a real desktop: `$HOME` normally sits
        // inside a root-owned subvolume, and a user service cannot snapshot it.
        // The product must discover that and fall back, not fail hourly.
        let home = match std::env::var_os("HOME") {
            Some(h) => PathBuf::from(h),
            None => return,
        };
        if !is_btrfs(&home) {
            eprintln!("skipping: HOME is not on btrfs");
            return;
        }
        let Some(subvolume) = containing_subvolume(&home) else {
            return;
        };
        let Some(scratch) = btrfs_scratch("home") else {
            return;
        };
        let usable = probe(&subvolume.root, &scratch).await;
        // Either answer is legitimate — it depends on who owns the subvolume —
        // but whatever the probe says, it must leave nothing behind.
        assert!(
            !scratch.join("bt-probe").exists(),
            "the probe cleaned up either way (reported usable: {usable})"
        );
        let _ = std::fs::remove_dir_all(&scratch);
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
