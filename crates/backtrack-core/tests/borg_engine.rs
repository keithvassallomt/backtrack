// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Real-borg integration tests. Run via `just test-integration`
//! (`cargo test --features integration`); skipped otherwise.
#![cfg(feature = "integration")]

use std::sync::Arc;

use backtrack_core::engine::{
    ArchiveId, BackupEngine, BorgCli, CheckLevel, Compression, CreateSpec, Encryption, EngineError,
    JobEvent, PrunePolicy, RepoSpec,
};
use backtrack_core::secret::{FileSecretStore, SecretStore};
use futures::StreamExt;
use tokio::io::AsyncReadExt;

const PASS: &str = "integration-passphrase";

/// A fixture: temp dir with a repo path, a source tree, and a store holding the
/// passphrase under `repo_id = "test"`.
struct Fixture {
    _dir: tempfile::TempDir,
    repo: String,
    src: std::path::PathBuf,
    secrets: Arc<dyn SecretStore>,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo").to_str().unwrap().to_string();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("hello.txt"), b"hello world").unwrap();
    std::fs::write(src.join("data.bin"), vec![7u8; 1024]).unwrap();

    let store = FileSecretStore::new(dir.path().join("secrets.json"));
    store.set("test", PASS).await.unwrap();
    Fixture {
        _dir: dir,
        repo,
        src,
        secrets: Arc::new(store),
    }
}

async fn engine(f: &Fixture) -> BorgCli {
    BorgCli::new(f.repo.clone(), "test".into(), f.secrets.clone())
        .await
        .expect("borg >= 1.2 available")
}

async fn run_to_finish(mut s: backtrack_core::engine::JobStream) -> Result<(), EngineError> {
    // Default to an error: a job that ends without ever emitting `Finished`
    // (e.g. a plumbing bug that drops the terminal event) must fail the test
    // loudly, not silently pass.
    let mut outcome = Err(EngineError::BorgFailed {
        code: -1,
        stderr: "job stream ended without a Finished event".into(),
    });
    while let Some(ev) = s.next().await {
        if let JobEvent::Finished(r) = ev {
            outcome = r.map(|_| ());
        }
    }
    outcome
}

#[tokio::test]
async fn full_round_trip() {
    let f = fixture().await;
    let eng = engine(&f).await;

    // init
    eng.init_repo(&RepoSpec {
        path: f.repo.clone(),
        encryption: Encryption::RepokeyBlake2,
    })
    .await
    .unwrap();

    // two archives
    for name in ["arch-1", "arch-2"] {
        let spec = CreateSpec {
            archive_name: name.into(),
            sources: vec![f.src.clone()],
            excludes: vec![],
            compression: Compression::Zstd,
            upload_limit_kib: None,
            one_file_system: false,
            created_at: std::time::SystemTime::now(),
            paths: Vec::new(),
        };
        run_to_finish(eng.create(&spec).await.unwrap())
            .await
            .unwrap();
    }

    // list streams the fixture files
    let items: Vec<_> = eng
        .list_archive(&ArchiveId("arch-1".into()))
        .await
        .unwrap()
        .collect()
        .await;
    let paths: Vec<String> = items.into_iter().map(|i| i.unwrap().path).collect();
    assert!(paths.iter().any(|p| p.ends_with("hello.txt")));
    assert!(paths.iter().any(|p| p.ends_with("data.bin")));

    // extract_stdout matches source bytes. Borg stores archive member paths
    // without a leading `/` (a source of `/tmp/xxx/src/hello.txt` is stored as
    // `tmp/xxx/src/hello.txt`), so strip it from the absolute source path
    // rather than guessing — and assert it's exactly what `list` reported.
    let rel = f
        .src
        .join("hello.txt")
        .strip_prefix("/")
        .unwrap()
        .display()
        .to_string();
    assert!(
        paths.contains(&rel),
        "expected list to report {rel:?}, got {paths:?}"
    );
    let mut reader = eng
        .extract_stdout(&ArchiveId("arch-1".into()), &rel)
        .await
        .unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"hello world");

    // prune keeps at least one, compact runs
    run_to_finish(
        eng.prune(&PrunePolicy {
            keep_hourly: 0,
            keep_daily: 0,
            keep_weekly: 0,
            keep_monthly: 1,
        })
        .await
        .unwrap(),
    )
    .await
    .unwrap();
    run_to_finish(eng.compact().await.unwrap()).await.unwrap();
    run_to_finish(eng.check(CheckLevel::Repository).await.unwrap())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_file_that_disappears_mid_backup_still_produces_a_successful_backup() {
    // The race this stands in for is real and common: a backup reads a file
    // that a browser, an editor, or a download has moved or deleted since the
    // scan. Borg writes the archive with everything that still exists and exits
    // 107 — non-zero, but a *warning*, not a failure.
    //
    // Reproduced deterministically by naming a source that is not there, which
    // is the same condition borg hits when a file vanishes under it. Racing a
    // real deletion would need gigabytes and a coin toss.
    //
    // Before this was fixed, the whole backup was reported as failed, no
    // successful backup was recorded, and the machine drifted towards a health
    // banner while holding a perfectly good archive.
    let f = fixture().await;
    let eng = engine(&f).await;
    eng.init_repo(&RepoSpec {
        path: f.repo.clone(),
        encryption: Encryption::RepokeyBlake2,
    })
    .await
    .unwrap();

    let spec = CreateSpec {
        archive_name: "with-a-gap".into(),
        sources: vec![f.src.join("hello.txt"), f.src.join("never-existed.txt")],
        excludes: vec![],
        compression: Compression::Zstd,
        upload_limit_kib: None,
        one_file_system: false,
        created_at: std::time::SystemTime::now(),
        paths: Vec::new(),
    };
    run_to_finish(eng.create(&spec).await.unwrap())
        .await
        .expect("a vanished file is a warning, not a failed backup");

    // And the point of insisting on that: the archive really is there, holding
    // the file that did exist.
    let paths: Vec<String> = eng
        .list_archive(&ArchiveId("with-a-gap".into()))
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(|i| i.unwrap().path)
        .collect();
    assert!(
        paths.iter().any(|p| p.ends_with("hello.txt")),
        "the surviving file was archived: {paths:?}"
    );
}

