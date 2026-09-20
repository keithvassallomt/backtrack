// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Grouping for the snapshot sidebar.
//!
//! Borg's retention is deliberately uneven — hourly for a day, daily for a
//! week, weekly for a month, monthly after that — so a flat list of archives
//! has no consistent rhythm to read. Grouping absorbs that: Today is a column
//! of times, the last week is a column of days, and everything older collapses
//! into months with a count. The user never has to notice that the spacing
//! changed.
//!
//! The labels shrink to fit their group. Inside Today a backup is "16:00",
//! because the day is the group's title; inside a month it is "Tue 9 Jun",
//! unless that day holds more than one backup, in which case the time comes
//! back so two rows are never indistinguishable.

use backtrack_core::index::ArchiveSummary;
use gtk4::glib;

use super::{format, local_day, week_start};

/// Which band of the past a group covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    Today,
    Yesterday,
    ThisWeek,
    LastWeek,
    /// Calendar month, as `(year, month)`.
    Month(i32, i32),
}

impl Bucket {
    /// Groups near the present open on their own; older ones are a count until
    /// asked for. The mockup's sidebar shows exactly this split.
    fn expanded_by_default(self) -> bool {
        matches!(self, Bucket::Today | Bucket::Yesterday | Bucket::ThisWeek)
    }
}

/// One selectable backup in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub seq: i64,
    pub ts: i64,
    /// What the row reads as, already shortened to suit its group.
    pub label: String,
    /// Held on this computer rather than at the destination — the spool and
    /// filesystem-snapshot archives from Stage 5.
    pub local_only: bool,
    /// Whether the archive's file list has been read. An uncatalogued archive
    /// cannot be browsed, so the sidebar says "cataloguing…" instead of
    /// offering an empty folder.
    pub catalogued: bool,
}

/// A band of the past and the backups in it, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub bucket: Bucket,
    /// The heading, already including the count where one is shown.
    pub title: String,
    pub expanded: bool,
    pub rows: Vec<Row>,
}

/// Group `archives` (newest first, as [`ArchiveSummary`] arrives) for display
/// at `now`.
pub fn sidebar(archives: &[ArchiveSummary], now: i64, tz: &glib::TimeZone) -> Vec<Group> {
    let today = local_day(now, tz);

    // Bucket every archive first, so labels can see how crowded their day is.
    let placed: Vec<(Bucket, &ArchiveSummary)> = archives
        .iter()
        .map(|a| (bucket_for(local_day(a.ts, tz), today), a))
        .collect();

    let mut groups: Vec<Group> = Vec::new();
    for (bucket, archive) in placed {
        let day = local_day(archive.ts, tz);
        let crowded = matches!(bucket, Bucket::Today | Bucket::Yesterday)
            || archives
                .iter()
                .filter(|other| local_day(other.ts, tz) == day)
                .count()
                > 1;

        let row = Row {
            seq: archive.seq,
            ts: archive.ts,
            label: label(bucket, archive.ts, crowded, tz),
            local_only: archive.repo != "primary",
            catalogued: archive.catalogued,
        };

        match groups.last_mut() {
            Some(group) if group.bucket == bucket => group.rows.push(row),
            _ => groups.push(Group {
                bucket,
                title: String::new(),
                expanded: bucket.expanded_by_default(),
                rows: vec![row],
            }),
        }
    }

    for group in &mut groups {
        group.title = title(group, tz);
    }
    groups
}

/// Which band `day` falls in, relative to `today`.
///
/// Yesterday is tested before the week, so that on a Monday the Sunday just
/// gone reads as "Yesterday" rather than disappearing into "Last week" — which
/// is where the calendar would otherwise put it.
fn bucket_for(day: i64, today: i64) -> Bucket {
    if day >= today {
        Bucket::Today
    } else if day == today - 1 {
        Bucket::Yesterday
    } else if week_start(day) == week_start(today) {
        Bucket::ThisWeek
    } else if week_start(day) == week_start(today) - 7 {
        Bucket::LastWeek
    } else {
        let (year, month, _) = super::day_to_date(day);
        Bucket::Month(year, month)
    }
}

fn label(bucket: Bucket, ts: i64, crowded: bool, tz: &glib::TimeZone) -> String {
    match bucket {
        Bucket::Today | Bucket::Yesterday => format::clock(ts, tz),
        _ if crowded => format::weekday_day_and_clock(ts, tz),
        _ => format::weekday_and_day(ts, tz),
    }
}

