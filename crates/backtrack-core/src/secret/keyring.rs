// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>

//! Secret Service (system keyring) passphrase store via `oo7`. Not exercised in
//! headless CI (no Secret Service); the file store is the tested path. The
//! attribute set (`app-id` + `repo-id`) is stable so entries survive upgrades.

use std::collections::HashMap;

use async_trait::async_trait;

use crate::engine::{EngineError, Result};
use crate::secret::{missing, SecretStore, APP_ID};

/// A passphrase store backed by the org.freedesktop.Secret service.
pub struct KeyringSecretStore;

impl KeyringSecretStore {
    pub fn new() -> KeyringSecretStore {
        KeyringSecretStore
    }
}

impl Default for KeyringSecretStore {
    fn default() -> Self {
        KeyringSecretStore::new()
    }
}

fn attrs(repo_id: &str) -> HashMap<&str, &str> {
    HashMap::from([("app-id", APP_ID), ("repo-id", repo_id)])
}

fn svc_err(e: impl std::fmt::Display) -> EngineError {
    EngineError::BorgFailed {
        code: -1,
        stderr: format!("keyring: {e}"),
    }
}

#[async_trait]
impl SecretStore for KeyringSecretStore {
    async fn get(&self, repo_id: &str) -> Result<String> {
        let keyring = oo7::Keyring::new().await.map_err(svc_err)?;
        let attributes = attrs(repo_id);
        let items = keyring.search_items(&attributes).await.map_err(svc_err)?;
        let item = items.into_iter().next().ok_or_else(missing)?;
        let secret = item.secret().await.map_err(svc_err)?;
        String::from_utf8(secret.to_vec()).map_err(svc_err)
    }

    async fn set(&self, repo_id: &str, passphrase: &str) -> Result<()> {
        let keyring = oo7::Keyring::new().await.map_err(svc_err)?;
        let attributes = attrs(repo_id);
        keyring
            .create_item(
                "Backtrack backup passphrase",
                &attributes,
                passphrase.as_bytes(),
                true, // replace
            )
            .await
            .map_err(svc_err)?;
        Ok(())
    }

    async fn delete(&self, repo_id: &str) -> Result<()> {
        let keyring = oo7::Keyring::new().await.map_err(svc_err)?;
        let attributes = attrs(repo_id);
        keyring.delete(&attributes).await.map_err(svc_err)?;
        Ok(())
    }
}
