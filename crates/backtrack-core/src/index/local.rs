// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! How long local snapshots are kept.
//!
//! Snapshots taken on this computer while the destination is away (the spool,
//! or filesystem snapshots) are removed on a schedule. The daemon removes them
//! and the window tells people when, so the schedule is written once, here: a
//! date somebody may be counting on must not be one the daemon disagrees with.

use std::time::Duration;

use super::Repo;

/// How long a local snapshot is kept once the destination has caught up.
///
/// Not arbitrary: what these still hold once a real backup has succeeded is the
/// *intermediate* versions from the offline window — the 10:00, 11:00 and 12:00
/// edits of a file that changed several times while away. A month is long
/// enough that somebody who realises on their return that they want the
/// mid-afternoon version can still have it, and short enough that a laptop does
/// not carry an offline week around forever.
pub const EXPIRE_AFTER_CATCH_UP: Duration = Duration::from_secs(30 * 24 * 3_600);

/// How long hourly filesystem snapshots are kept, caught up or not. They are
/// nearly free to take but they pin extents, so yesterday's hourlies stop
/// earning their keep quickly.
pub const SNAPSHOT_RETENTION: Duration = Duration::from_secs(24 * 3_600);

/// When a local snapshot taken at `taken` is removed, in epoch seconds, given
/// when the destination caught up with it (`None` if it has not). `None` when
/// nothing will remove it yet, and for anything not held locally.
///
/// - A spool snapshot is kept until the destination has caught up, and for
///   [`EXPIRE_AFTER_CATCH_UP`] after that. Before then it is the only copy of
///   what it holds, and removing it would be losing data to save disk.
/// - A filesystem snapshot is kept for [`SNAPSHOT_RETENTION`], or until the
///   spool's time if that comes first.
pub fn removed_at(repo: &str, taken: i64, caught_up: Option<i64>) -> Option<i64> {
    let after_catch_up = caught_up.map(|at| at + EXPIRE_AFTER_CATCH_UP.as_secs() as i64);
    if repo == Repo::FsSnapshot.as_str() {
        let by_age = taken + SNAPSHOT_RETENTION.as_secs() as i64;
        Some(after_catch_up.map_or(by_age, |at| at.min(by_age)))
    } else if repo == Repo::Spool.as_str() {
        after_catch_up
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    #[test]
    fn a_spool_snapshot_is_kept_until_the_destination_catches_up_then_a_month() {
        assert_eq!(removed_at("spool", 1_000, None), None, "the only copy");
        assert_eq!(
            removed_at("spool", 1_000, Some(5_000)),
            Some(5_000 + 30 * DAY)
        );
    }

    #[test]
    fn hourly_filesystem_snapshots_are_kept_for_a_day() {
        assert_eq!(removed_at("fs-snapshot", 1_000, None), Some(1_000 + DAY));
    }

    #[test]
    fn a_catch_up_expiry_wins_for_a_filesystem_snapshot_when_it_comes_first() {
        // "Whichever first", both ways round: a month-long marker must not keep
        // yesterday's hourlies alive, and nothing here outlives it either.
        assert_eq!(
            removed_at("fs-snapshot", 1_000, Some(1_000 - 30 * DAY + 3_600)),
            Some(1_000 + 3_600)
        );
        assert_eq!(
            removed_at("fs-snapshot", 1_000, Some(1_000 + 3_600)),
            Some(1_000 + DAY)
        );
    }

    #[test]
    fn a_backup_at_the_destination_is_never_removed_by_this() {
        assert_eq!(removed_at("primary", 1_000, Some(2_000)), None);
    }
}
