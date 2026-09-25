// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! S02-T4: repo init / key export / import (open + verify passphrase).
#![cfg(feature = "integration")]

use std::sync::Arc;

use backtrack_core::engine::{BackupEngine, BorgCli, Encryption, EngineError, RepoSpec};
use backtrack_core::secret::{FileSecretStore, SecretStore};

const PASS: &str = "lifecycle-pass";

async fn store(dir: &std::path::Path) -> Arc<dyn SecretStore> {
    let s = FileSecretStore::new(dir.join("secrets.json"));
    s.set("test", PASS).await.unwrap();
    Arc::new(s)
}

#[tokio::test]
async fn init_export_then_import_right_and_wrong() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo").to_str().unwrap().to_string();
    let secrets = store(dir.path()).await;

    // init
    let eng = BorgCli::new(repo.clone(), "test".into(), secrets.clone())
        .await
        .unwrap();
    eng.init_repo(&RepoSpec {
        path: repo.clone(),
        encryption: Encryption::RepokeyBlake2,
    })
    .await
    .unwrap();

    // key_export is non-empty text; real borg output starts with the
    // `BORG_KEY <repo-id-hex>` marker line, so check that too for a stronger
    // guarantee that this is a genuine recovery key, not just whitespace.
    let key = eng.key_export().await.unwrap();
    assert!(
        key.contains("BORG") || !key.trim().is_empty(),
        "recovery key should be text"
    );
    assert!(
        key.starts_with("BORG_KEY "),
        "expected a BORG_KEY marker header, got: {:?}",
        key.lines().next()
    );

    // import (a fresh engine over the same repo) with the right passphrase: repo_info works
    let eng2 = BorgCli::new(repo.clone(), "test".into(), secrets.clone())
        .await
        .unwrap();
    let info = eng2.repo_info().await.unwrap();
    assert!(!info.repository_id.is_empty());

    // import with the wrong passphrase
    secrets.set("test", "wrong").await.unwrap();
    let eng3 = BorgCli::new(repo.clone(), "test".into(), secrets.clone())
        .await
        .unwrap();
    assert_eq!(
        eng3.repo_info().await.unwrap_err(),
        EngineError::PassphraseWrong
    );
}

/// S09-T1: what the wizard learns about a destination before creating anything
/// there, against real Borg for each answer it can give.
#[tokio::test]
async fn presence_tells_the_wizard_what_is_already_there() {
    use backtrack_core::engine::Presence;

    let dir = tempfile::tempdir().unwrap();
    let secrets = store(dir.path()).await;
    let presence = |path: &std::path::Path| {
        let secrets = Arc::clone(&secrets);
        let path = path.to_str().unwrap().to_string();
        async move {
            BorgCli::new(path.clone(), "test".into(), secrets)
                .await
                .unwrap()
                .presence()
                .await
        }
    };

    // A folder on a drive that has never seen Backtrack: the repository's own
    // folder, and its parent, are both still to be made.
    let fresh = dir.path().join("drive/Backtrack/host");
    assert_eq!(presence(&fresh).await, Ok(Presence::Empty));

    // An empty folder is somewhere a repository can go, whatever Borg calls it.
    let empty = dir.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    assert_eq!(presence(&empty).await, Ok(Presence::Empty));

    // A folder holding something else is not written into.
    let occupied = dir.path().join("occupied");
    std::fs::create_dir(&occupied).unwrap();
    std::fs::write(occupied.join("holiday.jpg"), b"not a repository").unwrap();
    assert_eq!(presence(&occupied).await, Ok(Presence::Occupied));

    // An encrypted repository refuses the absent passphrase, which is itself
    // the answer: somebody's backups are here.
    let eng = BorgCli::new(
        fresh.to_str().unwrap().into(),
        "test".into(),
        Arc::clone(&secrets),
    )
    .await
    .unwrap();
    eng.init_repo(&RepoSpec {
        path: fresh.to_str().unwrap().into(),
        encryption: Encryption::RepokeyBlake2,
    })
    .await
    .expect("init creates the missing parent folders");
    assert_eq!(presence(&fresh).await, Ok(Presence::Existing));

    // A server that is not answering is an error, not an empty destination.
    let unreachable = BorgCli::new(
        "ssh://nobody@127.0.0.1:1/./repo".into(),
        "test".into(),
        Arc::clone(&secrets),
    )
    .await
    .unwrap()
    .presence()
    .await;
    assert_eq!(unreachable, Err(EngineError::RepoUnreachable));
}

/// A folder the user may not write into is refused before it is chosen, rather
/// than when the repository is created two pages later.
#[tokio::test]
async fn presence_refuses_a_folder_that_cannot_be_written() {
    use backtrack_core::engine::Presence;
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    if rustix::fs::access(&locked, rustix::fs::Access::WRITE_OK).is_ok() {
        // Running as root, where permissions do not bind; nothing to test.
        return;
    }
    let secrets = store(dir.path()).await;
    let target = locked.join("Backtrack/host");
    let presence = BorgCli::new(target.to_str().unwrap().into(), "test".into(), secrets)
        .await
        .unwrap()
        .presence()
        .await;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(presence, Ok(Presence::Unwritable));
}
