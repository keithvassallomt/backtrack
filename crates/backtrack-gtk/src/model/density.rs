// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The timeline density strip: one bar per day across the whole history.
//!
//! The strip is an indicator first and a control second. Its job is to show, at
//! a glance, where the backups are thick and where they are thin — the week you
//! were away, the month before the laptop was set up — which a list of groups
//! cannot. Days with nothing are drawn as gaps rather than skipped, because a
//! gap is the information.
//!
//! As a control it only ever jumps to a snapshot that exists: a position along
//! the strip resolves to the nearest real archive, never to a point in between.
//!
//! With Stage 15 the bars are the backups listed for the folder being viewed,
//! drawn across the whole history: the axis and its months stay put from
//! folder to folder, and only the bars change. A day can then be empty in two
//! ways, which the readout tells apart: nothing in the folder changed, or no
//! backup was taken at all.

use backtrack_core::index::ArchiveSummary;
use gtk4::glib;

use super::{day_to_date, local_day};

/// One day of the history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bar {
    /// Days since the epoch, local.
    pub day: i64,
    /// Backups listed that day, which is what the bar's height shows.
    pub count: u32,
    /// Every backup taken that day, listed or not.
    pub taken: u32,
}

/// Where a month begins, for the labels under the strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonthMark {
    /// Index into [`Density::bars`].
    pub bar: usize,
    /// Short month name, localized: `"Jun"`.
    pub text: String,
}

/// The whole strip.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Density {
    /// Oldest first, one entry per day with no days omitted.
    pub bars: Vec<Bar>,
    /// The busiest day, so bar heights can be scaled. Never 0 when `bars` is
    /// non-empty, so it is safe to divide by.
    pub max: u32,
    pub months: Vec<MonthMark>,
}

impl Density {
    pub fn is_empty(&self) -> bool {
        self.bars.is_empty()
    }

    /// Which bar an archive sits in — where the position marker is drawn.
    pub fn bar_of(
        &self,
        archives: &[ArchiveSummary],
        seq: i64,
        tz: &glib::TimeZone,
    ) -> Option<usize> {
        let archive = archives.iter().find(|a| a.seq == seq)?;
        let day = local_day(archive.ts, tz);
        self.bars.iter().position(|b| b.day == day)
    }
}

/// Build the strip from `archives` (any order), every one of them listed.
#[cfg(test)]
pub fn density(archives: &[ArchiveSummary], tz: &glib::TimeZone) -> Density {
    density_of(archives, archives, tz)
}

/// Build the strip for the backups `listed`, across the history `all` spans
/// (both in any order; `listed` drawn from `all`).
pub fn density_of(
    listed: &[ArchiveSummary],
    all: &[ArchiveSummary],
    tz: &glib::TimeZone,
) -> Density {
    let mut days: Vec<i64> = all.iter().map(|a| local_day(a.ts, tz)).collect();
    // A Trash entry can come after the newest backup (Stage 16), and is drawn
    // on the axis like any other place.
    let reach = listed.iter().map(|a| local_day(a.ts, tz));
    let (Some(first), Some(last)) = (
        days.iter().copied().chain(reach.clone()).min(),
        days.iter().copied().chain(reach).max(),
    ) else {
        return Density::default();
    };
    days.sort_unstable();

    // A history is bounded by how long the user has had the machine, so this
    // vector is days-of-history long: thousands at the very most.
    let mut bars: Vec<Bar> = (first..=last)
        .map(|day| Bar {
            day,
            count: 0,
            taken: 0,
        })
        .collect();
    for day in &days {
        bars[(day - first) as usize].taken += 1;
    }
    for archive in listed {
        let offset = local_day(archive.ts, tz) - first;
        if let Some(bar) = usize::try_from(offset).ok().and_then(|i| bars.get_mut(i)) {
            bar.count += 1;
        }
    }

    let max = bars.iter().map(|b| b.count).max().unwrap_or(1).max(1);
    let months = month_marks(&bars, tz);
    Density { bars, max, months }
}

