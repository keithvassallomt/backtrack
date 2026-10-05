// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Restoring a whole computer: what the window says about it.
//!
//! Screen 12 and mockups 21 and 22. The entry page offers it straight after
//! somebody's backups are imported onto a new computer; the progress window
//! follows it to the end, through a pause, a restart, or a closed window.
//! Everything here is a string or a number derived from what the daemon and
//! the catalogue say, so it is tested here rather than by looking.

use backtrack_core::dbus::RecoveryStatus;
use gtk4::glib;

use super::format;

/// The key of the step holding the hidden folders and loose files.
pub use backtrack_core::recovery::THE_REST;

/// The page title, from the mockup.
pub const WELCOME: &str = "Welcome back. Restore this computer?";

/// The two promises under the choices, from the mockup.
pub const PROMISES: &str = "Files already on this computer are never overwritten without asking.\n\
                            New backups start after the restore finishes.";

/// The three choices, in the mockup's words.
pub const EVERYTHING: &str = "Restore everything";
pub const SELECTED: &str = "Restore selected folders…";
pub const BROWSE: &str = "Just browse — restore things later";

/// One backup that can be restored from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub seq: i64,
    pub name: String,
    pub ts: i64,
}

/// The computer an archive came from, out of its name: `bt-{host}-{time}`.
/// `None` for a name Backtrack did not give it.
pub fn host_of(archive: &str) -> Option<String> {
    let rest = archive.strip_prefix("bt-")?;
    let (host, stamp) = rest.rsplit_once('-')?;
    let looks_like_a_time = stamp.len() == 16
        && stamp.ends_with('Z')
        && stamp.as_bytes()[8] == b'T'
        && stamp[..8].bytes().all(|b| b.is_ascii_digit());
    (looks_like_a_time && !host.is_empty() && host != "local").then(|| host.to_string())
}

/// The person whose home folder the backup holds: the folder's own name.
pub fn user_of(source: &str) -> Option<String> {
    let name = source.rsplit('/').next()?;
    (!name.is_empty()).then(|| name.to_string())
}

/// When a backup was taken, the way the dropdown says it: `Yesterday, 22:00`.
pub fn when(ts: i64, now: i64, tz: &glib::TimeZone) -> String {
    let day = super::local_day(ts, tz);
    let today = super::local_day(now, tz);
    let clock = format::clock(ts, tz);
    if day == today {
        format!("Today, {clock}")
    } else if day == today - 1 {
        format!("Yesterday, {clock}")
    } else if format::at(ts, tz, "%Y") == format::at(now, tz, "%Y") {
        format::weekday_day_and_clock(ts, tz)
    } else {
        format::at(ts, tz, "%e %b %Y, %H:%M")
    }
}

/// The line under the title: whose backups these are, and what is in them.
pub fn found_line(
    user: Option<&str>,
    host: Option<&str>,
    latest: i64,
    bytes: u64,
    count: usize,
    now: i64,
    tz: &glib::TimeZone,
) -> String {
    let whose = match (user, host) {
        (Some(user), Some(host)) => format!("Backups found for {user}@{host}"),
        (Some(name), None) | (None, Some(name)) => format!("Backups found for {name}"),
        (None, None) => "Backups found".to_string(),
    };
    format!(
        "{whose} — Latest: {} · {} · {}",
        when(latest, now, tz),
        size(bytes),
        match count {
            1 => "1 snapshot".to_string(),
            n => format!("{n} snapshots"),
        }
    )
}

/// Said under the choices when this computer has less room than restoring
/// everything needs, before anything is started rather than part-way through.
pub fn room_warning(needed: u64, free: u64) -> Option<String> {
    (needed > free).then(|| {
        format!(
            "Restoring everything needs about {} and this computer has {} free. \
             Choose fewer folders, or make room first.",
            size(needed),
            size(free)
        )
    })
}

/// What a step is called on screen.
pub fn step_name(key: &str) -> String {
    if key == THE_REST {
        "Settings and other files".to_string()
    } else {
        key.to_string()
    }
}

/// The percentage restored. Never 100 until it is all there: a bar that says
/// it is finished while the last file is still arriving is the one lie a
/// progress bar must not tell.
pub fn percent(done: u64, total: u64) -> u32 {
    if total == 0 {
        return 0;
    }
    if done >= total {
        return 100;
    }
    ((done as u128 * 100 / total as u128) as u32).min(99)
}

/// The unit `total` reads best in, and what a byte count looks like in it:
/// whole numbers from ten upwards, one decimal below.
fn in_units_of(total: u64) -> (&'static str, impl Fn(u64) -> String) {
    const UNITS: [&str; 5] = ["bytes", "kB", "MB", "GB", "TB"];
    let mut scale = 1u64;
    let mut unit = 0;
    while unit + 1 < UNITS.len() && total >= scale * 1000 {
        scale *= 1000;
        unit += 1;
    }
    let whole = unit == 0 || total as f64 / scale as f64 >= 10.0;
    (UNITS[unit], move |bytes: u64| {
        let value = bytes as f64 / scale as f64;
        if whole {
            format!("{}", value.round() as u64)
        } else {
            format!("{value:.1}")
        }
    })
}

