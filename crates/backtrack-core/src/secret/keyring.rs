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
//!
//! A locked keyring (S10-T9). Reading is never what unlocks one: a backup
//! that finds it locked has no passphrase, which is health.md's "Backtrack
//! needs your backup passphrase again.", and the next one carries on by
//! itself once the keyring is open. A locked item is not asked for its
//! secret at all, since KWallet answers that with an empty one rather than an
//! error, and counts every such ask as a failed attempt it warns about.
//! Saving is something the person is doing, so a locked keyring is unlocked
//! for it, with the keyring's own prompt.

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

/// The passphrase a keyring item holds. An empty one is no passphrase:
/// Backtrack never stores one, and it is how KWallet answers for a locked
/// item.
fn passphrase(secret: Vec<u8>) -> Result<String> {
    if secret.is_empty() {
        return Err(missing());
    }
    String::from_utf8(secret).map_err(svc_err)
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
        if item.is_locked().await.map_err(svc_err)? {
            return Err(missing());
        }
        let secret = item.secret().await.map_err(svc_err)?;
        passphrase(secret.to_vec())
    }

    async fn set(&self, repo_id: &str, passphrase: &str) -> Result<()> {
        let keyring = keyring().await?;
        if keyring.is_locked().await.map_err(svc_err)? {
            keyring
                .unlock()
                .await
                .map_err(|_| EngineError::KeyringLocked)?;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_secret_is_no_passphrase() {
        // What KWallet hands back for a locked item.
        assert_eq!(passphrase(Vec::new()), Err(EngineError::PassphraseMissing));
        assert_eq!(passphrase(b"hunter2".to_vec()), Ok("hunter2".to_string()));
    }
}
