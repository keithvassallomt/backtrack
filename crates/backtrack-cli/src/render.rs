// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Turning daemon replies into output.
//!
//! Two audiences with opposite needs. A person wants the answer in a sentence
//! and does not care about epoch seconds; a script wants a shape that will not
//! change under it. So `--json` is treated as an interface with the same
//! seriousness as the D-Bus one — field names and types are pinned by a golden
//! test, and the human rendering is free to be rewritten whenever it reads
//! better.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use backtrack_core::dbus::{SearchResult, Status};

/// How far the backup under way has got, as the daemon's
/// `GetBackupProgress` says (S06-T12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub job: u64,
    pub phase: String,
    /// Bytes read so far.
    pub read: u64,
    /// The sources' measured size, 0 while it is not known.
    pub total: u64,
}

impl Progress {
    /// The daemon's answer, or `None` when no backup is under way.
    pub fn from_reply((job, phase, read, total): (u64, String, u64, u64)) -> Option<Progress> {
        (job != 0).then_some(Progress {
            job,
            phase,
            read,
            total,
        })
    }

    /// The line the window's banner and the tray show for it.
    pub fn line(&self) -> String {
        backtrack_core::progress::line(&self.phase, self.read, self.total)
    }
}

/// The JSON form of `status`.
///
/// Deliberately close to the wire type: the extra fields are the *derived*
/// answers a script would otherwise have to compute (and get wrong), never a
/// reinterpretation of the raw ones.
pub fn status_json(
    status: &Status,
    progress: Option<&Progress>,
    now: SystemTime,
) -> serde_json::Value {
    serde_json::json!({
        "state": status.state,
        // What the state is about, or null when healthy: the same token the
        // daemon sends, so a script can tell a full disk from a lost
        // passphrase without parsing prose.
        "reason": if status.reason.is_empty() { serde_json::Value::Null } else { status.reason.clone().into() },
        "healthy": status.state == "HEALTHY",
        "configured": status.configured,
        "destination_reachable": status.destination_reachable,
        "last_backup": epoch_field(status.last_backup),
        "last_backup_age_seconds": age_seconds(status.last_backup, now),
        "next_backup": epoch_field(status.next_backup),
        "paused": status.paused_until > 0,
        "paused_until": epoch_field(status.paused_until),
        "active_job": if status.active_job == 0 { serde_json::Value::Null } else { status.active_job.into() },
        "spool_bytes": status.spool_bytes,
        // What is being held on this computer while the destination is away.
        // `offline` is derived rather than sent, because a script asking "am I
        // away from my backups?" should not have to know that the answer is
        // spelled as the negation of something else.
        "offline": !status.destination_reachable,
        "offline_mode": status.offline_mode,
        "local_snapshots": status.local_snapshots,
        // The backup under way, or null between backups. `total_bytes` and
        // `percent` are null until the sources have been measured.
        "progress": progress.map(|p| serde_json::json!({
            "job": p.job,
            "phase": p.phase,
            "read_bytes": p.read,
            "total_bytes": if p.total == 0 { serde_json::Value::Null } else { p.total.into() },
            "percent": backtrack_core::progress::percent(p.read, p.total),
            "line": p.line(),
        })),
        "expirable_snapshots": status.expirable_snapshots,
    })
}

/// `0` means "never" on the wire; JSON has null, so use it.
fn epoch_field(seconds: u64) -> serde_json::Value {
    match seconds {
        0 => serde_json::Value::Null,
        s => s.into(),
    }
}

/// How long ago `seconds` was, or null if it never happened or is in the future.
fn age_seconds(seconds: u64, now: SystemTime) -> serde_json::Value {
    let Some(then) = from_epoch(seconds) else {
        return serde_json::Value::Null;
    };
    match now.duration_since(then) {
        Ok(age) => age.as_secs().into(),
        Err(_) => serde_json::Value::Null,
    }
}

