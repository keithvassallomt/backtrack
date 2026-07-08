// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Passphrase storage. The real store is the Secret Service (system keyring) via
//! `oo7`; `BACKTRACK_DEV=1` selects a file-backed store so CI stays headless.
//!
//! A missing entry is [`EngineError::PassphraseMissing`], never a prompt —
//! prompting is the UI's job (see health.md), and this exact path powers the
//! passphrase-recovery dialog (mockup 24) later.

mod file;
mod keyring;

use std::sync::Arc;

use async_trait::async_trait;

use crate::engine::{EngineError, Result};

pub use file::FileSecretStore;
pub use keyring::KeyringSecretStore;

/// Stable Secret Service attribute: the application id.
pub const APP_ID: &str = "io.github.keithvassallomt.Backtrack";

/// Get/set/delete a repository passphrase under a stable attribute set
/// (`app-id` + `repo-id`).
#[async_trait]
pub trait SecretStore: Send + Sync {
    /// Returns the stored passphrase, or [`EngineError::PassphraseMissing`].
    async fn get(&self, repo_id: &str) -> Result<String>;
    async fn set(&self, repo_id: &str, passphrase: &str) -> Result<()>;
    async fn delete(&self, repo_id: &str) -> Result<()>;
}

/// The store the daemon uses by default: file-backed under `BACKTRACK_DEV=1`,
/// otherwise the system keyring.
pub fn default_store() -> Result<Arc<dyn SecretStore>> {
    if std::env::var("BACKTRACK_DEV").as_deref() == Ok("1") {
        Ok(Arc::new(FileSecretStore::dev_default()?))
    } else {
        Ok(Arc::new(KeyringSecretStore::new()))
    }
}

/// Shared helper: a keyring/file lookup that found nothing.
pub(crate) fn missing() -> EngineError {
    EngineError::PassphraseMissing
}
