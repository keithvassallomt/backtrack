// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The onboarding wizard, worked out before a widget is involved.
//!
//! What the wizard *decides* lives here: which folders "My personal files"
//! means, where on a drive a repository goes, what the space check says, how
//! strong a passphrase is, whether the recovery-key gate is open, and, on a
//! second run, which settings actually changed. Those are the parts where a
//! mistake costs somebody their backups or their settings, and the parts a
//! test can check without a display.

use std::path::{Path, PathBuf};

use backtrack_core::config::{self, Config, Frequency, Retention};
use gtk4::glib;

/// Dot-folders that go with "My personal files", beside the five the wizard
/// names.
///
/// Chosen for what is painful to lose and cheap to keep: SSH and GPG keys,
/// which cannot be recreated at all, and application settings, which are
/// small and take an afternoon to rebuild by hand. Not `.local/share`, which
/// is where games, Flatpak data and the Trash live, and routinely outweighs
/// everything else in a home folder.
pub const PERSONAL_EXTRAS: &[&str] = &[".config", ".ssh", ".gnupg"];

/// What "My personal files" means on this machine: the Documents, Pictures,
/// Music, Downloads and Desktop folders the XDG settings name, plus
/// [`PERSONAL_EXTRAS`], keeping those that exist.
///
/// `special` is GLib's answer for each of the five, which is how a German
/// desktop's `Dokumente` is found without guessing. A folder the settings
/// point at the home folder itself is left out: some setups do that to
/// Downloads, and taking it at its word would quietly turn "my personal
/// files" into "everything".
pub fn personal_sources(
    home: &Path,
    special: &[Option<PathBuf>],
    exists: impl Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    let mut sources: Vec<PathBuf> = Vec::new();
    let named = special.iter().flatten().cloned();
    let extras = PERSONAL_EXTRAS.iter().map(|name| home.join(name));
    for path in named.chain(extras) {
        if path == home || !path.starts_with(home) || sources.contains(&path) {
            continue;
        }
        if exists(&path) {
            sources.push(path);
        }
    }
    sources
}

/// Which of the two choices on the "what" page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sources {
    Personal,
    Chosen(Vec<PathBuf>),
}

/// Everything the wizard collects, before any of it is written.
///
/// Held here rather than sent to the daemon page by page, because a wizard
/// that is closed halfway must leave the settings as they were. On a second
/// run that is the whole difference between "reconfigure" and "break".
#[derive(Debug, Clone, PartialEq)]
pub struct Choices {
    pub sources: Sources,
    pub exclude: Vec<String>,
    pub frequency: Frequency,
    pub on_battery: bool,
    pub on_metered: bool,
    pub retention: Retention,
    pub remember: bool,
}

impl Choices {
    /// A first run: the recommended settings.
    pub fn fresh() -> Choices {
        Choices::from_config(&Config::default(), &[])
    }

    /// A second run, starting from what is configured now.
    pub fn from_config(config: &Config, personal: &[PathBuf]) -> Choices {
        let include = &config.backup.include;
        let sources = if include.is_empty() || same_set(include, personal) {
            Sources::Personal
        } else {
            Sources::Chosen(include.clone())
        };
        Choices {
            sources,
            exclude: config.backup.exclude.clone(),
            frequency: config.backup.frequency,
            on_battery: config.backup.on_battery,
            on_metered: config.backup.on_metered,
            retention: config.storage.retention,
            remember: config.security.remember_passphrase,
        }
    }

    /// The folders these choices back up.
    pub fn include(&self, personal: &[PathBuf]) -> Vec<PathBuf> {
        match &self.sources {
            Sources::Personal => personal.to_vec(),
            Sources::Chosen(chosen) => chosen.clone(),
        }
    }
}

fn same_set(a: &[PathBuf], b: &[PathBuf]) -> bool {
    a.len() == b.len() && a.iter().all(|path| b.contains(path))
}

