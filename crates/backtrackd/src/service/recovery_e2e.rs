// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Disaster recovery end to end, against a real Borg repository: a computer
//! backed up, then restored into an empty home folder somewhere else.
//!
//! Run by `just test-integration`; skipped otherwise, because it shells out to
//! `borg`.
//!
//! The comparisons are of whole trees: every file's contents and modification
//! time, every link's target, every folder, in the old home folder and the new
//! one. "Restored" means indistinguishable, less the parts that are never
//! restored on purpose.

#![cfg(all(test, feature = "integration"))]

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use backtrack_core::config::Config;
use backtrack_core::engine::{BackupEngine, BorgCli, Encryption, RepoSpec};
use backtrack_core::secret::{FileSecretStore, SecretStore};

use crate::jobs::{JobRegistry, JobState};
use crate::recovery::{Manifest, Stage, Status};
use crate::service::{Daemon1, Shared};

const PASS: &str = "recovery-passphrase";
const USER: &str = "keith";

/// A computer that was backed up, and the empty home folder of the one it is
/// being restored to.
struct Lost {
    dir: tempfile::TempDir,
    /// The old computer's home folder.
    old: PathBuf,
    /// The new one's.
    new: PathBuf,
    archive: String,
}

impl Lost {
    fn state(&self) -> PathBuf {
        self.dir.path().join("state")
    }

    fn repo(&self) -> String {
        self.dir.path().join("repo").to_string_lossy().into_owned()
    }

    fn manifest(&self) -> Option<Manifest> {
        let text = std::fs::read_to_string(self.state().join("dr-job.json")).ok()?;
        serde_json::from_str(&text).ok()
    }
}

/// Bytes nothing will compress, quickly.
fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn put(root: &Path, relative: &str, contents: &[u8]) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A daemon, as one would start on the new computer, keeping everything in
/// the fixture.
async fn daemon(lost: &Lost) -> Arc<Shared> {
    std::fs::create_dir_all(lost.state()).unwrap();
    let secrets: Arc<dyn SecretStore> =
        Arc::new(FileSecretStore::new(lost.dir.path().join("secrets.json")));
    let mut config = Config::default();
    config.storage.repository = Some(lost.repo());
    config.backup.exclude = vec![];
    // What the Welcome page's import leaves: a destination, nothing chosen.
    config.backup.include = vec![];
    let shared = Shared::in_dir(config, JobRegistry::new(), secrets.clone(), &lost.state());
    let engine = BorgCli::new(lost.repo(), lost.repo(), secrets)
        .await
        .expect("borg is available");
    shared.set_engine(Arc::new(engine));
    shared.set_index(Arc::new(std::sync::Mutex::new(
        backtrack_core::index::IndexWriter::open(&lost.state().join("index.db")).unwrap(),
    )));
    shared
}

