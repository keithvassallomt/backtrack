// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! A backup under way, in words (S06-T12).
//!
//! One line, from what `BackupProgress` carries, which the window's banner,
//! the tray and `backtrack status` all show, so that the three never tell
//! somebody different things about the same backup. The wording is the first
//! backup page's in the wizard: an amount read, of an "about" total while
//! one is known, held short of finished until the backup says it has ended.

use crate::dbus::{PHASE_ARCHIVING, PHASE_CATALOGUING, PHASE_PRUNING};

/// Bytes as people read them: decimal units, one decimal place ("2.5 GB").
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// How far through the reading `current` of `total` bytes is, as a whole
/// percentage, or `None` with no total to go by. Never 100: an estimate that
/// turned out low must not say a backup has finished while it is still
/// running; the end of the backup says that.
pub fn percent(current: u64, total: u64) -> Option<u8> {
    if total == 0 {
        return None;
    }
    Some(((current as f64 / total as f64) * 100.0).floor().min(99.0) as u8)
}

/// The line for a backup at `phase`, having read `current` bytes of an
/// estimated `total` (0 for none yet).
pub fn line(phase: &str, current: u64, total: u64) -> String {
    match phase {
        PHASE_ARCHIVING => match percent(current, total) {
            Some(percent) => format!(
                "Backing up… {} of about {} ({percent}%)",
                format_bytes(current),
                format_bytes(total)
            ),
            None => format!("Backing up… {} so far", format_bytes(current)),
        },
        PHASE_CATALOGUING => "Backing up… making your backup browsable".to_string(),
        PHASE_PRUNING => "Backing up… finishing up".to_string(),
        _ => "Backing up now…".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_read_the_way_people_say_them() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1_500), "1.5 kB");
        assert_eq!(format_bytes(2_500_000_000), "2.5 GB");
        assert_eq!(format_bytes(31_496_447_281), "31.5 GB");
    }

    #[test]
    fn archiving_says_how_far_of_about_how_much() {
        assert_eq!(
            line(PHASE_ARCHIVING, 31_496_447_281, 53_100_000_000),
            "Backing up… 31.5 GB of about 53.1 GB (59%)"
        );
        // Before the sources are measured, or if they cannot be.
        assert_eq!(
            line(PHASE_ARCHIVING, 31_496_447_281, 0),
            "Backing up… 31.5 GB so far"
        );
    }

    #[test]
    fn an_estimate_that_was_low_never_says_done() {
        assert_eq!(percent(53_100_000_000, 53_100_000_000), Some(99));
        assert_eq!(percent(60_000_000_000, 53_100_000_000), Some(99));
        assert_eq!(percent(0, 53_100_000_000), Some(0));
        assert_eq!(percent(5, 0), None);
    }

    #[test]
    fn the_phases_after_archiving_say_what_is_happening() {
        assert_eq!(
            line(PHASE_CATALOGUING, 1, 1),
            "Backing up… making your backup browsable"
        );
        assert_eq!(line(PHASE_PRUNING, 1, 1), "Backing up… finishing up");
        assert_eq!(line("", 0, 0), "Backing up now…");
    }
}