/// The human rendering of `status`: what happened, what is next, and whether
/// anything is wrong — in that order, because that is the order people ask.
pub fn status_human(status: &Status, progress: Option<&Progress>, now: SystemTime) -> String {
    // Being set up comes first, and overrides the health state entirely. A
    // machine with no destination reports HEALTHY — correctly, since nothing has
    // failed — but printing "Your files are backed up" above "no destination is
    // set up" would be a plain lie to somebody who has not started yet.
    if !status.configured {
        return "Backtrack isn't set up yet.\n\n\
                No backup destination has been chosen, so nothing is being backed up.\n\
                Run the setup wizard, or `backtrack config set storage.repository \"…\"`.\n"
            .to_string();
    }

    let mut out = format!("{}\n", headline(status));

    out.push_str(&format!(
        "\n  Last backup   {}\n",
        match from_epoch(status.last_backup) {
            Some(t) => format!("{} ({})", format_time(t), relative(t, now)),
            None => "never".to_string(),
        }
    ));

    out.push_str(&format!(
        "  Next backup   {}\n",
        match (
            from_epoch(status.paused_until),
            from_epoch(status.next_backup)
        ) {
            (Some(until), _) => format!("paused until {}", format_time(until)),
            (None, Some(t)) if t <= now => "due now".to_string(),
            (None, Some(t)) => format!("{} ({})", format_time(t), relative(t, now)),
            (None, None) => "not scheduled (manual backups only)".to_string(),
        }
    ));

    out.push_str(&format!(
        "  Destination   {}\n",
        if status.destination_reachable {
            "reachable"
        } else {
            "not reachable"
        }
    ));

    if status.local_snapshots > 0 {
        // Bytes only when there are bytes to speak of: filesystem snapshots
        // share their storage with the live files, so a "0 B" beside a count of
        // fourteen would read as a bug rather than as the truth.
        let held = if status.spool_bytes > 0 {
            format!(
                "{} · {}",
                plural(status.local_snapshots, "snapshot"),
                format_bytes(status.spool_bytes)
            )
        } else {
            plural(status.local_snapshots, "snapshot")
        };
        out.push_str(&format!("  On this PC    {held}\n"));
    }
    if status.active_job > 0 {
        match progress.filter(|p| p.job == status.active_job) {
            Some(p) => out.push_str(&format!(
                "  Running       job {} · {}\n",
                status.active_job,
                p.line()
            )),
            None => out.push_str(&format!("  Running       job {}\n", status.active_job)),
        }
    }
    out
}

/// The one-line summary, phrased for the state it describes.
fn headline(status: &Status) -> &'static str {
    match status.state.as_str() {
        "HEALTHY" => "Your files are backed up.",
        "PROTECTED_LOCALLY" => {
            "The backup destination isn't reachable — changes are being kept on this computer."
        }
        "PAUSED" => "Backups are paused.",
        "AT_RISK" => "Your files haven't been backed up recently.",
        "BROKEN" => "Backups can't run until something is fixed.",
        "DEGRADED" => "Backups are running, but something needs attention.",
        _ => "Backup status unknown.",
    }
}

/// The JSON form of one search result.
pub fn search_json(hits: &[SearchResult]) -> serde_json::Value {
    serde_json::Value::Array(
        hits.iter()
            .map(|hit| {
                serde_json::json!({
                    "path": hit.path,
                    "name": hit.name,
                    "kind": hit.kind,
                    "versions": hit.versions,
                    "gone_from_disk": hit.gone_from_disk,
                    "first_seq": hit.first_seq,
                    "last_seq": hit.last_seq,
                    "first_seen": hit.first_ts,
                    "last_seen": hit.last_ts,
                    "trash": hit.trash,
                    "trashed_at": hit.trashed_at,
                    "trashed_size": hit.trashed_size,
                })
            })
            .collect(),
    )
}

/// The human rendering of search results. Deleted files are called out, because
/// they are the ones somebody is usually hunting for.
pub fn search_human(hits: &[SearchResult]) -> String {
    if hits.is_empty() {
        return "Nothing found.\n".to_string();
    }
    let mut out = format!(
        "{} match{}\n\n",
        hits.len(),
        if hits.len() == 1 { "" } else { "es" }
    );
    let when = |ts: i64| {
        from_epoch(ts.max(0) as u64)
            .map(format_time)
            .unwrap_or_else(|| "unknown".into())
    };
    for hit in hits {
        let marker = if hit.gone_from_disk { "×" } else { " " };
        let history = match hit.versions {
            0 => "in no backup".to_string(),
            n => format!(
                "{n} version{}, last seen {}",
                if n == 1 { "" } else { "s" },
                when(hit.last_ts)
            ),
        };
        let trash = match hit.trash.as_str() {
            "" => String::new(),
            _ => format!("; in the Trash since {}", when(hit.trashed_at)),
        };
        out.push_str(&format!("{marker} {}\n    {history}{trash}\n", hit.path));
    }
    out.push_str("\n× = deleted; still restorable from an older snapshot.\n");
    out
}

