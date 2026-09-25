// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Preferences, worked out before a widget is involved.
//!
//! The most important thing in here is [`Setting`]: one variant per key in
//! `config.toml`, which is the only way the window can bind a control to a
//! setting. A test compares the variants with the keys the schema actually
//! writes, so a setting added to the configuration without a control, or a
//! control left behind for a setting that has gone, fails the build rather
//! than a person looking for it. The window checks the other half as it is
//! built: every variant bound, and none twice.

use std::path::{Path, PathBuf};

use backtrack_core::config::{Compression, Notifications};
use backtrack_core::dbus::{Status, StorageInfo};
use gtk4::glib;

use super::format;

/// Every setting in `config.toml`, as Preferences addresses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Setting {
    RunInBackground,
    Notifications,
    NautilusIntegration,
    DolphinIntegration,
    Frequency,
    OnBattery,
    OnMetered,
    Include,
    Exclude,
    Repository,
    RetentionAutomatic,
    KeepHourly,
    KeepDaily,
    KeepWeekly,
    KeepMonthly,
    OfflineEnabled,
    OfflineSpaceLimit,
    RememberPassphrase,
    Compression,
    UploadLimit,
}

impl Setting {
    pub const ALL: [Setting; 20] = [
        Setting::RunInBackground,
        Setting::Notifications,
        Setting::NautilusIntegration,
        Setting::DolphinIntegration,
        Setting::Frequency,
        Setting::OnBattery,
        Setting::OnMetered,
        Setting::Include,
        Setting::Exclude,
        Setting::Repository,
        Setting::RetentionAutomatic,
        Setting::KeepHourly,
        Setting::KeepDaily,
        Setting::KeepWeekly,
        Setting::KeepMonthly,
        Setting::OfflineEnabled,
        Setting::OfflineSpaceLimit,
        Setting::RememberPassphrase,
        Setting::Compression,
        Setting::UploadLimit,
    ];

    /// The dotted key `SetConfig` takes.
    pub fn key(self) -> &'static str {
        match self {
            Setting::RunInBackground => "general.run_in_background",
            Setting::Notifications => "general.notifications",
            Setting::NautilusIntegration => "general.nautilus_integration",
            Setting::DolphinIntegration => "general.dolphin_integration",
            Setting::Frequency => "backup.frequency",
            Setting::OnBattery => "backup.on_battery",
            Setting::OnMetered => "backup.on_metered",
            Setting::Include => "backup.include",
            Setting::Exclude => "backup.exclude",
            Setting::Repository => "storage.repository",
            Setting::RetentionAutomatic => "storage.retention.automatic",
            Setting::KeepHourly => "storage.retention.keep_hourly",
            Setting::KeepDaily => "storage.retention.keep_daily",
            Setting::KeepWeekly => "storage.retention.keep_weekly",
            Setting::KeepMonthly => "storage.retention.keep_monthly",
            Setting::OfflineEnabled => "storage.offline.enabled",
            Setting::OfflineSpaceLimit => "storage.offline.space_limit_gb",
            Setting::RememberPassphrase => "security.remember_passphrase",
            Setting::Compression => "advanced.compression",
            Setting::UploadLimit => "advanced.upload_limit_mbps",
        }
    }
}

/// What "Notifications" offers.
pub const NOTIFICATIONS: [(Notifications, &str); 3] = [
    (
        Notifications::AttentionOnly,
        "Only when attention is needed",
    ),
    (Notifications::All, "For every backup"),
    (Notifications::None, "Never"),
];

/// What "Compression" offers.
pub const COMPRESSION: [(Compression, &str); 3] = [
    (Compression::Zstd, "zstd (recommended)"),
    (Compression::Lz4, "lz4 (faster)"),
    (Compression::None, "None"),
];

/// The local snapshot space limits on offer, in gigabytes, with the one in
/// force added if somebody set a value by hand that is not among them.
pub fn space_limits(current: u32) -> Vec<u32> {
    let mut limits = vec![5, 10, 20, 50, 100];
    if !limits.contains(&current) {
        limits.push(current);
        limits.sort_unstable();
    }
    limits
}

pub fn space_limit_label(gigabytes: u32) -> String {
    format!("{gigabytes} GB")
}

/// "412 GB of 2 TB — 1.9 TB before deduplication", in the mockup's words, and
/// how full the bar under it is. Without a capacity (an SSH server) there is
/// no "of" and no bar.
pub fn space_used(info: &StorageInfo) -> (String, Option<f64>) {
    let stored = glib::format_size(info.stored);
    let original = glib::format_size(info.original);
    if info.capacity == 0 {
        return (format!("{stored} — {original} before deduplication"), None);
    }
    (
        format!(
            "{stored} of {} — {original} before deduplication",
            glib::format_size(info.capacity)
        ),
        Some((info.stored as f64 / info.capacity as f64).clamp(0.0, 1.0)),
    )
}

