// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! A backup that is a folder on this computer: a btrfs snapshot, Snapper's or
//! the daemon's own (S17-T3), read back as an engine so that preview, compare
//! and restore take it as they take an archive.
//!
//! Its "extract" is a copy out of the snapshot. What it copies is what the
//! catalogue holds of it: the snapshot copies a whole subvolume, and a backup
//! leaves some of that out (caches, the exclusions the person chose,
//! Backtrack's own folders, other filesystems below a folder), so the copy is
//! walked with the same rules the catalogue walk used, and anything the walk
//! would not list is not brought back.
//!
//! The rest of the engine is a repository's, and a snapshot is not one: those
//! calls fail.

use std::path::{Component, Path, PathBuf};
use std::pin::Pin;

use async_trait::async_trait;
use tokio::io::AsyncRead;

use super::{
    ArchiveId, BackupEngine, CheckLevel, CreateSpec, EngineError, JobEvent, JobStream, PrunePolicy,
    RepoInfo, RepoSpec, RepoStats, Result,
};
use crate::index::{ArchiveMeta, BorgItem, Kind};
use crate::pattern::ExcludeSet;
use crate::walk::{self, WalkSpec};

/// A snapshot of a subvolume, read as a backup of the folders in it.
#[derive(Debug, Clone)]
pub struct SnapshotFolder {
    /// The copy: Snapper's `<N>/snapshot`, or one of the daemon's own.
    copy: PathBuf,
    /// The subvolume it copies, which the catalogue's paths are under.
    subvolume: PathBuf,
    excludes: ExcludeSet,
    never: Vec<PathBuf>,
}

impl SnapshotFolder {
    /// `copy`, a snapshot of `subvolume`, leaving out what `excludes` and
    /// `never` leave out of a backup.
    pub fn new(
        copy: PathBuf,
        subvolume: PathBuf,
        excludes: ExcludeSet,
        never: Vec<PathBuf>,
    ) -> SnapshotFolder {
        SnapshotFolder {
            copy,
            subvolume,
            excludes,
            never,
        }
    }

    /// Where the catalogue's `path` (archive-relative) is in the copy: None for
    /// a path outside the subvolume, or one that climbs with `..`.
    pub fn locate(&self, path: &str) -> Option<PathBuf> {
        let real = Path::new("/").join(path.trim_start_matches('/'));
        if real
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
        {
            return None;
        }
        let rest = real.strip_prefix(&self.subvolume).ok()?;
        Some(if rest.as_os_str().is_empty() {
            self.copy.clone()
        } else {
            self.copy.join(rest)
        })
    }

    /// The file at `path`, opened for reading. Only a regular file is opened,
    /// and a link is not followed: a link in a snapshot can point anywhere,
    /// and what it points at now is not what the snapshot holds.
    pub fn open(&self, path: &str) -> std::io::Result<std::fs::File> {
        use std::os::unix::fs::OpenOptionsExt;
        let at = self.locate(path).ok_or_else(|| not_here(path))?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(&at)?;
        if !file.metadata()?.is_file() {
            return Err(std::io::Error::other(format!("{path} is not a file")));
        }
        Ok(file)
    }

    /// What the catalogue walk lists of `path` in the copy, at the paths it
    /// really lives at: the thing itself, and for a folder everything below it.
    fn listing(&self, path: &str) -> std::io::Result<Vec<BorgItem>> {
        let at = self.locate(path).ok_or_else(|| not_here(path))?;
        std::fs::symlink_metadata(&at)?;
        let spec = WalkSpec {
            sources: vec![at],
            excludes: self.excludes.clone(),
            one_file_system: true,
            never: self.never.clone(),
            include_dirs: true,
        };
        Ok(walk::walk_as(&spec, &self.copy, &self.subvolume).items)
    }

