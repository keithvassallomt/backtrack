// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>

//! File-backed passphrase store for `BACKTRACK_DEV=1` / headless CI. Stores a
//! flat JSON map `{ repo_id: passphrase }`. Not for production use.

use std::collections::BTreeMap;
use std::path::PathBuf;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::engine::{EngineError, Result};
use crate::secret::{missing, SecretStore};

/// A JSON-file passphrase store, serialised under a mutex for atomic updates.
pub struct FileSecretStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileSecretStore {
    /// A store backed by the given file (created on first `set`).
    pub fn new(path: PathBuf) -> FileSecretStore {
        FileSecretStore {
            path,
            lock: Mutex::new(()),
        }
    }

    /// The default dev location: `$XDG_DATA_HOME/backtrack-dev/secrets.json`
    /// (falling back to `~/.local/share`).
    pub fn dev_default() -> Result<FileSecretStore> {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .ok_or_else(|| EngineError::BorgFailed {
                code: -1,
                stderr: "neither XDG_DATA_HOME nor HOME is set".into(),
            })?;
        Ok(FileSecretStore::new(
            base.join("backtrack-dev/secrets.json"),
        ))
    }

    fn read_map(&self) -> Result<BTreeMap<String, String>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| EngineError::BorgFailed {
                code: -1,
                stderr: format!("corrupt dev secret store: {e}"),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(EngineError::BorgFailed {
                code: -1,
                stderr: format!("reading dev secret store: {e}"),
            }),
        }
    }

    fn write_map(&self, map: &BTreeMap<String, String>) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| EngineError::BorgFailed {
                code: -1,
                stderr: format!("creating dev secret dir: {e}"),
            })?;
        }
        let bytes = serde_json::to_vec_pretty(map).map_err(|e| EngineError::BorgFailed {
            code: -1,
            stderr: format!("serialising dev secret store: {e}"),
        })?;
        std::fs::write(&self.path, bytes).map_err(|e| EngineError::BorgFailed {
            code: -1,
            stderr: format!("writing dev secret store: {e}"),
        })
    }
}

#[async_trait]
impl SecretStore for FileSecretStore {
    async fn get(&self, repo_id: &str) -> Result<String> {
        let _g = self.lock.lock().await;
        self.read_map()?.get(repo_id).cloned().ok_or_else(missing)
    }

    async fn set(&self, repo_id: &str, passphrase: &str) -> Result<()> {
        let _g = self.lock.lock().await;
        let mut map = self.read_map()?;
        map.insert(repo_id.to_string(), passphrase.to_string());
        self.write_map(&map)
    }

    async fn delete(&self, repo_id: &str) -> Result<()> {
        let _g = self.lock.lock().await;
        let mut map = self.read_map()?;
        map.remove(repo_id);
        self.write_map(&map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineError;
    use crate::secret::SecretStore;

    #[tokio::test]
    async fn set_get_delete_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSecretStore::new(dir.path().join("secrets.json"));

        // Missing → PassphraseMissing.
        assert!(matches!(
            store.get("repo-1").await,
            Err(EngineError::PassphraseMissing)
        ));

        store.set("repo-1", "hunter2").await.unwrap();
        store.set("repo-2", "other").await.unwrap();
        assert_eq!(store.get("repo-1").await.unwrap(), "hunter2");
        assert_eq!(store.get("repo-2").await.unwrap(), "other");

        store.delete("repo-1").await.unwrap();
        assert!(matches!(
            store.get("repo-1").await,
            Err(EngineError::PassphraseMissing)
        ));
        // Deleting a second store instance sees the persisted file.
        let store2 = FileSecretStore::new(dir.path().join("secrets.json"));
        assert_eq!(store2.get("repo-2").await.unwrap(), "other");
    }
}
