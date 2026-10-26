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

use backtrack_core::config::{Compression, Config, Notifications};
use backtrack_core::dbus::{SnapperInfo, Status, StorageInfo};
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

/// A file-manager integration Backtrack ships, as General lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plugin {
    Nautilus,
    Dolphin,
}

impl Plugin {
    pub const ALL: [Plugin; 2] = [Plugin::Nautilus, Plugin::Dolphin];

    pub fn title(self) -> &'static str {
        match self {
            Plugin::Nautilus => "GNOME Files (Nautilus)",
            Plugin::Dolphin => "Dolphin",
        }
    }

    /// The switch that turns its items off without uninstalling it.
    pub fn setting(self) -> Setting {
        match self {
            Plugin::Nautilus => Setting::NautilusIntegration,
            Plugin::Dolphin => Setting::DolphinIntegration,
        }
    }

    /// What its switch says in `config`.
    pub fn enabled(self, config: &Config) -> bool {
        match self {
            Plugin::Nautilus => config.general.nautilus_integration,
            Plugin::Dolphin => config.general.dolphin_integration,
        }
    }

    /// Its file, relative to a data directory: `~/.local/share` for one
    /// installed for development, `/usr/share` for a package's. Dolphin's
    /// second file always comes with the first.
    fn file(self) -> &'static str {
        match self {
            Plugin::Nautilus => "nautilus-python/extensions/backtrack.py",
            Plugin::Dolphin => "kio/servicemenus/backtrack.desktop",
        }
    }

    /// What a Flatpak install says once it has put its copy in place: when
    /// the file manager shows the items.
    pub fn installed_note(self) -> &'static str {
        match self {
            Plugin::Nautilus => "Installed. Files shows Backtrack's items once it is restarted.",
            Plugin::Dolphin => "Installed. Backtrack's items are in Dolphin's menu now.",
        }
    }

    /// Whether it is installed in any of `data_dirs`, the directories both
    /// file managers load these from: the user's data directory, then the
    /// system's.
    pub fn installed_in(self, data_dirs: &[PathBuf]) -> bool {
        data_dirs.iter().any(|dir| dir.join(self.file()).is_file())
    }

    /// Its file manager's program.
    fn program(self) -> &'static str {
        match self {
            Plugin::Nautilus => "nautilus",
            Plugin::Dolphin => "dolphin",
        }
    }

    /// Whether its file manager is installed: its program in one of
    /// `program_dirs`.
    pub fn manager_in(self, program_dirs: &[PathBuf]) -> bool {
        program_dirs
            .iter()
            .any(|dir| dir.join(self.program()).is_file())
    }
}

/// The page How to Install… opens: how to install either integration.
pub fn install_page() -> String {
    format!(
        "{}/blob/main/docs/file-manager-integration.md#installing",
        env!("CARGO_PKG_REPOSITORY")
    )
}

/// The guide to Snapper (Stage 17), at `section` if one is named.
pub fn snapper_guide(section: Option<&str>) -> String {
    let page = format!("{}/blob/main/docs/snapper.md", env!("CARGO_PKG_REPOSITORY"));
    match section {
        Some(section) => format!("{page}#{section}"),
        None => page,
    }
}

/// The section of the guide on letting Backtrack read Snapper's snapshots.
pub const SNAPPER_ACCESS: &str = "giving-backtrack-access";

/// What Preferences → Storage says about Snapper's snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapperRow {
    pub title: String,
    pub subtitle: String,
    /// The guide, or the part of it, worth reading from here.
    pub guide: Option<String>,
}

/// The setup script's name, as every native package installs it (S17-T6).
pub const SNAPPER_SCRIPT: &str = "backtrack-snapper-setup";

/// Where this Backtrack comes from, as far as running the setup script goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packaged {
    /// Installed on the system, or a development build, whose copy of the
    /// script is in the checkout.
    Native { checkout: Option<PathBuf> },
    /// The Flatpak, whose data folder a terminal outside its sandbox reaches.
    Flatpak { data_dir: PathBuf },
}

/// How **Set Up Snapper…** runs the setup script (S17-T7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetUp {
    /// In the desktop's terminal, as this command line.
    Terminal(String),
    /// Pasted into a terminal by the person. `script`, when given, is where
    /// the script is to be put first: the Flatpak's copy is inside its
    /// sandbox, where no terminal outside it can run it.
    Paste {
        command: String,
        script: Option<PathBuf>,
    },
}