/// The settings a finished wizard writes: `SetConfig` key and value for each
/// one that differs from `current`, and nothing else.
///
/// Nothing else is the point. A second run that changes only the frequency
/// must leave every other line of the configuration as it found it, including
/// ones a person set by hand in Preferences that the wizard has no opinion
/// about. The destination is not in here at all: that is `SetupRepo`'s or
/// `ImportRepo`'s to write, because it comes with a repository.
pub fn changes(
    current: &Config,
    choices: &Choices,
    personal: &[PathBuf],
) -> config::Result<Vec<(&'static str, String)>> {
    let mut out = Vec::new();
    let mut compare = |key: &'static str, before: String, after: String| {
        if before != after {
            out.push((key, after));
        }
    };
    use config::toml_literal as lit;

    let now = &current.backup;
    let retention = &current.storage.retention;
    let wanted = &choices.retention;
    compare(
        "backup.include",
        lit(&now.include)?,
        lit(&choices.include(personal))?,
    );
    compare("backup.exclude", lit(&now.exclude)?, lit(&choices.exclude)?);
    compare(
        "backup.frequency",
        lit(&now.frequency)?,
        lit(&choices.frequency)?,
    );
    compare(
        "backup.on_battery",
        lit(&now.on_battery)?,
        lit(&choices.on_battery)?,
    );
    compare(
        "backup.on_metered",
        lit(&now.on_metered)?,
        lit(&choices.on_metered)?,
    );
    compare(
        "storage.retention.automatic",
        lit(&retention.automatic)?,
        lit(&wanted.automatic)?,
    );
    compare(
        "storage.retention.keep_hourly",
        lit(&retention.keep_hourly)?,
        lit(&wanted.keep_hourly)?,
    );
    compare(
        "storage.retention.keep_daily",
        lit(&retention.keep_daily)?,
        lit(&wanted.keep_daily)?,
    );
    compare(
        "storage.retention.keep_weekly",
        lit(&retention.keep_weekly)?,
        lit(&wanted.keep_weekly)?,
    );
    compare(
        "storage.retention.keep_monthly",
        lit(&retention.keep_monthly)?,
        lit(&wanted.keep_monthly)?,
    );
    compare(
        "security.remember_passphrase",
        lit(&current.security.remember_passphrase)?,
        lit(&choices.remember)?,
    );
    Ok(out)
}

/// The folder on a drive or share that holds Backtrack's repositories, one per
/// computer, so a drive shared between two machines keeps them apart and a
/// person looking at the drive can tell what the folder is.
pub const REPOSITORY_FOLDER: &str = "Backtrack";

/// Where backups go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// A removable drive, by where it is mounted.
    Drive { mount: PathBuf, name: String },
    /// A network share, mounted through GIO and used through the local path
    /// GIO gives it. Also any other location GIO can reach, including a plain
    /// folder, typed as a path or a `file://` address.
    Network { address: String, path: PathBuf },
    /// An SSH server, as Borg addresses it.
    Ssh { address: String },
    /// A second run keeping the repository it already has.
    Current { repository: String },
}

impl Destination {
    /// The repository this destination means, on the computer called `host`.
    ///
    /// On a drive or a share the repository gets a folder of its own. On an
    /// SSH server it is exactly what was typed, because a hosting service such
    /// as BorgBase hands out the full repository address and anything added to
    /// it would point somewhere that does not exist.
    pub fn repository(&self, host: &str) -> String {
        match self {
            Destination::Drive { mount: root, .. } | Destination::Network { path: root, .. } => {
                root.join(REPOSITORY_FOLDER)
                    .join(computer_folder(host))
                    .to_string_lossy()
                    .into_owned()
            }
            Destination::Ssh { address } => address.clone(),
            Destination::Current { repository } => repository.clone(),
        }
    }

    /// Where the free space is measured, when it can be.
    pub fn local_root(&self) -> Option<&Path> {
        match self {
            Destination::Drive { mount, .. } => Some(mount),
            Destination::Network { path, .. } => Some(path),
            Destination::Current { repository } => {
                let path = Path::new(repository);
                path.is_absolute().then_some(path)
            }
            Destination::Ssh { .. } => None,
        }
    }
}

/// A computer's name as a folder name. Host names do not normally contain a
/// slash, but one that did would put the repository somewhere unexpected.
fn computer_folder(host: &str) -> String {
    let cleaned: String = host
        .trim()
        .chars()
        .map(|c| if c == '/' { '-' } else { c })
        .collect();
    if cleaned.is_empty() {
        "this-computer".to_string()
    } else {
        cleaned
    }
}