/// The archive a position along the strip refers to: `fraction` is 0.0 at the
/// oldest bar and 1.0 at the newest, and the answer is always a real archive.
///
/// The strip is drawn per day but resolved per archive, so dragging across a
/// day that holds twenty hourlies lands on the one nearest the cursor rather
/// than on an arbitrary member of the day.
pub fn seq_at(
    density: &Density,
    archives: &[ArchiveSummary],
    fraction: f64,
    tz: &glib::TimeZone,
) -> Option<i64> {
    if density.is_empty() || archives.is_empty() {
        return None;
    }
    let span = density.bars.len() as f64;
    let offset = (fraction.clamp(0.0, 1.0) * span - 0.5).clamp(0.0, span - 1.0);
    let day = density.bars[0].day + offset.round() as i64;
    // Local midday, so a target day is equidistant from the days either side of
    // it rather than biased towards the earlier one — and so that a strip drawn
    // in local days is resolved in local days.
    let target = local_noon(day, tz) as f64;

    archives
        .iter()
        .min_by(|a, b| {
            let (da, db) = ((a.ts as f64 - target).abs(), (b.ts as f64 - target).abs());
            // Ties go to the newer archive, matching which one the sidebar
            // shows first for a day.
            da.partial_cmp(&db)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.ts.cmp(&a.ts))
        })
        .map(|a| a.seq)
}

/// Epoch seconds at midday on a local day.
fn local_noon(day: i64, tz: &glib::TimeZone) -> i64 {
    let (year, month, day_of_month) = day_to_date(day);
    glib::DateTime::new(tz, year, month, day_of_month, 12, 0, 0.0)
        .map(|dt| dt.to_unix())
        .unwrap_or(day * 86_400 + 43_200)
}

/// Month boundaries, plus the first bar so the leftmost month is always named.
fn month_marks(bars: &[Bar], tz: &glib::TimeZone) -> Vec<MonthMark> {
    let mut marks = Vec::new();
    let mut last_month = None;
    for (index, bar) in bars.iter().enumerate() {
        let (year, month, _) = day_to_date(bar.day);
        if last_month != Some((year, month)) {
            marks.push(MonthMark {
                bar: index,
                text: short_month(year, month, tz),
            });
            last_month = Some((year, month));
        }
    }
    marks
}

