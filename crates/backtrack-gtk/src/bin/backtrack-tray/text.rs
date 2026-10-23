// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! What the tray says, worked out from the daemon's status before the menu
//! is involved. Its first line is `backtrack_core::status_line`'s, which the
//! Flatpak's daemon also shows in GNOME's Background Apps.

use backtrack_core::dbus::Status;
use backtrack_core::status_line;

/// How loudly the icon asks to be looked at. health.md's two states that
/// earn attention: `AT_RISK`, the yellow banner, and `BROKEN`, the red one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attention {
    Warning,
    Error,
}

/// What the tray shows for `status`, `None` meaning the daemon is not
/// running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    /// The disabled line at the top of the menu, and the tooltip.
    pub line: String,
    pub attention: Option<Attention>,
    pub can_back_up: bool,
    pub can_pause: bool,
    pub paused: bool,
}

/// `progress` is how far a backup under way has got, in
/// `backtrack_core::progress`'s words, which says more than "Backing up now…"
/// while there is a backup to say it about (S06-T12).
pub fn shown(
    status: Option<&Status>,
    progress: Option<&str>,
    now: i64,
    tz: &glib::TimeZone,
) -> Shown {
    let Some(status) = status else {
        // Its menu still works: asking for a backup starts it.
        return Shown {
            line: "The Backtrack service is not running".to_string(),
            attention: None,
            can_back_up: true,
            can_pause: true,
            paused: false,
        };
    };
    let paused = status.paused_until > now.max(0) as u64;
    Shown {
        line: match progress {
            Some(progress) if status.active_job != 0 => progress.to_string(),
            _ => status_line::line(status, now, tz),
        },
        attention: match status.state.as_str() {
            "AT_RISK" => Some(Attention::Warning),
            "BROKEN" => Some(Attention::Error),
            _ => None,
        },
        can_back_up: status.configured && status.active_job == 0,
        can_pause: status.configured,
        paused,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc() -> glib::TimeZone {
        glib::TimeZone::utc()
    }

    /// Wednesday 2026-06-10 12:00:00 UTC.
    const NOW: i64 = 1_781_092_800;

    fn status() -> Status {
        Status {
            state: "HEALTHY".to_string(),
            last_backup: (NOW - 2 * 3_600) as u64,
            next_backup: (NOW + 3_600) as u64,
            destination_reachable: true,
            spool_bytes: 0,
            offline_mode: "spool".to_string(),
            local_snapshots: 0,
            expirable_snapshots: 0,
            active_job: 0,
            paused_until: 0,
            configured: true,
            reason: String::new(),
            since: 0,
            recovery_job: 0,
        }
    }

    #[test]
    fn a_healthy_computer_says_when_it_last_backed_up() {
        let shown = shown(Some(&status()), None, NOW, &utc());
        assert_eq!(shown.line, "Last backup: today 10:00");
        assert_eq!(shown.attention, None);
        assert!(shown.can_back_up && shown.can_pause && !shown.paused);
    }

    #[test]
    fn a_backup_under_way_says_so_and_cannot_be_started_again() {
        let running = Status {
            active_job: 7,
            ..status()
        };
        let shown = shown(Some(&running), None, NOW, &utc());
        assert_eq!(shown.line, "Backing up now…");
        // With the backup's progress, the line says how far it has got.
        let line = "Backing up… 31.5 GB of about 53.1 GB (59%)";
        assert_eq!(
            super::shown(Some(&running), Some(line), NOW, &utc()).line,
            line
        );
        // A line left over from a backup that has ended says nothing now.
        let idle = Status {
            active_job: 0,
            ..running.clone()
        };
        assert_ne!(
            super::shown(Some(&idle), Some(line), NOW, &utc()).line,
            line
        );
        assert!(!shown.can_back_up);
    }

    #[test]
    fn a_pause_says_when_it_ends_and_offers_resuming() {
        let paused = Status {
            state: "PAUSED".to_string(),
            paused_until: (NOW + 21 * 3_600) as u64,
            ..status()
        };
        let shown = shown(Some(&paused), None, NOW, &utc());
        assert_eq!(shown.line, "Backups are paused until tomorrow 09:00");
        assert!(shown.paused);
    }

    #[test]
    fn an_open_ended_pause_is_not_read_back_as_a_date() {
        let paused = Status {
            paused_until: NOW as u64 + status_line::INDEFINITE_PAUSE_THRESHOLD * 100,
            ..status()
        };
        assert_eq!(
            shown(Some(&paused), None, NOW, &utc()).line,
            "Backups are paused until you resume them"
        );
    }

    #[test]
    fn only_at_risk_and_broken_ask_for_attention() {
        for (state, attention) in [
            ("HEALTHY", None),
            ("PROTECTED_LOCALLY", None),
            ("PAUSED", None),
            ("DEGRADED", None),
            ("AT_RISK", Some(Attention::Warning)),
            ("BROKEN", Some(Attention::Error)),
        ] {
            let status = Status {
                state: state.to_string(),
                ..status()
            };
            let shown = shown(Some(&status), None, NOW, &utc());
            assert_eq!(shown.attention, attention, "{state}");
            if attention.is_some() {
                assert_eq!(shown.line, "Backups need your attention");
            }
        }
    }

    #[test]
    fn nothing_set_up_offers_nothing_to_do() {
        let unset = Status {
            configured: false,
            last_backup: 0,
            ..status()
        };
        let shown = shown(Some(&unset), None, NOW, &utc());
        assert_eq!(shown.line, "Backups are not set up yet");
        assert!(!shown.can_back_up && !shown.can_pause);
    }

    #[test]
    fn no_daemon_still_lets_a_backup_be_asked_for() {
        let shown = shown(None, None, NOW, &utc());
        assert_eq!(shown.line, "The Backtrack service is not running");
        assert!(shown.can_back_up);
    }
}
