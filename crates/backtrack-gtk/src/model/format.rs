// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Every string the timeline shows that is derived from a number.
//!
//! Sizes go through GLib rather than a hand-rolled formatter, so they match the
//! units the rest of the desktop uses (`1.2 kB`, not `1.2 KiB` or `1229 bytes`).
//! Dates go through `GDateTime`, which knows the user's locale and timezone;
//! the format strings keep the field order from the mockups.

use backtrack_core::index::Kind;

pub use backtrack_core::status_line::at;

/// `"08:58"` — a time within a day the user has already been told.
pub fn clock(ts: i64, tz: &glib::TimeZone) -> String {
    at(ts, tz, "%H:%M")
}

/// `"Tue 10 Jun"` — a day within a period the user has already been told.
pub fn weekday_and_day(ts: i64, tz: &glib::TimeZone) -> String {
    at(ts, tz, "%a %e %b")
}

/// `"Tue 10 Jun, 09:00"` — the same, when one day holds several backups.
pub fn weekday_day_and_clock(ts: i64, tz: &glib::TimeZone) -> String {
    at(ts, tz, "%a %e %b, %H:%M")
}

/// `"12 Jun 2026, 08:58"` — the Modified column, from an epoch-**microsecond**
/// file timestamp.
pub fn modified(mtime_micros: i64, tz: &glib::TimeZone) -> String {
    at(mtime_micros.div_euclid(1_000_000), tz, "%e %b %Y, %H:%M")
}

/// The Size column: a directory has no meaningful size of its own, and an
/// invented one (0, or the cost of its entry) would be read as fact.
pub fn size(bytes: i64, kind: Kind) -> String {
    match kind {
        Kind::Dir => "—".to_string(),
        _ if bytes < 0 => "—".to_string(),
        _ => glib::format_size(bytes as u64).to_string(),
    }
}

/// The position label: `"Wed 12 Jun 2026, 09:00 — backup 3 of 47"`.
///
/// `ordinal` counts from the newest backup, because the sidebar beside it is
/// ordered newest-first: the label and the list have to agree about which end
/// they are counting from, and the list's order is not negotiable.
///
/// `filtered` says that the folder leaves some backups out (Stage 15), and the
/// label then says what it is counting: "3 of 7" alone would read as though
/// there were only seven backups.
pub fn position(
    ts: i64,
    ordinal: usize,
    total: usize,
    filtered: bool,
    tz: &glib::TimeZone,
) -> String {
    let when = at(ts, tz, "%a %e %b %Y, %H:%M");
    if filtered {
        format!("{when} — backup {ordinal} of {total} with changes to this folder")
    } else {
        format!("{when} — backup {ordinal} of {total}")
    }
}

/// The position label at a Trash entry (Stage 16), which is no backup and
/// has no place in their count: `"Tue 20 Oct 2026, 12:45 — in the Trash"`.
pub fn position_in_trash(ts: i64, tz: &glib::TimeZone) -> String {
    let when = at(ts, tz, "%a %e %b %Y, %H:%M");
    format!("{when} — in the Trash")
}

/// What the position line says when the index holds no backups at all.
pub const NO_BACKUPS: &str = "No backups yet";

#[cfg(test)]
mod tests {
    use super::*;

    fn utc() -> glib::TimeZone {
        glib::TimeZone::utc()
    }

    /// 2026-06-09 09:00:00 UTC — a Tuesday with a single-digit day.
    const NINTH: i64 = 1_780_995_600;

    #[test]
    fn a_single_digit_day_is_not_padded() {
        assert_eq!(weekday_and_day(NINTH, &utc()), "Tue 9 Jun");
        assert_eq!(weekday_day_and_clock(NINTH, &utc()), "Tue 9 Jun, 09:00");
    }

    #[test]
    fn the_position_label_spells_the_position_out() {
        assert_eq!(
            position(NINTH, 3, 47, false, &utc()),
            "Tue 9 Jun 2026, 09:00 — backup 3 of 47"
        );
    }

    #[test]
    fn a_position_among_some_backups_says_which_ones_it_counts() {
        assert_eq!(
            position(NINTH, 2, 7, true, &utc()),
            "Tue 9 Jun 2026, 09:00 — backup 2 of 7 with changes to this folder"
        );
    }

    #[test]
    fn modified_reads_microseconds() {
        // The same instant, expressed the way the index stores file times.
        assert_eq!(modified(NINTH * 1_000_000, &utc()), "9 Jun 2026, 09:00");
    }

    #[test]
    fn a_directory_has_no_size() {
        assert_eq!(size(4096, Kind::Dir), "—");
        // GLib separates the number from its unit with a non-breaking space,
        // which is right on screen and has to be expected here.
        assert_eq!(size(1200, Kind::File), "1.2\u{a0}kB");
        assert!(size(0, Kind::File).starts_with('0'));
    }

    #[test]
    fn a_negative_size_is_not_rendered_as_an_enormous_one() {
        // `size` is an i64 out of SQLite; a cast to u64 would turn -1 into 16
        // exabytes on screen.
        assert_eq!(size(-1, Kind::File), "—");
    }
}
