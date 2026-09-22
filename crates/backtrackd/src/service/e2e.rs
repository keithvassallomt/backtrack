// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! End-to-end: a real zbus client driving a real backup through the real
//! interface, against a real Borg repository.
//!
//! Run by `just test-integration`; skipped otherwise, because it shells out to
//! `borg`.
//!
//! The two connections are a peer-to-peer pair rather than a session bus. That
//! exercises the same zbus machinery — method dispatch, argument marshalling,
//! signal delivery — without needing a bus daemon in the test environment, so
//! this runs in CI containers as happily as on a desktop.

#![cfg(all(test, feature = "integration"))]

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use backtrack_core::config::Config;
use backtrack_core::engine::{BackupEngine, BorgCli, Encryption, RepoSpec};
use backtrack_core::secret::{FileSecretStore, SecretStore};
use futures::StreamExt;
use zbus::object_server::SignalEmitter;

use crate::jobs::JobRegistry;
use crate::service::{fan_out_signals, Daemon1, Shared, SEARCH_LIMIT};

const PASS: &str = "e2e-passphrase";
const PATH: &str = "/org/backtrack/Daemon1";
const IFACE: &str = "org.backtrack.Daemon1";

/// A repository with something worth backing up in it.
struct Fixture {
    _dir: tempfile::TempDir,
    shared: Arc<Shared>,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo").to_str().unwrap().to_string();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("notes.txt"), b"the quick brown fox").unwrap();
    // Enough incompressible material that borg's progress reporting has
    // something to report. A handful of small files finishes inside a single
    // progress tick, which would make the progress assertion below a
    // coin toss rather than a check.
    for i in 0..200 {
        let mut block = vec![0u8; 256 * 1024];
        for (n, byte) in block.iter_mut().enumerate() {
            *byte = ((n as u64 * 2_654_435_761) ^ (i as u64 * 40_503)) as u8;
        }
        std::fs::write(src.join(format!("blob-{i}.bin")), block).unwrap();
    }

    let secrets: Arc<dyn SecretStore> = Arc::new(FileSecretStore::new(dir.path().join("s.json")));
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

    let mut config = Config::default();
    config.storage.repository = Some(repo);
    config.backup.include = vec![src];
    // The default exclusions do not matter here and only slow borg down.
    config.backup.exclude = vec![];

    // `in_dir` rather than `new`: this test writes real bookkeeping and a real
    // catalogue, and both must land in the fixture's temporary directory rather
    // than in the data directory of whoever is running the suite.
    let shared = Shared::in_dir(config, JobRegistry::new(), secrets, dir.path());
    shared.set_engine(Arc::new(engine));
    shared.set_index(Arc::new(std::sync::Mutex::new(
        backtrack_core::index::IndexWriter::open(&dir.path().join("index.db")).unwrap(),
    )));
    Fixture { _dir: dir, shared }
}

/// A connected client/server pair with the interface served on one end.
///
/// Both ends must be built *concurrently*. Each half of the handshake waits for
/// the other, so awaiting the server to completion before starting the client
/// deadlocks — the server would be waiting for an authentication message from a
/// peer that does not exist yet.
async fn connect(shared: Arc<Shared>) -> (zbus::Connection, zbus::Connection) {
    let guid = zbus::Guid::generate();
    let (server_side, client_side) = tokio::net::UnixStream::pair().unwrap();

    let server = zbus::connection::Builder::unix_stream(server_side)
        .server(guid)
        .unwrap()
        .p2p()
        .serve_at(PATH, Daemon1::new(Arc::clone(&shared)))
        .unwrap()
        .build();

    let client = zbus::connection::Builder::unix_stream(client_side)
        .p2p()
        .build();

    let (server, client) = futures::join!(server, client);
    (
        server.expect("server connection"),
        client.expect("client connection"),
    )
}

#[tokio::test]
async fn a_client_drives_a_backup_and_hears_progress_then_healthy() {
    let fixture = fixture().await;
    let shared = Arc::clone(&fixture.shared);

    // Start from a machine that needs attention, so the return to HEALTHY is a
    // real transition the client can observe rather than the state it was
    // already in. This is the recovery path a user actually cares about: the
    // banner clearing once a backup finally succeeds.
    shared.record_blocking_failure(true);
    assert_eq!(shared.health().as_str(), "BROKEN");

    let (server, client) = connect(Arc::clone(&shared)).await;
    let emitter = SignalEmitter::new(&server, PATH).unwrap();
    tokio::spawn(fan_out_signals(Arc::clone(&shared), emitter.to_owned()));

    // Listen before calling, so nothing emitted during the backup is missed.
    let mut messages = zbus::MessageStream::from(client.clone());

    let reply = client
        .call_method(None::<()>, PATH, Some(IFACE), "BackupNow", &())
        .await
        .expect("BackupNow accepted");
    let job: u64 = reply.body().deserialize().expect("a job id");
    assert!(job > 0, "a real job id");

    let mut progress = 0usize;
    let mut states = Vec::new();
    let mut finished: Vec<(u64, String, String)> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);

    while tokio::time::Instant::now() < deadline {
        let Ok(Some(Ok(message))) =
            tokio::time::timeout(Duration::from_secs(120), messages.next()).await
        else {
            break;
        };
        let header = message.header();
        if message.message_type() != zbus::message::Type::Signal {
            continue;
        }
        match header.member().map(|m| m.as_str()) {
            Some("BackupProgress") => {
                let (id, _phase, _current, _total): (u64, String, u64, u64) =
                    message.body().deserialize().expect("progress payload");
                assert_eq!(id, job, "progress must carry the job it belongs to");
                progress += 1;
            }
            Some("JobFinished") => {
                let (id, kind, outcome): (u64, String, String) =
                    message.body().deserialize().expect("job-finished payload");
                finished.push((id, kind, outcome));
            }
            Some("StatusChanged") => {
                let state: String = message.body().deserialize().expect("state payload");
                states.push(state.clone());
                if state == "HEALTHY" {
                    break;
                }
            }
            _ => {}
        }
    }

    assert_eq!(
        states.first().map(String::as_str),
        Some("BROKEN"),
        "the client should first hear the state it joined into: {states:?}"
    );
    assert_eq!(
        states.last().map(String::as_str),
        Some("HEALTHY"),
        "a successful backup must clear the banner: {states:?}"
    );
    assert!(
        progress > 0,
        "a real backup must report progress; heard {progress} progress signals"
    );

    // The signal that exists because `StatusChanged` cannot answer "is the
    // thing I started done yet?" — a client that starts a backup on an already
    // healthy machine hears no state change at all.
    let ours: Vec<&(u64, String, String)> = finished.iter().filter(|(id, ..)| *id == job).collect();
    assert_eq!(
        ours.len(),
        1,
        "exactly one JobFinished for the backup; heard {finished:?}"
    );
    assert_eq!(ours[0].1, "backup");
    assert_eq!(ours[0].2, "completed");

    // And the status the client can ask for agrees with what it was told.
    let reply = client
        .call_method(None::<()>, PATH, Some(IFACE), "GetStatus", &())
        .await
        .expect("GetStatus");
    let status: crate::service::Status = reply.body().deserialize().expect("status payload");
    assert_eq!(status.state, "HEALTHY");
    assert!(status.last_backup > 0, "the backup time is recorded");
    assert!(status.configured);
}

