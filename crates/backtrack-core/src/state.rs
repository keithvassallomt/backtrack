// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! `state.toml` — what the daemon must remember across a restart but the user
//! never chose.
//!
//! Deliberately separate from [`crate::config`]. Configuration is the user's
//! stated intent and is theirs to edit; this file is bookkeeping. Mixing them
//! would put a machine-written `paused_until` in the middle of a file people are
//! invited to open, and would make "reset my settings" also mean "forget that a
//! backup is paused".
//!
//! Loading is forgiving where configuration is strict: any problem yields
//! [`RuntimeState::default`] with a warning. The stakes are asymmetric — a
//! corrupt config could silently disable a setting the user believes is in
//! force, while a corrupt state file costs at worst a forgotten pause, and
//! forgetting a pause errs towards backing up rather than away from it.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Bookkeeping that outlives the process.
///
/// Times are epoch seconds so the file stays readable and the format cannot
/// drift with a library's idea of a timestamp. `None` means "never".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeState {
    /// When the user's pause lifts. Persisted because a pause the user set for
    /// the afternoon must survive a laptop reboot — otherwise "pause for 4
    /// hours" quietly means "pause until the next restart".
    pub paused_until: Option<u64>,
    /// When a scheduled backup was last *attempted*, successful or not. This is
    /// what the schedule counts from: a run that failed at 14:00 is retried by
    /// the next tick at 15:00, not immediately and not in a tight loop.
    pub last_attempt: Option<u64>,
    /// When `borg compact` last ran. Compaction is expensive and rewrites the
    /// repository, so it runs on its own daily cadence rather than after every
    /// backup.
    pub last_compact: Option<u64>,
    /// When the routine repository check last ran. Monthly, per health.md.
    pub last_check: Option<u64>,
    /// When this computer started backing up to its destination: set up,
    /// imported, or given its first folders to back up.
    ///
    /// The clock that "no successful backups for a day" is measured on starts
    /// no earlier than this. Without it a computer that has never managed a
    /// backup would never be at risk, and one that has just imported somebody's
    /// history would be at risk the moment the import finished, measured from
    /// the newest backup of a different machine.
    pub protected_since: Option<u64>,
    /// What the person has already been told about the current risk. Kept
    /// across restarts so that a laptop waking up does not tell them again.
    ///
    /// Tables last: TOML cannot put a plain value after one, so every field
    /// added above this line keeps the file writable.
    pub notified: Notified,
    /// The most recent health transitions, oldest first. What `doctor` shows
    /// of how the machine got into the state it is in, and where the current
    /// state's start is read from after a restart.
    pub health_history: Vec<Transition>,
    /// The last failure of each part of the daemon, for the dialogs that
    /// explain a problem and for `doctor`.
    pub last_errors: Vec<LastError>,
}

/// How one part of the daemon last failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastError {
    /// `backup`, `local`, `catalogue`, `check`, `maintenance` or `restore`.
    pub subsystem: String,
    pub at: u64,
    /// The health reason token when the failure is a catalogue row, else empty.
    pub reason: String,
    pub message: String,
}

/// When the person was last told about each kind of trouble.
///
/// Epoch seconds like the rest of this file. See the daemon's escalation rules
/// for how these are read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notified {
    /// Which stretch without a backup the at-risk notices were about, named by
    /// when protection was last in place. A new successful backup starts a new
    /// stretch, and with it the count again.
    pub at_risk_since: Option<u64>,
    /// How many at-risk notices that stretch has had.
    pub at_risk_count: u32,
    /// When the last of them went out.
    pub at_risk_at: Option<u64>,
    /// The failure the last broken notice was about, as its reason token.
    pub broken_reason: Option<String>,
    /// When it went out.
    pub broken_at: Option<u64>,
}

/// One change of health state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    pub at: u64,
    pub state: String,
    /// The reason token, empty when healthy.
    pub reason: String,
}

/// How many transitions are kept. Enough to see a pattern in a bug report; few
/// enough that the file stays small.
pub const HISTORY_LENGTH: usize = 50;

impl RuntimeState {
    /// Record a transition, keeping only the most recent [`HISTORY_LENGTH`].
    pub fn record_transition(&mut self, transition: Transition) {
        self.health_history.push(transition);
        let excess = self.health_history.len().saturating_sub(HISTORY_LENGTH);
        self.health_history.drain(..excess);
    }
}

impl RuntimeState {
    /// Load from `<data_dir>/state.toml`, falling back to defaults.
    pub fn load() -> RuntimeState {
        RuntimeState::load_from(&crate::paths::state_file())
    }

    /// Load from an explicit path. Never fails: an absent, unreadable or
    /// malformed file is reported and treated as "nothing remembered".
    pub fn load_from(path: &Path) -> RuntimeState {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return RuntimeState::default(),
            Err(e) => {
                tracing::warn!(path = %path.display(), "cannot read daemon state: {e}");
                return RuntimeState::default();
            }
        };
        match toml::from_str(&text) {
            Ok(state) => state,
            Err(e) => {
                tracing::warn!(path = %path.display(), "ignoring unreadable daemon state: {e}");
                RuntimeState::default()
            }
        }
    }

    /// Write to `<data_dir>/state.toml`.
    pub fn save(&self) -> Result<(), StateError> {
        self.save_to(&crate::paths::state_file())
    }

    /// Write to an explicit path, atomically (temporary file then rename), so an
    /// interrupted save cannot leave a half-written file behind.
    pub fn save_to(&self, path: &Path) -> Result<(), StateError> {
        let text =
            toml::to_string_pretty(self).map_err(|e| StateError::Serialise(e.to_string()))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| StateError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|source| StateError::Io {
            path: tmp.clone(),
            source,
        })?;
        std::fs::rename(&tmp, path).map_err(|source| StateError::Io {
            path: path.to_path_buf(),
            source,
        })
    }
}

