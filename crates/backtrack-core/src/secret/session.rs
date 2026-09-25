// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The "Remember passphrase" setting, as a store.
//!
//! With remembering on, a passphrase goes to the persistent store (the keyring)
//! and backups run unattended across restarts. With it off, the passphrase is
//! held in this process's memory only: backups keep running until the daemon
//! stops, and after that the passphrase has to be given again. That is what
//! the setting promises, and it is the only reading under which the wizard's
//! own first backup can run with the box unticked.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::engine::{EngineError, Result};
use crate::secret::SecretStore;

/// A passphrase store that writes through to a persistent one only while
/// remembering is switched on.
pub struct SessionSecretStore {
    persistent: Arc<dyn SecretStore>,
    held: Mutex<HashMap<String, String>>,
    remember: AtomicBool,
}

impl SessionSecretStore {
    pub fn new(persistent: Arc<dyn SecretStore>, remember: bool) -> SessionSecretStore {
        SessionSecretStore {
            persistent,
            held: Mutex::new(HashMap::new()),
            remember: AtomicBool::new(remember),
        }
    }

    /// Switch remembering on or off, moving `repo_id`'s passphrase, if there
    /// is a repository yet, to where the setting now says it lives.
    ///
    /// Turning it off keeps the passphrase in memory before deleting it from
    /// the keyring, so the change does not also stop the backups already
    /// running. Turning it on can only store what this process knows; a
    /// passphrase it was never given stays missing, which is the state the
    /// passphrase prompt exists to resolve.
    pub async fn set_remember(&self, remember: bool, repo_id: Option<&str>) -> Result<()> {
        self.remember.store(remember, Ordering::SeqCst);
        let Some(repo_id) = repo_id else {
            return Ok(());
        };
        if remember {
            let held = self.held.lock().unwrap().get(repo_id).cloned();
            if let Some(passphrase) = held {
                self.persistent.set(repo_id, &passphrase).await?;
            }
            return Ok(());
        }
        if !self.held.lock().unwrap().contains_key(repo_id) {
            match self.persistent.get(repo_id).await {
                Ok(passphrase) => {
                    self.held
                        .lock()
                        .unwrap()
                        .insert(repo_id.to_string(), passphrase);
                }
                Err(EngineError::PassphraseMissing) => return Ok(()),
                Err(other) => return Err(other),
            }
        }
        self.forget_persistent(repo_id).await
    }

    /// Delete from the persistent store, where absence is already the answer.
    async fn forget_persistent(&self, repo_id: &str) -> Result<()> {
        match self.persistent.delete(repo_id).await {
            Ok(()) | Err(EngineError::PassphraseMissing) => Ok(()),
            Err(other) => Err(other),
        }
    }
}

#[async_trait]
impl SecretStore for SessionSecretStore {
    async fn get(&self, repo_id: &str) -> Result<String> {
        let held = self.held.lock().unwrap().get(repo_id).cloned();
        match held {
            Some(passphrase) => Ok(passphrase),
            None => self.persistent.get(repo_id).await,
        }
    }

    async fn set(&self, repo_id: &str, passphrase: &str) -> Result<()> {
        if self.remember.load(Ordering::SeqCst) {
            self.persistent.set(repo_id, passphrase).await?;
        } else {
            self.forget_persistent(repo_id).await?;
        }
        self.held
            .lock()
            .unwrap()
            .insert(repo_id.to_string(), passphrase.to_string());
        Ok(())
    }

    async fn delete(&self, repo_id: &str) -> Result<()> {
        self.held.lock().unwrap().remove(repo_id);
        self.forget_persistent(repo_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret::FileSecretStore;

    fn persistent(dir: &std::path::Path) -> Arc<dyn SecretStore> {
        Arc::new(FileSecretStore::new(dir.join("secrets.json")))
    }

    #[tokio::test]
    async fn remembering_writes_through_to_the_keyring() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = persistent(dir.path());
        let store = SessionSecretStore::new(Arc::clone(&keyring), true);
        store.set("/repo", "hunter2").await.unwrap();
        assert_eq!(keyring.get("/repo").await.unwrap(), "hunter2");
        assert_eq!(store.get("/repo").await.unwrap(), "hunter2");
    }

    #[tokio::test]
    async fn not_remembering_keeps_it_in_memory_only() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = persistent(dir.path());
        let store = SessionSecretStore::new(Arc::clone(&keyring), false);
        store.set("/repo", "hunter2").await.unwrap();
        assert_eq!(store.get("/repo").await.unwrap(), "hunter2");
        assert_eq!(
            keyring.get("/repo").await,
            Err(EngineError::PassphraseMissing)
        );

        // A new process has nothing to go on, which is the promise.
        let restarted = SessionSecretStore::new(keyring, false);
        assert_eq!(
            restarted.get("/repo").await,
            Err(EngineError::PassphraseMissing)
        );
    }

    #[tokio::test]
    async fn switching_it_off_moves_the_passphrase_out_of_the_keyring() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = persistent(dir.path());
        keyring.set("/repo", "hunter2").await.unwrap();
        let store = SessionSecretStore::new(Arc::clone(&keyring), true);

        store.set_remember(false, Some("/repo")).await.unwrap();
        assert_eq!(
            keyring.get("/repo").await,
            Err(EngineError::PassphraseMissing),
            "the keyring must not keep what the person asked to forget"
        );
        assert_eq!(
            store.get("/repo").await.unwrap(),
            "hunter2",
            "and the backups already running must not stop because of it"
        );
    }

    #[tokio::test]
    async fn switching_it_back_on_stores_what_this_process_knows() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = persistent(dir.path());
        let store = SessionSecretStore::new(Arc::clone(&keyring), false);
        store.set("/repo", "hunter2").await.unwrap();

        store.set_remember(true, Some("/repo")).await.unwrap();
        assert_eq!(keyring.get("/repo").await.unwrap(), "hunter2");
    }

    #[tokio::test]
    async fn the_setting_holds_before_there_is_a_repository() {
        // The wizard settles "Remember" before it creates the repository, so
        // the first passphrase ever stored has to land where the setting says.
        let dir = tempfile::tempdir().unwrap();
        let keyring = persistent(dir.path());
        let store = SessionSecretStore::new(Arc::clone(&keyring), true);
        store.set_remember(false, None).await.unwrap();
        store.set("/repo", "hunter2").await.unwrap();
        assert_eq!(
            keyring.get("/repo").await,
            Err(EngineError::PassphraseMissing)
        );
    }

    #[tokio::test]
    async fn switching_off_with_nothing_stored_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionSecretStore::new(persistent(dir.path()), true);
        store.set_remember(false, Some("/repo")).await.unwrap();
        assert_eq!(
            store.get("/repo").await,
            Err(EngineError::PassphraseMissing)
        );
    }
}