/// The Security page's first row: whether the backups are encrypted and
/// where the key lives, from Borg's name for the mode.
pub fn encryption(mode: &str) -> (&'static str, &'static str) {
    if mode.starts_with("repokey") {
        ("Encrypted", "AES-256 — key stored in repository")
    } else if mode.starts_with("keyfile") {
        ("Encrypted", "AES-256 — key stored on this computer")
    } else {
        (
            "Not encrypted",
            "Anybody who can reach these backups can read them",
        )
    }
}

/// "Next backup: today at 17:00".
pub fn next_backup(status: &Status, now: i64, tz: &glib::TimeZone) -> String {
    if status.paused_until as i64 > now {
        return "Backups are paused".to_string();
    }
    match status.next_backup {
        0 => "No backups are scheduled".to_string(),
        due => format!("Next backup: {}", day_and_time(due as i64, now, tz, " at ")),
    }
}

/// "Today, 16:00", for the Storage page's last-backup row.
pub fn last_backup(status: &Status, now: i64, tz: &glib::TimeZone) -> String {
    match status.last_backup {
        0 => "Never".to_string(),
        at => {
            let text = day_and_time(at as i64, now, tz, ", ");
            let mut chars = text.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    }
}

fn day_and_time(ts: i64, now: i64, tz: &glib::TimeZone, separator: &str) -> String {
    let day = super::local_day(ts, tz) - super::local_day(now, tz);
    let clock = format::clock(ts, tz);
    match day {
        0 => format!("today{separator}{clock}"),
        1 => format!("tomorrow{separator}{clock}"),
        -1 => format!("yesterday{separator}{clock}"),
        _ => format!("{}{separator}{clock}", format::weekday_and_day(ts, tz)),
    }
}

/// "Currently using: 1.2 GB · 14 snapshots on this computer". Filesystem
/// snapshots share their space with the files themselves, so for those the
/// count is the whole answer (see `Status::spool_bytes`).
pub fn local_usage(status: &Status) -> String {
    let count = match status.local_snapshots {
        1 => "1 snapshot".to_string(),
        n => format!("{n} snapshots"),
    };
    if status.offline_mode == "fs-snapshot" {
        format!("Currently using: {count} on this computer")
    } else {
        format!(
            "Currently using: {} · {count} on this computer",
            glib::format_size(status.spool_bytes)
        )
    }
}

/// "47 backups indexed · 132 MB on disk".
pub fn catalogue_summary(backups: usize, bytes: u64) -> String {
    let count = match backups {
        1 => "1 backup".to_string(),
        n => format!("{n} backups"),
    };
    format!("{count} indexed · {} on disk", glib::format_size(bytes))
}

/// The backed-up folders, as the Backup page lists them: the ones inside the
/// home folder together under one "Home" row, as in mockup 16, and anything
/// elsewhere on a row of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folders {
    pub in_home: Vec<PathBuf>,
    pub elsewhere: Vec<PathBuf>,
}

pub fn folders(include: &[PathBuf], home: &Path) -> Folders {
    let (in_home, elsewhere) = include
        .iter()
        .cloned()
        .partition(|path| path.starts_with(home) && path != home);
    Folders { in_home, elsewhere }
}

