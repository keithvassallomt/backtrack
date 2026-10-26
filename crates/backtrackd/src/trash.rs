// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The desktop's Trash, kept current for the window (S16-T2).
//!
//! What a Trash holds, and whether each item holds something no backup has,
//! is `backtrack_core::trash`'s to say. This keeps a listing of the items from
//! the backed-up folders, and keeps it current: an inotify watch on each
//! Trash's `info` and `files` folders, a handful of folders however full the
//! Trash is, and a fresh look whenever a job ends, since a backup or a prune
//! can change what counts.
//!
//! The watch is read on a thread of its own, blocked in `read`, and handed to
//! the async side as a bare "something changed". The listing is then read
//! again in full: a Trash is small, and reading it again is simpler and more
//! honest than replaying events, which arrive out of order around a move.

use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use backtrack_core::dbus::TrashedItem;
use backtrack_core::index::IndexReader;
use backtrack_core::pattern::ExcludeSet;
use backtrack_core::trash::{self, Item, Trash};
use rustix::fs::inotify;
use tokio::sync::{mpsc, Notify};
use tracing::{debug, warn};

use crate::service::Shared;

/// How long a burst of changes is left to settle before the Trash is read
/// again. Trashing writes the `.trashinfo` first and moves the item after it.
const SETTLE: Duration = Duration::from_millis(250);

/// A trashed item, and whether it holds something no backup has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub item: Item,
    pub counts: bool,
}

impl Entry {
    /// As `ListTrashed` reports it.
    pub fn published(&self) -> TrashedItem {
        TrashedItem {
            archive: trash::archive_name(&self.item),
            path: backtrack_core::walk::archive_path(&self.item.original),
            deleted_at: self.item.deleted_at,
            kind: self.item.kind.as_str().to_string(),
            size: self.item.size,
            mtime: self.item.mtime,
            counts: self.counts,
        }
    }
}

/// The listing, and the two things that move it.
#[derive(Default)]
pub struct TrashState {
    entries: Mutex<Vec<Entry>>,
    /// Tripped when the listing changes, which is what `TrashChanged` says.
    pub changed: Notify,
    /// Tripped when something besides the Trash changes what counts: a job
    /// ended, or the backed-up folders changed.
    pub rescan: Notify,
}

impl TrashState {
    pub fn entries(&self) -> Vec<Entry> {
        self.entries.lock().unwrap().clone()
    }

    /// The entry an archive name (`trash:<info>`) names, if it is still in the
    /// listing. Only what is listed is ever read, so a client cannot name an
    /// arbitrary file and have the daemon open it.
    pub fn find(&self, archive: &str) -> Option<Entry> {
        let info = trash::info_of(archive)?;
        self.entries
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.item.info == info)
            .cloned()
    }

    /// Replace the listing, saying whether that changed it.
    fn replace(&self, entries: Vec<Entry>) -> bool {
        let mut current = self.entries.lock().unwrap();
        if *current == entries {
            return false;
        }
        *current = entries;
        true
    }
}

/// What a look at the Trash needs, taken from the daemon's state in one go.
pub struct Scope {
    pub data_home: PathBuf,
    pub sources: Vec<PathBuf>,
    pub excludes: ExcludeSet,
    pub index: PathBuf,
}

/// Read every Trash the backed-up folders can send things to, keep the items
/// from those folders, and measure each against the catalogue. Blocking.
pub fn scan(scope: &Scope) -> (Vec<Trash>, Vec<Entry>) {
    let trashes = trash::trashes_for(&scope.data_home, &scope.sources);
    let items: Vec<Item> = trashes.iter().flat_map(Trash::local_items).collect();
    let items = trash::in_backups(items, &scope.sources, &scope.excludes);
    // No catalogue yet means no backup yet, so everything counts.
    let reader = IndexReader::open(&scope.index).ok();
    let mut entries: Vec<Entry> = items
        .into_iter()
        .map(|item| {
            let counts = match &reader {
                None => true,
                Some(reader) => trash::counts(reader, &item, &scope.excludes).unwrap_or_else(|error| {
                    warn!(%error, path = %item.original.display(), "a trashed item could not be measured against the catalogue");
                    false
                }),
            };
            Entry { item, counts }
        })
        .collect();
    entries
        .sort_by(|a, b| (a.item.deleted_at, &a.item.info).cmp(&(b.item.deleted_at, &b.item.info)));
    (trashes, entries)
}

/// Keep `shared`'s listing of the Trash current, for as long as the daemon
/// runs.
pub async fn watch(shared: Arc<Shared>) {
    let (events, mut changes) = mpsc::unbounded_channel::<()>();
    let inotify = match inotify::init(inotify::CreateFlags::CLOEXEC) {
        Ok(fd) => {
            let fd = Arc::new(fd);
            let reading = Arc::clone(&fd);
            let started = std::thread::Builder::new()
                .name("backtrack-trash".to_string())
                .spawn(move || read_events(&reading, events));
            match started {
                Ok(_) => Some(fd),
                Err(error) => {
                    warn!(%error, "the Trash will be looked at again only when a job ends");
                    None
                }
            }
        }
        Err(error) => {
            warn!(%error, "the Trash will be looked at again only when a job ends");
            None
        }
    };

    // Whether the reading thread is still there to hear from.
    let mut listening = inotify.is_some();
    loop {
        let scope = shared.trash_scope();
        match tokio::task::spawn_blocking(move || scan(&scope)).await {
            Ok((trashes, entries)) => {
                if let Some(fd) = &inotify {
                    for trash in &trashes {
                        watch_trash(fd, trash);
                    }
                }
                let count = entries.len();
                if shared.trash.replace(entries) {
                    debug!(items = count, "the Trash changed");
                    shared.trash.changed.notify_one();
                }
            }
            // Waited out like any other look, rather than tried again at once.
            Err(error) => {
                warn!(%error, "reading the Trash failed; trying again at the next change")
            }
        }

        tokio::select! {
            change = changes.recv(), if listening => match change {
                Some(()) => {
                    tokio::time::sleep(SETTLE).await;
                    while changes.try_recv().is_ok() {}
                }
                // The reading thread ended. The rescans after each job go on.
                None => listening = false,
            },
            _ = shared.trash.rescan.notified() => {}
        }
    }
}

/// Watch a Trash's `info` and `files` folders. A Trash not made yet is watched
/// from the nearest folder above it that is, so the first thing put in it is
/// noticed. Asked again at every look: a watch on a folder that is already
/// watched is the same watch, and a folder deleted and made again needs a new
/// one.
fn watch_trash(fd: &OwnedFd, trash: &Trash) {
    let flags = inotify::WatchFlags::CREATE
        | inotify::WatchFlags::DELETE
        | inotify::WatchFlags::MOVED_FROM
        | inotify::WatchFlags::MOVED_TO
        | inotify::WatchFlags::CLOSE_WRITE
        | inotify::WatchFlags::DELETE_SELF
        | inotify::WatchFlags::MOVE_SELF;
    for folder in [trash.root.join("info"), trash.root.join("files")] {
        let Some(target) = nearest_existing(&folder) else {
            continue;
        };
        if let Err(error) = inotify::add_watch(fd, &target, flags) {
            debug!(%error, folder = %target.display(), "a Trash folder could not be watched");
        }
    }
}

fn nearest_existing(path: &Path) -> Option<PathBuf> {
    path.ancestors().find(|p| p.is_dir()).map(Path::to_path_buf)
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
                warn!(%error, "the Trash can no longer be watched");
                return;
            }
        }
    }
}
