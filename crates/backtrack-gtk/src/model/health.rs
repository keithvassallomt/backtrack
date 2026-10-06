// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! What the window says about health, and which fix it offers.
//!
//! health.md decides all of it. A yellow banner when the backups are at risk
//! and a red one when they have stopped, each naming its fix. For the two
//! states that are the product working as designed, a quiet line rather than a
//! warning. `DEGRADED` is Preferences' to mention, and the window's too only
//! once it has lasted a week.

use backtrack_core::dbus::{Reason, Status};
use gtk4::glib;

use super::format;

/// How long `DEGRADED` is left to Preferences before the window mentions it.
pub const DEGRADED_BANNER_AFTER: i64 = 7 * 86_400;

/// health.md's own words for the offline line, kept as they are written.
pub const DESTINATION_AWAY: &str =
    "Backup drive not reachable — protecting changes on this computer.";

/// How loudly a banner speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// `AT_RISK`: yellow.
    Warning,
    /// `BROKEN`: red.
    Error,
    /// The states that are not warnings: a status line, not an alarm.
    Info,
}

/// What a banner's button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Open the fix for this reason.
    Fix(Reason),
    /// Lift the pause.
    Resume,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Fix(_) => "Fix…",
            Action::Resume => "Resume",
        }
    }
}

/// One banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Banner {
    pub tone: Tone,
    pub title: String,
    pub action: Option<Action>,
}

/// The banner for `status`, or none when there is nothing to say.
///
/// `destination` is what the person calls the place their backups go, for the
/// "can't sign in" copy. `not_browsable` is how many backups the catalogue
/// has not read yet, for the copy that counts them.
pub fn banner(
    status: &Status,
    destination: &str,
    not_browsable: usize,
    now: i64,
    tz: &glib::TimeZone,
) -> Option<Banner> {
    let reason = Reason::parse(&status.reason);
    match status.state.as_str() {
        "AT_RISK" => Some(Banner {
            tone: Tone::Warning,
            title: at_risk(status, now),
            action: Some(Action::Fix(Reason::NoRecentBackup)),
        }),
        "BROKEN" => {
            let reason = reason?;
            Some(Banner {
                tone: Tone::Error,
                title: reason.failure()?.copy(destination),
                action: Some(Action::Fix(reason)),
            })
        }
        "PROTECTED_LOCALLY" => Some(Banner {
            tone: Tone::Info,
            title: DESTINATION_AWAY.to_string(),
            action: None,
        }),
        "PAUSED" => Some(Banner {
            tone: Tone::Info,
            title: paused(status, now, tz),
            action: Some(Action::Resume),
        }),
        "DEGRADED" if now - status.since as i64 >= DEGRADED_BANNER_AFTER => {
            let reason = reason?;
            Some(Banner {
                tone: Tone::Info,
                title: attention(reason, not_browsable)?,
                action: match reason {
                    // Nothing to do but wait for it.
                    Reason::CatalogueRebuilding => None,
                    reason => Some(Action::Fix(reason)),
                },
            })
        }
        _ => None,
    }
}

/// What Preferences badges while the backups are `DEGRADED`, or nothing.
pub fn attention_badge(status: &Status, not_browsable: usize) -> Option<String> {
    if status.state != "DEGRADED" {
        return None;
    }
    attention(Reason::parse(&status.reason)?, not_browsable)
}

/// The words for something that needs eventual attention.
fn attention(reason: Reason, not_browsable: usize) -> Option<String> {
    match reason {
        Reason::LocalDiskFull => reason.failure().map(|failure| failure.copy("")),
        Reason::CatalogueRebuilding => Some("Catalogue rebuilding…".to_string()),
        Reason::NotYetBrowsable => Some(match not_browsable {
            // The daemon counted some, and the window's copy of the catalogue
            // has not caught up: there is at least one.
            0 | 1 => "1 backup not yet browsable".to_string(),
            n => format!("{n} backups not yet browsable"),
        }),
        _ => None,
    }
}

/// "No successful backups for 3 days", in mockup 23's words.
pub fn at_risk(status: &Status, now: i64) -> String {
    match status.last_backup {
        0 => "No successful backups yet".to_string(),
        last => {
            let days = ((now - last as i64) / 86_400).max(1);
            format!(
                "No successful backups for {days} {}",
                if days == 1 { "day" } else { "days" }
            )
        }
    }
}