/// A size as the mockups write one: `118 GB`, `2.1 GB`.
pub fn size(bytes: u64) -> String {
    let (unit, show) = in_units_of(bytes);
    format!("{} {unit}", show(bytes))
}

/// `"72 of 118 GB"`: both in the unit of the total, so the two can be
/// compared at a glance.
pub fn amount(done: u64, total: u64) -> String {
    let (unit, show) = in_units_of(total);
    format!("{} of {} {unit}", show(done.min(total)), show(total))
}

/// How long is left, the way a person would say it, or what to say instead.
pub fn time_left(eta: i64) -> String {
    match eta {
        ..=-1 => "working out how long".to_string(),
        0..=59 => "less than a minute left".to_string(),
        60..=5_399 => {
            let minutes = (eta + 30) / 60;
            if minutes == 1 {
                "about 1 minute left".to_string()
            } else {
                format!("about {minutes} minutes left")
            }
        }
        _ => format!("about {} hours left", (eta + 1_800) / 3_600),
    }
}

/// What the window's heading says.
pub fn heading(status: &RecoveryStatus) -> &'static str {
    match status.state.as_str() {
        "running" => "Restoring your files…",
        "paused" => "Restoring is paused",
        "stopped" => "Restoring stopped",
        _ => "Your files are back",
    }
}

/// The line under the bar: `61% · 72 of 118 GB · about 40 minutes left`.
pub fn progress_line(status: &RecoveryStatus) -> String {
    let start = format!(
        "{}% · {}",
        percent(status.done, status.total),
        amount(status.done, status.total)
    );
    match status.state.as_str() {
        "running" => format!("{start} · {}", time_left(status.eta)),
        "paused" => format!("{start} · paused"),
        _ => start,
    }
}

/// What is being restored now, below the bar.
pub fn restoring_line(status: &RecoveryStatus) -> String {
    match status.state.as_str() {
        "running" if !status.current.is_empty() => format!("Restoring: {}", status.current),
        "running" => status
            .steps
            .iter()
            .find(|s| s.status == "current")
            .map(|s| format!("Restoring: {}", step_name(&s.key)))
            .unwrap_or_default(),
        "paused" => "Nothing is being restored until you resume.".to_string(),
        "stopped" if !status.error.is_empty() => format!("Why: {}.", status.error),
        "stopped" => "Try again to carry on from where it stopped.".to_string(),
        _ => String::new(),
    }
}

/// What the finished window says, with how many files came back.
pub fn finished_line(status: &RecoveryStatus) -> String {
    let restored = match status.restored {
        1 => "1 file restored.".to_string(),
        n => format!("{n} files restored."),
    };
    match status.conflicts {
        0 => format!("{restored} Backups start again now."),
        1 => format!(
            "{restored} 1 file already on this computer is different in the backup. Choose which to keep."
        ),
        n => format!(
            "{restored} {n} files already on this computer are different in the backup. Choose which to keep."
        ),
    }
}

/// What the cancel confirmation says.
pub const CANCEL_TITLE: &str = "Stop restoring your files?";
pub const CANCEL_BODY: &str = "What has been restored so far can stay, or be taken away again. \
                               Files that were on this computer before are not touched either way.";
pub const KEEP: &str = "Keep what's restored so far";
pub const DISCARD: &str = "Discard";

