// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Where Backtrack keeps its things.
//!
//! Every path the application touches derives from one base directory,
//! `~/.local/share/backtrack/` (see the data layout in `stack.md` §3). There is
//! deliberately no separate config directory: `config.toml` is the source of
//! truth and lives alongside the index it configures, so a single directory can
//! be backed up, inspected, or wiped as a unit.
//!
//! Every resolver honors `BACKTRACK_DEV`, which redirects the base to
//! `backtrack-dev/` so development runs can never touch real backups or logs.

use std::ffi::OsStr;
use std::path::PathBuf;

/// Base data directory: `$XDG_DATA_HOME/backtrack`, else `~/.local/share/backtrack`.
/// With `BACKTRACK_DEV` set, the leaf becomes `backtrack-dev` instead.
pub fn data_dir() -> PathBuf {
    data_dir_from(
        std::env::var_os("BACKTRACK_DEV").is_some(),
        std::env::var_os("XDG_DATA_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

/// The configuration file: `<data_dir>/config.toml`.
pub fn config_file() -> PathBuf {
    data_dir().join("config.toml")
}

/// The SQLite index: `<data_dir>/index.db`.
pub fn index_db() -> PathBuf {
    data_dir().join("index.db")
}

/// Daemon bookkeeping that outlives the process: `<data_dir>/state.toml`.
/// Separate from `config.toml` — see [`crate::state`].
pub fn state_file() -> PathBuf {
    data_dir().join("state.toml")
}

/// Rotating JSONL logs: `<data_dir>/logs`.
pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

/// The offline spool repository: `<data_dir>/spool` (Stage 5).
pub fn spool_dir() -> PathBuf {
    data_dir().join("spool")
}

/// Read-only filesystem snapshots taken while offline: `<data_dir>/snapshots`
/// (Stage 5, btrfs mode).
pub fn snapshots_dir() -> PathBuf {
    data_dir().join("snapshots")
}

/// The 30-day safety stash written by restores: `<data_dir>/replaced` (Stage 7).
pub fn replaced_dir() -> PathBuf {
    data_dir().join("replaced")
}

/// Restore staging area: `<data_dir>/staging` (Stage 7).
pub fn staging_dir() -> PathBuf {
    data_dir().join("staging")
}

/// Preview extraction cache: `<data_dir>/cache` (Stage 3, `PreviewFile`).
pub fn cache_dir() -> PathBuf {
    data_dir().join("cache")
}

/// Pure path resolution, split out so it can be tested without touching the
/// process environment.
fn data_dir_from(dev: bool, xdg_data_home: Option<&OsStr>, home: Option<&OsStr>) -> PathBuf {
    let leaf = if dev { "backtrack-dev" } else { "backtrack" };
    let base = xdg_data_home
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(leaf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn os(s: &str) -> OsString {
        OsString::from(s)
    }

    #[test]
    fn prefers_xdg_data_home_when_set() {
        let p = data_dir_from(false, Some(&os("/x/data")), Some(&os("/home/k")));
        assert_eq!(p, PathBuf::from("/x/data/backtrack"));
    }

    #[test]
    fn falls_back_to_home_local_share() {
        let p = data_dir_from(false, None, Some(&os("/home/k")));
        assert_eq!(p, PathBuf::from("/home/k/.local/share/backtrack"));
    }

    #[test]
    fn empty_xdg_data_home_is_ignored() {
        let p = data_dir_from(false, Some(&os("")), Some(&os("/home/k")));
        assert_eq!(p, PathBuf::from("/home/k/.local/share/backtrack"));
    }

    #[test]
    fn dev_mode_switches_the_leaf_directory() {
        let p = data_dir_from(true, None, Some(&os("/home/k")));
        assert_eq!(p, PathBuf::from("/home/k/.local/share/backtrack-dev"));
        // The real location must never be a prefix match of the dev one.
        assert!(!p.starts_with("/home/k/.local/share/backtrack/"));
    }

    #[test]
    fn last_resort_is_the_current_directory() {
        let p = data_dir_from(false, None, None);
        assert_eq!(p, PathBuf::from("./backtrack"));
    }
}