/// The old computer, backed up once.
async fn a_lost_computer() -> Lost {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("old/home").join(USER);
    let new = dir.path().join("new/home").join(USER);
    std::fs::create_dir_all(&new).unwrap();

    put(&old, "Documents/report.odt", b"the quarterly report");
    put(&old, "Documents/tax/2025.pdf", &noise(1, 40_000));
    std::fs::create_dir_all(old.join("Documents/empty")).unwrap();
    std::os::unix::fs::symlink("report.odt", old.join("Documents/latest")).unwrap();
    for i in 0..2 {
        put(
            &old,
            &format!("Music/track-{i}.ogg"),
            &noise(10 + i, 2 << 20),
        );
    }
    // The big one, in several files, so that a stop part-way through leaves
    // some whole and some not.
    for i in 0..12 {
        put(
            &old,
            &format!("Pictures/2025/IMG_{i:04}.jpg"),
            &noise(100 + i, 5 << 20),
        );
    }
    put(&old, ".config/app/settings.ini", b"[app]\ntheme=dark\n");
    put(&old, "todo.txt", b"buy milk");
    put(&old, ".local/share/notes.db", &noise(7, 10_000));
    // The old computer's Backtrack. Never brought back over the new one's.
    put(&old, ".local/share/backtrack/config.toml", b"[storage]\n");

    let secrets: Arc<dyn SecretStore> =
        Arc::new(FileSecretStore::new(dir.path().join("secrets.json")));
    let repo = dir.path().join("repo").to_string_lossy().into_owned();
    secrets.set(&repo, PASS).await.unwrap();
    let engine = BorgCli::new(repo.clone(), repo.clone(), Arc::clone(&secrets))
        .await
        .expect("borg is available");
    engine
        .init_repo(&RepoSpec {
            path: repo.clone(),
            encryption: Encryption::RepokeyBlake2,
        })
        .await
        .expect("repo created");

    let mut lost = Lost {
        dir,
        old: old.clone(),
        new,
        archive: String::new(),
    };
    let shared = daemon(&lost).await;
    let mut config = shared.config();
    config.backup.include = vec![old];
    shared.store_config(config).unwrap();
    let job = shared.submit_backup().await.expect("a backup starts");
    finished(&shared, job).await;
    lost.archive = shared
        .last_archive
        .lock()
        .unwrap()
        .clone()
        .expect("the backup names its archive");
    lost
}