#[tokio::test]
async fn preview_returns_a_readable_descriptor_for_an_archived_file() {
    let fixture = fixture().await;
    let shared = Arc::clone(&fixture.shared);
    let (_server, client) = connect(Arc::clone(&shared)).await;

    // Take a backup first so there is something to preview.
    let reply = client
        .call_method(None::<()>, PATH, Some(IFACE), "BackupNow", &())
        .await
        .unwrap();
    let job: u64 = reply.body().deserialize().unwrap();
    wait_for_job(&shared, job).await;

    let archive = newest_archive(&shared).await;
    let relative = archived_path(&shared, &archive, "notes.txt").await;

    let reply = client
        .call_method(
            None::<()>,
            PATH,
            Some(IFACE),
            "PreviewFile",
            &(archive.as_str(), relative.as_str()),
        )
        .await
        .expect("PreviewFile");
    let fd: zbus::zvariant::OwnedFd = reply.body().deserialize().expect("a descriptor");

    use std::io::Read as _;
    let mut file = std::fs::File::from(std::os::fd::OwnedFd::from(fd));
    let mut contents = String::new();
    file.read_to_string(&mut contents).expect("readable");
    assert_eq!(contents, "the quick brown fox");

    // Second call is a cache hit and must return the same bytes.
    let reply = client
        .call_method(
            None::<()>,
            PATH,
            Some(IFACE),
            "PreviewFile",
            &(archive.as_str(), relative.as_str()),
        )
        .await
        .expect("PreviewFile again");
    let fd: zbus::zvariant::OwnedFd = reply.body().deserialize().unwrap();
    let mut file = std::fs::File::from(std::os::fd::OwnedFd::from(fd));
    let mut again = String::new();
    file.read_to_string(&mut again).unwrap();
    assert_eq!(again, contents, "a cache hit must not change the contents");
}

#[tokio::test]
async fn errors_cross_the_bus_with_their_documented_names() {
    // An unconfigured daemon: no engine, so a backup cannot be started.
    let shared = Shared::new(
        Config::default(),
        JobRegistry::new(),
        Arc::new(backtrack_testkit::MockSecretStore::default()),
    );
    let (_server, client) = connect(Arc::clone(&shared)).await;

    let err = client
        .call_method(None::<()>, PATH, Some(IFACE), "BackupNow", &())
        .await
        .expect_err("an unconfigured daemon cannot back up");

    let zbus::Error::MethodError(name, _, _) = &err else {
        panic!("expected a named method error, got {err:?}");
    };
    assert_eq!(
        name.as_str(),
        "org.backtrack.Error.NotConfigured",
        "clients match on the name, so it is the contract"
    );
}

#[tokio::test]
async fn a_backup_that_was_never_catalogued_is_picked_up_on_the_next_start() {
    // The stage's acceptance criterion, against a real repository. The archive
    // exists in Borg but the catalogue never heard of it — which is exactly what
    // a `kill -9` between `borg create` finishing and the ingest committing
    // leaves behind. The next start must notice and catalogue it.
    let fixture = fixture().await;
    let shared = Arc::clone(&fixture.shared);

    // One real backup through the whole pipeline.
    let job = Daemon1::new(Arc::clone(&shared))
        .backup_now()
        .await
        .expect("backup starts");
    wait_for_job(&shared, job).await;
    let archive = newest_archive(&shared).await;

    // Now forget it, precisely as a crash before the ingest committed would.
    shared
        .index()
        .unwrap()
        .lock()
        .unwrap()
        .remove_archives(&[1])
        .expect("forget the archive");
    assert!(
        shared
            .index()
            .unwrap()
            .lock()
            .unwrap()
            .pending_archives()
            .unwrap()
            .is_empty(),
        "the catalogue now knows nothing about it at all"
    );

    // A second, uncatalogued archive too, so reconciliation has to handle both
    // an unknown archive and ordering.
    let job = Daemon1::new(Arc::clone(&shared))
        .backup_now()
        .await
        .expect("second backup starts");
    wait_for_job(&shared, job).await;

    // What the daemon does on every start.
    let job = shared
        .reconcile_catalogue()
        .await
        .expect("reconciliation starts");
    wait_for_job(&shared, job).await;

    let reader = backtrack_core::index::IndexReader::open(&shared.index_path).unwrap();
    let archives = reader.archives_overview().unwrap();
    assert_eq!(
        archives.len(),
        2,
        "both archives are catalogued: {:?}",
        archives.iter().map(|a| &a.name).collect::<Vec<_>>()
    );
    assert!(archives.iter().any(|a| a.name == archive));
    assert_eq!(
        shared.uncatalogued_count().await,
        0,
        "nothing is left un-browsable"
    );
    // And the contents really are there, not just the archive rows.
    let newest = archives.iter().map(|a| a.seq).max().unwrap();
    assert!(
        reader
            .search("notes")
            .unwrap()
            .iter()
            .any(|h| h.last_seq == newest),
        "the recovered catalogue can answer for the newest snapshot"
    );
}

