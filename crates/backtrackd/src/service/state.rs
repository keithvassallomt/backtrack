// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The daemon's shared, mutable state and the wire types built from it.
//!
//! Split out from the interface so the rules — when a pause expires, what a
//! restore policy means, how a configuration key is addressed — can be tested
//! without a bus.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use backtrack_core::config::Config;
use serde::{Deserialize, Serialize};

use super::error::{DaemonError, Result};

/// What to do when a restore would overwrite something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RestorePolicy {
    /// Stop and ask. The default, because silently overwriting someone's work is
    /// the one thing a restore must never do.
    #[default]
    Ask,
    /// Keep both, renaming the incoming file.
    KeepBoth,
    /// Overwrite.
    Replace,
    /// Skip files whose contents already match.
    SkipIdentical,
}

impl RestorePolicy {
    /// Parse the wire spelling.
    pub fn parse(value: &str) -> Result<RestorePolicy> {
        match value {
            "ask" => Ok(RestorePolicy::Ask),
            "keep-both" => Ok(RestorePolicy::KeepBoth),
            "replace" => Ok(RestorePolicy::Replace),
            "skip-identical" => Ok(RestorePolicy::SkipIdentical),
            other => Err(DaemonError::InvalidArgument(format!(
                "unknown restore policy {other:?}; expected one of \
                 ask, keep-both, replace, skip-identical"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RestorePolicy::Ask => "ask",
            RestorePolicy::KeepBoth => "keep-both",
            RestorePolicy::Replace => "replace",
            RestorePolicy::SkipIdentical => "skip-identical",
        }
    }
}

/// A pause the user asked for. Always self-expiring — the menu offers no
/// permanent "off", because a backup tool that can be silently disabled forever
/// from a menu is one that will be.
#[derive(Debug, Clone, Copy, Default)]
pub struct PauseState {
    until: Option<SystemTime>,
}

impl PauseState {
    /// Pause until `until`. A time already past is treated as no pause at all.
    pub fn pause_until(&mut self, until: SystemTime) {
        self.until = Some(until);
    }

    /// Lift the pause.
    pub fn resume(&mut self) {
        self.until = None;
    }

    /// Whether backups are paused at `now`, expiring the pause if it has run
    /// out. The scheduler (S04-T1) is the caller that consults this before a
    /// scheduled run; the interface itself reports [`PauseState::until`].
    #[allow(dead_code)]
    pub fn is_paused(&self, now: SystemTime) -> bool {
        self.until.is_some_and(|until| until > now)
    }

    /// When the pause lifts, if it is still in force at `now`.
    pub fn until(&self, now: SystemTime) -> Option<SystemTime> {
        self.until.filter(|until| *until > now)
    }
}

/// Seconds since the epoch, or 0 for "never"/"not applicable" — the convention
/// the whole interface uses, since D-Bus has no null.
pub fn to_epoch(time: Option<SystemTime>) -> u64 {
    time.and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The inverse of [`to_epoch`].
pub fn from_epoch(seconds: u64) -> Option<SystemTime> {
    (seconds > 0).then(|| UNIX_EPOCH + Duration::from_secs(seconds))
}

/// When the next scheduled backup is due, given the last one.
///
/// A missed schedule is due immediately rather than at some point in the past,
/// so a laptop that was asleep for a week reports "now" instead of a date that
/// has already been and gone.
pub fn next_due(
    config: &Config,
    last_backup: Option<SystemTime>,
    now: SystemTime,
) -> Option<SystemTime> {
    let interval = config.backup.frequency.interval()?;
    let due = match last_backup {
        Some(last) => last + interval,
        None => now,
    };
    Some(due.max(now))
}

/// Read a configuration value by dotted key, as a TOML literal.
///
/// Configuration crosses the bus as TOML rather than as D-Bus variants. Half of
/// the settings are lists (include paths, exclusion patterns), and a stringly
/// typed dictionary cannot express those without inventing an escaping
/// convention that both ends must implement identically. TOML is already the
/// on-disk format, so this adds no new representation to get wrong.
pub fn config_get(config: &Config, key: &str) -> Result<String> {
    let document = toml::Value::try_from(config)
        .map_err(|e| DaemonError::InvalidConfig(format!("cannot serialise configuration: {e}")))?;
    let mut current = &document;
    for segment in key.split('.') {
        current = current
            .get(segment)
            .ok_or_else(|| DaemonError::UnknownConfigKey(key.to_string()))?;
    }
    // Emit the value as it would appear on the right of `key = ` in the file.
    toml::to_string(&WrappedValue { v: current.clone() })
        .map(|s| s.trim_start_matches("v = ").trim_end().to_string())
        .map_err(|e| DaemonError::InvalidConfig(e.to_string()))
}

/// Set a configuration value by dotted key from a TOML literal, returning the
/// updated configuration.
///
/// The whole document is re-validated afterwards, so an edit that produces an
/// impossible configuration is rejected as a unit rather than half-applied.
pub fn config_set(config: &Config, key: &str, value: &str) -> Result<Config> {
    let mut document = toml::Value::try_from(config)
        .map_err(|e| DaemonError::InvalidConfig(format!("cannot serialise configuration: {e}")))?;

    let parsed: WrappedValue = toml::from_str(&format!("v = {value}"))
        .map_err(|e| DaemonError::InvalidArgument(format!("{value:?} is not a TOML value: {e}")))?;

    let segments: Vec<&str> = key.split('.').collect();
    let (last, parents) = segments
        .split_last()
        .ok_or_else(|| DaemonError::UnknownConfigKey(key.to_string()))?;

    let mut current = &mut document;
    for segment in parents {
        current = current
            .get_mut(*segment)
            .ok_or_else(|| DaemonError::UnknownConfigKey(key.to_string()))?;
    }
    let table = current
        .as_table_mut()
        .ok_or_else(|| DaemonError::UnknownConfigKey(key.to_string()))?;
    if !table.contains_key(*last) {
        return Err(DaemonError::UnknownConfigKey(key.to_string()));
    }
    table.insert((*last).to_string(), parsed.v);

    // Round-trip through the schema: this is what rejects a well-formed TOML
    // value of the wrong type for its key.
    let text = toml::to_string(&document).map_err(|e| DaemonError::InvalidConfig(e.to_string()))?;
    let (updated, unknown) = Config::parse(&text).map_err(DaemonError::InvalidConfig)?;
    if !unknown.is_empty() {
        return Err(DaemonError::UnknownConfigKey(unknown.join(", ")));
    }
    Ok(updated)
}

/// The whole configuration as TOML, for `GetConfig`.
pub fn config_document(config: &Config) -> Result<String> {
    toml::to_string_pretty(config).map_err(|e| DaemonError::InvalidConfig(e.to_string()))
}

/// Carrier that lets a bare TOML value be parsed and printed on its own — TOML
/// has no syntax for a document that is just a scalar.
#[derive(Serialize, Deserialize)]
struct WrappedValue {
    v: toml::Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use backtrack_core::config::Frequency;

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_000_000)
    }

    #[test]
    fn restore_policies_round_trip_through_their_wire_names() {
        for policy in [
            RestorePolicy::Ask,
            RestorePolicy::KeepBoth,
            RestorePolicy::Replace,
            RestorePolicy::SkipIdentical,
        ] {
            assert_eq!(RestorePolicy::parse(policy.as_str()).unwrap(), policy);
        }
    }

    #[test]
    fn an_unknown_policy_is_refused_by_name() {
        let err = RestorePolicy::parse("overwrite-everything").unwrap_err();
        assert!(
            matches!(err, DaemonError::InvalidArgument(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn the_default_policy_asks() {
        // Never silently overwrite: the default has to be the cautious one.
        assert_eq!(RestorePolicy::default(), RestorePolicy::Ask);
    }

    #[test]
    fn a_pause_expires_by_itself() {
        let mut pause = PauseState::default();
        assert!(!pause.is_paused(now()));

        pause.pause_until(now() + Duration::from_secs(3_600));
        assert!(pause.is_paused(now()));
        assert!(
            !pause.is_paused(now() + Duration::from_secs(3_601)),
            "the pause must lift on its own"
        );
    }

    #[test]
    fn resuming_clears_the_pause() {
        let mut pause = PauseState::default();
        pause.pause_until(now() + Duration::from_secs(3_600));
        pause.resume();
        assert!(!pause.is_paused(now()));
        assert_eq!(pause.until(now()), None);
    }

    #[test]
    fn epoch_conversion_round_trips_and_treats_zero_as_absent() {
        assert_eq!(to_epoch(None), 0);
        assert_eq!(from_epoch(0), None);
        let t = now();
        assert_eq!(from_epoch(to_epoch(Some(t))), Some(t));
    }

    #[test]
    fn the_next_backup_is_one_interval_after_the_last() {
        let config = Config::default();
        let last = now() - Duration::from_secs(600);
        assert_eq!(
            next_due(&config, Some(last), now()),
            Some(last + Duration::from_secs(3_600))
        );
    }

    #[test]
    fn a_missed_schedule_is_due_now_not_in_the_past() {
        let config = Config::default();
        let last = now() - Duration::from_secs(7 * 24 * 3_600);
        assert_eq!(
            next_due(&config, Some(last), now()),
            Some(now()),
            "a laptop that slept for a week is due immediately"
        );
    }

    #[test]
    fn a_manual_schedule_has_no_next_backup() {
        let mut config = Config::default();
        config.backup.frequency = Frequency::Manual;
        assert_eq!(next_due(&config, Some(now()), now()), None);
    }

    #[test]
    fn config_get_reads_nested_keys() {
        let config = Config::default();
        assert_eq!(
            config_get(&config, "backup.frequency").unwrap(),
            "\"hourly\""
        );
        assert_eq!(
            config_get(&config, "storage.retention.keep_daily").unwrap(),
            "7"
        );
        assert_eq!(config_get(&config, "backup.on_battery").unwrap(), "false");
    }

    #[test]
    fn config_get_rejects_an_unknown_key() {
        let err = config_get(&Config::default(), "backup.frequancy").unwrap_err();
        assert!(
            matches!(err, DaemonError::UnknownConfigKey(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn config_set_applies_a_scalar() {
        let updated = config_set(&Config::default(), "backup.frequency", "\"daily\"").unwrap();
        assert_eq!(updated.backup.frequency, Frequency::Daily);
    }

    #[test]
    fn config_set_applies_a_list() {
        // The reason configuration crosses as TOML rather than as strings.
        let updated = config_set(
            &Config::default(),
            "backup.include",
            "[\"/home/k/Documents\", \"/home/k/Pictures\"]",
        )
        .unwrap();
        assert_eq!(updated.backup.include.len(), 2);
        assert!(updated.backup.include[0].ends_with("Documents"));
    }

    #[test]
    fn config_set_rejects_a_value_of_the_wrong_type() {
        let err = config_set(&Config::default(), "backup.on_battery", "\"yes\"").unwrap_err();
        assert!(matches!(err, DaemonError::InvalidConfig(_)), "got {err:?}");
    }

    #[test]
    fn config_set_rejects_an_unknown_enum_variant() {
        let err =
            config_set(&Config::default(), "backup.frequency", "\"fortnightly\"").unwrap_err();
        assert!(matches!(err, DaemonError::InvalidConfig(_)), "got {err:?}");
    }

    #[test]
    fn config_set_rejects_an_unknown_key_rather_than_inventing_it() {
        // Accepting this would let a typo look like it worked, which is how a
        // user ends up believing a setting is in force when it is not.
        let err = config_set(&Config::default(), "backup.frequancy", "\"daily\"").unwrap_err();
        assert!(
            matches!(err, DaemonError::UnknownConfigKey(_)),
            "got {err:?}"
        );

        let err = config_set(&Config::default(), "nonexistent.key", "1").unwrap_err();
        assert!(
            matches!(err, DaemonError::UnknownConfigKey(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn config_set_leaves_everything_else_alone() {
        let before = Config::default();
        let after = config_set(&before, "backup.frequency", "\"daily\"").unwrap();
        assert_eq!(after.storage, before.storage);
        assert_eq!(after.security, before.security);
        assert_eq!(after.advanced, before.advanced);
        assert_eq!(after.backup.exclude, before.backup.exclude);
    }

    #[test]
    fn the_document_round_trips_through_get_and_parse() {
        let mut config = Config::default();
        config.storage.repository = Some("/mnt/backups".into());
        let text = config_document(&config).unwrap();
        let (parsed, unknown) = Config::parse(&text).unwrap();
        assert_eq!(parsed, config);
        assert!(unknown.is_empty());
    }
}
