// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The one line at the bottom of the window that says how backups are doing.
//!
//! It is written to be read in passing, so it answers in the order a worried
//! person asks: is something happening right now, is something stopping it,
//! and failing both, when did this last work? The full health model and its
//! banner are Stage 10; this is the quiet version that is true in the
//! meantime.

use backtrack_core::dbus::Status;
use gtk4::glib;

use super::format;

/// A pause this far ahead is not a date anybody wants read back to them — it
/// is what "until I resume" has to be expressed as, because the interface
/// deliberately has no way to say "off forever".
pub const INDEFINITE_PAUSE_THRESHOLD: u64 = 365 * 86_400;

/// What the status line reads, given the daemon's answer and the time now.
pub fn line(status: &Status, now: i64, tz: &glib::TimeZone) -> String {
    let now_secs = now.max(0) as u64;

    if status.paused_until > now_secs {
        return if status.paused_until - now_secs > INDEFINITE_PAUSE_THRESHOLD {
            "Backups are paused until you resume them".to_string()
        } else {
            format!(
                "Backups are paused until {}",
                format::clock(status.paused_until as i64, tz)
            )
        };
    }

    if status.active_job != 0 {
        return "Backing up now…".to_string();
    }

    if !status.configured {
        return "Backups are not set up yet".to_string();
    }

    let last = match status.last_backup {
        0 => return "No backups yet".to_string(),
        seconds => seconds as i64,
    };

    let protection = if status.destination_reachable {
        String::new()
    } else {
        match status.offline_mode.as_str() {
            // Not an error, and deliberately not phrased as one: being away
            // from the destination is a normal Tuesday, and the changes are
            // still being kept.
            "spool" | "fs-snapshot" => {
                " · your backup destination is away, so changes are being kept on this computer"
                    .to_string()
            }
            _ => " · your backup destination is away".to_string(),
        }
    };

    format!("Last backup {}{protection}", ago(last, now, tz))
}

/// How long ago, in words. Rounded the way people speak, not the way a clock
/// counts: "2 hours ago" rather than "1 hour 47 minutes ago".
pub fn ago(then: i64, now: i64, tz: &glib::TimeZone) -> String {
    let elapsed = now - then;
    match elapsed {
        // A backup timestamped in the future is a clock that moved, not a
        // prediction. Saying "just now" is closer to the truth than anything
        // involving a negative number.
        ..=59 => "just now".to_string(),
        60..=3_599 => {
            let minutes = elapsed / 60;
            plural(minutes, "minute")
        }
        3_600..=86_399 => {
            let hours = elapsed / 3_600;
            plural(hours, "hour")
        }
        86_400..=172_799 => format!("yesterday at {}", format::clock(then, tz)),
        172_800..=604_799 => format!("on {}", format::weekday_and_day(then, tz)),
        _ => format!("on {}", format::at(then, tz, "%e %b %Y")),
    }
}

fn plural(count: i64, unit: &str) -> String {
    if count == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{count} {unit}s ago")
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
            state: "PROTECTED".to_string(),
            last_backup: (NOW - 7_200) as u64,
            next_backup: (NOW + 3_600) as u64,
            destination_reachable: true,
            spool_bytes: 0,
            offline_mode: "spool".to_string(),
            local_snapshots: 0,
            expirable_snapshots: 0,
            active_job: 0,
            paused_until: 0,
            configured: true,
        }
    }

    #[test]
    fn the_quiet_case_says_when_it_last_worked() {
        assert_eq!(line(&status(), NOW, &utc()), "Last backup 2 hours ago");
    }

    #[test]
    fn a_running_backup_takes_precedence_over_the_last_one() {
        let mut status = status();
        status.active_job = 7;
        assert_eq!(line(&status, NOW, &utc()), "Backing up now…");
    }

    #[test]
    fn a_pause_says_when_it_lifts() {
        let mut status = status();
        status.paused_until = (NOW + 3_600) as u64;
        assert_eq!(line(&status, NOW, &utc()), "Backups are paused until 13:00");
    }

    #[test]
    fn an_until_i_resume_pause_is_not_read_back_as_a_date_in_the_next_century() {
        let mut status = status();
        status.paused_until = (NOW + 100 * 365 * 86_400) as u64;
        assert_eq!(
            line(&status, NOW, &utc()),
            "Backups are paused until you resume them"
        );
    }

    #[test]
    fn a_pause_outranks_a_running_backup_only_once_it_is_actually_in_force() {
        // Back Up Now deliberately runs while paused, and the line has to
        // report what is happening rather than what is configured.
        let mut status = status();
        status.paused_until = (NOW - 60) as u64;
        status.active_job = 7;
        assert_eq!(line(&status, NOW, &utc()), "Backing up now…");
    }

    #[test]
    fn being_away_from_the_destination_is_reported_without_alarm() {
        let mut status = status();
        status.destination_reachable = false;
        assert_eq!(
            line(&status, NOW, &utc()),
            "Last backup 2 hours ago · your backup destination is away, so changes are \
             being kept on this computer"
        );
    }

    #[test]
    fn an_unconfigured_machine_is_told_so_rather_than_shown_a_blank() {
        let mut status = status();
        status.configured = false;
        assert_eq!(line(&status, NOW, &utc()), "Backups are not set up yet");
    }

    #[test]
    fn elapsed_time_is_rounded_the_way_people_say_it() {
        assert_eq!(ago(NOW - 30, NOW, &utc()), "just now");
        assert_eq!(ago(NOW - 60, NOW, &utc()), "1 minute ago");
        assert_eq!(ago(NOW - 1_800, NOW, &utc()), "30 minutes ago");
        assert_eq!(ago(NOW - 3_600, NOW, &utc()), "1 hour ago");
        assert_eq!(ago(NOW - 6_900, NOW, &utc()), "1 hour ago");
        assert_eq!(ago(NOW - 90_000, NOW, &utc()), "yesterday at 11:00");
        assert_eq!(ago(NOW - 30 * 86_400, NOW, &utc()), "on 11 May 2026");
    }

    #[test]
    fn a_timestamp_from_the_future_does_not_produce_a_negative_duration() {
        // A machine whose clock jumped back, which happens.
        assert_eq!(ago(NOW + 5_000, NOW, &utc()), "just now");
    }
}