/// Wait for `job` to end, and say how.
async fn finished(shared: &Arc<Shared>, job: u64) -> JobState {
    for _ in 0..1200 {
        let state = shared.jobs.snapshot(job).expect("job exists").state;
        if state.is_terminal() {
            return state;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("job {job} never finished");
}

async fn completed(shared: &Arc<Shared>, job: u64) {
    let state = finished(shared, job).await;
    assert_eq!(
        state,
        JobState::Done(crate::jobs::Outcome::Completed),
        "job {job}"
    );
}

/// Poll the record on disk until `ready` holds of it.
async fn when(lost: &Lost, what: &str, ready: impl Fn(&Manifest) -> bool) -> Manifest {
    for _ in 0..4000 {
        if let Some(manifest) = lost.manifest() {
            if ready(&manifest) {
                return manifest;
            }
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    panic!("the recovery never reached: {what}");
}

/// Every entry under `root`, by relative path, with what it is: a folder, a
/// link and its target, or a file's size, contents and modification time.
fn tree(root: &Path, skip: &[&str]) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        for entry in std::fs::read_dir(root.join(&relative)).unwrap().flatten() {
            let child = relative.join(entry.file_name());
            let name = child.to_string_lossy().into_owned();
            if skip
                .iter()
                .any(|s| name == *s || name.starts_with(&format!("{s}/")))
            {
                continue;
            }
            let path = root.join(&child);
            let facts = std::fs::symlink_metadata(&path).unwrap();
            let what = if facts.file_type().is_symlink() {
                format!("link {}", std::fs::read_link(&path).unwrap().display())
            } else if facts.is_dir() {
                pending.push(child);
                "folder".to_string()
            } else {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                std::fs::read(&path).unwrap().hash(&mut hasher);
                format!(
                    "file {} bytes {:x} at {}",
                    facts.len(),
                    hasher.finish(),
                    facts.mtime()
                )
            };
            found.insert(name, what);
        }
    }
    found
}

/// What the new home folder should hold: the old one, less Backtrack's own.
fn expected(lost: &Lost) -> BTreeMap<String, String> {
    tree(&lost.old, &[".local/share/backtrack"])
}

#[tokio::test]
async fn a_computer_is_restored_whole_and_backups_start_once_it_is() {
    let lost = a_lost_computer().await;
    let shared = daemon(&lost).await;

    let job = shared
        .begin_recovery_into(&lost.archive, None, "ask", lost.new.clone(), USER.into())
        .await
        .expect("the recovery starts");

    // While it runs, nothing is backed up and nothing can be.
    assert!(shared.recovery.restoring());
    assert!(!shared.schedule_input().configured, "the schedule is held");
    assert!(
        Daemon1::new(Arc::clone(&shared))
            .backup_now()
            .await
            .is_err(),
        "a backup of a half-restored home folder is refused"
    );

    completed(&shared, job).await;
    let manifest = lost.manifest().expect("the record stays until handed over");
    assert_eq!(manifest.stage, Stage::Finished);
    let keys: Vec<&str> = manifest.steps.iter().map(|s| s.step.key.as_str()).collect();
    assert_eq!(
        keys,
        ["Documents", "Music", "Pictures", "."],
        "smallest first"
    );
    assert!(manifest.steps.iter().all(|s| s.status == Status::Done));
    assert_eq!(manifest.steps.iter().map(|s| s.failed).sum::<u64>(), 0);

    assert_eq!(tree(&lost.new, &[]), expected(&lost));
    assert!(
        !lost.new.join(".local/share/backtrack").exists(),
        "the old computer's Backtrack stays behind"
    );

    // What the window's signal fan-out does when the job ends.
    shared.after_recovery();
    assert!(lost.manifest().is_none(), "nothing left to remember");
    assert!(!lost.state().join("recovery").exists());
    assert_eq!(shared.config().backup.include, vec![lost.new.clone()]);
    let input = shared.schedule_input();
    assert!(input.configured);
    assert_eq!(
        crate::schedule::decide(&input, SystemTime::now(), Duration::ZERO),
        crate::schedule::Decision::Run,
        "the first backup after a recovery is due at once"
    );
}

#[tokio::test]
async fn a_recovery_stopped_dead_half_way_carries_on_in_the_next_daemon() {
    let lost = Arc::new(a_lost_computer().await);

    // The first daemon runs on a runtime of its own, so it can be stopped the
    // way a killed process stops: every task dropped where it stands, Borg
    // killed, nothing given a chance to tidy up.
    let first = Arc::clone(&lost);
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let shared = daemon(&first).await;
            shared
                .begin_recovery_into(&first.archive, None, "ask", first.new.clone(), USER.into())
                .await
                .unwrap();
            // Half way: the small folders back, the big one part-fetched.
            let pictures = first.state().join(format!(
                "recovery/staging/{}/Pictures/2025",
                first.old.strip_prefix("/").unwrap().display()
            ));
            when(&first, "fetching Pictures", |m| {
                m.steps[2].status == Status::Fetching
                    && std::fs::read_dir(&pictures).is_ok_and(|mut d| d.next().is_some())
            })
            .await;
        });
        runtime.shutdown_background();
    })
    .join()
    .unwrap();

    let stopped = lost.manifest().unwrap();
    assert_eq!(stopped.steps[0].status, Status::Done);
    assert_eq!(stopped.steps[1].status, Status::Done);
    assert_eq!(stopped.steps[2].status, Status::Fetching);

    let second = daemon(&lost).await;
    assert!(
        !second.schedule_input().configured,
        "held from the record alone, before anything has started"
    );
    second.recovery_at_start();
    let job = second
        .recovery
        .job()
        .expect("the recovery is running again");
    completed(&second, job).await;

    assert_eq!(tree(&lost.new, &[]), expected(&lost));
    second.after_recovery();
    assert!(lost.manifest().is_none());
}