/// Why the state file could not be written.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("cannot access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("cannot serialise daemon state: {0}")]
    Serialise(String),
}

/// Epoch seconds to a [`SystemTime`], treating `None` as "never".
pub fn from_epoch(seconds: Option<u64>) -> Option<SystemTime> {
    seconds.map(|s| UNIX_EPOCH + Duration::from_secs(s))
}

/// The inverse of [`from_epoch`]. Times before the epoch — only reachable from a
/// badly-set clock — are dropped rather than saturated to zero, since zero is a
/// real timestamp everywhere else in the system.
pub fn to_epoch(time: Option<SystemTime>) -> Option<u64> {
    time.and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_machine_remembers_nothing() {
        let state = RuntimeState::default();
        assert_eq!(state.paused_until, None);
        assert_eq!(state.last_attempt, None);
        assert_eq!(state.last_compact, None);
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("state.toml");
        let mut original = RuntimeState {
            paused_until: Some(1_700_000_000),
            last_attempt: Some(1_699_999_000),
            last_compact: None,
            last_check: Some(1_699_100_000),
            protected_since: Some(1_699_000_000),
            notified: Notified {
                at_risk_since: Some(1_699_500_000),
                at_risk_count: 2,
                at_risk_at: Some(1_699_800_000),
                broken_reason: Some("destination-full".into()),
                broken_at: Some(1_699_900_000),
            },
            health_history: Vec::new(),
            last_errors: vec![LastError {
                subsystem: "backup".into(),
                at: 1_699_900_000,
                reason: "destination-full".into(),
                message: "the backup destination is full".into(),
            }],
        };
        original.record_transition(Transition {
            at: 1_699_900_000,
            state: "BROKEN".into(),
            reason: "destination-full".into(),
        });
        original.save_to(&path).expect("saves, creating parents");
        assert_eq!(RuntimeState::load_from(&path), original);
    }

    #[test]
    fn the_history_keeps_only_the_most_recent_transitions() {
        let mut state = RuntimeState::default();
        for at in 0..(HISTORY_LENGTH as u64 + 7) {
            state.record_transition(Transition {
                at,
                state: "HEALTHY".into(),
                reason: String::new(),
            });
        }
        assert_eq!(state.health_history.len(), HISTORY_LENGTH);
        assert_eq!(state.health_history.first().map(|t| t.at), Some(7));
        assert_eq!(
            state.health_history.last().map(|t| t.at),
            Some(HISTORY_LENGTH as u64 + 6)
        );
    }

    #[test]
    fn a_file_from_before_the_health_bookkeeping_still_loads() {
        // Every machine that ran an earlier build has a state.toml without
        // these fields, and must keep its pause and its attempt clock.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        std::fs::write(&path, "paused_until = 42\nlast_attempt = 41\n").unwrap();
        let state = RuntimeState::load_from(&path);
        assert_eq!(state.paused_until, Some(42));
        assert_eq!(state.last_attempt, Some(41));
        assert_eq!(state.notified, Notified::default());
        assert!(state.health_history.is_empty());
    }

    #[test]
    fn an_absent_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let state = RuntimeState::load_from(&dir.path().join("absent.toml"));
        assert_eq!(state, RuntimeState::default());
    }

    #[test]
    fn a_corrupt_file_does_not_stop_the_daemon() {
        // The asymmetry with config.toml is the whole point: refusing to start
        // because a bookkeeping file got truncated would turn a trivial problem
        // into "no backups at all".
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        std::fs::write(&path, "this is not = = toml").unwrap();
        assert_eq!(RuntimeState::load_from(&path), RuntimeState::default());
    }

    #[test]
    fn an_unknown_key_from_a_newer_version_is_ignored() {
        // A downgrade must not throw away the keys this build does understand.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        std::fs::write(&path, "paused_until = 42\nsomething_new = true\n").unwrap();
        assert_eq!(RuntimeState::load_from(&path).paused_until, Some(42));
    }

    #[test]
    fn save_leaves_no_temporary_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        RuntimeState::default()
            .save_to(&dir.path().join("state.toml"))
            .unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .filter(|n| n != "state.toml")
            .collect();
        assert!(leftovers.is_empty(), "unexpected leftovers: {leftovers:?}");
    }

    #[test]
    fn epoch_conversion_round_trips() {
        assert_eq!(to_epoch(None), None);
        assert_eq!(from_epoch(None), None);
        let t = UNIX_EPOCH + Duration::from_secs(1_234_567);
        assert_eq!(from_epoch(to_epoch(Some(t))), Some(t));
    }

    #[test]
    fn times_before_the_epoch_are_dropped_not_saturated() {
        let before = UNIX_EPOCH - Duration::from_secs(1);
        assert_eq!(to_epoch(Some(before)), None);
    }
}
