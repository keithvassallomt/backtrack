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
use std::time::Duration;

use backtrack_core::config::Config;
use backtrack_core::engine::{BackupEngine, BorgCli, Encryption, RepoSpec};
use backtrack_core::secret::{FileSecretStore, SecretStore};
use futures::StreamExt;
use zbus::object_server::SignalEmitter;

use crate::jobs::JobRegistry;
use crate::service::{fan_out_signals, Daemon1, Shared};

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

/// The engine the fixture installed. Reaching into the private field is fine
/// here: this module is a child of `service`.
fn engine(shared: &Arc<Shared>) -> Arc<dyn backtrack_core::engine::BackupEngine> {
    shared.engine.lock().unwrap().clone().expect("engine set")
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
