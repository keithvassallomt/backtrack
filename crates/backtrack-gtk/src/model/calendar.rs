// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The month grid behind the calendar popover.
//!
//! A day either has backups or it does not, and the difference has to be
//! obvious at a glance: shading the days that do is only half of it, because a
//! day with nothing behind it must also refuse to be clicked. Offering a jump
//! to a day that has no snapshot is a promise the timeline cannot keep.

use backtrack_core::index::ArchiveSummary;
use gtk4::glib;

use super::{date_to_day, day_to_date, local_day, weekday};

/// One cell of the month grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Day {
    pub day_of_month: i32,
    /// Whether this day has any backups: what shading, and clickability, mean.
    pub has_backups: bool,
    /// How many, for the tooltip and the screen reader.
    pub count: usize,
    /// The archive a click jumps to: the newest of that day, which is the first
    /// one the newest-first sidebar shows for it.
    pub seq: Option<i64>,
    pub is_today: bool,
}

/// A month, ready to lay out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Month {
    pub year: i32,
    pub month: i32,
    /// Heading, localized: `"June 2026"`.
    pub title: String,
    /// Monday-based weekday index (0–6) of the 1st, i.e. how many blank cells
    /// the grid starts with.
    pub leading_blanks: usize,
    pub days: Vec<Day>,
}

/// The month before `(year, month)`.
pub fn previous(year: i32, month: i32) -> (i32, i32) {
    if month == 1 {
        (year - 1, 12)
    } else {
        (year, month - 1)
    }
}

/// The month after `(year, month)`.
pub fn next(year: i32, month: i32) -> (i32, i32) {
    if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    }
}

/// Build the grid for `year`/`month` from `archives` (newest first).
pub fn month(
    archives: &[ArchiveSummary],
    year: i32,
    month: i32,
    now: i64,
    tz: &glib::TimeZone,
) -> Month {
    let first = date_to_day(year, month, 1);
    let length = days_in_month(year, month);
    let today = local_day(now, tz);

    let days = (1..=length)
        .map(|day_of_month| {
            let day = first + i64::from(day_of_month) - 1;
            // Newest first in, newest first out: the first match is the newest.
            let mut hits = archives.iter().filter(|a| local_day(a.ts, tz) == day);
            let newest = hits.next();
            let count = newest.iter().count() + hits.count();
            Day {
                day_of_month,
                has_backups: newest.is_some(),
                count,
                seq: newest.map(|a| a.seq),
                is_today: day == today,
            }
        })
        .collect();

    Month {
        year,
        month,
        title: title(year, month, tz),
        leading_blanks: weekday(first) as usize,
        days,
    }
}

/// The month a given archive sequence sits in, so opening the popover starts
/// where the user already is rather than at the present day.
pub fn month_of(archives: &[ArchiveSummary], seq: i64, tz: &glib::TimeZone) -> Option<(i32, i32)> {
    let archive = archives.iter().find(|a| a.seq == seq)?;
    let (year, month, _) = day_to_date(local_day(archive.ts, tz));
    Some((year, month))
}

fn title(year: i32, month: i32, tz: &glib::TimeZone) -> String {
    // Midday on the 1st: far enough from either boundary that no timezone can
    // roll the name over into a neighbouring month.
    let noon = glib::DateTime::new(tz, year, month, 1, 12, 0, 0.0);
    noon.ok()
        .and_then(|dt| dt.format("%B %Y").ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{month:02}/{year}"))
}

/// Length of `month` in days, leap years included.
pub fn days_in_month(year: i32, month: i32) -> i32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 30,
    }
}

