// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! `xtask` — Backtrack's development tooling.
//!
//! Its one job today is `demo-repo`: build a real Borg repository with a scripted
//! 30-snapshot history over a fake home (files appearing, changing, and being
//! deleted on known dates) and ingest it into a fresh index under the dev data
//! directory. This is the dataset every GUI stage develops and screenshots
//! against.
//!
//! Run it with `just demo-repo` (which sets `BACKTRACK_DEV=1`, so it writes to
//! `~/.local/share/backtrack-dev/`).

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use backtrack_core::index::{ArchiveMeta, BorgItem, IndexWriter, Repo, ITEM_FORMAT};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// What a demo build produced, for reporting and testing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Summary {
    archives: usize,
    versions: i64,
    /// The acceptance signal: `old-client-folder` (deleted after snapshot 15) is
    /// flagged deleted-after when viewed at snapshot 10.
    old_client_deleted_at_10: bool,
}

fn main() {
    // Printed rather than returned: a `Result` from `main` is rendered with
    // `Debug`, which turns a multi-line explanation into one line of escapes.
    if let Err(error) = build() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn build() -> Result<()> {
    let dir = data_dir();
    refuse_if_the_daemon_owns_the_index()?;
    println!("Building demo repo + index under {}", dir.display());
    let summary = run(&dir)?;
    println!(
        "Done: {} snapshots, {} version rows.",
        summary.archives, summary.versions
    );
    println!(
        "  old-client-folder shows a 'deleted after this' badge at snapshot 10: {}",
        if summary.old_client_deleted_at_10 {
            "yes"
        } else {
            "NO — acceptance check failed"
        }
    );
    if !summary.old_client_deleted_at_10 {
        return Err("demo verification failed: old-client-folder not flagged deleted".into());
    }
    println!();
    println!("Browse it with");
    println!(
        "  just run-app --path '{}'",
        dir.join("demo-src/home").display()
    );
    Ok(())
}

/// Stop, with an explanation, if a daemon is running.
///
/// The daemon owns the index — bus-name ownership *is* its single-instance
/// lock — and this recipe deletes that index and builds a new one underneath
/// it. A daemon that is up will go on writing to the file it still has open and
/// will reconcile the rebuilt repository into it, and the result is a fixture
/// with every snapshot in it twice. That is confusing enough to debug once, and
/// the check that avoids it is one question to the bus.
fn refuse_if_the_daemon_owns_the_index() -> Result<()> {
    let name = backtrack_core::dbus::bus_name();
    // `NameHasOwner` asks; calling a method on the name would *start* one.
    let out = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.DBus",
            "--object-path",
            "/org/freedesktop/DBus",
            "--method",
            "org.freedesktop.DBus.NameHasOwner",
            name,
        ])
        .output();
    let Ok(out) = out else {
        // No gdbus, or no session bus: nothing to collide with, or nothing that
        // can be asked. Either way, not a reason to refuse to build a fixture.
        return Ok(());
    };
    if !String::from_utf8_lossy(&out.stdout).contains("true") {
        return Ok(());
    }
    Err(format!(
        "a Backtrack daemon is running on {name} and holds the index this would rebuild.\n\
         Stop it first, then re-run:\n  \
         systemctl --user stop backtrackd.service\n  \
         pkill backtrackd"
    )
    .into())
}