/// Check an SSH address before anything is asked of it.
///
/// Borg takes two spellings, `ssh://user@host:port/path` and the scp-style
/// `user@host:path`, and both are accepted exactly as typed. What is refused
/// is anything Borg would read as a local path, which would otherwise create
/// a repository in a folder called `nas.local:backups` in the current
/// directory.
pub fn parse_ssh(input: &str) -> Option<String> {
    let address = input.trim();
    if let Some(rest) = address.strip_prefix("ssh://") {
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        if host.is_empty() || path.is_empty() {
            return None;
        }
        return Some(address.to_string());
    }
    if address.starts_with('/') || address.contains("://") {
        return None;
    }
    let (authority, path) = address.split_once(':')?;
    let host = authority.rsplit('@').next()?;
    if host.is_empty() || path.is_empty() || host.contains('/') {
        return None;
    }
    Some(address.to_string())
}

/// Whether the selection fits at the destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    Plenty,
    Short,
}

/// The space check line: the lead in bold, and the numbers after it.
///
/// "Needed" is the size of the files, before Borg compresses and deduplicates
/// them, so it is the most a first backup can take. A destination with less
/// free space than that might still manage, but saying it fits would be a
/// promise nothing here can keep.
pub fn space_check(needed: u64, free: u64) -> (Fit, &'static str, String) {
    let numbers = format!(
        "{} needed, {} free",
        glib::format_size(needed),
        glib::format_size(free)
    );
    if needed <= free {
        (Fit::Plenty, "Plenty of space:", numbers)
    } else {
        (Fit::Short, "Not enough space:", numbers)
    }
}

/// How strong a passphrase is, as the meter shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Strength {
    /// Filled segments, out of [`Strength::SEGMENTS`].
    pub level: u32,
    pub label: &'static str,
}

impl Strength {
    pub const SEGMENTS: u32 = 4;
}

/// Rate a passphrase with zxcvbn.
///
/// "Strong" is zxcvbn's top score: more than 10^10 guesses to find, which is
/// the level its authors describe as surviving an offline attack against a
/// slow hash. That is the attack that matters here, since anybody holding a
/// copy of the backup drive can try passphrases against the repository key at
/// leisure. `user_inputs` are words the person is likely to have used, the
/// computer's name among them, which zxcvbn treats as already known.
pub fn strength(passphrase: &str, user_inputs: &[&str]) -> Strength {
    if passphrase.is_empty() {
        return Strength {
            level: 0,
            label: "",
        };
    }
    let score = u8::from(zxcvbn::zxcvbn(passphrase, user_inputs).score());
    match score {
        0 | 1 => Strength {
            level: 1,
            label: "weak",
        },
        2 => Strength {
            level: 2,
            label: "fair",
        },
        3 => Strength {
            level: 3,
            label: "good",
        },
        _ => Strength {
            level: 4,
            label: "strong",
        },
    }
}

/// The "Protect your backups" page's gate.
///
/// The recovery key only exists once the repository does, so the order is
/// forced: a passphrase that is usable, then the repository created with it,
/// then the key saved or printed, and only then the way forward. Holding it
/// as data is what lets a test prove that "Start First Backup" cannot be
/// reached any other way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Protect {
    pub passphrase: String,
    pub confirm: String,
    /// The repository created with this passphrase, once it has been.
    pub created: Option<String>,
    /// Whether the recovery key for `created` has been saved or printed.
    pub key_kept: bool,
}

impl Protect {
    /// Whether the passphrase can be used: something typed, typed twice the
    /// same.
    pub fn passphrase_ready(&self) -> bool {
        !self.passphrase.is_empty() && self.passphrase == self.confirm
    }

    /// Whether Save and Print can be offered.
    pub fn can_keep_key(&self) -> bool {
        self.created.is_some() || self.passphrase_ready()
    }

    /// Whether the passphrase fields can still be edited. Not once the
    /// repository exists: it is encrypted with what was typed, and a change
    /// shown on screen that the repository never heard about would be a lie.
    pub fn editable(&self) -> bool {
        self.created.is_none()
    }

    /// Record that the repository at `repository` now exists.
    pub fn created(&mut self, repository: &str) {
        self.created = Some(repository.to_string());
        self.key_kept = false;
    }

    /// Record that the key was saved or printed.
    pub fn kept(&mut self) {
        if self.created.is_some() {
            self.key_kept = true;
        }
    }

    /// Point the page at a (possibly different) repository. Anything done for
    /// another one no longer counts, which is what makes choosing a different
    /// drive after saving a key ask for the new drive's key.
    pub fn retarget(&mut self, repository: &str) {
        if self.created.as_deref() != Some(repository) {
            self.created = None;
            self.key_kept = false;
        }
    }

    /// Whether the last page can go on: the repository exists and its
    /// recovery key has been kept somewhere.
    pub fn can_start(&self) -> bool {
        self.created.is_some() && self.key_kept
    }
}