#[tokio::test]
async fn borgs_checkpoint_archives_never_reach_the_timeline() {
    // Borg leaves `<name>.checkpoint` behind when a create is interrupted, and
    // consumes it on the next run. It is scaffolding, not a snapshot: offering
    // one in the timeline would offer a restore from a backup that never
    // finished.
    //
    // Producing one honestly — killing a create mid-run — needs gigabytes of
    // incompressible data and a race with Borg's checkpoint timer, which is not
    // something to put in a test suite. `borg rename` reaches the same end state
    // in milliseconds: `create` *refuses* a `.checkpoint` name (it reports the
    // archive as already existing) but `rename` accepts one, leaving a real
    // archive under a real checkpoint name in a real repository.
    let fixture = fixture().await;
    let shared = Arc::clone(&fixture.shared);
    let repo = shared.config().storage.repository.clone().unwrap();

    let job = Daemon1::new(Arc::clone(&shared))
        .backup_now()
        .await
        .expect("backup starts");
    wait_for_job(&shared, job).await;
    let real = newest_archive(&shared).await;

    let job = Daemon1::new(Arc::clone(&shared))
        .backup_now()
        .await
        .expect("second backup starts");
    wait_for_job(&shared, job).await;
    let doomed = newest_archive(&shared).await;

    let renamed = format!("{doomed}.checkpoint");
    let status = tokio::process::Command::new("borg")
        .arg("rename")
        .arg(format!("{repo}::{doomed}"))
        .arg(&renamed)
        .env("BORG_PASSPHRASE", PASS)
        .status()
        .await
        .expect("borg rename runs");
    assert!(status.success(), "borg rename accepts a checkpoint name");

    // Our engine sees only the finished backup.
    let listed = engine(&shared).list_archives().await.expect("listing");
    assert_eq!(
        listed.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
        vec![real.as_str()],
        "a checkpoint archive is not a snapshot and must not be offered"
    );

    // Reconciliation therefore drops the row the checkpoint used to occupy,
    // rather than keeping a snapshot the repository will not admit to having.
    let job = shared
        .reconcile_catalogue()
        .await
        .expect("reconciliation starts");
    wait_for_job(&shared, job).await;

    let reader = backtrack_core::index::IndexReader::open(&shared.index_path).unwrap();
    let names: Vec<String> = reader
        .archives_overview()
        .unwrap()
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(names, vec![real]);
    assert!(
        !names.iter().any(|n| n.contains(".checkpoint")),
        "no checkpoint rows in the catalogue: {names:?}"
    );
    assert_eq!(shared.uncatalogued_count().await, 0);
}

#[tokio::test]
async fn importing_a_repository_leaves_the_newest_snapshot_browsable_at_once() {
    // The first-run acceptance criterion against a real repository: adopting one
    // that already holds history must return with something to show, not with an
    // empty timeline and a promise.
    let fixture = fixture().await;
    let shared = Arc::clone(&fixture.shared);
    let repo = shared.config().storage.repository.clone().unwrap();

    // Three real archives, written straight through the engine rather than
    // through the pipeline: the pipeline prunes, and Borg's hourly retention
    // quite correctly collapses three backups taken in the same minute into one.
    // The catalogue starts empty, which is the state a machine is in when it is
    // pointed at somebody's existing backups.
    let sources = shared.config().backup.include.clone();
    let names = [
        "bt-host-20260805T000000Z",
        "bt-host-20260806T000000Z",
        "bt-host-20260807T000000Z",
    ];
    for name in names {
        let spec = backtrack_core::engine::CreateSpec {
            archive_name: name.to_string(),
            sources: sources.clone(),
            excludes: vec![],
            compression: Default::default(),
            one_file_system: true,
            created_at: SystemTime::now(),
            paths: Vec::new(),
        };
        let mut stream = engine(&shared).create(&spec).await.expect("create starts");
        while let Some(event) = stream.next().await {
            if let backtrack_core::engine::JobEvent::Finished(r) = event {
                r.expect("create succeeds");
            }
        }
    }
    let newest = names[2].to_string();

    let (_server, client) = connect(Arc::clone(&shared)).await;
    client
        .call_method(
            None::<()>,
            PATH,
            Some(IFACE),
            "ImportRepo",
            &(repo.as_str(), PASS),
        )
        .await
        .expect("ImportRepo accepted");

    // By the time the call has returned, the newest snapshot is readable.
    let reader = backtrack_core::index::IndexReader::open(&shared.index_path).unwrap();
    let archives = reader.archives_overview().unwrap();
    assert_eq!(archives.len(), 3, "the whole history is known about");
    let top = archives.iter().max_by_key(|a| a.seq).unwrap();
    assert_eq!(top.name, newest);
    // Asked through search rather than `folder_at("")`: Borg archives the
    // source path as a single item and does *not* record the directories above
    // it, so the catalogue's tree root is legitimately empty for any source that
    // is not `/`. Stage 6's timeline has to open at the backed-up root for the
    // same reason.
    let hits = reader.search("notes").unwrap();
    assert!(
        hits.iter().any(|h| h.last_seq == top.seq),
        "the newest snapshot has contents the moment import returns: {hits:?}"
    );
    assert_eq!(
        shared.uncatalogued_count().await,
        2,
        "the older two are queued, not read yet"
    );

    // And the background job finishes them.
    for _ in 0..600 {
        if shared.uncatalogued_count().await == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        shared.uncatalogued_count().await,
        0,
        "the backfill completes on its own"
    );
    let reader = backtrack_core::index::IndexReader::open(&shared.index_path).unwrap();
    let hits = reader.search("notes").unwrap();
    assert!(
        hits.iter().any(|h| h.first_seq == 1),
        "the oldest snapshot is browsable too, once the backfill has run: {hits:?}"
    );
}