#[tokio::test]
async fn an_interrupted_fetch_keeps_what_is_whole_and_fetches_only_the_rest() {
    let lost = a_lost_computer().await;
    let shared = daemon(&lost).await;
    let source = lost.old.strip_prefix("/").unwrap().display().to_string();

    // A recovery of Pictures alone, taken straight back so that the record
    // can be made to say what a stop part-way through the fetch would have
    // left it saying.
    let job = shared
        .begin_recovery_into(
            &lost.archive,
            Some(vec!["Pictures".into()]),
            "ask",
            lost.new.clone(),
            USER.into(),
        )
        .await
        .unwrap();
    let _ = shared.jobs.cancel(job);
    finished(&shared, job).await;
    drop(shared.recovery.quiet().await);

    // What that stop left in the working folder: Pictures fetched in full,
    // then spoiled the three ways a stop can leave a file: cut short, written
    // but not yet given its time, and not reached at all.
    let staging = lost.state().join("recovery/staging");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).unwrap();
    let engine = shared.engine().unwrap();
    let mut stream = engine
        .extract(
            &backtrack_core::engine::ArchiveId(lost.archive.clone()),
            &[format!("{source}/Pictures")],
            &staging,
        )
        .await
        .unwrap();
    use futures::StreamExt;
    while let Some(event) = stream.next().await {
        if let backtrack_core::engine::JobEvent::Finished(outcome) = event {
            outcome.unwrap();
        }
    }
    let staged = staging.join(&source).join("Pictures/2025");
    let file = |i: u32| staged.join(format!("IMG_{i:04}.jpg"));
    std::fs::File::options()
        .write(true)
        .open(file(3))
        .unwrap()
        .set_len(1000)
        .unwrap();
    std::fs::File::options()
        .write(true)
        .open(file(4))
        .unwrap()
        .set_modified(SystemTime::now())
        .unwrap();
    std::fs::remove_file(file(5)).unwrap();
    let kept: Vec<(u32, u64)> = [0, 1, 2, 6, 7, 8, 9, 10, 11]
        .into_iter()
        .map(|i| (i, std::fs::metadata(file(i)).unwrap().ino()))
        .collect();
    let mut manifest = lost.manifest().unwrap();
    manifest.steps[0].status = Status::Fetching;
    shared.recovery.save(&manifest).unwrap();

    let job = shared.resume_recovery().unwrap();
    completed(&shared, job).await;

    let landed = lost.new.join("Pictures/2025");
    for (i, inode) in kept {
        assert_eq!(
            std::fs::metadata(landed.join(format!("IMG_{i:04}.jpg")))
                .unwrap()
                .ino(),
            inode,
            "IMG_{i:04}.jpg was whole and should have been kept, not fetched again"
        );
    }
    assert_eq!(
        tree(&lost.new, &[]),
        tree(&lost.old.join(""), &[])
            .into_iter()
            .filter(|(path, _)| path == "Pictures" || path.starts_with("Pictures/"))
            .collect::<BTreeMap<_, _>>()
    );
}

#[tokio::test]
async fn a_paused_recovery_holds_and_carries_on_when_resumed() {
    let lost = a_lost_computer().await;
    let shared = daemon(&lost).await;
    let daemon1 = Daemon1::new(Arc::clone(&shared));
    let job = shared
        .begin_recovery_into(&lost.archive, None, "ask", lost.new.clone(), USER.into())
        .await
        .unwrap();
    when(&lost, "fetching Pictures", |m| {
        m.steps[2].status == Status::Fetching
    })
    .await;

    daemon1.pause_job(job).await.expect("a recovery can pause");
    let paused = when(&lost, "the pause written down", |m| m.paused).await;
    assert_eq!(paused.stage, Stage::Restoring);
    assert_eq!(shared.jobs.snapshot(job).unwrap().state, JobState::Paused);
    assert_eq!(daemon1.get_recovery().await.unwrap().state, "paused");
    assert!(
        !shared.schedule_input().configured,
        "still held while paused"
    );

    // Nothing moves while it is paused.
    let before = tree(&lost.new, &[]);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(tree(&lost.new, &[]), before);

    assert_eq!(
        daemon1.resume_recovery().await.unwrap(),
        job,
        "the same job"
    );
    completed(&shared, job).await;
    assert_eq!(tree(&lost.new, &[]), expected(&lost));
}

