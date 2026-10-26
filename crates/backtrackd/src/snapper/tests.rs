// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Snapper's snapshots catalogued, against a real btrfs filesystem.
//!
//! Snapper needs root, and so does a loopback image, so these stand in for it
//! as Stage 5's tests do: subvolumes this user makes in their own cache
//! folder, with snapshots taken where and as Snapper takes them (a folder in
//! `.snapshots`, its `info.xml`, then a read-only snapshot of the subvolume).
//! Skipped with a warning where the cache folder is not on btrfs.

#![cfg(all(test, feature = "integration"))]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use backtrack_core::index::{ArchiveMeta, IndexReader, IndexWriter, Repo};
use backtrack_core::walk::{self, WalkSpec};
use futures::StreamExt;

use crate::pipeline::{start_snapper, SnapperPlan};

/// A machine with `/home` as a subvolume Snapper covers, and a second
/// subvolume, `data`, it does not.
pub(crate) struct Machine {
    scratch: PathBuf,
    pub home: PathBuf,
    pub data: PathBuf,
}

impl Machine {
    pub async fn new(name: &str) -> Option<Machine> {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
        if !crate::snapshot::is_btrfs(&base) {
            eprintln!(
                "skipping: {} is not on btrfs, so Snapper's snapshots cannot be made",
                base.display()
            );
            return None;
        }
        let scratch = base.join(format!("backtrack-snapper-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).unwrap();
        let machine = Machine {
            home: scratch.join("home"),
            data: scratch.join("data"),
            scratch,
        };
        for subvolume in [&machine.home, &machine.data] {
            let made = tokio::process::Command::new("btrfs")
                .args(["subvolume", "create"])
                .arg(subvolume)
                .output()
                .await
                .is_ok_and(|o| o.status.success());
            if !made {
                eprintln!("skipping: cannot create a subvolume here");
                return None;
            }
        }
        std::fs::create_dir_all(machine.documents()).unwrap();
        std::fs::create_dir_all(machine.photos()).unwrap();
        std::fs::create_dir_all(machine.home.join(".snapshots")).unwrap();
        Some(machine)
    }

    /// A backed-up folder Snapper covers.
    pub fn documents(&self) -> PathBuf {
        self.home.join("k/Documents")
    }

    /// A backed-up folder in the other subvolume.
    pub fn photos(&self) -> PathBuf {
        self.data.join("Photos")
    }

    pub fn sources(&self) -> Vec<PathBuf> {
        vec![self.documents(), self.photos()]
    }

    /// A folder for the daemon's own files, on the same filesystem, so that
    /// its own snapshots of `/home` can be kept there (Stage 5).
    pub fn state(&self) -> PathBuf {
        let state = self.scratch.join("state");
        std::fs::create_dir_all(&state).unwrap();
        state
    }

    /// Take snapshot `n` as Snapper does, dated `date` (epoch seconds).
    pub async fn take(&self, n: u64, date: i64) {
        self.take_pausing(n, date, std::time::Duration::ZERO).await;
    }

    /// Take snapshot `n`, pausing for `pause` between making its folder and
    /// filling it, as Snapper does on a busy machine.
    pub async fn take_pausing(&self, n: u64, date: i64, pause: std::time::Duration) {
        let folder = self.home.join(".snapshots").join(n.to_string());
        std::fs::create_dir(&folder).unwrap();
        tokio::time::sleep(pause).await;
        std::fs::write(
            folder.join("info.xml"),
            format!(
                "<?xml version=\"1.0\"?>\n<snapshot>\n  <type>single</type>\n  <num>{n}</num>\n  \
                 <date>{}</date>\n  <description>timeline</description>\n  \
                 <cleanup>timeline</cleanup>\n</snapshot>\n",
                utc(date)
            ),
        )
        .unwrap();
        crate::snapshot::create(&self.home, &folder.join("snapshot"))
            .await
            .expect("a read-only snapshot");
    }

    /// Remove snapshot `n`, as Snapper's cleanup does.
    pub async fn remove(&self, n: u64) {
        let folder = self.home.join(".snapshots").join(n.to_string());
        crate::snapshot::remove(&folder.join("snapshot"))
            .await
            .unwrap();
        std::fs::remove_dir_all(folder).unwrap();
    }

    /// Everything under `.snapshots`: each path with its size, modification
    /// time, mode and contents, to show that nothing there is changed.
    pub fn snapshots_as_they_are(&self) -> BTreeMap<PathBuf, (u64, i128, u32, Vec<u8>)> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mut seen = BTreeMap::new();
        let mut stack = vec![self.home.join(".snapshots")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                let meta = std::fs::symlink_metadata(&path).unwrap();
                let contents = if meta.is_file() {
                    std::fs::read(&path).unwrap()
                } else {
                    Vec::new()
                };
                if meta.is_dir() {
                    stack.push(path.clone());
                }
                let mtime =
                    i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec());
                seen.insert(
                    path,
                    (meta.len(), mtime, meta.permissions().mode(), contents),
                );
            }
        }
        seen
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        // Read-only snapshots, Snapper's and the daemon's own, made writable
        // to be removed.
        let snappers = std::fs::read_dir(self.home.join(".snapshots"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path().join("snapshot"));
        let own = std::fs::read_dir(self.scratch.join("state/snapshots"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path());
        for snapshot in snappers.chain(own).collect::<Vec<_>>() {
            let _ = std::process::Command::new("btrfs")
                .args(["property", "set"])
                .arg(&snapshot)
                .args(["ro", "false"])
                .output();
            let _ = std::fs::remove_dir_all(&snapshot);
        }
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

/// `at` as Snapper writes a date: `YYYY-MM-DD hh:mm:ss`, in UTC.
fn utc(at: i64) -> String {
    let (days, secs) = (at.div_euclid(86_400), at.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

/// 2026-10-26 09:00:00 UTC, and the hours after it.
pub(crate) const NINE: i64 = 1_793_005_200;
pub(crate) const HOUR: i64 = 3_600;

/// A catalogue, as the daemon keeps one.
struct Catalogue {
    _dir: tempfile::TempDir,
    path: PathBuf,
    writer: Arc<Mutex<IndexWriter>>,
}

fn catalogue() -> Catalogue {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("index.db");
    let writer = Arc::new(Mutex::new(IndexWriter::open(&path).unwrap()));
    Catalogue {
        _dir: dir,
        path,
        writer,
    }
}

impl Catalogue {
    /// A backup of the machine's folders as they are now, taken at `ts`: the
    /// listing Borg would give, the folders themselves included.
    fn back_up(&self, machine: &Machine, name: &str, ts: i64) {
        let spec = WalkSpec {
            sources: machine.sources(),
            excludes: Default::default(),
            one_file_system: true,
            never: Vec::new(),
            include_dirs: true,
        };
        let items = walk::walk(&spec).items;
        self.writer
            .lock()
            .unwrap()
            .ingest_archive(
                &ArchiveMeta {
                    borg_id: None,
                    name: name.to_string(),
                    ts,
                },
                Repo::Primary,
                items.into_iter(),
            )
            .unwrap();
    }

    /// One Snapper job, run to its end.
    async fn snapper_job(&self, machine: &Machine) {
        let plan = SnapperPlan {
            index: Arc::clone(&self.writer),
            sources: machine.sources(),
            excludes: Vec::new(),
            never: Vec::new(),
            failed: Default::default(),
        };
        let mut stream = start_snapper(plan);
        while let Some(event) = stream.next().await {
            if let backtrack_core::engine::JobEvent::Finished(outcome) = event {
                outcome.expect("the job succeeds");
                return;
            }
        }
        panic!("the job ended without saying how");
    }

    fn reader(&self) -> IndexReader {
        IndexReader::open(&self.path).unwrap()
    }

    /// The archives, oldest first, as (kind, name, catalogued).
    fn archives(&self) -> Vec<(String, String, bool)> {
        self.reader()
            .archives_overview()
            .unwrap()
            .into_iter()
            .rev()
            .map(|a| (a.repo, a.name, a.catalogued))
            .collect()
    }

    /// The versions of `path`, as (first, last, size).
    fn history(&self, path: &Path) -> Vec<(i64, i64, i64)> {
        let mut history: Vec<(i64, i64, i64)> = self
            .reader()
            .file_history(&walk::archive_path(path))
            .unwrap()
            .into_iter()
            .map(|v| (v.first_seq, v.last_seq, v.size))
            .collect();
        history.sort();
        history
    }

    /// What `folder` held at archive `seq`, by name and size.
    fn folder_at(&self, folder: &Path, seq: i64) -> Vec<(String, i64)> {
        self.reader()
            .folder_at(&walk::archive_path(folder), seq)
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.size))
            .collect()
    }
}

pub(crate) fn snapshot_name(machine: &Machine, n: u64) -> String {
    format!(
        "snapper:{}",
        machine
            .home
            .join(".snapshots")
            .join(n.to_string())
            .display()
    )
}

#[tokio::test]
async fn a_snapshot_is_catalogued_with_its_files_where_they_really_live() {
    let Some(machine) = Machine::new("real-paths").await else {
        return;
    };
    let report = machine.documents().join("report.odt");
    std::fs::write(&report, "first draft").unwrap();
    std::fs::write(machine.photos().join("beach.jpg"), "jpeg").unwrap();
    let catalogue = catalogue();
    catalogue.back_up(&machine, "bt-1", NINE);

    // Rewritten after the backup, and caught by Snapper's next snapshot.
    std::fs::write(&report, "the second draft").unwrap();
    machine.take(1, NINE + HOUR / 2).await;
    let before = machine.snapshots_as_they_are();
    catalogue.snapper_job(&machine).await;

    assert_eq!(
        catalogue.archives(),
        [
            ("primary".into(), "bt-1".into(), true),
            ("snapper".into(), snapshot_name(&machine, 1), true),
        ]
    );
    assert_eq!(
        catalogue.folder_at(&machine.documents(), 2),
        [("report.odt".to_string(), 16)],
        "browsed at the file's own path, not the snapshot's copy of it"
    );
    assert_eq!(
        catalogue.folder_at(&machine.home.join("k"), 2),
        [("Documents".to_string(), 0)],
        "and the backed-up folder is there in its parent"
    );
    assert_eq!(catalogue.history(&report), [(1, 1, 11), (2, 2, 16)]);
    let beach = machine.photos().join("beach.jpg");
    assert_eq!(
        catalogue.history(&beach),
        [(1, 2, 4)],
        "a folder in another subvolume is not deleted at a snapshot of this one"
    );
    assert!(catalogue
        .reader()
        .folder_at(&walk::archive_path(&machine.home.join(".snapshots")), 2)
        .unwrap()
        .is_empty());

    // A second look finds nothing new to do.
    catalogue.snapper_job(&machine).await;
    assert_eq!(catalogue.archives().len(), 2);
    assert_eq!(
        machine.snapshots_as_they_are(),
        before,
        "nothing under .snapshots is changed"
    );
}

#[tokio::test]
async fn one_older_than_the_newest_backup_is_slotted_in_by_time_and_leaves_when_removed() {
    let Some(machine) = Machine::new("slotted").await else {
        return;
    };
    let report = machine.documents().join("report.odt");
    let notes = machine.documents().join("notes.txt");
    let beach = machine.photos().join("beach.jpg");
    std::fs::write(&report, "first draft").unwrap();
    std::fs::write(&notes, "notes").unwrap();
    std::fs::write(&beach, "jpeg").unwrap();
    let catalogue = catalogue();
    catalogue.back_up(&machine, "bt-1", NINE);
    std::fs::write(&report, "the second draft").unwrap();
    machine.take(1, NINE + HOUR / 2).await;
    // The next backup, catalogued before the snapshot is.
    catalogue.back_up(&machine, "bt-2", NINE + HOUR);
    let before = machine.snapshots_as_they_are();

    catalogue.snapper_job(&machine).await;
    assert_eq!(
        catalogue.archives(),
        [
            ("primary".into(), "bt-1".into(), true),
            ("snapper".into(), snapshot_name(&machine, 1), true),
            ("primary".into(), "bt-2".into(), true),
        ],
        "between the backups either side of it"
    );
    assert_eq!(catalogue.history(&report), [(1, 1, 11), (2, 3, 16)]);
    assert_eq!(
        catalogue.history(&notes),
        [(1, 3, 5)],
        "one unbroken version"
    );
    assert_eq!(catalogue.history(&beach), [(1, 3, 4)]);

    // Snapper's cleanup takes it away.
    machine.remove(1).await;
    catalogue.snapper_job(&machine).await;
    assert_eq!(
        catalogue.archives(),
        [
            ("primary".into(), "bt-1".into(), true),
            ("primary".into(), "bt-2".into(), true),
        ]
    );
    assert_eq!(
        catalogue.history(&report),
        [(1, 1, 11), (2, 2, 16)],
        "as though it had never been"
    );
    assert_eq!(catalogue.history(&notes), [(1, 2, 5)], "rejoined");
    assert_eq!(catalogue.history(&beach), [(1, 2, 4)]);
    let after = machine.snapshots_as_they_are();
    assert_eq!(
        after
            .keys()
            .filter(|p| p.starts_with(machine.home.join(".snapshots/1")))
            .count(),
        0
    );
    let untouched: BTreeMap<_, _> = before
        .into_iter()
        .filter(|(p, _)| !p.starts_with(machine.home.join(".snapshots/1")))
        .collect();
    assert_eq!(
        after, untouched,
        "and nothing else under .snapshots changed"
    );
}

#[tokio::test]
async fn a_backlog_is_catalogued_a_snapshot_a_job_newest_first() {
    let Some(machine) = Machine::new("backlog").await else {
        return;
    };
    let report = machine.documents().join("report.odt");
    let catalogue = catalogue();
    for n in 1..=3 {
        std::fs::write(&report, "x".repeat(n as usize)).unwrap();
        machine.take(n, NINE + n as i64 * HOUR).await;
    }

    catalogue.snapper_job(&machine).await;
    let catalogued: Vec<bool> = catalogue.archives().iter().map(|a| a.2).collect();
    assert_eq!(
        catalogued,
        [false, false, true],
        "all three in place, the newest read"
    );
    catalogue.snapper_job(&machine).await;
    catalogue.snapper_job(&machine).await;
    let catalogued: Vec<bool> = catalogue.archives().iter().map(|a| a.2).collect();
    assert_eq!(catalogued, [true, true, true]);
    assert_eq!(
        catalogue.history(&report),
        [(1, 1, 1), (2, 2, 2), (3, 3, 3)]
    );
}
