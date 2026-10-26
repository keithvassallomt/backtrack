// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Snapper's snapshots, kept in the catalogue (S17-T2).
//!
//! `backtrack_core::snapper` says what Snapper has. This keeps the catalogue
//! matching it, looking again when the daemon starts, when a job ends (a
//! backup among them), when the backed-up folders change, and when a
//! `.snapshots` folder does: an inotify watch on each, on each snapshot's own
//! folder, in which Snapper writes `info.xml` and makes the copy after making
//! the folder itself, and on the top of each subvolume, where `.snapshots` is
//! made, removed, or opened to this user.
//!
//! A look is cheap: one `info.xml` a snapshot. Cataloguing is not, since it
//! walks every backed-up file in the snapshot, and it moves the backups after
//! it in the catalogue, so it is a job (`JobKind::Snapper`), queued only when
//! what was found differs from what the catalogue holds.

use std::collections::{BTreeSet, HashSet};
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use backtrack_core::dbus::SnapperInfo;
use backtrack_core::index::{IndexReader, Repo};
use backtrack_core::snapper::{self, Found, Snapper};
use rustix::fs::inotify;
use tokio::sync::{mpsc, Notify};
use tracing::{debug, warn};

use crate::service::Shared;

/// How long a burst of changes is left to settle before looking again. Snapper
/// makes a snapshot's folder, then its `info.xml`, then the copy.
const SETTLE: Duration = Duration::from_secs(1);

/// What the daemon knows of Snapper.
#[derive(Default)]
pub struct SnapperState {
    /// What the last look found, for `GetSnapper`.
    found: Mutex<SnapperInfo>,
    /// Snapshots that could not be catalogued. Left out until the daemon
    /// starts again, so that one that always fails is not tried after every
    /// job.
    pub failed: Arc<Mutex<HashSet<String>>>,
    /// Tripped when something besides a `.snapshots` folder may have changed
    /// what there is to catalogue: a job ended, or the backed-up folders
    /// changed.
    pub rescan: Notify,
}

impl SnapperState {
    /// What the last look found.
    pub fn info(&self) -> SnapperInfo {
        self.found.lock().unwrap().clone()
    }

    /// The names of the snapshots the catalogue should hold.
    fn wanted(&self, found: &[Snapper]) -> BTreeSet<String> {
        let failed = self.failed.lock().unwrap();
        found
            .iter()
            .flat_map(Snapper::snapshots)
            .map(snapper::archive_name)
            .filter(|name| !failed.contains(name))
            .collect()
    }
}

/// Whether the catalogue at `index` holds other than `wanted` of Snapper's
/// snapshots, or holds one it has not read yet.
fn differs(index: &Path, wanted: &BTreeSet<String>) -> bool {
    let Ok(archives) = IndexReader::open(index).and_then(|reader| reader.archives_overview())
    else {
        // No catalogue to read is nothing to put them in either.
        return false;
    };
    let mut held = BTreeSet::new();
    for archive in archives {
        if archive.repo != Repo::Snapper.as_str() {
            continue;
        }
        if !archive.catalogued {
            return true;
        }
        held.insert(archive.name);
    }
    held != *wanted
}

/// Keep the catalogue's Snapper snapshots current, for as long as the daemon
/// runs.
pub async fn watch(shared: Arc<Shared>) {
    let (events, mut changes) = mpsc::unbounded_channel::<()>();
    let inotify = match inotify::init(inotify::CreateFlags::CLOEXEC) {
        Ok(fd) => {
            let fd = Arc::new(fd);
            let reading = Arc::clone(&fd);
            let started = std::thread::Builder::new()
                .name("backtrack-snapper".to_string())
                .spawn(move || read_events(&reading, events));
            match started {
                Ok(_) => Some(fd),
                Err(error) => {
                    warn!(%error, "Snapper's snapshots will be looked at again only when a job ends");
                    None
                }
            }
        }
        Err(error) => {
            warn!(%error, "Snapper's snapshots will be looked at again only when a job ends");
            None
        }
    };

    let mut listening = inotify.is_some();
    loop {
        let sources = shared.snapper_sources();
        let index = shared.index_path();
        let looked = tokio::task::spawn_blocking(move || {
            let found = snapper::find(&sources);
            let tops = subvolumes(&sources);
            (found, tops)
        })
        .await;
        match looked {
            Ok((found, tops)) => {
                if let Some(fd) = &inotify {
                    watch_snapper(fd, &tops, &found);
                }
                let wanted = shared.snapper.wanted(&found);
                *shared.snapper.found.lock().unwrap() = summary(&found, &tops);
                let checking = wanted.clone();
                let stale = tokio::task::spawn_blocking(move || differs(&index, &checking))
                    .await
                    .unwrap_or(false);
                if stale {
                    debug!(
                        snapshots = wanted.len(),
                        "Snapper's snapshots differ from the catalogue's"
                    );
                    shared.catalogue_snapper();
                }
            }
            Err(error) => {
                warn!(%error, "reading Snapper's snapshots failed; trying again at the next change")
            }
        }

        tokio::select! {
            change = changes.recv(), if listening => match change {
                Some(()) => {
                    tokio::time::sleep(SETTLE).await;
                    while changes.try_recv().is_ok() {}
                }
                None => listening = false,
            },
            _ = shared.snapper.rescan.notified() => {}
        }
    }
}