/// Build (from scratch) a real Borg repo and index at `dir`.
fn run(dir: &Path) -> Result<Summary> {
    let repo = dir.join("demo-repo");
    let src = dir.join("demo-src");
    let index_path = dir.join("index.db");

    // Start fresh so the recipe is idempotent.
    fs::create_dir_all(dir)?;
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&src);
    for suffix in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(dir.join(format!("index.db{suffix}")));
    }
    let home = src.join("home");
    fs::create_dir_all(&home)?;

    borg(&["init", "-e", "none", &repo.to_string_lossy()])?;

    // Borg's clock has to be measured before anything is dated by it.
    let skew = calibrate(&repo, &src)?;
    if skew != 0 {
        println!("  borg's --timestamp is {skew}s away from the time it reports back");
    }

    // Seed the history: mutate the fake home incrementally (so unchanged files
    // keep their mtime and Borg dedups), one snapshot per entry, on the
    // schedule below.
    let script = history();
    let when = schedule(now(), script.len());
    let mut prev = BTreeMap::new();
    for (i, day_files) in script.iter().enumerate() {
        let day = i + 1;
        apply_day(&home, &prev, day_files, when[i])?;
        let date = utc_stamp(when[i] - skew);
        borg_create(&src, &repo, &format!("snapshot-{day:02}"), &date)?;
        prev = day_files.clone();
        print!("\r  seeded snapshot {day}/{}", script.len());
    }
    println!();

    // Ingest every archive in chronological order. With no removals, the total
    // version-row count is just the sum of new versions opened.
    let mut writer = IndexWriter::open(&index_path)?;
    let archives = list_archives(&repo)?;
    let mut versions: i64 = 0;
    for (name, id, ts) in &archives {
        let items = list_items(&repo, name)?;
        let stats = writer.ingest_archive(
            &ArchiveMeta {
                borg_id: Some(id.clone()),
                name: name.clone(),
                ts: *ts,
            },
            Repo::Primary,
            items.into_iter(),
        )?;
        versions += stats.new_versions as i64;
    }
    drop(writer);

    let old_client_deleted_at_10 = verify_old_client_deleted(&index_path, &member_path(&home))?;
    Ok(Summary {
        archives: archives.len(),
        versions,
        old_client_deleted_at_10,
    })
}

/// When each snapshot was taken, oldest first.
///
/// Dated relative to `now` rather than to a fixed month, and deliberately so:
/// the sidebar's bands — Today, Yesterday, This week, Last week — are relative
/// too, and a fixture pinned to June 2026 stopped exercising any of them the
/// moment June 2026 was over, collapsing the whole history into one closed
/// month group.
///
/// The shape is one snapshot a day going back about a month, then three a
/// couple of hours apart today, which is close enough to the real retention
/// (hourly recently, daily for a while, sparser after that) for the grouping to
/// have something to absorb.
///
/// Run within two hours of local midnight, today's three land on either side of
/// it and the first of them reads as yesterday. That is a real shape too, and
/// not worth a timezone database to avoid.
fn schedule(now: i64, count: usize) -> Vec<i64> {
    const DAY: i64 = 86_400;
    let today = [now - 2 * 3_600, now - 3_600, now - 600];
    let dailies = count.saturating_sub(today.len());

    // Walk back a day at a time, stepping over the gap, until enough days have
    // been collected.
    let mut offsets = Vec::with_capacity(dailies);
    let mut offset = 1i64;
    while offsets.len() < dailies {
        if !GAP_DAYS.contains(&offset) {
            offsets.push(offset);
        }
        offset += 1;
    }
    offsets.reverse();

    offsets
        .into_iter()
        .map(|days| now - days * DAY)
        .chain(today.into_iter().take(count))
        .collect()
}

/// Days the machine was off, counted back from today.
///
/// A fixture with a backup on every single day cannot demonstrate the two
/// places a hole in the history has to show: the calendar draws those days
/// plain and refuses to jump to them, and the density strip draws the gap
/// rather than closing it up. Neither behaviour has anything to stand on
/// without a hole to draw.
const GAP_DAYS: [i64; 2] = [9, 10];

/// Now, in seconds since the epoch.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// How far Borg's `--timestamp` is from the time Borg then reports for it.
///
/// The flag is documented (borg 1.4) as taking UTC, and the archive time that
/// comes back out is the machine's UTC offset away from what was asked for.
/// Rather than encode a guess about which end is wrong — and have that guess
/// rot when Borg changes — the generator measures it: one throwaway archive at
/// a known instant, read back, difference applied to every date in the script.
/// If a future Borg makes the two agree, the measurement becomes zero and
/// nothing else here changes.
fn calibrate(repo: &Path, src: &Path) -> Result<i64> {
    /// 2020-09-13T12:26:40Z — an arbitrary instant, far from any boundary that
    /// could round.
    const PROBE: i64 = 1_600_000_000;
    const NAME: &str = "bt-calibration";

    borg_create(src, repo, NAME, &utc_stamp(PROBE))?;
    let recorded = list_archives(repo)?
        .into_iter()
        .find(|(name, _, _)| name == NAME)
        .map(|(_, _, ts)| ts)
        .ok_or("the calibration archive did not come back from borg list")?;
    borg(&["delete", &format!("{}::{NAME}", repo.to_string_lossy())])?;
    Ok(recorded - PROBE)
}