/// What the window says once a cancel has gone through.
pub fn cancelled_line(discarded: bool) -> &'static str {
    if discarded {
        "What was restored has been taken away again. Your backups are untouched."
    } else {
        "What was restored so far has been kept."
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use backtrack_core::dbus::RecoveryStep;

    fn utc() -> glib::TimeZone {
        glib::TimeZone::utc()
    }

    /// Wednesday 2026-10-07 12:00:00 UTC.
    const NOW: i64 = 1_791_374_400;

    fn status(state: &str) -> RecoveryStatus {
        RecoveryStatus {
            state: state.into(),
            job: 3,
            archive: "bt-fedora-20261006T220000Z".into(),
            taken: NOW - 50_400,
            done: 72_000_000_000,
            total: 118_000_000_000,
            eta: 2_400,
            current: "Pictures/2025/holiday/IMG_2041.jpg".into(),
            steps: vec![
                RecoveryStep {
                    key: "Documents".into(),
                    bytes: 1,
                    status: "done".into(),
                },
                RecoveryStep {
                    key: "Pictures".into(),
                    bytes: 2,
                    status: "current".into(),
                },
                RecoveryStep {
                    key: ".".into(),
                    bytes: 3,
                    status: "pending".into(),
                },
            ],
            restored: 12_345,
            conflicts: 0,
            error: String::new(),
        }
    }

    #[test]
    fn the_progress_line_reads_as_the_mockup_does() {
        assert_eq!(
            progress_line(&status("running")),
            "61% · 72 of 118 GB · about 40 minutes left"
        );
        assert_eq!(
            restoring_line(&status("running")),
            "Restoring: Pictures/2025/holiday/IMG_2041.jpg"
        );
        assert_eq!(heading(&status("running")), "Restoring your files…");
    }

    #[test]
    fn a_paused_or_stopped_restore_says_so_and_promises_no_time() {
        assert_eq!(
            progress_line(&status("paused")),
            "61% · 72 of 118 GB · paused"
        );
        let mut stopped = status("stopped");
        stopped.error = "the backup drive could not be reached".into();
        assert_eq!(progress_line(&stopped), "61% · 72 of 118 GB");
        assert_eq!(
            restoring_line(&stopped),
            "Why: the backup drive could not be reached."
        );
        assert_eq!(heading(&stopped), "Restoring stopped");
    }

    #[test]
    fn between_files_the_step_is_named_instead() {
        let mut between = status("running");
        between.current.clear();
        assert_eq!(restoring_line(&between), "Restoring: Pictures");
        between.steps[1].status = "done".into();
        between.steps[2].status = "current".into();
        assert_eq!(
            restoring_line(&between),
            "Restoring: Settings and other files"
        );
    }

    #[test]
    fn the_time_left_is_said_the_way_people_say_it() {
        assert_eq!(time_left(-1), "working out how long");
        assert_eq!(time_left(20), "less than a minute left");
        assert_eq!(time_left(70), "about 1 minute left");
        assert_eq!(time_left(2_400), "about 40 minutes left");
        assert_eq!(time_left(80 * 60), "about 80 minutes left");
        assert_eq!(time_left(3 * 3_600 + 600), "about 3 hours left");
    }

    #[test]
    fn amounts_share_the_totals_unit() {
        assert_eq!(amount(72_000_000_000, 118_000_000_000), "72 of 118 GB");
        assert_eq!(amount(400_000_000, 2_100_000_000), "0.4 of 2.1 GB");
        assert_eq!(amount(500, 900), "500 of 900 bytes");
        assert_eq!(amount(0, 0), "0 of 0 bytes");
        assert_eq!(size(118_000_000_000), "118 GB");
        assert_eq!(size(2_100_000_000), "2.1 GB");
    }

    #[test]
    fn a_bar_says_100_only_when_it_is_all_there() {
        assert_eq!(percent(0, 0), 0);
        assert_eq!(percent(999, 1_000), 99);
        assert_eq!(percent(1_000, 1_000), 100);
        assert_eq!(percent(61, 100), 61);
    }

    #[test]
    fn backups_are_found_for_the_person_and_the_computer_they_came_from() {
        assert_eq!(
            host_of("bt-fedora-20261006T220000Z").as_deref(),
            Some("fedora")
        );
        assert_eq!(
            host_of("bt-thor-arch-vassallo-cloud-20261004T131944Z").as_deref(),
            Some("thor-arch-vassallo-cloud")
        );
        assert_eq!(
            host_of("bt-local-20261006T220000Z"),
            None,
            "a local snapshot"
        );
        assert_eq!(host_of("my-own-archive"), None);
        assert_eq!(user_of("home/keith").as_deref(), Some("keith"));
        assert_eq!(user_of(""), None);

        assert_eq!(
            found_line(
                Some("keith"),
                Some("fedora"),
                NOW - 50_400,
                118_000_000_000,
                47,
                NOW,
                &utc()
            ),
            "Backups found for keith@fedora — Latest: Yesterday, 22:00 · 118 GB · 47 snapshots"
        );
        assert_eq!(
            found_line(None, None, NOW, 1_000, 1, NOW, &utc()),
            "Backups found — Latest: Today, 12:00 · 1.0 kB · 1 snapshot"
        );
    }

    #[test]
    fn a_backup_is_dated_relative_to_today_where_that_is_clearer() {
        let tz = utc();
        assert_eq!(when(NOW - 3_600, NOW, &tz), "Today, 11:00");
        assert_eq!(when(NOW - 50_400, NOW, &tz), "Yesterday, 22:00");
        assert_eq!(when(NOW - 5 * 86_400, NOW, &tz), "Fri 2 Oct, 12:00");
        assert_eq!(when(NOW - 400 * 86_400, NOW, &tz), "2 Sep 2025, 12:00");
    }

    #[test]
    fn too_little_room_is_said_before_anything_starts() {
        assert_eq!(room_warning(10, 20), None);
        assert_eq!(
            room_warning(118_000_000_000, 80_000_000_000).as_deref(),
            Some(
                "Restoring everything needs about 118 GB and this computer has 80 GB free. \
                 Choose fewer folders, or make room first."
            )
        );
    }

    #[test]
    fn the_rest_has_a_name_a_person_can_read() {
        assert_eq!(step_name("Documents"), "Documents");
        assert_eq!(step_name("."), "Settings and other files");
    }

    #[test]
    fn the_end_says_how_many_and_what_is_waiting() {
        assert_eq!(
            finished_line(&status("finished")),
            "12345 files restored. Backups start again now."
        );
        let mut waiting = status("review");
        waiting.conflicts = 2;
        assert_eq!(
            finished_line(&waiting),
            "12345 files restored. 2 files already on this computer are different in the backup. Choose which to keep."
        );
    }
}
