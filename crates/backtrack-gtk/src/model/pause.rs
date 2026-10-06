// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The pauses on offer, and how long each one lasts.
//!
//! Only self-expiring ones: there is no "off" in here, deliberately.
//! Switching backups off permanently belongs in Preferences, where it has to
//! be looked for, not one click away where it can be done by accident and
//! forgotten about.
//!
//! Both menus that offer a pause read it from here: the window's primary menu
//! and the tray's, which is a binary of its own and includes this file by
//! path. So it uses `glib` itself rather than through `gtk4`, and the tray
//! links no GTK.

/// The pauses, as `(option, label)` in menu order. The option is what
/// [`duration`] takes.
pub const OPTIONS: [(&str, &str); 3] = [
    ("hour", "For 1 hour"),
    ("tomorrow", "Until tomorrow"),
    ("indefinite", "Until I resume"),
];

/// A pause this far ahead is not a date anybody wants read back to them — it
/// is what "until I resume" has to be expressed as, because the interface
/// deliberately has no way to say "off forever".
pub const INDEFINITE_PAUSE_THRESHOLD: u64 = 365 * 86_400;

/// How long "For 1 hour" actually lasts.
///
/// One minute under `BACKTRACK_DEV`, because a self-expiring pause that takes
/// an hour to expire cannot be tested in a working session, and an untested
/// expiry is one that does not work.
pub fn hour_pause() -> u64 {
    if std::env::var_os("BACKTRACK_DEV").is_some() {
        60
    } else {
        3_600
    }
}

/// Seconds from `now` until the pause each option asks for lifts.
///
/// "Until I resume" has no natural end, but the daemon's `Pause` takes a time
/// and refuses one in the past — by design, so that "off forever" cannot be
/// set from a menu. It is expressed here as a date far enough out that nothing
/// will reach it, and the status line recognises that and says "until you
/// resume" rather than reading the date back.
pub fn duration(option: &str, now: i64, tz: &glib::TimeZone) -> Option<u64> {
    match option {
        "hour" => Some(hour_pause()),
        "tomorrow" => tomorrow_morning(now, tz).map(|until| (until - now).max(60) as u64),
        "indefinite" => Some(INDEFINITE_PAUSE_THRESHOLD * 100),
        _ => None,
    }
}

/// 09:00 tomorrow, local — the start of the next working day rather than
/// midnight, which is when "until tomorrow" would otherwise lift while nobody
/// is there to notice.
fn tomorrow_morning(now: i64, tz: &glib::TimeZone) -> Option<i64> {
    let today = glib::DateTime::from_unix_local(now)
        .ok()?
        .to_timezone(tz)
        .ok()?;
    let tomorrow = today.add_days(1).ok()?;
    glib::DateTime::new(
        tz,
        tomorrow.year(),
        tomorrow.month(),
        tomorrow.day_of_month(),
        9,
        0,
        0.0,
    )
    .ok()
    .map(|dt| dt.to_unix())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc() -> glib::TimeZone {
        glib::TimeZone::utc()
    }

    /// Wednesday 2026-06-10 12:00:00 UTC.
    const NOW: i64 = 1_781_092_800;

    #[test]
    fn until_tomorrow_lands_on_tomorrow_morning_not_on_midnight() {
        let seconds = duration("tomorrow", NOW, &utc()).unwrap();
        // 12:00 today to 09:00 tomorrow is 21 hours.
        assert_eq!(seconds, 21 * 3_600);
    }

    #[test]
    fn until_tomorrow_is_still_in_the_future_late_at_night() {
        // 23:30, when "tomorrow at 09:00" is only nine and a half hours away —
        // and, more to the point, has not already gone.
        let late = NOW + 11 * 3_600 + 1_800;
        let seconds = duration("tomorrow", late, &utc()).unwrap();
        assert_eq!(seconds, 9 * 3_600 + 1_800);
    }

    #[test]
    fn until_i_resume_is_far_enough_out_to_read_as_indefinite() {
        let seconds = duration("indefinite", NOW, &utc()).unwrap();
        assert!(
            seconds > INDEFINITE_PAUSE_THRESHOLD,
            "the status line has to recognise it as open-ended"
        );
    }

    #[test]
    fn an_unknown_option_pauses_nothing() {
        assert_eq!(duration("forever", NOW, &utc()), None);
    }

    #[test]
    fn every_option_on_offer_has_a_duration() {
        for (option, _) in OPTIONS {
            assert!(duration(option, NOW, &utc()).is_some(), "{option}");
        }
    }
}
