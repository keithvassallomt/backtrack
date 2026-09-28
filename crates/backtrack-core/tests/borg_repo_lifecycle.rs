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
    assert!(info.encrypted, "a repokey repository says it is encrypted");

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

/// S09-T4: Preferences → Security's Change Passphrase, and the figures the
/// Storage and Security pages show, against real Borg.
#[tokio::test]
async fn the_passphrase_changes_and_the_repository_describes_itself() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo").to_str().unwrap().to_string();
    let secrets = store(dir.path()).await;
    let eng = BorgCli::new(repo.clone(), "test".into(), secrets.clone())
        .await
        .unwrap();
    eng.init_repo(&RepoSpec {
        path: repo.clone(),
        encryption: Encryption::RepokeyBlake2,
    })
    .await
    .unwrap();

    let stats = eng.repo_stats().await.expect("borg info answers");
    assert_eq!(stats.encryption, "repokey-blake2");
    assert_eq!(stats.stored_bytes, 0, "nothing is backed up yet");

    eng.change_passphrase(PASS, "a completely new passphrase")
        .await
        .expect("borg accepts the change");
    assert_eq!(
        eng.repo_info().await.unwrap_err(),
        EngineError::PassphraseWrong,
        "the old passphrase no longer opens it"
    );
    secrets
        .set("test", "a completely new passphrase")
        .await
        .unwrap();
    assert!(eng.repo_info().await.is_ok(), "the new one does");

    // And back, which is how a half-finished change is undone.
    eng.change_passphrase("a completely new passphrase", PASS)
        .await
        .unwrap();
    secrets.set("test", PASS).await.unwrap();
    assert!(eng.repo_info().await.is_ok());
}

/// Run a job to its end and return how it finished.
async fn outcome(stream: backtrack_core::engine::JobStream) -> Result<(), EngineError> {
    use futures::StreamExt;
    let mut stream = stream;
    while let Some(event) = stream.next().await {
        if let backtrack_core::engine::JobEvent::Finished(result) = event {
            return result.map(|_| ());
        }
    }
    panic!("the job ended without saying how");
}

/// A repository holding one backup, and an engine over it.
async fn backed_up(dir: &std::path::Path) -> (String, BorgCli) {
    let repo = dir.join("repo").to_str().unwrap().to_string();
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    // Incompressible, so the archive has a data segment worth damaging.
    let block: Vec<u8> = (0..2_000_000u32)
        .map(|n| (n.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    std::fs::write(src.join("a.bin"), block).unwrap();
    let engine = BorgCli::new(repo.clone(), "test".into(), store(dir).await)
        .await
        .unwrap();
    engine
        .init_repo(&RepoSpec {
            path: repo.clone(),
            encryption: Encryption::RepokeyBlake2,
        })
        .await
        .unwrap();
    let spec = backtrack_core::engine::CreateSpec {
        archive_name: "one".into(),
        created_at: std::time::SystemTime::now(),
        sources: vec![src],
        excludes: Vec::new(),
        paths: Vec::new(),
        compression: backtrack_core::engine::Compression::None,
        upload_limit_kib: None,
        one_file_system: true,
    };
    outcome(engine.create(&spec).await.unwrap()).await.unwrap();
    (repo, engine)
}

/// Flip bytes in the middle of the largest data segment.
fn damage(repo: &str) {
    let segments = std::path::Path::new(repo).join("data/0");
    let largest = std::fs::read_dir(&segments)
        .unwrap()
        .flatten()
        .max_by_key(|entry| entry.metadata().map(|m| m.len()).unwrap_or(0))
        .unwrap()
        .path();
    let mut bytes = std::fs::read(&largest).unwrap();
    let middle = bytes.len() / 2;
    for byte in &mut bytes[middle..middle + 64] {
        *byte ^= 0xff;
    }
    std::fs::write(&largest, bytes).unwrap();
}

/// S10-T5: Borg reports damage by exiting in its *warning* band, and a check
/// that read that as success would call a corrupt repository healthy.
#[tokio::test]
async fn a_damaged_repository_fails_its_check_and_repair_brings_it_back() {
    use backtrack_core::engine::CheckLevel;

    let dir = tempfile::tempdir().unwrap();
    let (repo, engine) = backed_up(dir.path()).await;
    assert_eq!(
        outcome(engine.check(CheckLevel::Full).await.unwrap()).await,
        Ok(()),
        "an undamaged repository passes"
    );

    damage(&repo);
    assert_eq!(
        outcome(engine.check(CheckLevel::Full).await.unwrap()).await,
        Err(EngineError::RepoCorrupt)
    );

    outcome(engine.repair().await.unwrap())
        .await
        .expect("repair runs without a terminal to answer it");
    assert_eq!(
        outcome(engine.check(CheckLevel::Full).await.unwrap()).await,
        Ok(()),
        "repaired, the repository passes again"
    );
}

/// S10-T4: "Lost the passphrase? Use your recovery key…" puts the saved key
/// back, after which the passphrase it was saved with opens the repository.
#[tokio::test]
async fn a_saved_key_restores_the_passphrase_it_was_saved_with() {
    let dir = tempfile::tempdir().unwrap();
    let (_, engine) = backed_up(dir.path()).await;
    let saved = engine.key_export().await.unwrap();
    engine
        .change_passphrase(PASS, "changed-elsewhere")
        .await
        .unwrap();
    assert_eq!(
        engine.repo_info().await.unwrap_err(),
        EngineError::PassphraseWrong,
        "the key has changed under the stored passphrase"
    );

    engine.key_import(&saved).await.unwrap();
    engine
        .repo_info()
        .await
        .expect("the saved key opens with the passphrase it was saved with");

    // Refused, and said so, for a key from somewhere else or not a key at all.
    let other_dir = tempfile::tempdir().unwrap();
    let (_, other) = backed_up(other_dir.path()).await;
    assert_eq!(
        other.key_import(&saved).await.unwrap_err(),
        EngineError::KeyForAnotherRepository
    );
    assert_eq!(
        engine.key_import("this is not a key").await.unwrap_err(),
        EngineError::NotARecoveryKey
    );
}