/// A fixture without the 50 MB of blobs: for the offline tests, where what
/// matters is which files are archived, not how long borg takes.
async fn small_fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo").to_str().unwrap().to_string();
    let src = dir.path().join("src");
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::create_dir_all(src.join(".cache")).unwrap();
    for name in ["docs/a.txt", "docs/b.txt", "docs/c.txt", "docs/d.txt"] {
        std::fs::write(src.join(name), b"original").unwrap();
    }
    // Churn that must never reach the spool, however often it changes.
    std::fs::write(src.join(".cache/junk"), b"junk").unwrap();

    let secrets: Arc<dyn SecretStore> = Arc::new(FileSecretStore::new(dir.path().join("s.json")));
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

    let mut config = Config::default();
    config.storage.repository = Some(repo);
    config.backup.include = vec![src];

    let shared = Shared::in_dir(config, JobRegistry::new(), secrets, dir.path());
    shared.set_engine(Arc::new(engine));
    shared.set_index(Arc::new(std::sync::Mutex::new(
        backtrack_core::index::IndexWriter::open(&dir.path().join("index.db")).unwrap(),
    )));
    Fixture { _dir: dir, shared }
}

/// The source tree the fixture is backing up.
fn source(shared: &Arc<Shared>) -> std::path::PathBuf {
    shared.config().backup.include[0].clone()
}

/// Make the destination unreachable, exactly as unplugging a drive or losing a
/// mount would.
fn cut_the_destination(shared: &Arc<Shared>) {
    let repo = shared.config().storage.repository.clone().unwrap();
    std::fs::rename(&repo, format!("{repo}.away")).expect("destination removed");
}

fn restore_the_destination(shared: &Arc<Shared>) {
    let repo = shared.config().storage.repository.clone().unwrap();
    std::fs::rename(format!("{repo}.away"), &repo).expect("destination back");
}

#[tokio::test]
async fn losing_the_destination_protects_the_changed_files_on_this_computer() {
    // The stage's acceptance criterion, end to end against real borg: cut the
    // destination, touch three files, run the tick that would have been a
    // backup, and find exactly those three files held locally.
    let fixture = small_fixture().await;
    let shared = Arc::clone(&fixture.shared);
    let src = source(&shared);

    // A real backup first, so there is a baseline to compare against.
    let job = Daemon1::new(Arc::clone(&shared))
        .backup_now()
        .await
        .expect("backup starts");
    wait_for_job(&shared, job).await;

    cut_the_destination(&shared);

    // Three edits, and one to a file nobody wants backed up.
    for name in ["docs/a.txt", "docs/b.txt", "docs/c.txt"] {
        std::fs::write(src.join(name), b"edited while away").unwrap();
    }
    std::fs::write(src.join(".cache/junk"), b"churn churn churn").unwrap();

    let job = shared
        .start_scheduled_backup()
        .await
        .expect("the tick is not an error")
        .expect("an unreachable destination starts local protection instead");
    wait_for_job(&shared, job).await;

    // The spool holds one archive, and it holds exactly the three edited files.
    let spool = shared.spool_engine().await.expect("spool engine");
    let archives = spool.list_archives().await.expect("spool listing");
    assert_eq!(archives.len(), 1, "one local snapshot: {archives:?}");
    assert!(archives[0].name.starts_with("bt-local-"));

    let mut items = spool
        .list_archive(&backtrack_core::engine::ArchiveId(archives[0].name.clone()))
        .await
        .unwrap();
    let mut held = Vec::new();
    while let Some(item) = items.next().await {
        held.push(item.unwrap().path);
    }
    held.sort();
    let expected: Vec<String> = ["docs/a.txt", "docs/b.txt", "docs/c.txt"]
        .iter()
        .map(|n| src.join(n).strip_prefix("/").unwrap().display().to_string())
        .collect();
    assert_eq!(
        held, expected,
        "exactly the changed files, and nothing from the cache"
    );

    // The catalogue knows it is a local snapshot, and browsing it shows the
    // whole tree rather than only the three files that moved.
    let reader = backtrack_core::index::IndexReader::open(&shared.index_path).unwrap();
    let catalogued = reader.archives_overview().unwrap();
    assert_eq!(catalogued.len(), 2, "the backup and the local snapshot");
    assert_eq!(
        catalogued[0].repo, "spool",
        "flagged as held on this computer"
    );
    assert!(catalogued[0].catalogued, "and browsable");

    let docs = src
        .join("docs")
        .strip_prefix("/")
        .unwrap()
        .display()
        .to_string();
    let at_local = reader.folder_at(&docs, catalogued[0].seq).unwrap();
    assert_eq!(
        at_local.len(),
        4,
        "all four documents are visible in the local snapshot, got {:?}",
        at_local.iter().map(|e| &e.name).collect::<Vec<_>>()
    );
    let edited = at_local.iter().find(|e| e.name == "a.txt").unwrap();
    let untouched = at_local.iter().find(|e| e.name == "d.txt").unwrap();
    assert_eq!(edited.size, "edited while away".len() as i64);
    assert_eq!(untouched.size, "original".len() as i64);

    restore_the_destination(&shared);
}