    /// Copy what `paths` name into `dest`, each at its whole archive path, as
    /// `borg extract` lays it out: files with their contents, modes and
    /// modification times, links as links, folders with their modes and times
    /// once everything in them is there. Blocking. `copied` hears the bytes
    /// so far.
    pub fn copy_into(
        &self,
        paths: &[String],
        dest: &Path,
        mut copied: impl FnMut(u64, u64),
    ) -> std::io::Result<()> {
        let mut items = Vec::new();
        for path in paths {
            items.extend(self.listing(path)?);
        }
        let total: u64 = items
            .iter()
            .filter(|i| i.kind == Kind::File)
            .map(|i| i.size.max(0) as u64)
            .sum();
        let mut done = 0u64;
        let mut folders = Vec::new();
        for item in &items {
            let from = self
                .locate(&item.path)
                .ok_or_else(|| not_here(&item.path))?;
            let to = dest.join(&item.path);
            let meta = std::fs::symlink_metadata(&from)?;
            match item.kind {
                Kind::Dir => {
                    std::fs::create_dir_all(&to)?;
                    folders.push((to, meta));
                }
                Kind::File => {
                    if let Some(parent) = to.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::copy(&from, &to)?;
                    set_times(&to, &meta)?;
                    done += meta.len();
                    copied(done, total);
                }
                Kind::Symlink => {
                    if let Some(parent) = to.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::os::unix::fs::symlink(std::fs::read_link(&from)?, &to)?;
                    set_times(&to, &meta)?;
                }
                // A device or a pipe, which a backup does not bring back either.
                Kind::Other => {}
            }
        }
        // Innermost first, so that a folder's time is set after everything in
        // it is written, and a read-only folder is made so last.
        for (folder, meta) in folders.iter().rev() {
            std::fs::set_permissions(folder, meta.permissions())?;
            set_times(folder, meta)?;
        }
        Ok(())
    }
}

fn not_here(path: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("{path} is not in this snapshot"),
    )
}

/// Give `path` the access and modification times in `meta`, to the
/// nanosecond, without following a link.
fn set_times(path: &Path, meta: &std::fs::Metadata) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let times = rustix::fs::Timestamps {
        last_access: rustix::fs::Timespec {
            tv_sec: meta.atime(),
            tv_nsec: meta.atime_nsec() as _,
        },
        last_modification: rustix::fs::Timespec {
            tv_sec: meta.mtime(),
            tv_nsec: meta.mtime_nsec() as _,
        },
    };
    rustix::fs::utimensat(
        rustix::fs::CWD,
        path,
        &times,
        rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(std::io::Error::from)
}

fn not_a_repository() -> EngineError {
    EngineError::Local("a snapshot on this computer is read from, not written to".into())
}

#[async_trait]
impl BackupEngine for SnapshotFolder {
    async fn init_repo(&self, _spec: &RepoSpec) -> Result<()> {
        Err(not_a_repository())
    }

    async fn repo_info(&self) -> Result<RepoInfo> {
        Err(not_a_repository())
    }

    async fn key_export(&self) -> Result<String> {
        Err(not_a_repository())
    }

    async fn change_passphrase(&self, _old: &str, _new: &str) -> Result<()> {
        Err(not_a_repository())
    }

    async fn repo_stats(&self) -> Result<RepoStats> {
        Err(not_a_repository())
    }

    async fn create(&self, _spec: &CreateSpec) -> Result<JobStream> {
        Err(not_a_repository())
    }

    async fn list_archives(&self) -> Result<Vec<ArchiveMeta>> {
        Err(not_a_repository())
    }