/// The name the recovery key is offered under in the save dialog.
pub fn recovery_file_name(host: &str) -> String {
    format!("backtrack-recovery-key-{}.txt", computer_folder(host))
}

/// What the printed recovery sheet says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sheet {
    pub title: &'static str,
    /// Label and value for each line that says which backups this is for.
    pub facts: Vec<(&'static str, String)>,
    pub key: String,
    pub advice: Vec<String>,
}

/// The printed sheet: the key, what it belongs to, and what to do with it.
///
/// The one thing it must not leave out is that the key is not enough on its
/// own. Borg exports the key encrypted with the passphrase ("you will need
/// both KEY AND PASSPHRASE", it says itself when a repository is created), so
/// a sheet that let somebody believe it could stand in for a forgotten
/// passphrase would fail them at the worst possible moment.
pub fn recovery_sheet(key: &str, repository: &str, host: &str, printed: &str) -> Sheet {
    Sheet {
        title: "Backtrack recovery key",
        facts: vec![
            ("Computer", host.to_string()),
            ("Backups", repository.to_string()),
            ("Printed", printed.to_string()),
        ],
        key: key.trim_end().to_string(),
        advice: vec![
            "Keep this page somewhere safe, away from the computer it protects.".to_string(),
            "If the copy of this key stored with your backups is ever damaged, this key \
             makes them readable again. It works together with your passphrase: you will \
             need both."
                .to_string(),
            format!("Backtrack: {}", env!("CARGO_PKG_REPOSITORY")),
        ],
    }
}

/// The collapsed "Advanced: schedule & retention" row's summary, which is the
/// mockup's wording when nothing has been changed.
pub fn schedule_summary(frequency: Frequency, retention: &Retention) -> String {
    let cadence = match frequency {
        Frequency::Hourly => "Hourly backups",
        Frequency::Daily => "Daily backups",
        Frequency::Weekly => "Weekly backups",
        Frequency::Manual => "Backups when you ask",
    };
    format!("{cadence} · {}", retention_summary(retention))
}

/// What the retention policy keeps, in words.
pub fn retention_summary(retention: &Retention) -> String {
    let recommended = Retention::default();
    let counts = if retention.automatic {
        &recommended
    } else {
        retention
    };
    if counts.keep_hourly == recommended.keep_hourly
        && counts.keep_daily == recommended.keep_daily
        && counts.keep_weekly == recommended.keep_weekly
        && counts.keep_monthly == recommended.keep_monthly
    {
        return "keeps hourly 24h, daily a week, weekly a month, monthly 6 months".to_string();
    }
    format!(
        "keeps {} hourly, {} daily, {} weekly, {} monthly",
        counts.keep_hourly, counts.keep_daily, counts.keep_weekly, counts.keep_monthly
    )
}