/// What a look found, as `GetSnapper` gives it: `found` from the subvolumes
/// at `tops`.
fn summary(found: &[Snapper], tops: &[PathBuf]) -> SnapperInfo {
    let mut info = SnapperInfo {
        on_btrfs: !tops.is_empty(),
        ..SnapperInfo::default()
    };
    for at in found {
        match &at.found {
            Found::Snapshots(snapshots) => {
                info.folders += 1;
                info.snapshots += snapshots.len() as u32;
            }
            Found::Unreadable => info.unreadable.push(at.folder.display().to_string()),
        }
    }
    info
}

/// The top of each subvolume holding one of `sources`, once each.
fn subvolumes(sources: &[PathBuf]) -> Vec<PathBuf> {
    let mut tops: Vec<PathBuf> = sources
        .iter()
        .filter_map(|s| backtrack_core::btrfs::containing_subvolume(s))
        .map(|s| s.root)
        .collect();
    tops.sort();
    tops.dedup();
    tops
}

/// Watch what can change what Snapper has: the top of each subvolume, for its
/// `.snapshots` folder coming, going or being opened to this user; each
/// `.snapshots` folder that can be read, for snapshots coming and going; and
/// each snapshot's folder, for the `info.xml` and the copy Snapper puts in it
/// once it has made it. A watch on a folder already watched is the same one,
/// so this is asked again at every look.
fn watch_snapper(fd: &OwnedFd, tops: &[PathBuf], found: &[Snapper]) {
    use inotify::WatchFlags as W;
    let add = |path: &Path, flags: W| {
        if let Err(error) = inotify::add_watch(fd, path, flags) {
            debug!(%error, folder = %path.display(), "a folder of Snapper's could not be watched");
        }
    };
    let comings_and_goings = W::CREATE | W::DELETE | W::MOVED_FROM | W::MOVED_TO;
    for top in tops {
        add(top, comings_and_goings | W::ATTRIB);
    }
    for at in found {
        if matches!(at.found, Found::Unreadable) {
            continue;
        }
        add(
            &at.folder,
            comings_and_goings | W::DELETE_SELF | W::MOVE_SELF,
        );
        // Every folder in it, not only the snapshots found: one still being
        // made is not among them yet, and its folder is where it finishes.
        if let Ok(listing) = std::fs::read_dir(&at.folder) {
            for entry in listing.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    add(&entry.path(), comings_and_goings | W::CLOSE_WRITE);
                }
            }
        }
    }
}

/// Pass on that something changed, for as long as anyone is listening.
fn read_events(fd: &OwnedFd, changed: mpsc::UnboundedSender<()>) {
    let mut buf = [MaybeUninit::<u8>::uninit(); 4096];
    let mut reader = inotify::Reader::new(fd, &mut buf);
    loop {
        match reader.next() {
            Ok(_) => {
                if changed.send(()).is_err() {
                    return;
                }
            }
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => {
                warn!(%error, "Snapper's snapshots can no longer be watched");
                return;
            }
        }
    }
}

pub(crate) mod tests;

#[cfg(test)]
mod summary {
    use super::*;
    use backtrack_core::snapper::{Snapshot, Type};

    fn at(folder: &str, found: Found) -> Snapper {
        Snapper {
            subvolume: PathBuf::from(folder).parent().unwrap().to_path_buf(),
            folder: PathBuf::from(folder),
            found,
        }
    }

    fn snapshots(n: u64) -> Found {
        Found::Snapshots(
            (1..=n)
                .map(|number| Snapshot {
                    number,
                    kind: Type::Single,
                    date: 0,
                    description: String::new(),
                    path: PathBuf::from(format!("/x/{number}/snapshot")),
                })
                .collect(),
        )
    }

    #[test]
    fn a_look_is_summed_up_for_preferences() {
        assert_eq!(summary(&[], &[]), SnapperInfo::default(), "not on btrfs");
        assert_eq!(
            summary(&[], &[PathBuf::from("/home")]),
            SnapperInfo {
                on_btrfs: true,
                ..SnapperInfo::default()
            },
            "on btrfs, with no Snapper"
        );
        assert_eq!(
            summary(
                &[
                    at("/home/.snapshots", snapshots(3)),
                    at("/data/.snapshots", Found::Unreadable),
                    at("/srv/.snapshots", snapshots(0)),
                ],
                &[
                    PathBuf::from("/home"),
                    PathBuf::from("/data"),
                    PathBuf::from("/srv")
                ],
            ),
            SnapperInfo {
                on_btrfs: true,
                folders: 2,
                snapshots: 3,
                unreadable: vec!["/data/.snapshots".to_string()],
            }
        );
    }
}