/// "Backups are paused until 17:00", or until they are resumed.
fn paused(status: &Status, now: i64, tz: &glib::TimeZone) -> String {
    let until = status.paused_until as i64;
    if until - now > super::pause::INDEFINITE_PAUSE_THRESHOLD as i64 {
        "Backups are paused until you resume them".to_string()
    } else {
        format!("Backups are paused until {}", format::clock(until, tz))
    }
}

/// Where the backups go, as far as signing in again is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// An SSH server, which Backtrack signs in to with this computer's key.
    Server,
    /// A network share GIO mounted, which can be signed in to again.
    Share,
    /// A drive or a folder on this computer.
    Folder,
}

/// What kind of place `repository` is.
pub fn place(repository: &str) -> Place {
    if backtrack_core::destination::ssh_endpoint(repository).is_some() {
        return Place::Server;
    }
    let mut parts = repository.split('/').filter(|part| !part.is_empty());
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("run"), Some("user"), Some(_), Some("gvfs")) => Place::Share,
        _ => Place::Folder,
    }
}

/// The address of the share a GIO-mounted repository lives on, read from the
/// name GIO gives the mount's folder, for when the share is not mounted and
/// GIO cannot be asked. `None` for the kinds of share this does not know.
pub fn share_uri(repository: &str) -> Option<String> {
    let mount = repository
        .split('/')
        .filter(|part| !part.is_empty())
        .nth(4)?;
    let (kind, settings) = mount.split_once(':')?;
    let setting = |key: &str| {
        settings.split(',').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == key).then_some(v)
        })
    };
    match kind {
        "smb-share" => Some(format!(
            "smb://{}/{}",
            setting("server")?,
            setting("share")?
        )),
        "sftp" => Some(match setting("user") {
            Some(user) => format!("sftp://{user}@{}/", setting("host")?),
            None => format!("sftp://{}/", setting("host")?),
        }),
        _ => None,
    }
}

/// How to install Borg on this distribution, from `/etc/os-release`. `None`
/// when it is one this does not know, which is said in general terms instead.
pub fn install_command(os_release: &str) -> Option<&'static str> {
    let field = |key: &str| {
        os_release.lines().find_map(|line| {
            let value = line.strip_prefix(key)?.strip_prefix('=')?;
            Some(value.trim_matches('"').to_string())
        })
    };
    let ids = format!(
        "{} {}",
        field("ID").unwrap_or_default(),
        field("ID_LIKE").unwrap_or_default()
    );
    let family = |name: &str| ids.split_whitespace().any(|id| id == name);
    if family("fedora") || family("rhel") {
        Some("sudo dnf install borgbackup")
    } else if family("debian") || family("ubuntu") {
        Some("sudo apt install borgbackup")
    } else if family("arch") {
        Some("sudo pacman -S borg")
    } else if family("suse") || family("opensuse") {
        Some("sudo zypper install borgbackup")
    } else {
        None
    }
}