/// What **Set Up Snapper…** does, or None where it is not offered: where the
/// script would not run, and where Snapper's snapshots are already being
/// read. Offered where Snapper is not set up, or its snapshots cannot be
/// read, which the script sees to as well.
pub fn snapper_setup(info: &SnapperInfo, packaged: &Packaged) -> Option<SetUp> {
    let wanted = info.can_set_up && (info.folders == 0 || !info.unreadable.is_empty());
    if !wanted {
        return None;
    }
    Some(match packaged {
        Packaged::Native { checkout } => {
            let program = match checkout {
                Some(script) => quoted(script),
                None => SNAPPER_SCRIPT.to_string(),
            };
            // A terminal opened for one command closes when it ends, before
            // anyone could read what it said.
            SetUp::Terminal(format!("{program} --pause"))
        }
        Packaged::Flatpak { data_dir } => {
            let script = data_dir.join(SNAPPER_SCRIPT);
            SetUp::Paste {
                command: format!("bash {}", quoted(&script)),
                script: Some(script),
            }
        }
    })
}

/// When no terminal could be opened: the same command, to paste into one.
pub fn snapper_setup_by_hand(terminal: &str) -> SetUp {
    SetUp::Paste {
        command: terminal.trim_end_matches(" --pause").to_string(),
        script: None,
    }
}

fn quoted(path: &Path) -> String {
    glib::shell_quote(path).to_string_lossy().into_owned()
}

