// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The folders Backtrack backs up, published for the file-manager plugins.
//!
//! The Nautilus extension offers its menu items only inside those folders. It
//! runs inside the file manager, which asks it for items on every right-click
//! and waits for the answer, so it must not call the daemon: a daemon being
//! activated, or busy, would hold up the file manager's menu. The daemon writes
//! this small file instead, whenever the configuration changes, and the
//! extension reads it.
//!
//! ```json
//! { "version": 1, "nautilus": true, "roots": ["/home/keith"] }
//! ```
//!
//! `nautilus` is the integration toggle in Preferences → General: switched
//! off, the extension stays installed and offers nothing.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::Config;

/// The format this module writes. The extension shows nothing for a version
/// it does not know.
pub const VERSION: u32 = 1;

/// What the file-manager plugins need to know.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Roots {
    pub version: u32,
    /// Whether the Nautilus menu items are wanted.
    pub nautilus: bool,
    /// The configured backup sources, as absolute paths. Empty until a
    /// destination is set up, since nothing is being backed up before then.
    pub roots: Vec<PathBuf>,
}

impl Roots {
    /// What `config` says the plugins should offer.
    pub fn of(config: &Config) -> Roots {
        let roots = if config.is_configured() {
            config.backup.include.clone()
        } else {
            Vec::new()
        };
        Roots {
            version: VERSION,
            nautilus: config.general.nautilus_integration,
            roots,
        }
    }

    /// Write to `path`, unless it already says this. Returns whether it was
    /// written.
    ///
    /// Written through a temporary file and a rename, so the extension never
    /// reads half of one.
    pub fn save_to(&self, path: &Path) -> std::io::Result<bool> {
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)? + "\n";
        if std::fs::read_to_string(path).is_ok_and(|current| current == text) {
            return Ok(false);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured(include: &[&str]) -> Config {
        let mut config = Config::default();
        config.storage.repository = Some("/mnt/backups".into());
        config.backup.include = include.iter().map(PathBuf::from).collect();
        config
    }

    #[test]
    fn the_roots_are_the_configured_sources() {
        let roots = Roots::of(&configured(&["/home/k", "/srv/photos"]));
        assert_eq!(roots.version, VERSION);
        assert!(roots.nautilus);
        assert_eq!(
            roots.roots,
            vec![PathBuf::from("/home/k"), PathBuf::from("/srv/photos")]
        );
    }

    #[test]
    fn nothing_is_offered_before_a_destination_is_set_up() {
        let mut config = configured(&["/home/k"]);
        config.storage.repository = None;
        assert!(Roots::of(&config).roots.is_empty());
    }

    #[test]
    fn the_preference_toggle_is_carried_through() {
        let mut config = configured(&["/home/k"]);
        config.general.nautilus_integration = false;
        let roots = Roots::of(&config);
        assert!(!roots.nautilus);
        // The roots are still published: the toggle is the extension's to
        // honour, and switching it back on must not wait for a restart.
        assert_eq!(roots.roots, vec![PathBuf::from("/home/k")]);
    }

    #[test]
    fn the_file_is_written_only_when_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data/roots.json");
        let roots = Roots::of(&configured(&["/home/k"]));

        assert!(roots.save_to(&path).unwrap());
        assert!(!roots.save_to(&path).unwrap());
        assert!(!dir.path().join("data/roots.json.tmp").exists());

        let read: Roots = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read, roots);

        let moved = Roots::of(&configured(&["/home/k/Documents"]));
        assert!(moved.save_to(&path).unwrap());
    }

    #[test]
    fn the_format_is_the_one_the_extension_reads() {
        let text = serde_json::to_value(Roots::of(&configured(&["/home/k"]))).unwrap();
        assert_eq!(
            text,
            serde_json::json!({ "version": 1, "nautilus": true, "roots": ["/home/k"] })
        );
    }
}