/// What the at-risk fix says about why nothing has been backed up.
///
/// `last_error` is the last failed backup: when it was, and what it said.
pub fn at_risk_detail(
    status: &Status,
    last_error: Option<(i64, &str)>,
    now: i64,
    tz: &glib::TimeZone,
) -> String {
    let last = status.last_backup as i64;
    let mut text = match last {
        0 => "Backtrack has not yet managed a backup.".to_string(),
        _ => format!("The last backup was {}.", super::status::ago(last, now, tz)),
    };
    if !status.destination_reachable {
        text.push_str(" The backup destination is not reachable");
        text.push_str(if status.offline_mode == "off" {
            ", and changes are not being kept on this computer while it is away."
        } else {
            "."
        });
    } else if let Some((at, message)) = last_error.filter(|(at, _)| *at > last) {
        text.push_str(&format!(
            " The last attempt, {}, did not finish: {message}",
            super::status::ago(at, now, tz)
        ));
        if !message.ends_with('.') {
            text.push('.');
        }
    } else {
        text.push_str(
            " Backups have not run since then. The computer may have been off or asleep \
             when they were due.",
        );
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wednesday 2026-06-10 12:00:00 UTC.
    const NOW: i64 = 1_781_092_800;
    const DAY: i64 = 86_400;

    fn utc() -> glib::TimeZone {
        glib::TimeZone::utc()
    }

    fn status(state: &str, reason: &str) -> Status {
        Status {
            state: state.into(),
            last_backup: (NOW - 3 * DAY - 3_600) as u64,
            next_backup: 0,
            destination_reachable: true,
            spool_bytes: 0,
            offline_mode: "spool".into(),
            local_snapshots: 0,
            expirable_snapshots: 0,
            active_job: 0,
            paused_until: 0,
            configured: true,
            reason: reason.into(),
            since: (NOW - 60) as u64,
            recovery_job: 0,
        }
    }

    fn shown(status: &Status) -> Option<Banner> {
        banner(status, "nas.local", 0, NOW, &utc())
    }

    #[test]
    fn at_risk_is_mockup_23_word_for_word() {
        assert_eq!(
            shown(&status("AT_RISK", "no-recent-backup")),
            Some(Banner {
                tone: Tone::Warning,
                title: "No successful backups for 3 days".into(),
                action: Some(Action::Fix(Reason::NoRecentBackup)),
            })
        );
        assert_eq!(Action::Fix(Reason::NoRecentBackup).label(), "Fix…");
    }

    #[test]
    fn at_risk_counts_whole_days_and_never_says_none() {
        let mut s = status("AT_RISK", "no-recent-backup");
        s.last_backup = (NOW - DAY - 60) as u64;
        assert_eq!(at_risk(&s, NOW), "No successful backups for 1 day");
        s.last_backup = (NOW - 600) as u64;
        assert_eq!(at_risk(&s, NOW), "No successful backups for 1 day");
        s.last_backup = 0;
        assert_eq!(at_risk(&s, NOW), "No successful backups yet");
    }

    #[test]
    fn every_broken_row_is_red_in_its_catalogue_words_and_opens_its_fix() {
        for failure in backtrack_core::engine::HealthFailure::ALL {
            let reason = Reason::from(*failure);
            let banner = shown(&status("BROKEN", reason.as_str())).unwrap();
            assert_eq!(banner.tone, Tone::Error);
            assert_eq!(banner.title, failure.copy("nas.local"));
            assert_eq!(banner.action, Some(Action::Fix(reason)));
        }
        assert_eq!(
            shown(&status("BROKEN", "auth-failed")).unwrap().title,
            "Backtrack can't sign in to nas.local."
        );
    }

    #[test]
    fn being_away_is_a_quiet_line_with_nothing_to_fix() {
        assert_eq!(
            shown(&status("PROTECTED_LOCALLY", "destination-away")),
            Some(Banner {
                tone: Tone::Info,
                title: DESTINATION_AWAY.into(),
                action: None,
            })
        );
    }

    #[test]
    fn a_pause_says_when_it_lifts_and_offers_to_lift_it() {
        let mut s = status("PAUSED", "paused");
        s.paused_until = (NOW + 5 * 3_600) as u64;
        assert_eq!(
            shown(&s),
            Some(Banner {
                tone: Tone::Info,
                title: "Backups are paused until 17:00".into(),
                action: Some(Action::Resume),
            })
        );
        s.paused_until = (NOW + 100 * 365 * DAY) as u64;
        assert_eq!(
            shown(&s).unwrap().title,
            "Backups are paused until you resume them"
        );
    }

    #[test]
    fn degraded_waits_a_week_before_the_window_mentions_it() {
        let mut s = status("DEGRADED", "local-disk-full");
        s.since = (NOW - 6 * DAY) as u64;
        assert_eq!(shown(&s), None, "Preferences only, for now");
        assert_eq!(
            attention_badge(&s, 0).as_deref(),
            Some("Not enough space on this computer to keep protecting changes.")
        );

        s.since = (NOW - 7 * DAY) as u64;
        assert_eq!(
            shown(&s),
            Some(Banner {
                tone: Tone::Info,
                title: "Not enough space on this computer to keep protecting changes.".into(),
                action: Some(Action::Fix(Reason::LocalDiskFull)),
            })
        );
    }

    #[test]
    fn the_catalogue_s_own_attention_copy_is_health_md_s() {
        let rebuilding = status("DEGRADED", "catalogue-rebuilding");
        assert_eq!(
            attention_badge(&rebuilding, 0).as_deref(),
            Some("Catalogue rebuilding…")
        );
        let pending = status("DEGRADED", "not-yet-browsable");
        assert_eq!(
            attention_badge(&pending, 1).as_deref(),
            Some("1 backup not yet browsable")
        );
        assert_eq!(
            attention_badge(&pending, 4).as_deref(),
            Some("4 backups not yet browsable")
        );

        let mut old = rebuilding.clone();
        old.since = (NOW - 8 * DAY) as u64;
        assert_eq!(
            shown(&old).unwrap().action,
            None,
            "a rebuild is waited for, not fixed"
        );
    }

    #[test]
    fn a_destination_is_a_server_a_share_or_a_folder() {
        assert_eq!(place("ssh://keith@nas.local/./backups"), Place::Server);
        assert_eq!(place("x1@x1.repo.borgbase.com:repo"), Place::Server);
        assert_eq!(
            place("/run/user/1000/gvfs/smb-share:server=nas.local,share=backups/Backtrack/t"),
            Place::Share
        );
        assert_eq!(place("/run/media/keith/WD/Backtrack/t"), Place::Folder);
        assert_eq!(place("/srv/backups"), Place::Folder);
    }

    #[test]
    fn a_share_s_address_is_read_from_the_folder_gio_mounted_it_on() {
        assert_eq!(
            share_uri("/run/user/1000/gvfs/smb-share:server=nas.local,share=backups/Backtrack/t")
                .as_deref(),
            Some("smb://nas.local/backups")
        );
        assert_eq!(
            share_uri("/run/user/1000/gvfs/sftp:host=files.example.org,user=k/Backtrack/t")
                .as_deref(),
            Some("sftp://k@files.example.org/")
        );
        assert_eq!(
            share_uri("/run/user/1000/gvfs/afp-volume:host=old,volume=v/x"),
            None
        );
        assert_eq!(share_uri("/srv/backups"), None);
    }

    #[test]
    fn borg_is_installed_the_way_each_family_installs_things() {
        for (release, command) in [
            ("ID=fedora\n", Some("sudo dnf install borgbackup")),
            (
                "ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n",
                Some("sudo dnf install borgbackup"),
            ),
            (
                "ID=ubuntu\nID_LIKE=debian\n",
                Some("sudo apt install borgbackup"),
            ),
            (
                "ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n",
                Some("sudo apt install borgbackup"),
            ),
            ("ID=arch\n", Some("sudo pacman -S borg")),
            (
                "ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n",
                Some("sudo zypper install borgbackup"),
            ),
            ("ID=gentoo\n", None),
            ("", None),
        ] {
            assert_eq!(install_command(release), command, "{release:?}");
        }
    }

    #[test]
    fn the_at_risk_fix_says_what_it_knows_about_why() {
        let mut s = status("AT_RISK", "no-recent-backup");
        s.last_backup = (NOW - 3 * DAY) as u64;
        assert_eq!(
            at_risk_detail(
                &s,
                Some((NOW - 3_600, "borg exited with code 2: boom")),
                NOW,
                &utc()
            ),
            "The last backup was on Sun 7 Jun. The last attempt, 1 hour ago, did not \
             finish: borg exited with code 2: boom."
        );

        s.destination_reachable = false;
        s.offline_mode = "off".into();
        assert_eq!(
            at_risk_detail(&s, None, NOW, &utc()),
            "The last backup was on Sun 7 Jun. The backup destination is not reachable, \
             and changes are not being kept on this computer while it is away."
        );

        s.destination_reachable = true;
        assert!(
            at_risk_detail(&s, Some((NOW - 4 * DAY, "an older failure")), NOW, &utc())
                .ends_with("may have been off or asleep when they were due."),
            "a failure from before the last backup explains nothing"
        );

        s.last_backup = 0;
        assert!(at_risk_detail(&s, None, NOW, &utc()).starts_with("Backtrack has not yet managed"));
    }

    #[test]
    fn a_healthy_machine_shows_nothing_and_badges_nothing() {
        assert_eq!(shown(&status("HEALTHY", "")), None);
        assert_eq!(attention_badge(&status("HEALTHY", ""), 3), None);
    }

    #[test]
    fn a_state_or_reason_from_a_newer_daemon_shows_nothing_rather_than_nonsense() {
        assert_eq!(shown(&status("SIDEWAYS", "")), None);
        assert_eq!(shown(&status("BROKEN", "a-new-row")), None);
    }
}