/// Monday-first weekday initials for the grid header, localized.
pub fn weekday_initials(tz: &glib::TimeZone) -> Vec<String> {
    // 2024-01-01 was a Monday; walk that week for the locale's own names.
    (0..7)
        .map(|offset| {
            glib::DateTime::new(tz, 2024, 1, 1 + offset, 12, 0, 0.0)
                .ok()
                .and_then(|dt| dt.format("%a").ok())
                .map(|s| s.to_string())
                .unwrap_or_default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc() -> glib::TimeZone {
        glib::TimeZone::utc()
    }

    /// Wednesday 2026-06-10 12:00:00 UTC.
    const NOW: i64 = 1_781_092_800;
    const DAY: i64 = 86_400;

    fn archive(seq: i64, ts: i64) -> ArchiveSummary {
        ArchiveSummary {
            seq,
            borg_id: None,
            name: format!("snapshot-{seq}"),
            ts,
            repo: "primary".to_string(),
            catalogued: true,
        }
    }

    /// The fixture's shape: a backup every day from 1 to 30 June except the
    /// 4th and the 5th, which are the gap days.
    fn june_with_a_gap() -> Vec<ArchiveSummary> {
        let first = date_to_day(2026, 6, 1);
        (1..=30)
            .rev()
            .filter(|d| !matches!(d, 4 | 5))
            .map(|d| {
                let ts = (first + i64::from(d) - 1) * DAY + 12 * 3600;
                archive(i64::from(d), ts)
            })
            .collect()
    }

    #[test]
    fn a_gap_day_is_unshaded_and_has_nothing_to_jump_to() {
        let grid = month(&june_with_a_gap(), 2026, 6, NOW, &utc());
        let fourth = &grid.days[3];
        assert_eq!(fourth.day_of_month, 4);
        assert!(!fourth.has_backups, "the 4th has no backup");
        assert_eq!(fourth.seq, None, "so there is nothing for a click to do");

        let third = &grid.days[2];
        assert!(third.has_backups);
        assert_eq!(third.seq, Some(3));
    }

    #[test]
    fn the_grid_starts_on_the_right_weekday() {
        // 1 June 2026 was a Monday, so the month begins flush with the column.
        let grid = month(&june_with_a_gap(), 2026, 6, NOW, &utc());
        assert_eq!(grid.leading_blanks, 0);
        assert_eq!(grid.days.len(), 30);
        assert_eq!(grid.title, "June 2026");

        // 1 July 2026 was a Wednesday: two blanks.
        let july = month(&[], 2026, 7, NOW, &utc());
        assert_eq!(july.leading_blanks, 2);
        assert_eq!(july.days.len(), 31);
    }

    #[test]
    fn today_is_marked_only_in_its_own_month() {
        let june = month(&[], 2026, 6, NOW, &utc());
        assert!(june.days[9].is_today, "the 10th is today");
        assert_eq!(june.days.iter().filter(|d| d.is_today).count(), 1);

        let may = month(&[], 2026, 5, NOW, &utc());
        assert!(!may.days.iter().any(|d| d.is_today));
    }

    #[test]
    fn a_day_with_several_backups_jumps_to_the_newest_and_counts_them_all() {
        let base = date_to_day(2026, 6, 12) * DAY;
        let archives = vec![
            archive(3, base + 16 * 3600),
            archive(2, base + 12 * 3600),
            archive(1, base + 9 * 3600),
        ];
        let grid = month(&archives, 2026, 6, NOW, &utc());
        let twelfth = &grid.days[11];
        assert_eq!(twelfth.count, 3);
        assert_eq!(
            twelfth.seq,
            Some(3),
            "the newest of the day, which the sidebar shows first"
        );
    }

    #[test]
    fn february_knows_about_leap_years() {
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
    }

    #[test]
    fn month_navigation_wraps_at_the_year_boundary() {
        assert_eq!(previous(2026, 1), (2025, 12));
        assert_eq!(next(2026, 12), (2027, 1));
        assert_eq!(previous(2026, 6), (2026, 5));
        assert_eq!(next(2026, 6), (2026, 7));
    }

    #[test]
    fn the_popover_opens_on_the_month_being_viewed() {
        let archives = june_with_a_gap();
        assert_eq!(month_of(&archives, 3, &utc()), Some((2026, 6)));
        assert_eq!(month_of(&archives, 999, &utc()), None);
    }
}
