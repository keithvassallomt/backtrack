// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! What Backtrack is doing, in one line: the first line of the tray's menu,
//! and in the Flatpak the line GNOME's Background Apps shows under Backtrack
//! (S14-T10). One wording, so the two say the same thing.
//!
//! Times are given as times of day, "today 16:00", rather than as "2 hours
//! ago" the way the window's status line puts them: a tray menu is rebuilt
//! when somebody opens it, and the daemon sends its line when something
//! changes, so neither is redrawn every minute, and a time of day cannot go
//! stale while it waits.
//!
//! Dates go through GLib's `GDateTime`, which knows the user's locale and
//! time zone, so this module is built only with the `glib` feature: the tray
//! and the daemon turn it on, the command line does not.

use crate::dbus::Status;

/// A pause this far ahead is not a date anybody wants read back to them — it
/// is what "until I resume" has to be expressed as, because the interface
/// deliberately has no way to say "off forever".
pub const INDEFINITE_PAUSE_THRESHOLD: u64 = 365 * 86_400;

/// The line for `status` at `now`, with times in `tz`.
pub fn line(status: &Status, now: i64, tz: &glib::TimeZone) -> String {
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

/// [`line`] in this computer's own time zone.
pub fn local_line(status: &Status, now: i64) -> String {
    line(status, now, &glib::TimeZone::local())
}

/// Format `ts` (epoch seconds) in `tz` with a `strftime` pattern, collapsing the
/// double space `%e` leaves in front of a single-digit day.
pub fn at(ts: i64, tz: &glib::TimeZone, pattern: &str) -> String {
    let Ok(dt) = glib::DateTime::from_unix_local(ts).and_then(|dt| dt.to_timezone(tz)) else {
        return String::new();
    };
    let text = dt.format(pattern).unwrap_or_default();
    collapse_spaces(&text)
}

/// `ts` as a day and a time of day, the day in words when it is close to
/// `now`: "today 16:00", "yesterday 22:00", "tomorrow 09:00", "Mon 8 Jun
/// 14:00".
fn when(ts: i64, now: i64, tz: &glib::TimeZone) -> String {
    let clock = at(ts, tz, "%H:%M");
    match days_from(now, ts, tz) {
        Some(0) => format!("today {clock}"),
        Some(-1) => format!("yesterday {clock}"),
        Some(1) => format!("tomorrow {clock}"),
        _ => at(ts, tz, "%a %e %b %H:%M"),
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

/// Squeeze runs of spaces down to one. `%e` pads a single-digit day, and
/// depending on the locale it pads with either an ordinary space or a figure
/// space (U+2007) — both of which read as a typo in the middle of a sentence.
fn collapse_spaces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_was_space = false;
    for ch in text.chars() {
        if ch == ' ' || ch == '\u{2007}' {
            if !last_was_space {
                out.push(' ');
            }
            last_was_space = true;
        } else {
            out.push(ch);
            last_was_space = false;
        }
    }
    out.trim().to_string()
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
    fn a_single_digit_day_is_not_padded() {
        // 2026-06-09 09:00:00 UTC.
        assert_eq!(at(1_780_995_600, &utc(), "%a %e %b"), "Tue 9 Jun");
    }

    #[test]
    fn every_line_fits_the_background_portal() {
        // The portal takes a status message shorter than 96 characters. The
        // longest line is a pause that ends on a day not named in words.
        let long_pause = Status {
            paused_until: (NOW + 3 * 86_400) as u64,
            ..status()
        };
        let lines = [
            line(&status(), NOW, &utc()),
            line(&long_pause, NOW, &utc()),
            line(
                &Status {
                    active_job: 3,
                    ..status()
                },
                NOW,
                &utc(),
            ),
            line(
                &Status {
                    state: "BROKEN".to_string(),
                    ..status()
                },
                NOW,
                &utc(),
            ),
        ];
        assert_eq!(lines[1], "Backups are paused until Sat 13 Jun 12:00");
        for text in lines {
            assert!(text.chars().count() < 96, "{text}");
        }
    }
}