    async fn list_archive(
        &self,
        _id: &ArchiveId,
    ) -> Result<futures::stream::BoxStream<'static, Result<BorgItem>>> {
        Err(not_a_repository())
    }

    async fn extract(&self, _id: &ArchiveId, paths: &[String], dest: &Path) -> Result<JobStream> {
        let (sink, stream) = JobStream::channel(16);
        let folder = self.clone();
        let paths = paths.to_vec();
        let dest = dest.to_path_buf();
        tokio::spawn(async move {
            let (progress, mut heard) = tokio::sync::mpsc::unbounded_channel();
            let copying = tokio::task::spawn_blocking(move || {
                folder.copy_into(&paths, &dest, |current, total| {
                    let _ = progress.send((current, total));
                })
            });
            while let Some((current, total)) = heard.recv().await {
                let event = JobEvent::Progress {
                    current,
                    total: Some(total),
                    phase: String::new(),
                };
                if !sink.send(event).await {
                    break;
                }
            }
            let outcome = match copying.await {
                Ok(Ok(())) => Ok(Default::default()),
                Ok(Err(e)) => Err(EngineError::Local(e.to_string())),
                Err(e) => Err(EngineError::Local(e.to_string())),
            };
            sink.send(JobEvent::Finished(outcome)).await;
        });
        Ok(stream)
    }

    async fn extract_patterns(
        &self,
        _id: &ArchiveId,
        _patterns: &Path,
        _dest: &Path,
    ) -> Result<JobStream> {
        Err(EngineError::Local(
            "a snapshot on this computer is restored from by path".into(),
        ))
    }

    async fn extract_stdout(
        &self,
        _id: &ArchiveId,
        path: &str,
    ) -> Result<Pin<Box<dyn AsyncRead + Send>>> {
        let file = self
            .open(path)
            .map_err(|e| EngineError::Local(e.to_string()))?;
        Ok(Box::pin(tokio::fs::File::from_std(file)))
    }

    async fn prune(&self, _policy: &PrunePolicy) -> Result<JobStream> {
        Err(not_a_repository())
    }

    async fn delete_archives(&self, _ids: &[ArchiveId]) -> Result<JobStream> {
        Err(not_a_repository())
    }

    async fn compact(&self) -> Result<JobStream> {
        Err(not_a_repository())
    }

    async fn check(&self, _level: CheckLevel) -> Result<JobStream> {
        Err(not_a_repository())
    }

    async fn repair(&self) -> Result<JobStream> {
        Err(not_a_repository())
    }

    async fn key_import(&self, _key: &str) -> Result<()> {
        Err(not_a_repository())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    /// A snapshot in a temporary folder: its copy holds `k/Documents` of a
    /// subvolume at `/home`. Not a btrfs snapshot; a folder reads the same.
    struct Fixture {
        _dir: tempfile::TempDir,
        copy: PathBuf,
        folder: SnapshotFolder,
    }

    fn fixture(excludes: &[&str]) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("snapshot");
        let docs = copy.join("k/Documents");
        std::fs::create_dir_all(docs.join("drafts")).unwrap();
        std::fs::create_dir_all(docs.join(".cache")).unwrap();
        std::fs::write(docs.join("report.odt"), "the report").unwrap();
        std::fs::write(docs.join("drafts/one.txt"), "one").unwrap();
        std::fs::write(docs.join(".cache/blob"), "cached").unwrap();
        std::os::unix::fs::symlink("report.odt", docs.join("latest")).unwrap();
        std::fs::set_permissions(docs.join("drafts"), std::fs::Permissions::from_mode(0o750))
            .unwrap();
        let excludes: Vec<String> = excludes.iter().map(|s| s.to_string()).collect();
        let folder = SnapshotFolder::new(
            copy.clone(),
            PathBuf::from("/home"),
            ExcludeSet::compile(&excludes),
            Vec::new(),
        );
        Fixture {
            _dir: dir,
            copy,
            folder,
        }
    }

    #[test]
    fn a_path_is_found_in_the_copy_of_its_subvolume() {
        let f = fixture(&[]);
        assert_eq!(
            f.folder.locate("home/k/Documents/report.odt"),
            Some(f.copy.join("k/Documents/report.odt"))
        );
        assert_eq!(f.folder.locate("home"), Some(f.copy.clone()));
        assert_eq!(f.folder.locate("srv/data/x"), None, "another subvolume");
        assert_eq!(f.folder.locate("home/k/../../etc/passwd"), None);
    }

    #[test]
    fn a_file_is_opened_but_a_link_or_a_folder_is_not() {
        use std::io::Read as _;
        let f = fixture(&[]);
        let mut read = String::new();
        f.folder
            .open("home/k/Documents/report.odt")
            .unwrap()
            .read_to_string(&mut read)
            .unwrap();
        assert_eq!(read, "the report");
        assert!(f.folder.open("home/k/Documents/latest").is_err());
        assert!(f.folder.open("home/k/Documents").is_err());
        assert_eq!(
            f.folder
                .open("home/k/Documents/gone.txt")
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotFound
        );
    }

    #[test]
    fn a_folder_is_copied_as_borg_would_extract_it() {
        let f = fixture(&["**/.cache"]);
        let dest = tempfile::tempdir().unwrap();
        let mut heard = Vec::new();
        f.folder
            .copy_into(
                &["home/k/Documents".to_string()],
                dest.path(),
                |done, total| heard.push((done, total)),
            )
            .unwrap();
        let docs = dest.path().join("home/k/Documents");
        assert_eq!(
            std::fs::read_to_string(docs.join("report.odt")).unwrap(),
            "the report"
        );
        assert_eq!(
            std::fs::read_to_string(docs.join("drafts/one.txt")).unwrap(),
            "one"
        );
        assert_eq!(
            std::fs::read_link(docs.join("latest")).unwrap(),
            PathBuf::from("report.odt"),
            "a link is copied as the link"
        );
        assert!(
            !docs.join(".cache").exists(),
            "what a backup leaves out is not brought back"
        );
        for (from, to) in [
            (
                f.copy.join("k/Documents/report.odt"),
                docs.join("report.odt"),
            ),
            (f.copy.join("k/Documents/drafts"), docs.join("drafts")),
            (f.copy.join("k/Documents"), docs.clone()),
        ] {
            let (a, b) = (
                std::fs::symlink_metadata(&from).unwrap(),
                std::fs::symlink_metadata(&to).unwrap(),
            );
            assert_eq!(
                (a.mtime(), a.mtime_nsec(), a.mode()),
                (b.mtime(), b.mtime_nsec(), b.mode()),
                "{} keeps its time and mode",
                to.display()
            );
        }
        assert_eq!(heard.last(), Some(&(13, 13)), "every byte reported");
    }

    #[test]
    fn a_single_file_is_copied_at_its_whole_path() {
        let f = fixture(&[]);
        let dest = tempfile::tempdir().unwrap();
        f.folder
            .copy_into(
                &["home/k/Documents/drafts/one.txt".to_string()],
                dest.path(),
                |_, _| {},
            )
            .unwrap();
        let found: Vec<PathBuf> = walkdir(dest.path());
        assert_eq!(
            found,
            [
                PathBuf::from("home"),
                PathBuf::from("home/k"),
                PathBuf::from("home/k/Documents"),
                PathBuf::from("home/k/Documents/drafts"),
                PathBuf::from("home/k/Documents/drafts/one.txt"),
            ]
        );
    }

    #[test]
    fn something_the_snapshot_does_not_hold_is_an_error() {
        let f = fixture(&[]);
        let dest = tempfile::tempdir().unwrap();
        let err = f
            .folder
            .copy_into(
                &["home/k/Documents/gone.txt".to_string()],
                dest.path(),
                |_, _| {},
            )
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        assert!(f
            .folder
            .copy_into(&["etc/passwd".to_string()], dest.path(), |_, _| {})
            .is_err());
    }

    /// Everything below `root`, relative, sorted.
    fn walkdir(root: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path.clone());
                }
                found.push(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
        found.sort();
        found
    }
}