#[tokio::test]
async fn status_reports_what_is_held_on_this_computer() {
    // S05-T5's acceptance: the offline block has to carry real numbers during
    // the offline cycle, not placeholders.
    let fixture = small_fixture().await;
    let shared = Arc::clone(&fixture.shared);
    let src = source(&shared);
    let daemon = Daemon1::new(Arc::clone(&shared));

    let job = daemon.backup_now().await.expect("backup starts");
    wait_for_job(&shared, job).await;

    let before = daemon.get_status().await.expect("status");
    assert!(before.destination_reachable);
    assert_eq!(before.local_snapshots, 0, "nothing held yet");
    assert_eq!(before.offline_mode, "spool");

    cut_the_destination(&shared);
    std::fs::write(src.join("docs/a.txt"), b"edited while away").unwrap();
    let job = shared
        .start_scheduled_backup()
        .await
        .expect("not an error")
        .expect("local protection runs");
    wait_for_job(&shared, job).await;

    let during = daemon.get_status().await.expect("status");
    assert!(!during.destination_reachable, "the drive is not there");
    assert_eq!(
        during.local_snapshots, 1,
        "and one snapshot is held locally"
    );
    assert_eq!(
        during.expirable_snapshots, 0,
        "nothing may expire until the destination catches up"
    );
    assert!(
        during.spool_bytes > 0,
        "the spool occupies real bytes and says so"
    );
    assert_eq!(during.state, "PROTECTED_LOCALLY", "and it is not a warning");

    restore_the_destination(&shared);
}

#[tokio::test]
async fn an_offline_hour_with_no_edits_takes_no_local_snapshot() {
    // The quiet case, and the one that decides whether a laptop left closed on
    // a desk fills its disk with identical snapshots.
    let fixture = small_fixture().await;
    let shared = Arc::clone(&fixture.shared);

    let job = Daemon1::new(Arc::clone(&shared))
        .backup_now()
        .await
        .expect("backup starts");
    wait_for_job(&shared, job).await;
    cut_the_destination(&shared);

    let job = shared.start_scheduled_backup().await.expect("not an error");
    if let Some(job) = job {
        wait_for_job(&shared, job).await;
    }

    let spool = shared.spool_engine().await.expect("spool engine");
    assert!(
        spool.list_archives().await.unwrap().is_empty(),
        "nothing changed, so there is nothing to protect and no snapshot to take"
    );
    restore_the_destination(&shared);
}

#[tokio::test]
async fn the_storage_cap_drops_the_oldest_local_snapshots_first() {
    // Driven through the pipeline directly so the cap can be set in bytes: the
    // user-facing setting is in whole gigabytes, and filling one to test it
    // would be an unkind thing to do to a test suite.
    let fixture = small_fixture().await;
    let shared = Arc::clone(&fixture.shared);
    let src = source(&shared);

    let job = Daemon1::new(Arc::clone(&shared))
        .backup_now()
        .await
        .expect("backup starts");
    wait_for_job(&shared, job).await;
    cut_the_destination(&shared);

    let spool = shared.spool_engine().await.expect("spool engine");
    let excludes = vec![format!("pp:{}", shared.spool_dir.display())];

    let take_one = |n: usize, cap_bytes: u64| {
        let spool = Arc::clone(&spool);
        let shared = Arc::clone(&shared);
        let src = src.clone();
        let excludes = excludes.clone();
        async move {
            let plan = crate::pipeline::OfflinePlan {
                engine: spool,
                index: shared.index().unwrap(),
                index_path: shared.index_path.clone(),
                walk: backtrack_core::walk::WalkSpec {
                    sources: vec![src],
                    excludes: backtrack_core::pattern::ExcludeSet::compile(&excludes),
                    one_file_system: true,
                    never: vec![shared.spool_dir.clone()],
                    include_dirs: false,
                },
                excludes,
                compression: Default::default(),
                cap_bytes,
                spool_dir: shared.spool_dir.clone(),
                created_at: SystemTime::now() + Duration::from_secs(n as u64 * 60),
            };
            let (mut stream, _handle) = crate::pipeline::start_offline_backup(plan);
            while let Some(event) = stream.next().await {
                if let backtrack_core::engine::JobEvent::Finished(result) = event {
                    result.expect("the local snapshot succeeds");
                }
            }
        }
    };

    // Two snapshots with no cap at all, to find out what a spool holding two
    // actually occupies. A Borg repository carries fixed overhead — config,
    // index, segment headers — that dwarfs a few kilobytes of test data, so
    // guessing a byte figure here would be testing an assumption about Borg
    // rather than the eviction rule.
    let names = ["docs/a.txt", "docs/b.txt", "docs/c.txt", "docs/d.txt"];
    for (n, name) in names.iter().enumerate().take(2) {
        std::fs::write(src.join(name), vec![b'x'; 64 * 1024 * (n + 1)]).unwrap();
        take_one(n, 0).await;
    }
    assert_eq!(
        spool.list_archives().await.unwrap().len(),
        2,
        "no cap, so both are kept"
    );
    let occupied = crate::offline::directory_bytes(&shared.spool_dir);
    assert!(occupied > 0, "the spool is holding something");

    // Now a cap that two snapshots plus another delta cannot fit inside.
    for (n, name) in names.iter().enumerate().skip(2) {
        std::fs::write(src.join(name), vec![b'x'; 64 * 1024 * (n + 1)]).unwrap();
        take_one(n, occupied).await;
    }

    let held = spool.list_archives().await.unwrap();
    assert!(
        held.len() < 4,
        "the cap must have dropped something, but {} snapshots are held",
        held.len()
    );
    assert!(!held.is_empty(), "and it must not have dropped everything");

    // Whatever survived is the newest: the oldest go first.
    let names: Vec<&str> = held.iter().map(|a| a.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);

    // And the catalogue followed the repository rather than keeping rows for
    // snapshots that no longer exist.
    let reader = backtrack_core::index::IndexReader::open(&shared.index_path).unwrap();
    let spool_rows: Vec<_> = reader
        .archives_overview()
        .unwrap()
        .into_iter()
        .filter(|a| a.repo == "spool")
        .collect();
    assert_eq!(
        spool_rows.len(),
        held.len(),
        "catalogue and spool repository agree on what is held"
    );

    restore_the_destination(&shared);
}

