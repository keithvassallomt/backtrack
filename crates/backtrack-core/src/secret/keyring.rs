// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Secret Service (system keyring) passphrase store via `oo7`. Not exercised in
//! headless CI (no Secret Service); the file store is the tested path. The
//! attribute set (`app-id` + `repo-id`) is stable so entries survive upgrades.
//!
//! The Secret Service in every build, the Flatpak included (S14-T6).
//! `oo7::Keyring::new()` changes backend inside a sandbox, to a file under
//! `~/.var/app` unlocked through the Secret portal; asking the Secret Service
//! directly keeps a remembered passphrase where GNOME Keyring or KWallet shows
//! it, whichever way Backtrack was installed. The Flatpak reaches it with
//! `--talk-name=org.freedesktop.secrets`.

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

/// The user's default collection, over the Secret Service.
async fn keyring() -> Result<oo7::Keyring> {
    let service = oo7::dbus::Service::new().await.map_err(svc_err)?;
    let collection = service.default_collection().await.map_err(svc_err)?;
    Ok(oo7::Keyring::DBus(collection))
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
        let keyring = keyring().await?;
        let attributes = attrs(repo_id);
        let items = keyring.search_items(&attributes).await.map_err(svc_err)?;
        let item = items.into_iter().next().ok_or_else(missing)?;
        let secret = item.secret().await.map_err(svc_err)?;
        String::from_utf8(secret.to_vec()).map_err(svc_err)
    }

    async fn set(&self, repo_id: &str, passphrase: &str) -> Result<()> {
        let keyring = keyring().await?;
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
        let keyring = keyring().await?;
        let attributes = attrs(repo_id);
        keyring.delete(&attributes).await.map_err(svc_err)?;
        Ok(())
    }
}