fn short_month(year: i32, month: i32, tz: &glib::TimeZone) -> String {
    glib::DateTime::new(tz, year, month, 1, 12, 0, 0.0)
        .ok()
        .and_then(|dt| dt.format("%b").ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| month.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::date_to_day;

    fn utc() -> glib::TimeZone {
        glib::TimeZone::utc()
    }

    const DAY: i64 = 86_400;

    fn at(year: i32, month: i32, day: i32, hour: i64, seq: i64) -> ArchiveSummary {
        ArchiveSummary {
            seq,
            borg_id: None,
            name: format!("snapshot-{seq}"),
            ts: date_to_day(year, month, day) * DAY + hour * 3600,
            repo: "primary".to_string(),
            catalogued: true,
            caught_up: None,
            holds_intermediate: false,
        }
    }

    #[test]
    fn only_the_backups_listed_are_drawn_across_the_whole_history() {
        // Four days of history; the folder lists the backups of the 1st and
        // the 4th only.
        let all = vec![
            at(2026, 6, 4, 12, 5),
            at(2026, 6, 3, 12, 4),
            at(2026, 6, 3, 9, 3),
            at(2026, 6, 2, 12, 2),
            at(2026, 6, 1, 12, 1),
        ];
        let listed = vec![all[0].clone(), all[4].clone()];
        let strip = density_of(&listed, &all, &utc());
        let counts: Vec<u32> = strip.bars.iter().map(|b| b.count).collect();
        let taken: Vec<u32> = strip.bars.iter().map(|b| b.taken).collect();
        assert_eq!(counts, [1, 0, 0, 1], "bars for the listed backups only");
        assert_eq!(
            taken,
            [1, 1, 2, 1],
            "on an axis from the first backup to the last"
        );
        assert_eq!(strip.max, 1, "scaled to what is drawn");
    }

    #[test]
    fn the_strip_resolves_only_to_backups_listed() {
        let all = vec![
            at(2026, 6, 4, 12, 5),
            at(2026, 6, 3, 12, 4),
            at(2026, 6, 3, 9, 3),
            at(2026, 6, 2, 12, 2),
            at(2026, 6, 1, 12, 1),
        ];
        let listed = vec![all[0].clone(), all[4].clone()];
        let strip = density_of(&listed, &all, &utc());
        for step in 0..=20 {
            let fraction = f64::from(step) / 20.0;
            let seq = seq_at(&strip, &listed, fraction, &utc()).unwrap();
            assert!(seq == 5 || seq == 1, "{fraction} landed on {seq}");
        }
        assert_eq!(strip.bar_of(&listed, 1, &utc()), Some(0));
        assert_eq!(strip.bar_of(&listed, 5, &utc()), Some(3));
    }

    #[test]
    fn a_day_with_no_backups_is_a_gap_rather_than_a_missing_bar() {
        // 1 June and 4 June, nothing in between.
        let archives = vec![at(2026, 6, 4, 12, 2), at(2026, 6, 1, 12, 1)];
        let strip = density(&archives, &utc());
        let counts: Vec<u32> = strip.bars.iter().map(|b| b.count).collect();
        assert_eq!(
            counts,
            [1, 0, 0, 1],
            "the two empty days are drawn, at zero"
        );
        assert_eq!(strip.max, 1);
    }

    #[test]
    fn the_busiest_day_sets_the_scale() {
        let archives = vec![
            at(2026, 6, 2, 9, 3),
            at(2026, 6, 2, 10, 4),
            at(2026, 6, 2, 11, 5),
            at(2026, 6, 1, 12, 1),
        ];
        let strip = density(&archives, &utc());
        assert_eq!(
            strip.bars.iter().map(|b| b.count).collect::<Vec<_>>(),
            [1, 3]
        );
        assert_eq!(strip.max, 3);
    }

    #[test]
    fn months_are_marked_where_they_start_and_the_first_one_is_always_named() {
        let archives = vec![at(2026, 6, 2, 12, 2), at(2026, 5, 30, 12, 1)];
        let strip = density(&archives, &utc());
        let marks: Vec<(usize, &str)> = strip
            .months
            .iter()
            .map(|m| (m.bar, m.text.as_str()))
            .collect();
        assert_eq!(marks, [(0, "May"), (2, "Jun")]);
    }

    #[test]
    fn a_position_along_the_strip_always_lands_on_a_real_snapshot() {
        let archives = vec![
            at(2026, 6, 10, 12, 3),
            at(2026, 6, 5, 12, 2),
            at(2026, 6, 1, 12, 1),
        ];
        let strip = density(&archives, &utc());
        let real: Vec<i64> = archives.iter().map(|a| a.seq).collect();

        // Sweep the whole strip; every point resolves to something that exists.
        for step in 0..=100 {
            let seq = seq_at(&strip, &archives, f64::from(step) / 100.0, &utc());
            assert!(
                real.contains(&seq.expect("a non-empty strip always resolves")),
                "fraction {step}/100 resolved to {seq:?}, which is not an archive"
            );
        }
    }

    #[test]
    fn the_ends_of_the_strip_are_the_ends_of_the_history() {
        let archives = vec![
            at(2026, 6, 10, 12, 3),
            at(2026, 6, 5, 12, 2),
            at(2026, 6, 1, 12, 1),
        ];
        let strip = density(&archives, &utc());
        assert_eq!(seq_at(&strip, &archives, 0.0, &utc()), Some(1));
        assert_eq!(seq_at(&strip, &archives, 1.0, &utc()), Some(3));
        // Out of range is clamped rather than refused: a drag can leave the widget.
        assert_eq!(seq_at(&strip, &archives, -3.0, &utc()), Some(1));
        assert_eq!(seq_at(&strip, &archives, 4.0, &utc()), Some(3));
    }

    #[test]
    fn a_day_of_hourlies_resolves_to_the_one_nearest_the_cursor() {
        let archives = vec![
            at(2026, 6, 1, 20, 3),
            at(2026, 6, 1, 12, 2),
            at(2026, 6, 1, 4, 1),
        ];
        let strip = density(&archives, &utc());
        // One bar, so every fraction is that day — midday is the nearest.
        assert_eq!(seq_at(&strip, &archives, 0.5, &utc()), Some(2));
    }

    #[test]
    fn the_marker_sits_on_the_bar_for_its_own_day() {
        let archives = vec![at(2026, 6, 4, 12, 2), at(2026, 6, 1, 12, 1)];
        let strip = density(&archives, &utc());
        assert_eq!(strip.bar_of(&archives, 1, &utc()), Some(0));
        assert_eq!(strip.bar_of(&archives, 2, &utc()), Some(3));
        assert_eq!(strip.bar_of(&archives, 99, &utc()), None);
    }

    #[test]
    fn an_empty_history_has_an_empty_strip_and_nothing_to_resolve() {
        let strip = density(&[], &utc());
        assert!(strip.is_empty());
        assert_eq!(seq_at(&strip, &[], 0.5, &utc()), None);
    }
}