#[tokio::test]
async fn reconnecting_catches_up_and_dates_the_local_snapshots() {
    // The stage's end-to-end criterion: go offline, protect changes locally
    // twice, come back, and find a catch-up archive in the real repository with
    // the local snapshots marked for expiry rather than deleted on the spot —
    // they still hold the intermediate versions from the offline window.
    let fixture = small_fixture().await;
    let shared = Arc::clone(&fixture.shared);
    let src = source(&shared);

    // The signal fan-out is what folds a finished job back into the daemon's
    // state, so the reconnect bookkeeping only happens with it running — as it
    // always is in the real daemon.
    let (server, _client) = connect(Arc::clone(&shared)).await;
    let emitter = SignalEmitter::new(&server, PATH).unwrap();
    tokio::spawn(fan_out_signals(Arc::clone(&shared), emitter.to_owned()));

    let job = Daemon1::new(Arc::clone(&shared))
        .backup_now()
        .await
        .expect("backup starts");
    wait_for_job(&shared, job).await;
    cut_the_destination(&shared);

    // Two offline hours, each editing something.
    for name in ["docs/a.txt", "docs/b.txt"] {
        std::fs::write(src.join(name), format!("edited {name}")).unwrap();
        let job = shared
            .start_scheduled_backup()
            .await
            .expect("not an error")
            .expect("local protection runs");
        wait_for_job(&shared, job).await;
    }

    let spool = shared.spool_engine().await.expect("spool engine");
    assert_eq!(
        spool.list_archives().await.unwrap().len(),
        2,
        "two local snapshots were taken"
    );
    // Nothing may expire yet: these are the only copy of what they hold.
    let held = local_rows(&shared).await;
    assert_eq!(held.len(), 2);
    assert!(
        held.iter().all(|row| row.expirable_at.is_none()),
        "nothing expires while it is the only copy"
    );

    restore_the_destination(&shared);

    // The reconnect path, as the destination watcher would drive it.
    let before = engine(&shared).list_archives().await.unwrap().len();
    shared.destination_changed(true).await;
    // Wait for the catch-up backup, and for the bookkeeping that follows it.
    let mut marked = Vec::new();
    for _ in 0..300 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        marked = local_rows(&shared).await;
        if !marked.is_empty() && marked.iter().all(|row| row.expirable_at.is_some()) {
            break;
        }
    }

    let after = engine(&shared).list_archives().await.unwrap();
    assert!(
        after.len() > before,
        "reconnecting must take a backup to the real destination at once, \
         had {before} archives and now has {}",
        after.len()
    );
    assert_eq!(marked.len(), 2, "the local snapshots are still there");
    assert!(
        marked.iter().all(|row| row.expirable_at.is_some()),
        "and are now dated for expiry rather than deleted"
    );
}

#[tokio::test]
async fn local_snapshots_are_removed_once_their_time_is_up() {
    // The time-travel half. Rather than waiting 30 days, the marking is moved
    // into the past and the sweep is asked to run at "now".
    let fixture = small_fixture().await;
    let shared = Arc::clone(&fixture.shared);
    let src = source(&shared);

    let job = Daemon1::new(Arc::clone(&shared))
        .backup_now()
        .await
        .expect("backup starts");
    wait_for_job(&shared, job).await;
    cut_the_destination(&shared);

    std::fs::write(src.join("docs/a.txt"), b"edited while away").unwrap();
    let job = shared
        .start_scheduled_backup()
        .await
        .expect("not an error")
        .expect("local protection runs");
    wait_for_job(&shared, job).await;
    restore_the_destination(&shared);

    let long_ago =
        SystemTime::now() - crate::offline::EXPIRE_AFTER_CATCH_UP - Duration::from_secs(1);
    let at = backtrack_core::state::to_epoch(Some(long_ago)).unwrap() as i64;
    let index = shared.index().unwrap();
    let marked = tokio::task::spawn_blocking(move || index.lock().unwrap().mark_expirable(at))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        marked, 1,
        "the local snapshot is dated as if the destination caught up a month ago"
    );

    shared
        .expire_local_snapshots(SystemTime::now())
        .await
        .expect("the sweep runs");

    let spool = shared.spool_engine().await.expect("spool engine");
    assert!(
        spool.list_archives().await.unwrap().is_empty(),
        "the expired snapshot is gone from the spool repository"
    );
    assert!(
        local_rows(&shared).await.is_empty(),
        "and from the catalogue"
    );
    // The real backup is untouched: expiry removes local copies, never the
    // repository the user is actually protected by.
    assert!(!engine(&shared).list_archives().await.unwrap().is_empty());
}

#[tokio::test]
async fn an_unmarked_local_snapshot_is_never_swept_away() {
    // The rule that stops expiry losing data: while the destination has not
    // caught up, the local snapshot is the only copy of what it holds.
    let fixture = small_fixture().await;
    let shared = Arc::clone(&fixture.shared);
    let src = source(&shared);

    let job = Daemon1::new(Arc::clone(&shared))
        .backup_now()
        .await
        .expect("backup starts");
    wait_for_job(&shared, job).await;
    cut_the_destination(&shared);

    std::fs::write(src.join("docs/a.txt"), b"only copy").unwrap();
    let job = shared
        .start_scheduled_backup()
        .await
        .expect("not an error")
        .expect("local protection runs");
    wait_for_job(&shared, job).await;

    // A sweep far in the future still takes nothing, because nothing is marked.
    shared
        .expire_local_snapshots(SystemTime::now() + Duration::from_secs(365 * 24 * 3_600))
        .await
        .expect("the sweep runs");

    assert_eq!(
        local_rows(&shared).await.len(),
        1,
        "an unmarked snapshot survives any amount of time passing"
    );
    restore_the_destination(&shared);
}

