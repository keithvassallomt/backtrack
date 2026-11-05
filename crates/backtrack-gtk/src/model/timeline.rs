// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Rewind's timeline down the right-hand edge (S18-T5): where each backup's
//! tick goes, which ticks carry a label, what hovering one says, and how much
//! the pointer magnifies them.
//!
//! The measures are the S18-T1 prototype's
//! (`backtrack_plan/mockups/rewind/rewind.html`), as mockups 25 and 26 show
//! them: a tick for the folder as it is now and one for each backup listed for
//! it, evenly spaced, the oldest at the top and now at the bottom; a label for
//! today, yesterday, then each month, at the group's newest backup.
//!
//! Ticks are counted as the stack counts its backups: 0 is now, 1 the newest
//! backup, and so on back.

use super::group::Kept;
use super::{format, local_day};

/// How far from the top of the timeline's room the oldest tick goes, as a
/// share of its height.
const TOP: f64 = 0.07;

/// How far above the bottom now's tick goes, leaving room for the lines and
/// the buttons under the stack.
const BOTTOM: f64 = 112.0;

/// The least distance between two labels: a group whose label would come
/// closer than this to the one below it goes without.
pub const LABEL_GAP: f64 = 15.0;

/// How far the magnification reaches: at this distance from the pointer a
/// tick is magnified by a little over a third, and it is gone a few ticks
/// further on.
pub const REACH: f64 = 40.0;

/// The least a tick reaches either side of its line, for the pointer: the
/// prototype's ticks are at least 8 px tall, however close together.
const LEAST_REACH: f64 = 4.0;

/// What now's tick, and every backup made today, is labelled.
pub const TODAY: &str = "Today (Now)";

/// The span the ticks take in a timeline `height` tall: the oldest's line at
/// the first, now's at the second.
pub fn band(height: f64) -> (f64, f64) {
    let top = height * TOP;
    (top, (height - BOTTOM).max(top))
}

/// The distance between two neighbouring ticks, `count` of them in `band`.
pub fn spacing(count: usize, (top, bottom): (f64, f64)) -> f64 {
    if count < 2 {
        0.0
    } else {
        (bottom - top) / (count - 1) as f64
    }
}

/// Where tick `index` of `count` goes in `band`, from the top. A lone tick,
/// now's, is at the bottom.
pub fn y_of(index: usize, count: usize, band: (f64, f64)) -> f64 {
    if count < 2 {
        return band.1;
    }
    band.0 + (count - 1 - index.min(count - 1)) as f64 * spacing(count, band)
}

/// The tick at `y`, if any: the nearest, when `y` is within half the distance
/// to its neighbours of it. The ticks never move, so this is the tick drawn
/// there, however magnified.
pub fn tick_at(y: f64, count: usize, band: (f64, f64)) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let step = spacing(count, band);
    let index = if count < 2 {
        0
    } else {
        let from_top = ((y - band.0) / step).round().clamp(0.0, (count - 1) as f64) as usize;
        count - 1 - from_top
    };
    let reach = (step / 2.0).max(LEAST_REACH);
    ((y - y_of(index, count, band)).abs() <= reach).then_some(index)
}

/// How much a tick or label `distance` from the pointer is magnified: 1 at the
/// pointer, falling off as a dock's icons do, and nothing once it is too
/// little to see.
pub fn magnification(distance: f64) -> f64 {
    let m = (-(distance / REACH).powi(2)).exp();
    if m < 0.01 {
        0.0
    } else {
        m
    }
}

/// The group a backup made at `ts` belongs to, at `now`: today's (with the
/// folder as it is now, `None`), yesterday's, then the months, this year's by
/// name and earlier ones with their year.
pub fn group(ts: Option<i64>, now: i64, tz: &glib::TimeZone) -> String {
    let Some(ts) = ts else {
        return TODAY.to_owned();
    };
    let (day, today) = (local_day(ts, tz), local_day(now, tz));
    if day >= today {
        TODAY.to_owned()
    } else if day == today - 1 {
        "Yesterday".to_owned()
    } else if format::at(ts, tz, "%Y") == format::at(now, tz, "%Y") {
        format::at(ts, tz, "%B")
    } else {
        format::at(ts, tz, "%b %Y")
    }
}

/// Which ticks carry their group's label, front first, the ticks `spacing`
/// apart: a group is labelled at its newest backup, unless that would come
/// within [`LABEL_GAP`] of the label below it. Now's is always labelled.
pub fn labelled(groups: &[String], spacing: f64) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        let first = index == 0 || groups[index - 1] != *group;
        let clear = out
            .last()
            .is_none_or(|below| (index - below) as f64 * spacing >= LABEL_GAP);
        if first && (index == 0 || clear) {
            out.push(index);
        }
    }
    out
}

