// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! `config.toml` — the source of truth for every user-facing setting.
//!
//! The GUI never writes this file directly; it edits through the daemon's
//! `SetConfig` method, so there is exactly one writer. The schema mirrors the
//! Preferences window one section per page (General, Backup, Storage, Security,
//! Advanced), which keeps the settings inventory and the UI in step.
//!
//! Two rules govern loading, both in service of the same goal — a configuration
//! problem must never be the reason backups stop running:
//!
//! - **Every field has a default.** A missing file, or a file missing any
//!   subset of keys, still yields a usable [`Config`].
//! - **Unknown keys warn, they do not fail.** Downgrading the application, or
//!   hand-editing a typo into the file, leaves the daemon running on defaults
//!   for the affected key rather than dead at startup.
//!
//! A malformed file (invalid TOML, or a value of the wrong type) *is* an error:
//! silently discarding a setting the user believes is in force would be worse
//! than refusing to start.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Errors surfaced when loading or saving the configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read or written.
    #[error("cannot access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The file is not valid TOML, or a key holds a value of the wrong type.
    #[error("{path} is not valid configuration: {message}")]
    Parse { path: PathBuf, message: String },

    /// The configuration could not be serialised back to TOML.
    #[error("cannot serialise configuration: {0}")]
    Serialise(String),
}

/// A convenience result alias for the configuration layer.
pub type Result<T> = std::result::Result<T, ConfigError>;

/// The complete Backtrack configuration.
///
/// Load with [`Config::load`]; every section defaults, so
/// `Config::default()` is a valid running configuration for a machine that has
/// not been through the wizard yet (the absent [`Storage::repository`] is what
/// marks it as unconfigured — see [`Config::is_configured`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub backup: Backup,
    pub storage: Storage,
    pub security: Security,
    pub advanced: Advanced,
}

/// Preferences → General.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// Keep backing up while the window is closed.
    pub run_in_background: bool,
    /// How talkative the notifications are.
    pub notifications: Notifications,
    /// File-manager integration toggles. Whether the plugin is *installed* is
    /// detected at runtime (Stage 12); this is only whether it is wanted.
    pub nautilus_integration: bool,
    pub dolphin_integration: bool,
}

impl Default for General {
    fn default() -> General {
        General {
            run_in_background: true,
            notifications: Notifications::default(),
            nautilus_integration: true,
            dolphin_integration: true,
        }
    }
}

/// How much the application has to say for itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Notifications {
    /// Every backup start and finish.
    All,
    /// Only failures and things needing a decision. The default: a backup tool
    /// that announces itself hourly is a backup tool people turn off.
    #[default]
    AttentionOnly,
    /// Nothing, ever.
    None,
}

/// Preferences → Backup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Backup {
    pub frequency: Frequency,
    /// Run when on battery. Off by default — an hourly backup is not worth
    /// someone's remaining laptop charge.
    pub on_battery: bool,
    /// Run over a connection the user has marked metered.
    pub on_metered: bool,
    /// Absolute paths to back up. Empty until the wizard fills it in.
    pub include: Vec<PathBuf>,
    /// Exclusion patterns, in Borg pattern syntax.
    pub exclude: Vec<String>,
}

impl Default for Backup {
    fn default() -> Backup {
        Backup {
            frequency: Frequency::default(),
            on_battery: false,
            on_metered: false,
            include: Vec::new(),
            exclude: default_exclusions(),
        }
    }
}

/// The seed exclusion list offered by the wizard: caches and regenerable bulk
/// that would otherwise dominate both backup time and repository size.
fn default_exclusions() -> Vec<String> {
    [
        "**/.cache",
        "**/Cache",
        "**/Caches",
        "**/.local/share/Trash",
        "**/node_modules",
        "**/target/debug",
        "**/*.iso",
        "**/*.qcow2",
        "**/*.vdi",
        "**/*.vmdk",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// How often a scheduled backup runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Frequency {
    /// The product default, and what makes the timeline feel like Time Machine.
    #[default]
    Hourly,
    Daily,
    Weekly,
    /// Scheduled backups off; manual triggers only.
    Manual,
}

impl Frequency {
    /// The scheduling interval, or `None` when backups are manual-only.
    pub fn interval(self) -> Option<std::time::Duration> {
        match self {
            Frequency::Hourly => Some(std::time::Duration::from_secs(3_600)),
            Frequency::Daily => Some(std::time::Duration::from_secs(86_400)),
            Frequency::Weekly => Some(std::time::Duration::from_secs(7 * 86_400)),
            Frequency::Manual => None,
        }
    }
}

/// Preferences → Storage.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Storage {
    /// The Borg repository location. `None` until the wizard has run — this is
    /// what [`Config::is_configured`] tests.
    pub repository: Option<String>,
    pub retention: Retention,
    pub offline: Offline,
}