/// The path a Borg archive stores `path` under: absolute, without its leading `/`.
///
/// This is the spelling every index query and every restore uses, so the
/// generator computes it rather than writing it out — the fixture lives
/// wherever the dev data directory is, which is not the same place on two
/// machines.
fn member_path(path: &Path) -> String {
    path.to_string_lossy().trim_start_matches('/').to_string()
}

/// `epoch` as the naive UTC string Borg's `--timestamp` wants.
fn utc_stamp(epoch: i64) -> String {
    let (year, month, day) = civil_from_days(epoch.div_euclid(86_400));
    let seconds = epoch.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        seconds / 3_600,
        (seconds % 3_600) / 60,
        seconds % 60
    )
}

/// Gregorian date from a day number (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// The scripted 30-day history: for each day (1-based), the file tree relative to
/// `home/`, mapping path to content. Files appear, change, and disappear on known
/// dates; names echo the mockups.
fn history() -> Vec<BTreeMap<String, String>> {
    (1..=30)
        .map(|day| {
            let mut files = BTreeMap::new();

            // report.odt — three distinct versions (distinct lengths, so the
            // change is visible by size regardless of mtime).
            let report = if day < 8 {
                "report draft"
            } else if day < 20 {
                "report reviewed copy"
            } else {
                "report final signed version!!"
            };
            files.insert("Documents/report.odt".to_string(), report.to_string());

            // notes.txt — one change, late.
            let notes = if day < 25 {
                "meeting notes"
            } else {
                "meeting notes plus action items"
            };
            files.insert("Documents/notes.txt".to_string(), notes.to_string());

            // invoice appears mid-month and stays.
            if day >= 5 {
                files.insert(
                    "Documents/invoice-may.pdf".to_string(),
                    "invoice may 2026 total due".to_string(),
                );
            }
            // a photo shows up on day 10.
            if day >= 10 {
                files.insert(
                    "Pictures/vacation.jpg".to_string(),
                    "JPEG-BINARY".to_string(),
                );
            }
            // old-client-folder exists days 1–15, then is deleted.
            if day <= 15 {
                files.insert(
                    "old-client-folder/contract.pdf".to_string(),
                    "signed contract".to_string(),
                );
                files.insert(
                    "old-client-folder/proposal.odt".to_string(),
                    "project proposal".to_string(),
                );
            }
            files
        })
        .collect()
}

/// Apply the difference between `prev` and `cur` to the on-disk `home` tree:
/// write added/changed files, delete removed ones, then prune emptied
/// directories so a deleted folder actually disappears from the next snapshot.
///
/// Everything written is dated to `when`, the instant the day's snapshot is
/// taken at, rather than left at the wall clock. The generator runs in about
/// twelve seconds, so without this every file and folder in a history claiming
/// to span a month carries a modification time inside the same minute, and the
/// Modified column reads "today" beside a backup from five weeks ago. Files are
/// dated a little before the backup that captures them, because that is the
/// order those two things happen in.
///
/// Only what actually changed is dated, which is the point: a file nobody
/// touched keeps the modification time it had, so Borg dedups it and the index
/// extends its interval instead of opening a new version. Directories follow
/// the same rule by the same reasoning as the filesystem's — a directory's
/// modification time moves when an entry is added to it or removed from it,
/// and not when a file inside it is rewritten.
fn apply_day(
    home: &Path,
    prev: &BTreeMap<String, String>,
    cur: &BTreeMap<String, String>,
    when: i64,
) -> Result<()> {
    /// How long before the backup the day's edits were made.
    const EDITED_BEFORE_BACKUP: i64 = 40 * 60;

    for rel in prev.keys() {
        if !cur.contains_key(rel) {
            let _ = fs::remove_file(home.join(rel));
        }
    }
    let mut written = Vec::new();
    for (rel, content) in cur {
        if prev.get(rel) != Some(content) {
            let path = home.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, content)?;
            written.push(path);
        }
    }
    prune_empty_dirs(home)?;

    // Dated after the writing and the pruning, both of which move a
    // directory's modification time as a side effect.
    for path in written {
        set_mtime(&path, when - EDITED_BEFORE_BACKUP)?;
    }
    let before = entries_by_dir(prev);
    let after = entries_by_dir(cur);
    for (dir, names) in &after {
        if before.get(dir) != Some(names) {
            set_mtime(&home.join(dir), when - EDITED_BEFORE_BACKUP)?;
        }
    }
    Ok(())
}