#[tokio::test]
async fn cancelling_keeps_what_was_restored_or_takes_exactly_that_away() {
    for discard in [false, true] {
        let lost = a_lost_computer().await;
        put(&lost.new, "Documents/mine.txt", b"made on the new computer");
        let shared = daemon(&lost).await;
        let job = shared
            .begin_recovery_into(&lost.archive, None, "ask", lost.new.clone(), USER.into())
            .await
            .unwrap();
        when(&lost, "fetching Pictures", |m| {
            m.steps[2].status == Status::Fetching
        })
        .await;

        let clearing = shared.cancel_recovery(discard).unwrap();
        completed(&shared, clearing).await;
        assert_eq!(
            finished(&shared, job).await,
            JobState::Done(crate::jobs::Outcome::Cancelled)
        );

        assert!(lost.manifest().is_none(), "forgotten either way");
        assert!(!lost.state().join("recovery").exists());
        assert!(shared.schedule_input().configured || shared.config().backup.include.is_empty());
        assert_eq!(
            std::fs::read(lost.new.join("Documents/mine.txt")).unwrap(),
            b"made on the new computer",
            "never the recovery's to take"
        );
        let report = lost.new.join("Documents/report.odt");
        assert_eq!(report.exists(), !discard, "discard: {discard}");
        if discard {
            assert!(
                !lost.new.join("Music").exists(),
                "a folder it made, emptied"
            );
            assert!(lost.new.join("Documents").exists(), "it holds mine.txt");
        } else {
            assert_eq!(std::fs::read(report).unwrap(), b"the quarterly report");
        }
    }
}

#[tokio::test]
async fn files_that_clash_wait_for_one_summary_and_nothing_is_overwritten() {
    let lost = a_lost_computer().await;
    // A new computer with a few things of its own already, two of which the
    // backup also has, differently.
    put(&lost.new, "todo.txt", b"buy bread");
    put(&lost.new, "Documents/report.odt", b"a draft made here");
    let shared = daemon(&lost).await;
    let daemon1 = Daemon1::new(Arc::clone(&shared));

    let job = shared
        .begin_recovery_into(&lost.archive, None, "ask", lost.new.clone(), USER.into())
        .await
        .unwrap();
    completed(&shared, job).await;
    shared.after_recovery();

    // Everything else is back; the two are untouched, and waiting.
    assert_eq!(
        std::fs::read(lost.new.join("todo.txt")).unwrap(),
        b"buy bread"
    );
    assert_eq!(
        std::fs::read(lost.new.join("Documents/report.odt")).unwrap(),
        b"a draft made here"
    );
    assert!(lost.new.join("Pictures/2025/IMG_0011.jpg").exists());
    let waiting = daemon1.get_recovery().await.unwrap();
    assert_eq!(waiting.state, "review");
    assert_eq!(waiting.conflicts, 2);
    assert!(
        shared.schedule_input().configured,
        "a question waiting does not hold back the backups"
    );

    let review = daemon1.prepare_recovery_review().await.unwrap();
    completed(&shared, review).await;
    let preview = daemon1.get_restore_preview(review).await.unwrap();
    assert_eq!(preview.conflicts, 2);
    let mut asked: Vec<&str> = preview.entries.iter().map(|e| e.path.as_str()).collect();
    asked.sort();
    assert_eq!(asked, ["Documents/report.odt", "todo.txt"]);

    // Keep this computer's draft, take the backup's list.
    let carried = daemon1
        .execute_restore(
            review,
            "skip",
            vec![
                ("todo.txt".into(), "replace".into()),
                ("Documents/report.odt".into(), "skip".into()),
            ],
        )
        .await
        .unwrap();
    completed(&shared, carried).await;
    shared.recovery.tidy();

    assert_eq!(
        std::fs::read(lost.new.join("todo.txt")).unwrap(),
        b"buy milk"
    );
    assert_eq!(
        std::fs::read(lost.new.join("Documents/report.odt")).unwrap(),
        b"a draft made here"
    );
    assert_eq!(daemon1.get_recovery().await.unwrap().state, "none");
    assert!(lost.manifest().is_none());
}
