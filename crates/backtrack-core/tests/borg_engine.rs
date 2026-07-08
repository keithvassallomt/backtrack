// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>

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
    let mut outcome = Ok(());
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
            one_file_system: false,
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