/// The names directly inside each directory of a day's tree, keyed by the
/// directory's path relative to the fixture root (the empty string being the
/// root itself).
///
/// This is what decides whether a directory's modification time moved: it did
/// if the set of names inside it is not the one it had yesterday.
fn entries_by_dir(files: &BTreeMap<String, String>) -> BTreeMap<String, BTreeSet<String>> {
    let mut dirs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    dirs.entry(String::new()).or_default();
    for rel in files.keys() {
        let parts: Vec<&str> = rel.split('/').collect();
        for depth in 0..parts.len() {
            dirs.entry(parts[..depth].join("/"))
                .or_default()
                .insert(parts[depth].to_string());
        }
    }
    dirs
}

/// Set `path`'s modification time, for a file or a directory alike.
fn set_mtime(path: &Path, epoch: i64) -> Result<()> {
    let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(epoch.max(0) as u64);
    // Read-only is enough on Linux, and is the only thing a directory allows.
    let handle = fs::File::options().read(true).open(path)?;
    handle.set_times(fs::FileTimes::new().set_modified(when))?;
    Ok(())
}

/// Remove empty subdirectories, bottom-up (never removing `dir` itself).
fn prune_empty_dirs(dir: &Path) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            prune_empty_dirs(&path)?;
            if fs::read_dir(&path)?.next().is_none() {
                fs::remove_dir(&path)?;
            }
        }
    }
    Ok(())
}

/// Open the index and check the acceptance signal.
fn verify_old_client_deleted(index_path: &Path, home: &str) -> Result<bool> {
    use backtrack_core::index::IndexReader;
    let reader = IndexReader::open(index_path)?;
    let at_10 = reader.folder_at(home, 10)?;
    Ok(at_10
        .iter()
        .any(|e| e.name == "old-client-folder" && e.deleted_after))
}

// ── Borg helpers ────────────────────────────────────────────────────────────

fn borg(args: &[&str]) -> Result<()> {
    let status = Command::new("borg")
        .args(args)
        .env("BORG_UNKNOWN_UNENCRYPTED_REPO_ACCESS_IS_OK", "yes")
        .status()?;
    if !status.success() {
        return Err(format!("borg {args:?} exited with {status}").into());
    }
    Ok(())
}

/// Archive `src` under the path it actually occupies.
///
/// Borg stores a member by the path it was given, with the leading `/`
/// removed, so *how* the source is named decides what a restore will later
/// aim at. Naming it relatively — `home`, from inside the fixture directory —
/// produces archives whose members are `home/Documents/…`, which correspond to
/// nothing on this machine: browsable, but restorable only to `/home`, and
/// disjoint from the archives the daemon makes of the same files. Naming it
/// absolutely puts the fixture's history and the daemon's own backups on the
/// same paths, which is what makes the fixture restorable and the timeline one
/// history instead of two.
fn borg_create(src: &Path, repo: &Path, name: &str, date: &str) -> Result<()> {
    let target = format!("{}::{name}", repo.to_string_lossy());
    let status = Command::new("borg")
        .args(["create", "--timestamp", date, &target])
        .arg(src)
        .env("BORG_UNKNOWN_UNENCRYPTED_REPO_ACCESS_IS_OK", "yes")
        .status()?;
    if !status.success() {
        return Err(format!("borg create {name} exited with {status}").into());
    }
    Ok(())
}