/// The engine the fixture installed. Reaching into the private field is fine
/// here: this module is a child of `service`.
fn engine(shared: &Arc<Shared>) -> Arc<dyn backtrack_core::engine::BackupEngine> {
    shared.engine.lock().unwrap().clone().expect("engine set")
}

/// The locally-held snapshot rows, read without taking the catalogue lock on
/// the runtime thread.
///
/// The trap S04-T5 found, from the other side: an ingest in flight holds that
/// lock from a blocking thread while waiting for the next batch from its async
/// half. Blocking a `#[tokio::test]`'s single worker on it means the async half
/// can never run, and the whole thing wedges.
async fn local_rows(shared: &Arc<Shared>) -> Vec<backtrack_core::index::LocalArchiveRow> {
    let index = shared.index().unwrap();
    tokio::task::spawn_blocking(move || index.lock().unwrap().local_archives().unwrap())
        .await
        .unwrap()
}

/// Block until a job reaches a terminal state.
async fn wait_for_job(shared: &Arc<Shared>, job: u64) {
    for _ in 0..1200 {
        let state = shared.jobs.snapshot(job).expect("job exists").state;
        if state.is_terminal() {
            assert!(
                !matches!(state, crate::jobs::JobState::Failed(_)),
                "job {job} failed: {state:?}"
            );
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("job {job} never finished");
}

/// The name of the archive the backup just wrote.
async fn newest_archive(shared: &Arc<Shared>) -> String {
    let info = engine(shared).repo_info().await.expect("repo info");
    assert!(info.archive_count > 0, "the backup produced an archive");
    shared
        .last_archive
        .lock()
        .unwrap()
        .clone()
        .expect("a backup recorded its archive name")
}

/// The archive-relative path of `name` within `archive`.
async fn archived_path(shared: &Arc<Shared>, archive: &str, name: &str) -> String {
    use backtrack_core::engine::ArchiveId;
    let mut items = engine(shared)
        .list_archive(&ArchiveId(archive.to_string()))
        .await
        .expect("listing");
    while let Some(item) = items.next().await {
        let item = item.expect("item parses");
        if item.path.ends_with(name) {
            return item.path;
        }
    }
    panic!("{name} is not in {archive}");
}

/// A restore, end to end through the interface: work it out, look at what it
/// would do, carry it out, then put it back.
#[tokio::test]
async fn a_client_works_out_a_restore_looks_at_it_carries_it_out_and_undoes_it() {
    let fixture = small_fixture().await;
    let shared = Arc::clone(&fixture.shared);
    let src = source(&shared);

    // A backup to restore from.
    let backup = Daemon1::new(Arc::clone(&shared))
        .backup_now()
        .await
        .expect("backup starts");
    wait_for_job(&shared, backup).await;
    let archive = shared
        .last_archive
        .lock()
        .unwrap()
        .clone()
        .expect("the backup named its archive");

    // Now edit a backed-up file and add one the backup has never seen, which
    // is the situation the whole conflict design exists for.
    let edited = src.join("docs/a.txt");
    std::fs::write(&edited, b"edited after the backup was taken").unwrap();
    std::fs::write(src.join("docs/mine.txt"), b"never backed up").unwrap();

    let (server, client) = connect(Arc::clone(&shared)).await;
    let emitter = SignalEmitter::new(&server, PATH).unwrap();
    tokio::spawn(fan_out_signals(Arc::clone(&shared), emitter.to_owned()));

    // Restore the source tree back over itself, which is what "restore this
    // folder" means: the member path is the tree, the destination is `/`.
    let member = src.strip_prefix("/").unwrap().to_string_lossy().to_string();

    // ── Work it out ──
    let reply = client
        .call_method(
            None::<()>,
            PATH,
            Some(IFACE),
            "PrepareRestore",
            &(archive.as_str(), vec![member], "/"),
        )
        .await
        .expect("PrepareRestore accepted");
    let job: u64 = reply.body().deserialize().expect("a job id");
    wait_for_job(&shared, job).await;

    // ── Look at it ──
    let reply = client
        .call_method(None::<()>, PATH, Some(IFACE), "GetRestorePreview", &(job,))
        .await
        .expect("GetRestorePreview");
    let preview: backtrack_core::dbus::RestorePreview =
        reply.body().deserialize().expect("a preview");

    assert_eq!(preview.conflicts, 1, "the edited file, and only it");
    assert_eq!(preview.disk_newer, 1, "and the disk copy is the newer one");
    assert!(
        preview.only_on_disk >= 1,
        "the file the backup has never seen is counted as kept: {preview:?}"
    );
    assert!(
        preview.entries.iter().any(|e| e.path.ends_with("a.txt")),
        "the conflicting file belongs in the review list: {:?}",
        preview.entries
    );
    assert_eq!(
        std::fs::read_to_string(&edited).unwrap(),
        "edited after the backup was taken",
        "working out a restore must not touch anything"
    );

    // ── Carry it out ──
    let reply = client
        .call_method(
            None::<()>,
            PATH,
            Some(IFACE),
            "ExecuteRestore",
            &(job, "replace", Vec::<(String, String)>::new()),
        )
        .await
        .expect("ExecuteRestore accepted");
    let run: u64 = reply.body().deserialize().expect("a job id");
    wait_for_job(&shared, run).await;

    assert_eq!(
        std::fs::read_to_string(&edited).unwrap(),
        "original",
        "the backed-up version should be back"
    );
    assert_eq!(
        std::fs::read_to_string(src.join("docs/mine.txt")).unwrap(),
        "never backed up",
        "a restore merges; it never deletes"
    );

    // ── Put it back ──
    let reply = client
        .call_method(None::<()>, PATH, Some(IFACE), "UndoRestore", &(job,))
        .await
        .expect("UndoRestore accepted");
    let undo: u64 = reply.body().deserialize().expect("a job id");
    wait_for_job(&shared, undo).await;

    assert_eq!(
        std::fs::read_to_string(&edited).unwrap(),
        "edited after the backup was taken",
        "undo should bring back the edit the restore replaced"
    );
}

/// `PathsOnDisk` over the real interface, covering all three answers.
///
/// The three-valued part is the point. A caller that treated "I could not read
/// that folder" as "the file is gone" would paint a deletion badge across every
/// file in an archive taken on another machine, so the unknown case is tested
/// as deliberately as the other two.
#[tokio::test]
async fn a_client_can_ask_which_catalogued_paths_are_still_on_this_computer() {
    let fixture = small_fixture().await;
    let shared = Arc::clone(&fixture.shared);
    let src = source(&shared);

    let backup = Daemon1::new(Arc::clone(&shared))
        .backup_now()
        .await
        .expect("backup starts");
    wait_for_job(&shared, backup).await;

    // One file survives, one is deleted after the backup — the case the file
    // pane has no answer for, since the catalogue still has it in the newest
    // archive.
    let kept = src.join("docs/a.txt");
    let deleted = src.join("docs/b.txt");
    assert!(kept.exists() && deleted.exists(), "the fixture has both");
    std::fs::remove_file(&deleted).unwrap();

    let member = |path: &std::path::Path| {
        path.strip_prefix("/")
            .unwrap()
            .to_string_lossy()
            .to_string()
    };
    let asked = vec![
        member(&kept),
        member(&deleted),
        // A folder this machine does not have, as an archive from another
        // computer would describe.
        "somewhere/else/entirely/report.odt".to_string(),
    ];

    let (_server, client) = connect(Arc::clone(&shared)).await;
    let reply = client
        .call_method(None::<()>, PATH, Some(IFACE), "PathsOnDisk", &(asked,))
        .await
        .expect("PathsOnDisk accepted");
    let answers: Vec<u8> = reply.body().deserialize().expect("a byte per path");

    assert_eq!(
        answers,
        vec![2, 1, 0],
        "present, absent, unknown — in the order they were asked",
    );
}

/// Charlie's story through the interface: find a file that is gone.
///
/// Three things at once, because they only mean anything together — the search
/// reaches back into snapshots the file no longer appears in, what is gone from
/// the computer is ranked above what is still on it, and the tag that says so
/// is a fact about the disk rather than about the newest backup.
#[tokio::test]
async fn search_puts_what_is_gone_from_the_computer_first() {
    let fixture = small_fixture().await;
    let shared = Arc::clone(&fixture.shared);
    let src = source(&shared);

    // Two files that will both match "note", one of which does not survive.
    let docs = src.join("docs");
    std::fs::write(docs.join("keeper-note.txt"), b"kept").unwrap();
    std::fs::write(docs.join("lost-note.txt"), b"lost").unwrap();

    let backup = Daemon1::new(Arc::clone(&shared))
        .backup_now()
        .await
        .expect("backup starts");
    wait_for_job(&shared, backup).await;

    // Deleted after the backup, so the catalogue still has it in its newest
    // archive: the case the old `exists_today` could not see.
    std::fs::remove_file(docs.join("lost-note.txt")).unwrap();

    let (_server, client) = connect(Arc::clone(&shared)).await;
    let reply = client
        .call_method(None::<()>, PATH, Some(IFACE), "SearchFiles", &("note",))
        .await
        .expect("SearchFiles accepted");
    let hits: Vec<backtrack_core::dbus::SearchResult> =
        reply.body().deserialize().expect("search results");

    let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
    assert!(
        names.contains(&"lost-note.txt") && names.contains(&"keeper-note.txt"),
        "both files match the query: {names:?}",
    );
    assert_eq!(
        names.first(),
        Some(&"lost-note.txt"),
        "the one that is gone from the computer ranks first: {names:?}",
    );

    let gone = |name: &str| {
        hits.iter()
            .find(|h| h.name == name)
            .expect("the hit is present")
            .gone_from_disk
    };
    assert!(gone("lost-note.txt"), "deleted from the computer");
    assert!(
        !gone("keeper-note.txt"),
        "still on the computer, though both are in the same newest archive",
    );
}

/// The two limits the daemon keeps because a client cannot be relied on to.
#[tokio::test]
async fn search_refuses_a_one_character_query_and_caps_what_it_returns() {
    let fixture = small_fixture().await;
    let shared = Arc::clone(&fixture.shared);
    let src = source(&shared);

    // Comfortably more matches than the cap allows through.
    let many = src.join("many");
    std::fs::create_dir_all(&many).unwrap();
    for i in 0..(SEARCH_LIMIT + 50) {
        std::fs::write(many.join(format!("plentiful-{i}.txt")), b"x").unwrap();
    }

    let backup = Daemon1::new(Arc::clone(&shared))
        .backup_now()
        .await
        .expect("backup starts");
    wait_for_job(&shared, backup).await;

    let (_server, client) = connect(Arc::clone(&shared)).await;

    let capped = client
        .call_method(
            None::<()>,
            PATH,
            Some(IFACE),
            "SearchFiles",
            &("plentiful",),
        )
        .await
        .expect("SearchFiles accepted");
    let hits: Vec<backtrack_core::dbus::SearchResult> =
        capped.body().deserialize().expect("search results");
    assert_eq!(hits.len(), SEARCH_LIMIT, "capped, not merely large");

    // A one-character query is refused rather than answered expensively.
    let refused = client
        .call_method(None::<()>, PATH, Some(IFACE), "SearchFiles", &("p",))
        .await;
    assert!(refused.is_err(), "a one-character query is refused");

    // Two characters is the floor, and whitespace does not pad it out.
    let padded = client
        .call_method(None::<()>, PATH, Some(IFACE), "SearchFiles", &("  p  ",))
        .await;
    assert!(padded.is_err(), "trimmed before it is measured");
}
