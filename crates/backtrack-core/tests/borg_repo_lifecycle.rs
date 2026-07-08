// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>

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