/// Retention policy. With `automatic` on, the four counts below are the
/// recommended ladder and the UI shows them read-only; turning it off exposes
/// them as steppers. The values are kept either way so toggling is lossless.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Retention {
    pub automatic: bool,
    /// Hourly archives kept (24 = one day).
    pub keep_hourly: u32,
    /// Daily archives kept (7 = one week).
    pub keep_daily: u32,
    /// Weekly archives kept (4 ≈ one month).
    pub keep_weekly: u32,
    /// Monthly archives kept (6 = six months).
    pub keep_monthly: u32,
}

impl Default for Retention {
    fn default() -> Retention {
        Retention {
            automatic: true,
            keep_hourly: 24,
            keep_daily: 7,
            keep_weekly: 4,
            keep_monthly: 6,
        }
    }
}

/// What happens when the destination is unreachable (see offline-strategy.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Offline {
    /// Keep protecting changes locally while away from the destination.
    pub enabled: bool,
    /// Cap on the local spool, in gigabytes. Oldest spool archives are dropped
    /// first once this is hit.
    pub space_limit_gb: u32,
}

impl Default for Offline {
    fn default() -> Offline {
        Offline {
            enabled: true,
            space_limit_gb: 10,
        }
    }
}

/// Preferences → Security.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Security {
    /// Store the passphrase in the Secret Service keyring. Off means prompting
    /// for it, which in practice means unattended backups stop working — the
    /// wizard says so plainly.
    pub remember_passphrase: bool,
}

impl Default for Security {
    fn default() -> Security {
        Security {
            remember_passphrase: true,
        }
    }
}

/// Preferences → Advanced.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Advanced {
    pub compression: Compression,
    /// Upload ceiling in megabytes per second; `None` is unlimited.
    pub upload_limit_mbps: Option<u32>,
}

/// Repository compression. Names match what Borg accepts on the command line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Compression {
    /// The recommended default: near-lz4 speed at meaningfully better ratios.
    #[default]
    Zstd,
    Lz4,
    None,
}

impl Compression {
    /// The value to hand Borg's `--compression` flag.
    pub fn as_borg_arg(self) -> &'static str {
        match self {
            Compression::Zstd => "zstd",
            Compression::Lz4 => "lz4",
            Compression::None => "none",
        }
    }
}

impl Config {
    /// Load from the default location ([`crate::paths::config_file`]).
    ///
    /// An absent file yields [`Config::default`]: a fresh machine is
    /// unconfigured, not broken.
    pub fn load() -> Result<Config> {
        Config::load_from(&crate::paths::config_file())
    }