/// A folder's name as a row shows it.
pub fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// The list with `added` folded in: nothing twice, and nothing already
/// inside a folder that is backed up anyway.
pub fn with_folders(include: &[PathBuf], added: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = include.to_vec();
    for path in added {
        if !out.iter().any(|kept| path.starts_with(kept)) {
            out.push(path.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use backtrack_core::config::Config;

    /// S09-T4's acceptance: every key `config.toml` can hold has a control,
    /// and exactly one. The list of keys is generated from the schema, not
    /// written out here, so it cannot fall behind it.
    #[test]
    fn every_setting_in_the_file_has_exactly_one_control() {
        let mut config = Config::default();
        // Absent values are not written, so give every optional one a value
        // to make the schema list all of its keys.
        config.storage.repository = Some("/mnt/backups".into());
        config.advanced.upload_limit_mbps = Some(10);
        let mut schema = config.keys().unwrap();
        schema.sort();

        let mut controls: Vec<String> = Setting::ALL
            .iter()
            .map(|setting| setting.key().to_string())
            .collect();
        controls.sort();
        let before = controls.len();
        controls.dedup();
        assert_eq!(controls.len(), before, "a key with two controls");
        assert_eq!(controls, schema);
    }

    #[test]
    fn a_hand_set_space_limit_is_offered_rather_than_lost() {
        assert_eq!(space_limits(10), vec![5, 10, 20, 50, 100]);
        assert_eq!(space_limits(15), vec![5, 10, 15, 20, 50, 100]);
    }

    fn info(stored: u64, original: u64, capacity: u64) -> StorageInfo {
        StorageInfo {
            encryption: "repokey-blake2".into(),
            stored,
            original,
            capacity,
            free: 0,
        }
    }

    #[test]
    fn space_used_reads_like_the_mockup() {
        let (text, fraction) =
            space_used(&info(412_000_000_000, 1_900_000_000_000, 2_000_000_000_000));
        assert_eq!(
            text,
            "412.0\u{a0}GB of 2.0\u{a0}TB — 1.9\u{a0}TB before deduplication"
        );
        assert_eq!(fraction, Some(0.206));
        let (text, fraction) = space_used(&info(1_000, 2_000, 0));
        assert!(!text.contains(" of "), "{text}");
        assert_eq!(
            fraction, None,
            "no bar without a capacity to measure against"
        );
    }

    #[test]
    fn an_unencrypted_repository_is_not_called_encrypted() {
        assert_eq!(encryption("repokey-blake2").0, "Encrypted");
        assert_eq!(
            encryption("keyfile").1,
            "AES-256 — key stored on this computer"
        );
        assert_eq!(encryption("none").0, "Not encrypted");
        assert_eq!(encryption("authenticated-blake2").0, "Not encrypted");
    }

    fn status() -> Status {
        Status {
            state: "HEALTHY".into(),
            last_backup: 0,
            next_backup: 0,
            destination_reachable: true,
            spool_bytes: 1_200_000_000,
            offline_mode: "spool".into(),
            local_snapshots: 14,
            expirable_snapshots: 0,
            active_job: 0,
            paused_until: 0,
            configured: true,
        }
    }

    /// Wednesday 2026-06-10 12:00:00 UTC.
    const NOW: i64 = 1_781_092_800;

    #[test]
    fn the_next_backup_is_said_the_way_the_mockup_says_it() {
        let utc = glib::TimeZone::utc();
        let mut s = status();
        s.next_backup = (NOW + 5 * 3_600) as u64;
        assert_eq!(next_backup(&s, NOW, &utc), "Next backup: today at 17:00");
        s.next_backup = (NOW + 21 * 3_600) as u64;
        assert_eq!(next_backup(&s, NOW, &utc), "Next backup: tomorrow at 09:00");
        s.next_backup = 0;
        assert_eq!(next_backup(&s, NOW, &utc), "No backups are scheduled");
        s.paused_until = (NOW + 60) as u64;
        assert_eq!(next_backup(&s, NOW, &utc), "Backups are paused");
    }

    #[test]
    fn the_last_backup_reads_like_the_mockup() {
        let utc = glib::TimeZone::utc();
        let mut s = status();
        assert_eq!(last_backup(&s, NOW, &utc), "Never");
        s.last_backup = (NOW - 4 * 3_600) as u64;
        assert_eq!(last_backup(&s, NOW, &utc), "Today, 08:00");
    }

    #[test]
    fn local_usage_leaves_out_a_size_it_cannot_know() {
        let mut s = status();
        assert_eq!(
            local_usage(&s),
            "Currently using: 1.2\u{a0}GB · 14 snapshots on this computer"
        );
        s.offline_mode = "fs-snapshot".into();
        s.local_snapshots = 1;
        assert_eq!(
            local_usage(&s),
            "Currently using: 1 snapshot on this computer"
        );
    }

    #[test]
    fn folders_in_the_home_folder_are_listed_together() {
        let home = PathBuf::from("/home/k");
        let found = folders(
            &[
                home.join("Documents"),
                PathBuf::from("/srv/data"),
                home.join(".ssh"),
            ],
            &home,
        );
        assert_eq!(
            found.in_home,
            vec![home.join("Documents"), home.join(".ssh")]
        );
        assert_eq!(found.elsewhere, vec![PathBuf::from("/srv/data")]);
    }

    #[test]
    fn adding_a_folder_already_covered_changes_nothing() {
        let home = PathBuf::from("/home/k");
        let now = vec![home.join("Documents")];
        assert_eq!(
            with_folders(
                &now,
                &[home.join("Documents/Taxes"), home.join("Documents")]
            ),
            now
        );
        assert_eq!(
            with_folders(&now, &[home.join("Music")]),
            vec![home.join("Documents"), home.join("Music")]
        );
    }

    #[test]
    fn the_catalogue_line_counts_properly() {
        assert_eq!(
            catalogue_summary(47, 132_000_000),
            "47 backups indexed · 132.0\u{a0}MB on disk"
        );
        assert!(catalogue_summary(1, 0).starts_with("1 backup indexed"));
    }
}