/// (name, borg id, ts-epoch-seconds) for every archive, chronological order.
///
/// `{time:%s}` for the same reason [`list_items`] avoids the JSON: the `time`
/// field in `borg list --json` is local, naive and unlabelled, so reading it
/// puts every archive in the catalogue the machine's UTC offset away from when
/// it was actually taken. The name goes last and the fields are separated by
/// tabs, because an archive name is text somebody chose.
fn list_archives(repo: &Path) -> Result<Vec<(String, String, i64)>> {
    let out = borg_output(&[
        "list",
        "--format",
        "{id}\t{time:%s}\t{barchive}{NL}",
        &repo.to_string_lossy(),
    ])?;
    let text = String::from_utf8(out)?;
    let mut archives = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let mut fields = line.splitn(3, '\t');
        let id = fields.next().ok_or("archive without id")?.to_string();
        let ts: i64 = fields
            .next()
            .ok_or("archive without time")?
            .trim()
            .parse()
            .map_err(|_| format!("archive time is not an epoch: {line:?}"))?;
        let name = fields
            .next()
            .ok_or("archive without name")?
            .trim_end_matches(['\r', '\n'])
            .to_string();
        archives.push((name, id, ts));
    }
    Ok(archives)
}

/// Read an archive's contents the way the daemon reads one.
///
/// `--format ITEM_FORMAT`, not `--json-lines`: Borg's JSON renders `mtime` as a
/// naive local-time string with no offset on it, so a catalogue built from it
/// holds every modification time shifted by the machine's UTC offset. The
/// engine has read listings this way since Stage 2; the fixture did not, which
/// is why the demo index dated a file an hour later than the file itself and
/// the Modified column disagreed with the restore dialog beside it.
fn list_items(repo: &Path, name: &str) -> Result<Vec<BorgItem>> {
    let target = format!("{}::{name}", repo.to_string_lossy());
    let out = borg_output(&["list", "--format", ITEM_FORMAT, &target])?;
    let text = String::from_utf8(out)?;
    let mut items = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        items.push(BorgItem::from_format_line(line).map_err(|e| e.to_string())?);
    }
    Ok(items)
}

