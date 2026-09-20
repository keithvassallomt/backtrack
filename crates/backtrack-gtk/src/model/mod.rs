// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The view models: everything the window shows, worked out as plain data
//! before a widget is involved.
//!
//! Nothing in here touches GTK. That is deliberate — the sidebar's grouping,
//! the calendar's shaded days, the density strip's bars and every piece of
//! formatted text are the parts most likely to be wrong, and they are the parts
//! a test can check without a display server. The widgets underneath `ui` are
//! then a thin rendering of these structures.
//!
//! Time is the one dependency they cannot avoid, so it is passed in rather than
//! read from the clock: every entry point takes `now` and a [`glib::TimeZone`],
//! which makes "what does the sidebar look like on a Tuesday in June" a
//! question a test can ask.

pub mod calendar;
pub mod density;
pub mod format;
pub mod group;
pub mod status;

use gtk4::glib;

/// Days since the Unix epoch, in `tz`. The unit the calendar, the density strip
/// and the sidebar all bucket by: two timestamps belong to the same day exactly
/// when this returns the same number for both.
pub fn local_day(ts: i64, tz: &glib::TimeZone) -> i64 {
    let dt = glib::DateTime::from_unix_local(ts)
        .and_then(|dt| dt.to_timezone(tz))
        .expect("epoch seconds are always a valid date");
    // Julian day is what GLib counts internally, and it steps at local midnight
    // in `tz` — which is the boundary we want, and the one that arithmetic on
    // `ts / 86400` gets wrong by an offset for most of the world.
    julian_day(dt.year(), dt.month(), dt.day_of_month()) - JULIAN_EPOCH
}

/// The day number of a local calendar date — the inverse of [`day_to_date`],
/// and how the calendar grid turns "the 1st of June" into something it can
/// compare against an archive's day.
pub fn date_to_day(year: i32, month: i32, day: i32) -> i64 {
    julian_day(year, month, day) - JULIAN_EPOCH
}

/// Julian day number for a proleptic Gregorian date (Fliegel–Van Flandern).
fn julian_day(year: i32, month: i32, day: i32) -> i64 {
    let a = ((14 - month) / 12) as i64;
    let y = year as i64 + 4800 - a;
    let m = month as i64 + 12 * a - 3;
    day as i64 + (153 * m + 2) / 5 + 365 * y + y / 4 - y / 100 + y / 400 - 32045
}

/// Julian day number of 1970-01-01.
const JULIAN_EPOCH: i64 = 2_440_588;

/// The local date a day number lands on, as `(year, month, day)`.
pub fn day_to_date(day: i64) -> (i32, i32, i32) {
    // Inverse of [`julian_day`], same source.
    let jd = day + JULIAN_EPOCH;
    let a = jd + 32044;
    let b = (4 * a + 3) / 146097;
    let c = a - 146097 * b / 4;
    let d = (4 * c + 3) / 1461;
    let e = c - 1461 * d / 4;
    let m = (5 * e + 2) / 153;
    let day_of_month = e - (153 * m + 2) / 5 + 1;
    let month = m + 3 - 12 * (m / 10);
    let year = 100 * b + d - 4800 + m / 10;
    (year as i32, month as i32, day_of_month as i32)
}

/// Monday-based weekday index (0 = Monday) for a day number. 1970-01-01 was a
/// Thursday, which is index 3.
pub fn weekday(day: i64) -> i64 {
    (day + 3).rem_euclid(7)
}

/// The day number of the Monday that starts `day`'s week.
pub fn week_start(day: i64) -> i64 {
    day - weekday(day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc() -> glib::TimeZone {
        glib::TimeZone::utc()
    }

    #[test]
    fn the_epoch_is_day_zero() {
        assert_eq!(local_day(0, &utc()), 0);
        assert_eq!(local_day(86_399, &utc()), 0);
        assert_eq!(local_day(86_400, &utc()), 1);
    }

    #[test]
    fn day_numbers_round_trip_through_dates() {
        for day in [0, 1, 59, 60, 10_000, 20_000, 20_635] {
            let (y, m, d) = day_to_date(day);
            assert_eq!(
                date_to_day(y, m, d),
                day,
                "{y}-{m:02}-{d:02} did not survive the round trip"
            );
        }
    }

    #[test]
    fn the_epoch_was_a_thursday() {
        assert_eq!(weekday(0), 3);
        assert_eq!(
            week_start(0),
            -3,
            "the week containing 1 Jan 1970 began on 29 Dec 1969"
        );
    }

    #[test]
    fn a_timezone_east_of_utc_rolls_over_first() {
        // 1970-01-01 23:30 UTC is already 1970-01-02 in Sydney (UTC+10 then).
        let sydney = glib::TimeZone::new(Some("Australia/Sydney"));
        assert_eq!(local_day(84_600, &utc()), 0);
        assert_eq!(local_day(84_600, &sydney), 1);
    }
}