/// The heading. A count is shown only where the group is closed by default —
/// on an open group the rows are their own count.
fn title(group: &Group, tz: &glib::TimeZone) -> String {
    let n = group.rows.len();
    match group.bucket {
        Bucket::Today => "Today".to_string(),
        Bucket::Yesterday => "Yesterday".to_string(),
        Bucket::ThisWeek => "This week".to_string(),
        Bucket::LastWeek => format!("Last week ({n})"),
        Bucket::Month(..) => {
            let name = format::at(group.rows[0].ts, tz, "%B %Y");
            format!("{name} ({n})")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc() -> glib::TimeZone {
        glib::TimeZone::utc()
    }

    /// Wednesday 2026-06-10 12:00:00 UTC. Every test dates backwards from here,
    /// so "this week" is Mon 8 – Sun 14 June.
    const NOW: i64 = 1_781_092_800;
    const HOUR: i64 = 3_600;
    const DAY: i64 = 86_400;

    fn archive(seq: i64, ts: i64) -> ArchiveSummary {
        ArchiveSummary {
            seq,
            borg_id: Some(format!("id-{seq}")),
            name: format!("snapshot-{seq}"),
            ts,
            repo: "primary".to_string(),
            catalogued: true,
        }
    }

    /// Newest first, the way `archives_overview` hands them over.
    fn at_offsets(offsets: &[i64]) -> Vec<ArchiveSummary> {
        offsets
            .iter()
            .enumerate()
            .map(|(i, back)| archive(1000 - i as i64, NOW - back))
            .collect()
    }

    #[test]
    fn a_days_worth_of_hourlies_is_one_group_of_times() {
        let groups = sidebar(&at_offsets(&[0, HOUR, 2 * HOUR]), NOW, &utc());
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].title, "Today");
        assert!(groups[0].expanded);
        let labels: Vec<_> = groups[0].rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["12:00", "11:00", "10:00"]);
    }

    #[test]
    fn the_bands_run_today_yesterday_this_week_last_week_then_months() {
        let groups = sidebar(
            &at_offsets(&[
                0,        // today
                DAY,      // yesterday, Tue 9 Jun
                2 * DAY,  // Mon 8 Jun — still this week
                4 * DAY,  // Sat 6 Jun — last week
                20 * DAY, // 21 May
                45 * DAY, // 26 Apr
            ]),
            NOW,
            &utc(),
        );
        let titles: Vec<_> = groups.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Today",
                "Yesterday",
                "This week",
                "Last week (1)",
                "May 2026 (1)",
                "April 2026 (1)"
            ]
        );
    }

    #[test]
    fn only_the_recent_bands_start_open() {
        let groups = sidebar(
            &at_offsets(&[0, DAY, 2 * DAY, 4 * DAY, 20 * DAY]),
            NOW,
            &utc(),
        );
        let open: Vec<_> = groups.iter().map(|g| g.expanded).collect();
        assert_eq!(open, [true, true, true, false, false]);
    }

    #[test]
    fn a_day_label_grows_a_time_when_that_day_has_more_than_one_backup() {
        // Two on Sat 6 June (last week), one on Fri 5 June.
        let groups = sidebar(
            &at_offsets(&[4 * DAY, 4 * DAY + HOUR, 5 * DAY]),
            NOW,
            &utc(),
        );
        let labels: Vec<_> = groups[0].rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            ["Sat 6 Jun, 12:00", "Sat 6 Jun, 11:00", "Fri 5 Jun"]
        );
    }

    #[test]
    fn on_a_monday_the_sunday_before_is_still_yesterday() {
        // Monday 2026-06-08 12:00 UTC.
        let monday = NOW - 2 * DAY;
        let groups = sidebar(
            &[archive(2, monday), archive(1, monday - DAY)],
            monday,
            &utc(),
        );
        let titles: Vec<_> = groups.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["Today", "Yesterday"]);
    }

    #[test]
    fn a_backup_held_on_this_computer_is_marked_as_such() {
        let mut spool = archive(9, NOW);
        spool.repo = "spool".to_string();
        let mut snapshot = archive(8, NOW - HOUR);
        snapshot.repo = "fs-snapshot".to_string();
        let groups = sidebar(&[spool, snapshot, archive(7, NOW - 2 * HOUR)], NOW, &utc());
        let local: Vec<_> = groups[0].rows.iter().map(|r| r.local_only).collect();
        assert_eq!(local, [true, true, false]);
    }

    #[test]
    fn an_archive_still_being_catalogued_is_flagged_rather_than_hidden() {
        let mut pending = archive(9, NOW);
        pending.catalogued = false;
        let groups = sidebar(&[pending], NOW, &utc());
        assert!(!groups[0].rows[0].catalogued);
    }

    #[test]
    fn no_archives_means_no_groups() {
        assert!(sidebar(&[], NOW, &utc()).is_empty());
    }

    #[test]
    fn a_backup_dated_in_the_future_lands_in_today_rather_than_a_band_of_its_own() {
        // Clock skew, or a machine that woke up with the wrong time: the
        // sidebar must not grow a "June 2027" group above Today.
        let groups = sidebar(&at_offsets(&[-2 * DAY, 0]), NOW, &utc());
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].title, "Today");
    }
}