fn borg_output(args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("borg")
        .args(args)
        .env("BORG_UNKNOWN_UNENCRYPTED_REPO_ACCESS_IS_OK", "yes")
        .output()?;
    if !out.status.success() {
        return Err(format!(
            "borg {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(out.stdout)
}

// ── Data-directory resolution (mirrors backtrack_core::logging) ──────────────

fn data_dir() -> PathBuf {
    let dev = std::env::var_os("BACKTRACK_DEV").is_some();
    let leaf = if dev { "backtrack-dev" } else { "backtrack" };
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(leaf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use backtrack_core::index::{IndexReader, Kind};
    use std::collections::BTreeSet;

    /// Convert a day's file tree into `BorgItem`s, including directory entries for
    /// every ancestor (as Borg's listing would). Content-derived mtime means an
    /// unchanged file compares equal across snapshots, so the borg-free build
    /// produces the same interval structure as the real one.
    fn to_borg_items(files: &BTreeMap<String, String>) -> Vec<BorgItem> {
        let mut dirs: BTreeSet<String> = BTreeSet::new();
        dirs.insert("home".to_string());
        let mut items = Vec::new();
        for (rel, content) in files {
            let full = format!("home/{rel}");
            let parts: Vec<&str> = full.split('/').collect();
            for depth in 1..parts.len() {
                dirs.insert(parts[..depth].join("/"));
            }
            let checksum: i64 = content.bytes().map(|b| b as i64).sum();
            items.push(BorgItem {
                path: full,
                kind: Kind::File,
                size: content.len() as i64,
                mtime: checksum,
                mode: 0o644,
                chunk_hash: None,
            });
        }
        for dir in dirs {
            items.push(BorgItem {
                path: dir,
                kind: Kind::Dir,
                size: 0,
                mtime: 0,
                mode: 0o755,
                chunk_hash: None,
            });
        }
        items
    }

    fn index_from_history(writer: &mut IndexWriter) {
        for (i, files) in history().iter().enumerate() {
            let day = i + 1;
            writer
                .ingest_archive(
                    &ArchiveMeta {
                        borg_id: None,
                        name: format!("snapshot-{day:02}"),
                        ts: day as i64 * 86_400,
                    },
                    Repo::Primary,
                    to_borg_items(files).into_iter(),
                )
                .unwrap();
        }
    }

    #[test]
    fn the_schedule_ends_today_and_reaches_back_about_a_month() {
        // Midday, so "today" is unambiguous.
        let now = 1_781_092_800;
        let when = schedule(now, 30);
        assert_eq!(when.len(), 30);
        assert!(
            when.windows(2).all(|w| w[0] < w[1]),
            "oldest first, strictly"
        );

        // Calendar days apart, not raw seconds: a snapshot ten minutes ago is
        // the same day, and a subtraction would floor it to yesterday.
        let day = |ts: i64| ts.div_euclid(86_400) - now.div_euclid(86_400);
        assert_eq!(day(when[29]), 0, "the newest is today");
        assert_eq!(day(when[28]), 0);
        assert_eq!(day(when[27]), 0, "three of them are");
        assert_eq!(day(when[26]), -1, "then yesterday");
        assert!(
            (-32..=-25).contains(&day(when[0])),
            "and back about a month, {} days",
            day(when[0])
        );
        assert!(when[29] < now, "nothing is dated in the future");
    }

    #[test]
    fn the_schedule_fills_every_band_the_sidebar_has() {
        // The point of dating the fixture relative to now: Today, Yesterday,
        // This week, Last week and at least one month group all have something
        // in them, whichever day of the week it is run on.
        for weekday_offset in 0..7 {
            let now = 1_781_092_800 + weekday_offset * 86_400;
            let when = schedule(now, 30);
            let days_ago: Vec<i64> = when
                .iter()
                .map(|ts| now.div_euclid(86_400) - ts.div_euclid(86_400))
                .collect();
            assert!(days_ago.contains(&0), "today");
            assert!(days_ago.contains(&1), "yesterday");
            assert!(days_ago.iter().any(|d| (2..=7).contains(d)), "the week");
            assert!(days_ago.iter().any(|d| *d > 14), "and something old");
        }
    }

    #[test]
    fn the_machine_was_off_for_a_couple_of_days() {
        // The hole the calendar and the density strip are meant to show.
        let now = 1_781_092_800;
        let when = schedule(now, 30);
        let days_ago: Vec<i64> = when
            .iter()
            .map(|ts| now.div_euclid(86_400) - ts.div_euclid(86_400))
            .collect();
        for gap in GAP_DAYS {
            assert!(
                !days_ago.contains(&gap),
                "{gap} days ago should have no backup, got {days_ago:?}"
            );
        }
        // And the days either side of it do, so it reads as a hole rather than
        // as the end of the history.
        assert!(days_ago.contains(&(GAP_DAYS[0] - 1)));
        assert!(days_ago.contains(&(GAP_DAYS[1] + 1)));
    }

    /// The modification time `path` carries, in whole seconds.
    fn mtime_of(path: &Path) -> i64 {
        fs::metadata(path)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    #[test]
    fn a_day_dates_what_it_changed_and_leaves_the_rest_alone() {
        const DAY_ONE: i64 = 1_600_000_000;
        const DAY_TWO: i64 = DAY_ONE + 86_400;

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        let day_one: BTreeMap<String, String> = [
            ("Documents/report.odt", "draft"),
            ("Documents/notes.txt", "notes"),
            ("old/contract.pdf", "signed"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        apply_day(home, &BTreeMap::new(), &day_one, DAY_ONE).unwrap();

        // Everything a day writes is dated to that day, not to the wall clock.
        assert!(mtime_of(&home.join("Documents/report.odt")) < DAY_ONE);
        assert_eq!(
            mtime_of(&home.join("Documents/report.odt")),
            mtime_of(&home.join("Documents")),
        );

        // Day two rewrites one file, deletes a folder's only file, and touches
        // nothing else.
        let mut day_two = day_one.clone();
        day_two.insert("Documents/report.odt".to_string(), "reviewed".to_string());
        day_two.remove("old/contract.pdf");
        apply_day(home, &day_one, &day_two, DAY_TWO).unwrap();

        let rewritten = mtime_of(&home.join("Documents/report.odt"));
        assert!(rewritten > DAY_ONE, "the rewritten file moved to day two");

        // A file nobody touched keeps its time, which is what lets Borg dedup
        // it and the index extend its interval rather than open a version.
        assert!(mtime_of(&home.join("Documents/notes.txt")) < DAY_ONE);

        // Rewriting a file does not move its directory; losing an entry does.
        assert!(
            mtime_of(&home.join("Documents")) < DAY_ONE,
            "a directory whose entries are unchanged keeps its time",
        );
        assert!(!home.join("old").exists(), "the emptied folder is gone");
    }

    #[test]
    fn a_directorys_entries_are_the_names_directly_inside_it() {
        let files: BTreeMap<String, String> = [("a/b/c.txt", "x"), ("a/d.txt", "y")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let dirs = entries_by_dir(&files);
        assert_eq!(dirs[""], BTreeSet::from(["a".to_string()]));
        assert_eq!(
            dirs["a"],
            BTreeSet::from(["b".to_string(), "d.txt".to_string()]),
        );
        assert_eq!(dirs["a/b"], BTreeSet::from(["c.txt".to_string()]));
    }

    #[test]
    fn a_timestamp_is_rendered_the_way_borg_wants_it() {
        assert_eq!(utc_stamp(0), "1970-01-01T00:00:00");
        assert_eq!(utc_stamp(1_600_000_000), "2020-09-13T12:26:40");
        // A leap day, which the calendar arithmetic has to get right.
        assert_eq!(utc_stamp(1_709_208_000), "2024-02-29T12:00:00");
    }

    #[test]
    fn history_spans_30_days_with_known_appearances_and_deletions() {
        let h = history();
        assert_eq!(h.len(), 30);
        // invoice appears on day 5 (index 4).
        assert!(!h[3].contains_key("Documents/invoice-may.pdf"));
        assert!(h[4].contains_key("Documents/invoice-may.pdf"));
        // old-client-folder present through day 15, gone from day 16.
        assert!(h[14].contains_key("old-client-folder/contract.pdf"));
        assert!(!h[15].contains_key("old-client-folder/contract.pdf"));
        // report.odt takes three distinct contents over the month.
        let reports: BTreeSet<_> = h
            .iter()
            .map(|d| d["Documents/report.odt"].clone())
            .collect();
        assert_eq!(reports.len(), 3);
    }

    #[test]
    fn scripted_history_produces_the_expected_index() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("index.db");
        {
            let mut w = IndexWriter::open(&path).unwrap();
            index_from_history(&mut w);
        }
        let r = IndexReader::open(&path).unwrap();

        assert_eq!(r.archives_overview().unwrap().len(), 30);

        // The acceptance signal: at snapshot 10 the folder is present but flagged
        // deleted-after; by snapshot 20 it is gone entirely.
        let at_10 = r.folder_at("home", 10).unwrap();
        let ocf = at_10
            .iter()
            .find(|e| e.name == "old-client-folder")
            .expect("old-client-folder present at snapshot 10");
        assert!(ocf.deleted_after);
        let at_20 = r.folder_at("home", 20).unwrap();
        assert!(!at_20.iter().any(|e| e.name == "old-client-folder"));

        // report.odt accumulated exactly three versions.
        assert_eq!(
            r.file_history("home/Documents/report.odt").unwrap().len(),
            3
        );
    }

    #[cfg(feature = "integration")]
    #[test]
    fn full_demo_roundtrip_via_real_borg() {
        let tmp = tempfile::tempdir().unwrap();
        let summary = run(tmp.path()).expect("demo build");
        assert_eq!(summary.archives, 30);
        assert!(summary.old_client_deleted_at_10);
        assert!(summary.versions > 0);
    }
}