#[tokio::test]
async fn wrong_passphrase_yields_passphrase_wrong() {
    let f = fixture().await;
    let eng = engine(&f).await;
    eng.init_repo(&RepoSpec {
        path: f.repo.clone(),
        encryption: Encryption::RepokeyBlake2,
    })
    .await
    .unwrap();

    // Overwrite the stored passphrase with the wrong one.
    f.secrets.set("test", "not-the-passphrase").await.unwrap();
    let err = eng.repo_info().await.unwrap_err();
    assert_eq!(err, EngineError::PassphraseWrong);
}

#[tokio::test]
async fn unreachable_path_yields_repo_unreachable() {
    let f = fixture().await;
    // Point at a repo that does not exist; never initialised.
    let eng = BorgCli::new(
        format!("{}/does-not-exist", f.repo),
        "test".into(),
        f.secrets.clone(),
    )
    .await
    .unwrap();
    let err = eng.repo_info().await.unwrap_err();
    assert_eq!(err, EngineError::RepoUnreachable);
}

/// The modification time Borg reports has to be the one the file actually has.
///
/// Borg renders timestamps through Python's local-time conversion and writes
/// them with no offset attached, so a listing read as UTC is the machine's own
/// offset away from the truth. The adapter avoids that by asking for
/// `{mtime:%s.%f}` rather than the JSON, and every comment saying so is a claim
/// about what comes back from a real Borg — which only a real Borg can settle.
/// This compares the listed time against what the filesystem says, in seconds,
/// with nothing in between that could absorb the error.
///
/// It can only fail where the machine is not already on UTC, so CI in UTC will
/// pass it either way. That is worth having anyway: it costs nothing there, and
/// it fails immediately on any developer machine with a real timezone the day
/// somebody swaps the listing back to `--json-lines`.
#[tokio::test]
async fn a_listed_mtime_is_the_one_the_file_has() {
    let f = fixture().await;
    let eng = engine(&f).await;
    eng.init_repo(&RepoSpec {
        path: f.repo.clone(),
        encryption: Encryption::RepokeyBlake2,
    })
    .await
    .unwrap();

    // A time with a deliberate offset from now, so a pass cannot come from the
    // file happening to have been written this second.
    let hello = f.src.join("hello.txt");
    let when = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_789_376_200);
    std::fs::File::options()
        .read(true)
        .open(&hello)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(when))
        .unwrap();

    run_to_finish(
        eng.create(&CreateSpec {
            archive_name: "arch-1".into(),
            sources: vec![f.src.clone()],
            excludes: vec![],
            compression: Compression::Zstd,
            upload_limit_kib: None,
            one_file_system: false,
            created_at: std::time::SystemTime::now(),
            paths: Vec::new(),
        })
        .await
        .unwrap(),
    )
    .await
    .unwrap();

    let rel = hello.strip_prefix("/").unwrap().display().to_string();
    let items: Vec<_> = eng
        .list_archive(&ArchiveId("arch-1".into()))
        .await
        .unwrap()
        .collect()
        .await;
    let listed = items
        .into_iter()
        .map(|i| i.unwrap())
        .find(|i| i.path == rel)
        .expect("hello.txt is in the listing");

    assert_eq!(
        listed.mtime / 1_000_000,
        1_789_376_200,
        "the listed mtime is the file's own, not its local rendering read as UTC",
    );
}