    /// Load from an explicit path. Unknown keys are logged at `warn` and
    /// otherwise ignored.
    pub fn load_from(path: &Path) -> Result<Config> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!(path = %path.display(), "no config file; using defaults");
                return Ok(Config::default());
            }
            Err(source) => {
                return Err(ConfigError::Io {
                    path: path.to_path_buf(),
                    source,
                })
            }
        };
        let (config, unknown) = Config::parse(&text).map_err(|message| ConfigError::Parse {
            path: path.to_path_buf(),
            message,
        })?;
        for key in &unknown {
            tracing::warn!(
                path = %path.display(),
                key = %key,
                "unknown configuration key ignored"
            );
        }
        Ok(config)
    }

    /// Parse TOML into a [`Config`], returning it alongside the dotted paths of
    /// any keys the schema does not know. Split out from [`Config::load_from`]
    /// so the unknown-key behaviour is testable without touching the disk.
    pub fn parse(text: &str) -> std::result::Result<(Config, Vec<String>), String> {
        // `parse` reads the whole document up front, so a syntax error surfaces
        // here, before any field is visited.
        let de = toml::Deserializer::parse(text).map_err(|e| e.to_string())?;
        let mut unknown = Vec::new();
        let config = serde_ignored::deserialize(de, |path| unknown.push(path.to_string()))
            .map_err(|e| e.to_string())?;
        Ok((config, unknown))
    }

    /// Write to the default location, creating the data directory if needed.
    pub fn save(&self) -> Result<()> {
        self.save_to(&crate::paths::config_file())
    }

    /// Write to an explicit path.
    ///
    /// The write is atomic — a temporary file in the same directory, then a
    /// rename — so an interrupted save cannot leave a half-written config that
    /// would fail to parse on the next start.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let text =
            toml::to_string_pretty(self).map_err(|e| ConfigError::Serialise(e.to_string()))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|source| ConfigError::Io {
            path: tmp.clone(),
            source,
        })?;
        std::fs::rename(&tmp, path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Whether the wizard has run: a destination is the one setting without a
    /// sensible default, so its presence is what distinguishes a configured
    /// machine from a fresh one.
    pub fn is_configured(&self) -> bool {
        self.storage.repository.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_defaults() {
        let (config, unknown) = Config::parse("").expect("empty TOML is valid");
        assert_eq!(config, Config::default());
        assert!(unknown.is_empty());
    }

    #[test]
    fn defaults_match_the_documented_recommendations() {
        let c = Config::default();
        assert_eq!(c.backup.frequency, Frequency::Hourly);
        assert!(!c.backup.on_battery, "battery backups off by default");
        assert!(!c.backup.on_metered, "metered backups off by default");
        // The retention ladder from the Storage page: hourly 24h, daily 1 week,
        // weekly 1 month, monthly 6 months.
        assert_eq!(c.storage.retention.keep_hourly, 24);
        assert_eq!(c.storage.retention.keep_daily, 7);
        assert_eq!(c.storage.retention.keep_weekly, 4);
        assert_eq!(c.storage.retention.keep_monthly, 6);
        assert_eq!(c.storage.offline.space_limit_gb, 10);
        assert!(c.security.remember_passphrase);
        assert_eq!(c.advanced.compression, Compression::Zstd);
        assert_eq!(c.advanced.upload_limit_mbps, None);
    }

    #[test]
    fn a_fresh_config_is_not_configured() {
        assert!(!Config::default().is_configured());
        let mut c = Config::default();
        c.storage.repository = Some("/mnt/backups".into());
        assert!(c.is_configured());
    }

    #[test]
    fn partial_file_keeps_defaults_for_absent_keys() {
        let (config, unknown) = Config::parse(
            r#"
            [backup]
            frequency = "daily"
            "#,
        )
        .expect("partial TOML is valid");
        assert!(unknown.is_empty());
        assert_eq!(config.backup.frequency, Frequency::Daily);
        // Untouched keys in the same section keep their defaults.
        assert_eq!(config.backup.exclude, default_exclusions());
        // As do entirely absent sections.
        assert_eq!(config.storage.retention, Retention::default());
    }

    #[test]
    fn unknown_keys_are_reported_but_do_not_fail() {
        let (config, unknown) = Config::parse(
            r#"
            [backup]
            frequency = "daily"
            frequancy = "hourly"

            [nonexistent]
            whatever = 1
            "#,
        )
        .expect("unknown keys must not fail the load");
        assert_eq!(config.backup.frequency, Frequency::Daily);
        assert!(
            unknown.contains(&"backup.frequancy".to_string()),
            "typo'd key reported: {unknown:?}"
        );
        assert!(
            unknown.iter().any(|k| k.starts_with("nonexistent")),
            "unknown section reported: {unknown:?}"
        );
    }

    #[test]
    fn malformed_toml_is_an_error() {
        assert!(Config::parse("this is not toml = = =").is_err());
    }

    #[test]
    fn wrong_value_type_is_an_error() {
        // Silently defaulting here would leave the user believing a setting is
        // in force when it is not.
        let err = Config::parse("[backup]\non_battery = \"yes\"\n").unwrap_err();
        assert!(err.contains("on_battery"), "error names the key: {err}");
    }

    #[test]
    fn unknown_enum_variant_is_an_error() {
        assert!(Config::parse("[backup]\nfrequency = \"fortnightly\"\n").is_err());
    }

    #[test]
    fn round_trips_through_toml() {
        let mut original = Config::default();
        original.storage.repository = Some("ssh://nas.local/./backups".into());
        original.backup.include = vec![PathBuf::from("/home/k/Documents")];
        original.backup.frequency = Frequency::Daily;
        original.advanced.upload_limit_mbps = Some(5);
        original.general.notifications = Notifications::None;

        let text = toml::to_string_pretty(&original).expect("serialises");
        let (parsed, unknown) = Config::parse(&text).expect("re-parses");
        assert_eq!(parsed, original);
        assert!(
            unknown.is_empty(),
            "our own output must not contain unknown keys: {unknown:?}"
        );
    }

    #[test]
    fn enum_names_are_kebab_case_on_disk() {
        let mut c = Config::default();
        c.general.notifications = Notifications::AttentionOnly;
        let text = toml::to_string_pretty(&c).expect("serialises");
        assert!(
            text.contains("attention-only"),
            "expected kebab-case in:\n{text}"
        );
    }

    #[test]
    fn save_then_load_round_trips_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        let mut original = Config::default();
        original.storage.repository = Some("/mnt/backups".into());
        original.save_to(&path).expect("saves, creating parents");

        let loaded = Config::load_from(&path).expect("loads");
        assert_eq!(loaded, original);
    }

    #[test]
    fn absent_file_loads_as_default() {
        let dir = tempfile::tempdir().unwrap();
        let loaded =
            Config::load_from(&dir.path().join("absent.toml")).expect("absent is not an error");
        assert_eq!(loaded, Config::default());
    }

    #[test]
    fn save_leaves_no_temporary_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        Config::default().save_to(&path).expect("saves");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .filter(|n| n != "config.toml")
            .collect();
        assert!(leftovers.is_empty(), "unexpected leftovers: {leftovers:?}");
    }

    #[test]
    fn frequency_intervals() {
        assert_eq!(
            Frequency::Hourly.interval(),
            Some(std::time::Duration::from_secs(3_600))
        );
        assert_eq!(
            Frequency::Daily.interval(),
            Some(std::time::Duration::from_secs(86_400))
        );
        assert_eq!(
            Frequency::Weekly.interval(),
            Some(std::time::Duration::from_secs(604_800))
        );
        assert_eq!(Frequency::Manual.interval(), None);
    }
}