/// `0`/absent-aware epoch conversion.
pub fn from_epoch(seconds: u64) -> Option<SystemTime> {
    (seconds > 0).then(|| UNIX_EPOCH + Duration::from_secs(seconds))
}

/// Format an absolute time as local-ish ISO-8601 without pulling in a date
/// library: seconds since the epoch, rendered by the platform's `date` rules is
/// overkill here, so this stays UTC and unambiguous.
pub fn format_time(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let (y, mo, d, h, mi, s) = civil_from_epoch(secs);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}Z")
}

/// "3 hours ago" / "in 20 minutes" — the form people actually read.
pub fn relative(time: SystemTime, now: SystemTime) -> String {
    let (delta, suffix, prefix) = match now.duration_since(time) {
        Ok(ago) => (ago, " ago", ""),
        Err(e) => (e.duration(), "", "in "),
    };
    let secs = delta.as_secs();
    let (n, unit) = match secs {
        s if s < 60 => (s.max(1), "second"),
        s if s < 3_600 => (s / 60, "minute"),
        s if s < 86_400 => (s / 3_600, "hour"),
        s => (s / 86_400, "day"),
    };
    format!(
        "{prefix}{n} {unit}{}{suffix}",
        if n == 1 { "" } else { "s" }
    )
}

/// "1 snapshot" / "14 snapshots".
fn plural(count: u32, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

/// Human-readable byte counts, worded as the window and the tray word them.
pub use backtrack_core::progress::format_bytes;

/// Days-from-epoch to civil date (Howard Hinnant's algorithm). Avoids a date
/// dependency for the handful of timestamps this tool prints.
fn civil_from_epoch(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (
        y,
        m,
        d,
        (rem / 3_600) as u32,
        ((rem % 3_600) / 60) as u32,
        (rem % 60) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn status() -> Status {
        Status {
            state: "HEALTHY".into(),
            last_backup: 1_700_000_000,
            next_backup: 1_700_003_600,
            destination_reachable: true,
            spool_bytes: 0,
            offline_mode: "spool".into(),
            local_snapshots: 0,
            expirable_snapshots: 0,
            active_job: 0,
            paused_until: 0,
            configured: true,
            reason: String::new(),
            since: 0,
            recovery_job: 0,
        }
    }

    #[test]
    fn the_json_shape_is_the_golden_one() {
        // This is an interface. Changing a field name or type breaks whatever
        // someone has scripted against it, so the shape is pinned here and any
        // change has to be made deliberately, in this test, in the same commit.
        let json = status_json(&status(), None, at(1_700_001_800));
        let expected = serde_json::json!({
            "state": "HEALTHY",
            "reason": null,
            "healthy": true,
            "configured": true,
            "destination_reachable": true,
            "last_backup": 1_700_000_000u64,
            "last_backup_age_seconds": 1_800u64,
            "next_backup": 1_700_003_600u64,
            "paused": false,
            "paused_until": null,
            "active_job": null,
            "spool_bytes": 0u64,
            "offline": false,
            "offline_mode": "spool",
            "local_snapshots": 0u32,
            "expirable_snapshots": 0u32,
            "progress": null,
        });
        assert_eq!(json, expected);
    }

    #[test]
    fn a_backup_under_way_says_how_far_it_has_got() {
        let status = Status {
            active_job: 2,
            ..status()
        };
        let progress =
            Progress::from_reply((2, "archiving".to_string(), 31_496_447_281, 53_100_000_000));
        let json = status_json(&status, progress.as_ref(), at(1_700_001_800));
        assert_eq!(
            json["progress"],
            serde_json::json!({
                "job": 2u64,
                "phase": "archiving",
                "read_bytes": 31_496_447_281u64,
                "total_bytes": 53_100_000_000u64,
                "percent": 59u8,
                "line": "Backing up… 31.5 GB of about 53.1 GB (59%)",
            })
        );
        let text = status_human(&status, progress.as_ref(), at(1_700_001_800));
        assert!(
            text.contains("  Running       job 2 · Backing up… 31.5 GB of about 53.1 GB (59%)\n"),
            "got: {text}"
        );
        // Not measured yet: no total and no percentage, rather than zeros.
        let early = Progress::from_reply((2, "archiving".to_string(), 1_500, 0));
        let json = status_json(&status, early.as_ref(), at(1_700_001_800));
        assert_eq!(json["progress"]["total_bytes"], serde_json::Value::Null);
        assert_eq!(json["progress"]["percent"], serde_json::Value::Null);
        // Between backups the daemon answers job 0: nothing under way.
        assert_eq!(Progress::from_reply((0, String::new(), 0, 0)), None);
    }

    #[test]
    fn never_and_not_applicable_are_null_not_zero() {
        // A script doing `if last_backup < cutoff` would treat 0 as 1970 and
        // conclude the machine is desperately overdue. Null cannot be compared
        // by accident.
        let status = Status {
            last_backup: 0,
            next_backup: 0,
            configured: false,
            ..status()
        };
        let json = status_json(&status, None, at(1_700_001_800));
        assert_eq!(json["last_backup"], serde_json::Value::Null);
        assert_eq!(json["next_backup"], serde_json::Value::Null);
        assert_eq!(json["last_backup_age_seconds"], serde_json::Value::Null);
    }

    #[test]
    fn a_paused_status_says_so_in_json() {
        let status = Status {
            paused_until: 1_700_010_000,
            ..status()
        };
        let json = status_json(&status, None, at(1_700_001_800));
        assert_eq!(json["paused"], serde_json::Value::Bool(true));
        assert_eq!(json["paused_until"], serde_json::json!(1_700_010_000u64));
    }

    #[test]
    fn the_offline_block_reports_what_is_held_on_this_computer() {
        let status = Status {
            destination_reachable: false,
            spool_bytes: 2_500_000_000,
            offline_mode: "spool".into(),
            local_snapshots: 14,
            expirable_snapshots: 3,
            ..status()
        };
        let json = status_json(&status, None, at(1_700_001_800));
        assert_eq!(json["offline"], serde_json::Value::Bool(true));
        assert_eq!(json["offline_mode"], "spool");
        assert_eq!(json["local_snapshots"], serde_json::json!(14u32));
        assert_eq!(json["expirable_snapshots"], serde_json::json!(3u32));

        let text = status_human(&status, None, at(1_700_001_800));
        assert!(text.contains("14 snapshots"), "got: {text}");
        assert!(text.contains("2.5 GB"), "got: {text}");
    }

    #[test]
    fn a_snapshot_count_without_bytes_does_not_print_a_misleading_zero() {
        // Filesystem snapshots share their storage with the live files, so
        // "0 B" beside a count of fourteen would read as a bug.
        let status = Status {
            offline_mode: "fs-snapshot".into(),
            local_snapshots: 14,
            spool_bytes: 0,
            ..status()
        };
        let text = status_human(&status, None, at(1_700_001_800));
        assert!(text.contains("14 snapshots"), "got: {text}");
        assert!(!text.contains("0 B"), "got: {text}");
    }

    #[test]
    fn nothing_held_locally_says_nothing_at_all() {
        let text = status_human(&status(), None, at(1_700_001_800));
        assert!(!text.contains("On this PC"), "got: {text}");
    }

    #[test]
    fn one_snapshot_is_singular() {
        assert_eq!(plural(1, "snapshot"), "1 snapshot");
        assert_eq!(plural(0, "snapshot"), "0 snapshots");
        assert_eq!(plural(14, "snapshot"), "14 snapshots");
    }

    #[test]
    fn the_headline_covers_every_documented_state() {
        for state in [
            "HEALTHY",
            "PROTECTED_LOCALLY",
            "PAUSED",
            "AT_RISK",
            "BROKEN",
            "DEGRADED",
        ] {
            let status = Status {
                state: state.into(),
                ..status()
            };
            assert_ne!(
                headline(&status),
                "Backup status unknown.",
                "{state} has no phrasing of its own"
            );
        }
    }

    #[test]
    fn an_unconfigured_machine_is_told_what_to_do_not_shown_empty_fields() {
        let status = Status {
            configured: false,
            ..status()
        };
        let text = status_human(&status, None, at(1_700_001_800));
        assert!(text.contains("setup wizard"), "got: {text}");
        assert!(
            !text.contains("Last backup"),
            "empty rows help nobody: {text}"
        );
    }

    #[test]
    fn an_unconfigured_machine_is_never_told_its_files_are_backed_up() {
        // A machine with no destination reports HEALTHY, because nothing has
        // failed. Leading with that would tell someone who has not finished
        // setting up that they are protected.
        let status = Status {
            configured: false,
            state: "HEALTHY".into(),
            ..status()
        };
        let text = status_human(&status, None, at(1_700_001_800));
        assert!(
            !text.contains("Your files are backed up"),
            "this is the lie the check exists to prevent:\n{text}"
        );
        assert!(text.contains("isn't set up yet"), "got: {text}");
    }

    #[test]
    fn a_missed_schedule_reads_as_due_now() {
        let status = Status {
            next_backup: 1_700_000_100,
            ..status()
        };
        let text = status_human(&status, None, at(1_700_001_800));
        assert!(text.contains("due now"), "got: {text}");
    }

    #[test]
    fn a_paused_machine_shows_when_it_resumes_rather_than_a_next_run() {
        let status = Status {
            paused_until: 1_700_010_000,
            ..status()
        };
        let text = status_human(&status, None, at(1_700_001_800));
        assert!(text.contains("paused until"), "got: {text}");
    }

    #[test]
    fn relative_times_read_naturally() {
        let now = at(1_700_000_000);
        assert_eq!(relative(at(1_699_999_970), now), "30 seconds ago");
        assert_eq!(relative(at(1_699_999_940), now), "1 minute ago");
        assert_eq!(relative(at(1_699_996_400), now), "1 hour ago");
        assert_eq!(relative(at(1_699_913_600), now), "1 day ago");
        assert_eq!(relative(at(1_700_001_200), now), "in 20 minutes");
    }

    #[test]
    fn times_format_as_utc_iso() {
        assert_eq!(format_time(at(1_700_000_000)), "2023-11-14 22:13:20Z");
        assert_eq!(format_time(at(0)), "1970-01-01 00:00:00Z");
    }

    #[test]
    fn byte_counts_stay_readable() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1_500), "1.5 kB");
        assert_eq!(format_bytes(2_500_000_000), "2.5 GB");
    }

    #[test]
    fn search_marks_deleted_files() {
        let hits = vec![
            SearchResult {
                path: "home/k/kept.txt".into(),
                name: "kept.txt".into(),
                kind: "file".into(),
                first_seq: 1,
                last_seq: 30,
                first_ts: 1_700_000_000,
                last_ts: 1_700_000_000,
                versions: 2,
                size: 4_096,
                gone_from_disk: false,
                trash: String::new(),
                trashed_at: 0,
                trashed_size: 0,
            },
            SearchResult {
                path: "home/k/gone.txt".into(),
                name: "gone.txt".into(),
                kind: "file".into(),
                first_seq: 1,
                last_seq: 4,
                first_ts: 1_700_000_000,
                last_ts: 1_700_000_000,
                versions: 1,
                size: 2_048,
                gone_from_disk: true,
                trash: String::new(),
                trashed_at: 0,
                trashed_size: 0,
            },
        ];
        let text = search_human(&hits);
        assert!(text.contains("× home/k/gone.txt"), "got: {text}");
        assert!(text.contains("  home/k/kept.txt"), "got: {text}");
        assert!(text.contains("still restorable"));
        assert!(!text.contains("Trash"), "got: {text}");
    }

    #[test]
    fn search_says_what_is_in_the_trash_and_since_when() {
        let letter = SearchResult {
            path: "home/k/letter.txt".into(),
            name: "letter.txt".into(),
            kind: "file".into(),
            first_seq: 0,
            last_seq: 0,
            first_ts: 0,
            last_ts: 0,
            versions: 0,
            size: 9,
            gone_from_disk: true,
            trash: "trash:/home/k/.local/share/Trash/info/letter.txt.trashinfo".into(),
            trashed_at: 1_700_000_000,
            trashed_size: 9,
        };
        let text = search_human(std::slice::from_ref(&letter));
        assert!(
            text.contains(&format!(
                "in no backup; in the Trash since {}",
                format_time(from_epoch(1_700_000_000).unwrap())
            )),
            "got: {text}"
        );
        assert_eq!(search_json(&[letter])[0]["trashed_at"], 1_700_000_000);
    }

    #[test]
    fn an_empty_search_says_so_plainly() {
        assert_eq!(search_human(&[]), "Nothing found.\n");
    }

    #[test]
    fn search_json_is_an_array_even_for_one_hit() {
        let json = search_json(&[]);
        assert_eq!(json, serde_json::json!([]));
    }
}