/// What to say about Snapper, from what the daemon found. None where no
/// backed-up folder is on btrfs, which Snapper needs, and where Snapper is
/// not set up and the setup script would not run here (on a distribution it
/// does not know, or with `/home` inside a larger subvolume, as Ubuntu's
/// installer lays btrfs out): there is nothing to offer, and saying so would
/// only be noise. Snapshots found, or found and not readable, are reported
/// wherever they are.
pub fn snapper_row(info: &SnapperInfo) -> Option<SnapperRow> {
    if !info.on_btrfs {
        return None;
    }
    if info.folders == 0 && info.unreadable.is_empty() && !info.can_set_up {
        return None;
    }
    let reading = match info.snapshots {
        1 => "Reading 1 snapshot taken by Snapper".to_string(),
        n => format!("Reading {n} snapshots taken by Snapper"),
    };
    if !info.unreadable.is_empty() {
        let folders = info.unreadable.join(", ");
        let also = if info.folders > 0 {
            format!(" {reading}, elsewhere.")
        } else {
            String::new()
        };
        return Some(SnapperRow {
            title: "Snapper's snapshots cannot be read".to_string(),
            subtitle: format!(
                "Snapper keeps them in {folders}, which only the administrator can \
                 read until Snapper is told to let you.{also}"
            ),
            guide: Some(snapper_guide(Some(SNAPPER_ACCESS))),
        });
    }
    if info.folders > 0 {
        return Some(SnapperRow {
            title: if info.snapshots == 0 {
                "Snapper is set up, and has taken no snapshots yet".to_string()
            } else {
                reading
            },
            subtitle: "They are shown in the timeline among your backups. They are kept on \
                       this computer's own disk until Snapper removes them, and are not backups."
                .to_string(),
            guide: None,
        });
    }
    Some(SnapperRow {
        title: "Snapper is not set up".to_string(),
        subtitle: "Your backed-up folders are on btrfs, where Snapper can take a snapshot \
                   every few minutes, and Backtrack can show them in the timeline."
            .to_string(),
        guide: Some(snapper_guide(None)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use backtrack_core::config::Config;

    /// Keys of `config.toml` whose one control is in the window rather than
    /// in Preferences, and where it is.
    const CONTROLLED_ELSEWHERE: &[(&str, &str)] = &[(
        "general.show_all_backups",
        "Show All Backups in the primary menu (S15-T7)",
    )];

    /// S09-T4's acceptance: every key `config.toml` can hold has a control,
    /// and exactly one, in Preferences or in [`CONTROLLED_ELSEWHERE`]. The
    /// list of keys is generated from the schema, not written out here, so it
    /// cannot fall behind it.
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
            .chain(CONTROLLED_ELSEWHERE.iter().map(|(key, _)| key.to_string()))
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
            reason: String::new(),
            since: 0,
            recovery_job: 0,
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
    fn each_integration_is_detected_on_its_own() {
        // S12-T5's acceptance: all four combinations, with one in the user's
        // data directory and the other where a package puts it.
        let user = tempfile::tempdir().unwrap();
        let system = tempfile::tempdir().unwrap();
        let dirs = vec![user.path().to_path_buf(), system.path().to_path_buf()];
        let install = |dir: &Path, plugin: Plugin| {
            let file = dir.join(plugin.file());
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "").unwrap();
        };
        let found = || Plugin::ALL.map(|plugin| plugin.installed_in(&dirs));

        assert_eq!(found(), [false, false]);
        install(user.path(), Plugin::Nautilus);
        assert_eq!(found(), [true, false]);
        install(system.path(), Plugin::Dolphin);
        assert_eq!(found(), [true, true]);
        std::fs::remove_file(user.path().join(Plugin::Nautilus.file())).unwrap();
        assert_eq!(found(), [false, true]);
    }

    #[test]
    fn each_file_manager_is_found_by_its_program() {
        let local = tempfile::tempdir().unwrap();
        let usr = tempfile::tempdir().unwrap();
        let dirs = vec![local.path().to_path_buf(), usr.path().to_path_buf()];
        let found = || Plugin::ALL.map(|plugin| plugin.manager_in(&dirs));

        assert_eq!(found(), [false, false]);
        std::fs::write(usr.path().join("dolphin"), "").unwrap();
        assert_eq!(found(), [false, true]);
        std::fs::write(local.path().join("nautilus"), "").unwrap();
        assert_eq!(found(), [true, true]);
        // A folder of the name is not the program.
        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir(other.path().join("nautilus")).unwrap();
        assert!(!Plugin::Nautilus.manager_in(&[other.path().to_path_buf()]));
    }

    #[test]
    fn the_install_page_is_a_section_of_the_integration_doc() {
        let doc = include_str!("../../../../docs/file-manager-integration.md");
        let anchor = install_page().rsplit_once('#').unwrap().1.to_string();
        assert!(
            doc.lines()
                .any(|line| line == format!("## {}", capitalised(&anchor))),
            "docs/file-manager-integration.md has no heading for #{anchor}"
        );
    }

    /// What the daemon finds, on a computer where the setup script would run.
    fn snapper(folders: u32, snapshots: u32, unreadable: &[&str]) -> SnapperInfo {
        SnapperInfo {
            on_btrfs: true,
            folders,
            snapshots,
            unreadable: unreadable.iter().map(|s| s.to_string()).collect(),
            can_set_up: true,
        }
    }

    /// The same, where it would not.
    fn cannot_set_up(info: SnapperInfo) -> SnapperInfo {
        SnapperInfo {
            can_set_up: false,
            ..info
        }
    }

    #[test]
    fn where_the_script_would_not_run_neither_it_nor_snapper_not_being_set_up_is_offered() {
        let native = Packaged::Native { checkout: None };
        let nothing = cannot_set_up(snapper(0, 0, &[]));
        assert_eq!(snapper_row(&nothing), None);
        assert_eq!(snapper_setup(&nothing, &native), None);

        // What was found is still said, without the button.
        let unreadable = cannot_set_up(snapper(0, 0, &["/data/.snapshots"]));
        assert_eq!(
            snapper_row(&unreadable).map(|r| r.title),
            Some("Snapper's snapshots cannot be read".to_string())
        );
        assert_eq!(snapper_setup(&unreadable, &native), None);
        let read = cannot_set_up(snapper(1, 3, &[]));
        assert_eq!(
            snapper_row(&read).map(|r| r.title),
            Some("Reading 3 snapshots taken by Snapper".to_string())
        );
    }

    #[test]
    fn with_no_folder_on_btrfs_there_is_nothing_to_say_about_snapper() {
        assert_eq!(snapper_row(&SnapperInfo::default()), None);
    }

    #[test]
    fn on_btrfs_with_no_snapper_the_guide_is_offered() {
        let row = snapper_row(&snapper(0, 0, &[])).unwrap();
        assert_eq!(row.title, "Snapper is not set up");
        assert_eq!(row.guide, Some(snapper_guide(None)));
    }

    #[test]
    fn snapshots_that_cannot_be_read_are_named_with_the_way_to_read_them() {
        let row = snapper_row(&snapper(0, 0, &["/home/.snapshots"])).unwrap();
        assert_eq!(row.title, "Snapper's snapshots cannot be read");
        assert!(
            row.subtitle.contains("/home/.snapshots"),
            "{}",
            row.subtitle
        );
        assert!(!row.subtitle.contains("Reading"), "{}", row.subtitle);
        assert_eq!(row.guide, Some(snapper_guide(Some(SNAPPER_ACCESS))));

        let mixed = snapper_row(&snapper(1, 5, &["/data/.snapshots"])).unwrap();
        assert!(
            mixed
                .subtitle
                .ends_with("Reading 5 snapshots taken by Snapper, elsewhere."),
            "{}",
            mixed.subtitle
        );
    }

    #[test]
    fn snapshots_being_read_are_counted() {
        let row = snapper_row(&snapper(1, 24, &[])).unwrap();
        assert_eq!(row.title, "Reading 24 snapshots taken by Snapper");
        assert!(row.subtitle.contains("are not backups"), "{}", row.subtitle);
        assert_eq!(row.guide, None);
        assert_eq!(
            snapper_row(&snapper(1, 1, &[])).unwrap().title,
            "Reading 1 snapshot taken by Snapper"
        );
        assert_eq!(
            snapper_row(&snapper(1, 0, &[])).unwrap().title,
            "Snapper is set up, and has taken no snapshots yet"
        );
    }

    #[test]
    fn set_up_snapper_is_offered_where_there_is_something_to_set_up() {
        let native = Packaged::Native { checkout: None };
        assert_eq!(
            snapper_setup(&SnapperInfo::default(), &native),
            None,
            "off btrfs"
        );
        assert_eq!(
            snapper_setup(&snapper(1, 24, &[]), &native),
            None,
            "being read already"
        );
        for info in [snapper(0, 0, &[]), snapper(0, 0, &["/home/.snapshots"])] {
            assert_eq!(
                snapper_setup(&info, &native),
                Some(SetUp::Terminal("backtrack-snapper-setup --pause".into())),
                "{info:?}"
            );
        }
        assert!(
            snapper_setup(&snapper(1, 5, &["/data/.snapshots"]), &native).is_some(),
            "some read, some not"
        );
    }

    #[test]
    fn a_development_build_runs_the_checkouts_script() {
        let checkout = Packaged::Native {
            checkout: Some(PathBuf::from(
                "/home/k/Code/back track/packaging/snapper/backtrack-snapper-setup",
            )),
        };
        assert_eq!(
            snapper_setup(&snapper(0, 0, &[]), &checkout),
            Some(SetUp::Terminal(
                "'/home/k/Code/back track/packaging/snapper/backtrack-snapper-setup' --pause"
                    .into()
            ))
        );
    }

    #[test]
    fn the_flatpak_hands_over_the_script_and_a_command_to_paste() {
        let flatpak = Packaged::Flatpak {
            data_dir: PathBuf::from("/home/k/.var/app/tools.backtrack.Backtrack/data/backtrack"),
        };
        let script = PathBuf::from(
            "/home/k/.var/app/tools.backtrack.Backtrack/data/backtrack/backtrack-snapper-setup",
        );
        assert_eq!(
            snapper_setup(&snapper(0, 0, &[]), &flatpak),
            Some(SetUp::Paste {
                command: format!("bash '{}'", script.display()),
                script: Some(script),
            })
        );
    }

    #[test]
    fn with_no_terminal_to_open_the_command_is_given_to_paste() {
        assert_eq!(
            snapper_setup_by_hand("backtrack-snapper-setup --pause"),
            SetUp::Paste {
                command: "backtrack-snapper-setup".into(),
                script: None
            }
        );
    }

    #[test]
    fn the_access_section_is_a_section_of_the_snapper_guide() {
        let doc = include_str!("../../../../docs/snapper.md");
        let anchor = snapper_guide(Some(SNAPPER_ACCESS));
        let anchor = anchor.rsplit_once('#').unwrap().1;
        assert!(
            doc.lines().any(|line| line == "## Giving Backtrack access"),
            "docs/snapper.md has no heading for #{anchor}"
        );
        assert_eq!(anchor, "giving-backtrack-access");
    }

    fn capitalised(word: &str) -> String {
        let mut chars = word.chars();
        chars
            .next()
            .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
            .unwrap_or_default()
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
