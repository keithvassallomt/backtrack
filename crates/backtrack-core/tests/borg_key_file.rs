// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! S19-T16: backups whose key is kept in a file of its own (Borg's key-file
//! encryption, which Vorta offers), made on another computer, are opened here
//! once that file is given.
//!
//! A test binary of its own: it points this process's Borg at a keys folder of
//! its own, so that the key it puts in place lands there rather than in the
//! keys folder of whoever runs the tests.
#![cfg(feature = "integration")]

use std::path::Path;
use std::sync::Arc;

use backtrack_core::engine::{BackupEngine, BorgCli, EngineError, Presence};
use backtrack_core::secret::{FileSecretStore, SecretStore, NO_PASSPHRASE};

/// Make a key-file repository at `repo` with another computer's Borg, whose
/// folders are under `elsewhere`, and return the key that computer kept.
fn made_elsewhere(elsewhere: &Path, repo: &Path, passphrase: &str) -> String {
    let keys = elsewhere.join("keys");
    let made = std::process::Command::new("borg")
        .env("BORG_BASE_DIR", elsewhere)
        .env("BORG_KEYS_DIR", &keys)
        .env("BORG_NEW_PASSPHRASE", passphrase)
        .arg("init")
        .arg("--encryption=keyfile-blake2")
        .arg(repo)
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    // The file Borg keeps in its keys folder, as a person would copy it from
    // the computer that made the backups.
    let mut kept = std::fs::read_dir(&keys).unwrap();
    let file = kept.next().unwrap().unwrap().path();
    assert!(kept.next().is_none(), "one key for one repository");
    let key = std::fs::read_to_string(&file).unwrap();
    std::fs::remove_file(&file).unwrap();
    key
}

#[tokio::test]
async fn backups_whose_key_is_in_a_file_open_once_it_is_given() {
    let dir = tempfile::tempdir().unwrap();
    let here = dir.path().join("here-keys");
    std::env::set_var("BORG_KEYS_DIR", &here);
    let elsewhere = dir.path().join("elsewhere");
    let secrets: Arc<dyn SecretStore> = Arc::new(FileSecretStore::new(dir.path().join("s.json")));

    let repo = dir.path().join("vorta");
    let key = made_elsewhere(&elsewhere, &repo, "a passphrase");
    let path = repo.to_str().unwrap().to_string();
    secrets.set(&path, "a passphrase").await.unwrap();
    let eng = BorgCli::new(path.clone(), path.clone(), Arc::clone(&secrets))
        .await
        .unwrap();

    // Without the key there is nothing to try the passphrase with.
    assert_eq!(eng.presence().await, Ok(Presence::NeedsKeyFile));
    assert!(eng.repo_info().await.is_err());

    // Another repository's key, or something that is no key at all, is
    // refused, and nothing is put in place.
    let other = dir.path().join("other");
    let other_key = made_elsewhere(&dir.path().join("other-computer"), &other, "x");
    assert_eq!(
        eng.key_import(&other_key).await.unwrap_err(),
        EngineError::KeyForAnotherRepository
    );
    assert_eq!(
        eng.key_import("Lentil soup").await.unwrap_err(),
        EngineError::NotARecoveryKey
    );
    assert_eq!(eng.presence().await, Ok(Presence::NeedsKeyFile));

    // The key itself: from then on the passphrase opens them, as any others.
    eng.key_import(&key).await.unwrap();
    assert_eq!(eng.presence().await, Ok(Presence::Existing));
    eng.repo_info()
        .await
        .unwrap_or_else(|e| panic!("the passphrase opens them now: {e:?}"));
    assert_eq!(
        std::fs::read_dir(&here).unwrap().count(),
        1,
        "the key is kept in this computer's keys folder"
    );

    // A key made with an empty passphrase opens with nothing typed.
    let empty = dir.path().join("empty");
    let key = made_elsewhere(&dir.path().join("third-computer"), &empty, "");
    let path = empty.to_str().unwrap().to_string();
    secrets.set(&path, NO_PASSPHRASE).await.unwrap();
    let eng = BorgCli::new(path.clone(), path, Arc::clone(&secrets))
        .await
        .unwrap();
    assert_eq!(eng.presence().await, Ok(Presence::NeedsKeyFile));
    eng.key_import(&key).await.unwrap();
    assert_eq!(eng.presence().await, Ok(Presence::Open));
    eng.repo_info().await.unwrap();
}