/// The first backup's progress, as far as it can honestly be told.
///
/// Borg reports how much it has read but not how much there is, so the total
/// is the wizard's own estimate from the "what" page, and the bar stops short
/// of full until the backup says it has finished: an estimate that turned out
/// low must not show a finished bar over a backup still running.
pub fn first_backup_progress(read: u64, estimate: Option<u64>) -> (Option<f64>, String) {
    match estimate.filter(|total| *total > 0) {
        Some(total) => (
            Some((read as f64 / total as f64).min(0.99)),
            format!(
                "{} of about {}",
                glib::format_size(read),
                glib::format_size(total)
            ),
        ),
        None => (None, format!("{} so far", glib::format_size(read))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/home/k")
    }

    fn special(names: &[&str]) -> Vec<Option<PathBuf>> {
        names
            .iter()
            .map(|n| (!n.is_empty()).then(|| home().join(n)))
            .collect()
    }

    #[test]
    fn personal_files_are_the_named_folders_and_the_extras_that_exist() {
        let found = personal_sources(
            &home(),
            &special(&["Documents", "Pictures", "Music", "Downloads", "Desktop"]),
            |p| !p.ends_with(".gnupg") && !p.ends_with("Music"),
        );
        assert_eq!(
            found,
            vec![
                home().join("Documents"),
                home().join("Pictures"),
                home().join("Downloads"),
                home().join("Desktop"),
                home().join(".config"),
                home().join(".ssh"),
            ]
        );
    }

    #[test]
    fn a_special_folder_set_to_the_home_folder_is_not_taken_at_its_word() {
        // XDG_DOWNLOAD_DIR=$HOME is a real configuration, and "my personal
        // files" must not quietly become the entire home folder because of it.
        let mut dirs = special(&["Documents", "", "", "", ""]);
        dirs[3] = Some(home());
        let found = personal_sources(&home(), &dirs, |_| true);
        assert!(!found.contains(&home()), "{found:?}");
        assert!(found.contains(&home().join("Documents")));
    }

    #[test]
    fn a_second_run_recognises_personal_files_whatever_their_order() {
        let personal = vec![home().join("Documents"), home().join("Pictures")];
        let mut config = Config::default();
        config.backup.include = vec![home().join("Pictures"), home().join("Documents")];
        assert_eq!(
            Choices::from_config(&config, &personal).sources,
            Sources::Personal
        );
        config.backup.include = vec![home().join("Projects")];
        assert_eq!(
            Choices::from_config(&config, &personal).sources,
            Sources::Chosen(vec![home().join("Projects")])
        );
    }

    /// S09-T5's acceptance: a second run that changes only the frequency
    /// writes only the frequency.
    #[test]
    fn changing_only_the_frequency_changes_only_the_frequency() {
        let personal = vec![home().join("Documents")];
        let mut config = Config::default();
        config.storage.repository = Some("/run/media/k/WD/Backtrack/host".into());
        config.backup.include = personal.clone();
        // Settings the wizard never shows, and must never touch.
        config.backup.exclude.push("**/*.tmp".into());
        config.backup.on_metered = true;
        config.storage.retention.automatic = false;
        config.storage.retention.keep_daily = 14;
        config.advanced.upload_limit_mbps = Some(5);

        let mut choices = Choices::from_config(&config, &personal);
        assert_eq!(
            changes(&config, &choices, &personal).unwrap(),
            vec![],
            "a second run that changes nothing writes nothing"
        );

        choices.frequency = Frequency::Daily;
        assert_eq!(
            changes(&config, &choices, &personal).unwrap(),
            vec![("backup.frequency", "\"daily\"".to_string())]
        );
    }

    #[test]
    fn a_first_run_writes_what_differs_from_the_defaults() {
        let personal = vec![home().join("Documents")];
        let mut choices = Choices::fresh();
        choices.remember = false;
        let written = changes(&Config::default(), &choices, &personal).unwrap();
        assert_eq!(
            written,
            vec![
                ("backup.include", "[\"/home/k/Documents\"]".to_string()),
                ("security.remember_passphrase", "false".to_string()),
            ]
        );
    }

    #[test]
    fn a_drive_gets_a_folder_per_computer_and_ssh_is_taken_as_typed() {
        let drive = Destination::Drive {
            mount: PathBuf::from("/run/media/k/WD"),
            name: "WD My Passport".into(),
        };
        assert_eq!(
            drive.repository("thinkpad"),
            "/run/media/k/WD/Backtrack/thinkpad"
        );
        let ssh = Destination::Ssh {
            address: "ssh://x1y2@x1y2.repo.borgbase.com/./repo".into(),
        };
        assert_eq!(
            ssh.repository("thinkpad"),
            "ssh://x1y2@x1y2.repo.borgbase.com/./repo"
        );
        assert_eq!(ssh.local_root(), None, "no free space to measure over SSH");
        assert_eq!(
            drive.repository(""),
            "/run/media/k/WD/Backtrack/this-computer"
        );
    }

    #[test]
    fn ssh_addresses_in_either_spelling_are_accepted() {
        for good in [
            "ssh://keith@nas.local/./backups",
            "ssh://keith@nas.local:2222/volume1/backups",
            "keith@nas.local:backups",
            "  nas.local:/srv/backups  ",
        ] {
            assert!(parse_ssh(good).is_some(), "{good}");
        }
        assert_eq!(
            parse_ssh("  nas.local:/srv/backups  ").as_deref(),
            Some("nas.local:/srv/backups")
        );
    }

    #[test]
    fn anything_borg_would_read_as_a_local_path_is_refused() {
        for bad in [
            "",
            "/srv/backups",
            "nas.local",
            "keith@nas.local:",
            "ssh://nas.local",
            "smb://nas.local/backups",
            "./nas:backups",
        ] {
            assert_eq!(parse_ssh(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_space_check_says_whether_it_fits() {
        let (fit, lead, _) = space_check(118_000_000_000, 2_000_000_000_000);
        assert_eq!((fit, lead), (Fit::Plenty, "Plenty of space:"));
        let (fit, lead, numbers) = space_check(118_000_000_000, 50_000_000_000);
        assert_eq!((fit, lead), (Fit::Short, "Not enough space:"));
        // GLib keeps the number and its unit together with a no-break space.
        assert_eq!(numbers, "118.0\u{a0}GB needed, 50.0\u{a0}GB free");
    }

    #[test]
    fn strength_follows_zxcvbn_not_the_character_classes() {
        // Upper case, a digit and a symbol, and still one of the first
        // passwords anybody tries.
        assert_eq!(strength("Password1!", &[]).label, "weak");
        assert_eq!(
            strength("correct horse battery staple", &[]).label,
            "strong"
        );
        assert_eq!(strength("", &[]).level, 0);
        // The computer's own name is no secret.
        assert!(strength("thinkpad2026", &["thinkpad"]).level <= 2);
    }

    /// S09-T2's acceptance, at the level the page is built on: Start cannot
    /// be reached without the key being kept.
    #[test]
    fn start_is_gated_on_the_recovery_key() {
        let mut page = Protect {
            passphrase: "correct horse battery staple".into(),
            confirm: "correct horse battery staple".into(),
            ..Protect::default()
        };
        assert!(page.can_keep_key());
        assert!(
            !page.can_start(),
            "a good passphrase alone does not open it"
        );

        page.kept();
        assert!(
            !page.can_start(),
            "there is no key to keep until the repository exists"
        );

        page.created("/drive/Backtrack/host");
        assert!(!page.can_start(), "created, but the key is not kept yet");
        assert!(!page.editable());

        page.kept();
        assert!(page.can_start());
    }

    #[test]
    fn choosing_another_destination_asks_for_that_destinations_key() {
        let mut page = Protect {
            passphrase: "p".into(),
            confirm: "p".into(),
            ..Protect::default()
        };
        page.created("/a/Backtrack/host");
        page.kept();
        page.retarget("/a/Backtrack/host");
        assert!(page.can_start(), "the same destination keeps its key");
        page.retarget("/b/Backtrack/host");
        assert!(!page.can_start());
        assert!(page.editable());
    }

    #[test]
    fn mismatched_or_empty_passphrases_offer_nothing() {
        let mut page = Protect::default();
        assert!(!page.can_keep_key());
        page.passphrase = "one".into();
        page.confirm = "two".into();
        assert!(!page.can_keep_key());
    }

    #[test]
    fn the_schedule_summary_matches_the_mockup_by_default() {
        assert_eq!(
            schedule_summary(Frequency::Hourly, &Retention::default()),
            "Hourly backups · keeps hourly 24h, daily a week, weekly a month, monthly 6 months"
        );
        let custom = Retention {
            automatic: false,
            keep_hourly: 12,
            ..Retention::default()
        };
        assert_eq!(
            retention_summary(&custom),
            "keeps 12 hourly, 7 daily, 4 weekly, 6 monthly"
        );
        // Automatic means the recommended ladder, whatever is stored beside it.
        let stored = Retention {
            automatic: true,
            ..custom
        };
        assert!(retention_summary(&stored).starts_with("keeps hourly 24h"));
    }

    #[test]
    fn progress_never_shows_a_finished_bar_over_a_running_backup() {
        let (fraction, _) = first_backup_progress(200, Some(100));
        assert_eq!(fraction, Some(0.99));
        let (fraction, text) = first_backup_progress(50, None);
        assert_eq!(fraction, None);
        assert_eq!(text, "50 bytes so far");
    }

    #[test]
    fn the_printed_sheet_says_the_passphrase_is_needed_too() {
        let sheet = recovery_sheet(
            "BORG_KEY 1234\nhqlhbGdvcml0aG2m\n",
            "/run/media/k/WD/Backtrack/thinkpad",
            "thinkpad",
            "25 Sep 2026",
        );
        assert_eq!(sheet.key, "BORG_KEY 1234\nhqlhbGdvcml0aG2m");
        assert!(sheet
            .facts
            .contains(&("Backups", "/run/media/k/WD/Backtrack/thinkpad".to_string())));
        assert!(
            sheet.advice.iter().any(|line| line.contains("need both")),
            "{:?}",
            sheet.advice
        );
    }

    #[test]
    fn the_recovery_file_is_named_for_the_computer() {
        assert_eq!(
            recovery_file_name("thinkpad"),
            "backtrack-recovery-key-thinkpad.txt"
        );
    }
}