/// What hovering a backup's tick says it is: when it was made, as the line
/// under the stack says it, with "Today" and "Yesterday" for the last two
/// days.
pub fn name(ts: Option<i64>, now: i64, tz: &glib::TimeZone) -> String {
    let Some(ts) = ts else {
        return TODAY.to_owned();
    };
    let (day, today) = (local_day(ts, tz), local_day(now, tz));
    if day == today {
        format!("Today, {}", format::clock(ts, tz))
    } else if day == today - 1 {
        format!("Yesterday, {}", format::clock(ts, tz))
    } else {
        format::at(ts, tz, "%a %e %b %Y, %H:%M")
            .trim_start()
            .to_owned()
    }
}

/// How a backup's tick is marked, as the sidebar marks it: Snapper's
/// snapshots and local ones in one colour, the Trash's entries and backups
/// only on this computer in another. The sidebar's style classes name them.
pub fn mark(kept: Kept) -> Option<&'static str> {
    match kept {
        Kept::Normally => None,
        Kept::Snapper => Some("snapper"),
        Kept::LocalSnapshot { .. } => Some("local-snapshot"),
        Kept::InTrash => Some("in-trash"),
        Kept::LocalOnly => Some("local-only"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc() -> glib::TimeZone {
        glib::TimeZone::utc()
    }

    /// Epoch seconds of a UTC date and time.
    fn on(year: i32, month: i32, day: i32, hour: i32) -> i64 {
        glib::DateTime::from_utc(year, month, day, hour, 0, 0.0)
            .unwrap()
            .to_unix()
    }

    /// The prototype's history, seen at 16:30 on Tuesday 27 October 2026:
    /// eight backups today, four yesterday, a daily one back to the 13th, two
    /// a month back to January, and a few from the year before.
    fn year() -> (i64, Vec<Option<i64>>) {
        let now = on(2026, 10, 27, 16) + 1800;
        let mut times = vec![None];
        times.extend((9..=16).rev().map(|h| Some(on(2026, 10, 27, h))));
        times.extend([22, 18, 12, 8].map(|h| Some(on(2026, 10, 26, h))));
        times.extend((13..=25).rev().map(|d| Some(on(2026, 10, d, 18))));
        for month in (1..=9).rev() {
            times.extend([20, 6].map(|d| Some(on(2026, month, d, 18))));
        }
        times.extend([Some(on(2025, 12, 14, 18)), Some(on(2025, 11, 3, 18))]);
        (now, times)
    }

    fn groups(now: i64, times: &[Option<i64>]) -> Vec<String> {
        times.iter().map(|ts| group(*ts, now, &utc())).collect()
    }

    fn labels(now: i64, times: &[Option<i64>], spacing: f64) -> Vec<String> {
        let groups = groups(now, times);
        labelled(&groups, spacing)
            .into_iter()
            .map(|i| groups[i].clone())
            .collect()
    }

    #[test]
    fn backups_are_grouped_by_today_yesterday_and_the_months() {
        let (now, times) = year();
        let groups = groups(now, &times);
        assert_eq!(groups[0], "Today (Now)");
        assert_eq!(
            groups[8], "Today (Now)",
            "this morning's backups are today's"
        );
        assert_eq!(groups[9], "Yesterday");
        assert_eq!(groups[13], "October", "the rest of this month");
        assert_eq!(groups[26], "September");
        assert_eq!(groups[43], "January");
        assert_eq!(groups[44], "Dec 2025", "last year's months say their year");
    }

    #[test]
    fn with_room_every_group_is_labelled_at_its_newest_backup() {
        let (now, times) = year();
        let groups = groups(now, &times);
        let at = labelled(&groups, 20.0);
        assert_eq!(at, [0, 9, 13, 26, 28, 30, 32, 34, 36, 38, 40, 42, 44, 45]);
        assert_eq!(
            at.iter().map(|i| groups[*i].as_str()).collect::<Vec<_>>(),
            [
                "Today (Now)",
                "Yesterday",
                "October",
                "September",
                "August",
                "July",
                "June",
                "May",
                "April",
                "March",
                "February",
                "January",
                "Dec 2025",
                "Nov 2025",
            ]
        );
    }

    #[test]
    fn a_label_too_close_to_the_one_below_it_is_left_out() {
        let (now, times) = year();
        // 7 px apart: the months, two ticks each, come 14 px apart, so every
        // other one goes; yesterday, nine ticks above now, keeps its label.
        assert_eq!(
            labels(now, &times, 7.0),
            [
                "Today (Now)",
                "Yesterday",
                "October",
                "September",
                "July",
                "May",
                "March",
                "January",
                "Nov 2025",
            ]
        );
        // 1 px apart: now's label is kept however crowded. Yesterday's and
        // October's newest backups are 9 and 13 px above it, too close;
        // September's is 26 px, and January's 16 px above September's. A group
        // is never labelled anywhere but at its newest backup.
        assert_eq!(
            labels(now, &times, 1.0),
            ["Today (Now)", "September", "January"]
        );
    }

    #[test]
    fn a_label_exactly_the_gap_away_is_kept() {
        let groups: Vec<String> = [TODAY, TODAY, TODAY, "Yesterday"]
            .map(str::to_owned)
            .to_vec();
        assert_eq!(labelled(&groups, LABEL_GAP / 3.0), [0, 3]);
        assert_eq!(labelled(&groups, LABEL_GAP / 3.0 - 0.01), [0]);
    }

    #[test]
    fn a_backup_from_a_clock_ahead_is_todays() {
        let (now, _) = year();
        // Stamped tomorrow, 01:30, by a clock nine hours ahead.
        assert_eq!(group(Some(now + 9 * 3600), now, &utc()), TODAY);
    }

    #[test]
    fn now_alone_is_labelled() {
        assert_eq!(labelled(&[TODAY.to_owned()], 0.0), [0]);
        assert!(labelled(&[], 10.0).is_empty());
    }

    #[test]
    fn the_oldest_is_at_the_top_and_now_at_the_bottom() {
        let room = band(1000.0);
        assert_eq!(room, (70.0, 888.0));
        assert_eq!(y_of(0, 3, room), 888.0);
        assert_eq!(y_of(1, 3, room), 479.0);
        assert_eq!(y_of(2, 3, room), 70.0);
        assert_eq!(y_of(0, 1, room), 888.0, "now alone, at the bottom");
        let (top, bottom) = band(100.0);
        assert_eq!(top, bottom, "no room: one line");
    }

    #[test]
    fn the_pointer_finds_the_tick_drawn_where_it_is() {
        let room = (100.0, 500.0);
        // Five ticks, 100 px apart: now at 500, the oldest at 100.
        assert_eq!(tick_at(500.0, 5, room), Some(0));
        assert_eq!(tick_at(451.0, 5, room), Some(0));
        assert_eq!(tick_at(449.0, 5, room), Some(1));
        assert_eq!(tick_at(100.0, 5, room), Some(4));
        assert_eq!(tick_at(40.0, 5, room), None, "above the oldest");
        assert_eq!(tick_at(560.0, 5, room), None, "below now");
        // Close together, a tick still reaches 4 px either side.
        let tight = (100.0, 102.0);
        assert_eq!(tick_at(105.0, 3, tight), Some(0));
        assert_eq!(tick_at(107.0, 3, tight), None);
        assert_eq!(tick_at(500.0, 1, room), Some(0));
        assert_eq!(tick_at(300.0, 1, room), None);
        assert_eq!(tick_at(300.0, 0, room), None);
    }

    #[test]
    fn the_pointer_magnifies_what_is_near_it() {
        assert_eq!(magnification(0.0), 1.0);
        assert!((magnification(REACH) - (-1f64).exp()).abs() < 1e-12);
        assert!(magnification(20.0) > magnification(30.0));
        assert_eq!(magnification(-20.0), magnification(20.0));
        assert_eq!(magnification(3.0 * REACH), 0.0, "a few ticks on, nothing");
    }

    #[test]
    fn hovering_names_the_backup() {
        let (now, _) = year();
        let tz = utc();
        assert_eq!(name(None, now, &tz), "Today (Now)");
        assert_eq!(name(Some(on(2026, 10, 27, 9)), now, &tz), "Today, 09:00");
        assert_eq!(
            name(Some(on(2026, 10, 26, 22)), now, &tz),
            "Yesterday, 22:00"
        );
        assert_eq!(
            name(Some(on(2026, 9, 20, 18)), now, &tz),
            "Sun 20 Sep 2026, 18:00"
        );
    }

    #[test]
    fn ticks_are_marked_as_the_sidebar_marks_backups() {
        assert_eq!(mark(Kept::Normally), None);
        assert_eq!(mark(Kept::Snapper), Some("snapper"));
        assert_eq!(
            mark(Kept::LocalSnapshot { until: 0 }),
            Some("local-snapshot")
        );
        assert_eq!(mark(Kept::InTrash), Some("in-trash"));
        assert_eq!(mark(Kept::LocalOnly), Some("local-only"));
    }
}
