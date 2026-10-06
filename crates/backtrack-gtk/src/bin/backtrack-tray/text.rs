// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! What the tray says, worked out from the daemon's status before the menu
//! is involved.
//!
//! Times are given as times of day, "today 16:00", rather than as "2 hours
//! ago" the way the window's status line puts them: a tray menu is rebuilt
//! when somebody opens it, not every minute, and a time of day cannot go
//! stale while it waits.

use backtrack_core::dbus::Status;

use crate::format;
use crate::pause::INDEFINITE_PAUSE_THRESHOLD;

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

pub fn shown(status: Option<&Status>, now: i64, tz: &glib::TimeZone) -> Shown {
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
        line: line(status, now, tz),
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

/// The status line.
fn line(status: &Status, now: i64, tz: &glib::TimeZone) -> String {
    let now_secs = now.max(0) as u64;
    if !status.configured {
        return "Backups are not set up yet".to_string();
    }
    if status.active_job != 0 {
        return "Backing up now…".to_string();
    }
    if status.paused_until > now_secs {
        return if status.paused_until - now_secs > INDEFINITE_PAUSE_THRESHOLD {
            "Backups are paused until you resume them".to_string()
        } else {
            format!(
                "Backups are paused until {}",
                when(status.paused_until as i64, now, tz)
            )
        };
    }
    if matches!(status.state.as_str(), "AT_RISK" | "BROKEN") {
        return "Backups need your attention".to_string();
    }
    match status.last_backup {
        0 => "No backups yet".to_string(),
        last => format!("Last backup: {}", when(last as i64, now, tz)),
    }
}

/// `ts` as a day and a time of day, the day in words when it is close to
/// `now`: "today 16:00", "yesterday 22:00", "tomorrow 09:00", "Mon 8 Jun
/// 14:00".
fn when(ts: i64, now: i64, tz: &glib::TimeZone) -> String {
    let clock = format::clock(ts, tz);
    match days_from(now, ts, tz) {
        Some(0) => format!("today {clock}"),
        Some(-1) => format!("yesterday {clock}"),
        Some(1) => format!("tomorrow {clock}"),
        _ => format::at(ts, tz, "%a %e %b %H:%M"),
    }
}

/// Calendar days from the day `now` falls on to the day `ts` does, locally.
/// Counted between midnights, rounded, so a day of 23 or 25 hours at a clock
/// change still counts as one.
fn days_from(now: i64, ts: i64, tz: &glib::TimeZone) -> Option<i64> {
    Some((midnight(ts, tz)? - midnight(now, tz)? + 43_200).div_euclid(86_400))
}

/// The start of the local day `ts` falls on.
fn midnight(ts: i64, tz: &glib::TimeZone) -> Option<i64> {
    let dt = glib::DateTime::from_unix_local(ts)
        .ok()?
        .to_timezone(tz)
        .ok()?;
    glib::DateTime::new(tz, dt.year(), dt.month(), dt.day_of_month(), 0, 0, 0.0)
        .ok()
        .map(|midnight| midnight.to_unix())
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
        let shown = shown(Some(&status()), NOW, &utc());
        assert_eq!(shown.line, "Last backup: today 10:00");
        assert_eq!(shown.attention, None);
        assert!(shown.can_back_up && shown.can_pause && !shown.paused);
    }

    #[test]
    fn days_close_by_are_named() {
        let tz = utc();
        assert_eq!(when(NOW - 86_400, NOW, &tz), "yesterday 12:00");
        assert_eq!(when(NOW + 21 * 3_600, NOW, &tz), "tomorrow 09:00");
        assert_eq!(when(NOW - 2 * 86_400, NOW, &tz), "Mon 8 Jun 12:00");
    }

    #[test]
    fn yesterday_is_the_calendar_day_not_the_last_24_hours() {
        // 00:30 on the 10th, and a backup at 23:00 on the 9th.
        let just_after_midnight = NOW - 11 * 3_600 - 1_800;
        assert_eq!(
            when(just_after_midnight - 5_400, just_after_midnight, &utc()),
            "yesterday 23:00"
        );
    }

    #[test]
    fn the_new_year_is_still_the_next_day() {
        // 2026-12-31 22:00 and 2027-01-01 08:00 UTC.
        let new_years_eve = 1_798_754_400;
        assert_eq!(
            when(new_years_eve + 10 * 3_600, new_years_eve, &utc()),
            "tomorrow 08:00"
        );
    }

    #[test]
    fn a_backup_under_way_says_so_and_cannot_be_started_again() {
        let running = Status {
            active_job: 7,
            ..status()
        };
        let shown = shown(Some(&running), NOW, &utc());
        assert_eq!(shown.line, "Backing up now…");
        assert!(!shown.can_back_up);
    }

    #[test]
    fn a_pause_says_when_it_ends_and_offers_resuming() {
        let paused = Status {
            state: "PAUSED".to_string(),
            paused_until: (NOW + 21 * 3_600) as u64,
            ..status()
        };
        let shown = shown(Some(&paused), NOW, &utc());
        assert_eq!(shown.line, "Backups are paused until tomorrow 09:00");
        assert!(shown.paused);
    }

    #[test]
    fn an_open_ended_pause_is_not_read_back_as_a_date() {
        let paused = Status {
            paused_until: NOW as u64 + INDEFINITE_PAUSE_THRESHOLD * 100,
            ..status()
        };
        assert_eq!(
            shown(Some(&paused), NOW, &utc()).line,
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
            let shown = shown(Some(&status), NOW, &utc());
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
        let shown = shown(Some(&unset), NOW, &utc());
        assert_eq!(shown.line, "Backups are not set up yet");
        assert!(!shown.can_back_up && !shown.can_pause);
    }

    #[test]
    fn no_daemon_still_lets_a_backup_be_asked_for() {
        let shown = shown(None, NOW, &utc());
        assert_eq!(shown.line, "The Backtrack service is not running");
        assert!(shown.can_back_up);
    }
}
