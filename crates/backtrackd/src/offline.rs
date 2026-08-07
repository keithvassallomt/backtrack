// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Protecting changes while the backup destination is away.
//!
//! When the destination does not answer, everything unchanged since the last
//! successful backup is already safe — it is sitting on the NAS. The only data
//! at risk is what changes during the offline window, which is normally
//! megabytes an hour rather than the whole backup set. So the offline job is
//! not "a backup of everything"; it is "archive what changed, locally, and keep
//! it until the real destination has caught up".
//!
//! Two ways to hold it, chosen by what the filesystem can do — see
//! [`crate::snapshot`] for the btrfs half. This module owns the spool
//! repository, which is the one that works everywhere: a small Borg repository
//! under the data directory, encrypted with the same key as the primary, and
//! size-capped.
//!
//! The rules that shape it:
//!
//! - **Never an error, never a nag.** Being away from the backup drive is a
//!   normal Tuesday. Nothing here logs at error level and nothing here raises a
//!   health banner; the state is `PROTECTED_LOCALLY`, which is the product
//!   working, not failing.
//! - **The same passphrase.** The spool is keyed to the primary repository's
//!   secret, so the user never meets a second one and never has to know this
//!   repository exists.
//! - **A cap that is enforced, never silently.** Local disk is the user's, not
//!   ours. When the cap is reached the oldest local snapshots go first; when a
//!   single hour's delta is bigger than the whole cap, it is taken once anyway —
//!   protecting something beats protecting nothing — and then the spool holds
//!   rather than quietly growing without limit.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One local snapshot, as the cap planner sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalArchive {
    pub seq: i64,
    pub name: String,
}

/// What to do about the cap before writing a new archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapPlan {
    /// Archives to remove first, oldest first.
    pub evict: Vec<LocalArchive>,
    /// Whether to write the new archive at all.
    pub proceed: bool,
    /// Whether the user should be told the local safety net is under strain.
    pub degraded: bool,
}

/// Decide what the cap allows.
///
/// `held` is what the spool currently occupies, `incoming` the projected size of
/// the archive about to be written, and `existing` the local snapshots oldest
/// first. `cap` of zero means no limit.
///
/// `incoming` is the sum of the changed files' sizes, which is an over-estimate:
/// Borg deduplicates and compresses, so the archive will usually cost far less.
/// Over-estimating is the right way to be wrong — it evicts slightly sooner than
/// strictly necessary, rather than sailing past a cap the user set.
///
/// A pure function because the interesting cases are the awkward ones — a cap
/// already blown, a single delta larger than the whole allowance, an empty spool
/// — and none of them should need a full disk to test.
pub fn plan_cap(cap: u64, held: u64, incoming: u64, existing: &[LocalArchive]) -> CapPlan {
    if cap == 0 {
        return CapPlan {
            evict: Vec::new(),
            proceed: true,
            degraded: false,
        };
    }

    // Evict oldest-first until what is held plus what is arriving fits. The
    // eviction saving is approximated by sharing the held bytes evenly across
    // the archives: Borg's deduplication means no archive has a well-defined
    // size of its own, and asking the repository for one costs a full scan.
    // Over the run of a spool — same sources, similar hourly deltas — an even
    // share is a fair estimate, and being wrong only changes how many snapshots
    // are dropped in one go, never whether the cap is respected.
    let per_archive = if existing.is_empty() {
        0
    } else {
        held / existing.len() as u64
    };

    let mut evict = Vec::new();
    let mut remaining = held;
    for archive in existing {
        if remaining.saturating_add(incoming) <= cap {
            break;
        }
        evict.push(archive.clone());
        remaining = remaining.saturating_sub(per_archive);
    }

    let fits = remaining.saturating_add(incoming) <= cap;
    if fits {
        return CapPlan {
            evict,
            proceed: true,
            degraded: false,
        };
    }

    // Nothing left to evict and it still does not fit: this single delta is
    // bigger than the entire allowance.
    //
    // If the spool is empty, take it anyway — once. Somebody who spent an
    // offline afternoon editing a video is exactly the person who needs the
    // local copy, and refusing to protect anything because the allowance is too
    // small would be the wrong answer to give them. If the spool already holds
    // that oversized archive, hold rather than grow without limit, and say so.
    CapPlan {
        proceed: existing.is_empty(),
        evict,
        degraded: true,
    }
}

/// Bytes occupied by a directory tree.
///
/// Used for the spool's own size. Walks rather than asking Borg, because
/// `borg info` on a repository costs a cache sync and this needs to be cheap
/// enough to run before every offline tick. Unreadable entries are skipped: an
/// under-estimate delays an eviction by one tick, which is a far better failure
/// than refusing to protect anything because a directory could not be read.
pub fn directory_bytes(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let Ok(listing) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in listing.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

/// How changes are held while the destination is away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// A capped Borg repository of changed files. Works everywhere.
    Spool,
    /// Read-only btrfs snapshots of the subvolume containing the sources.
    FsSnapshot(crate::snapshot::Subvolume),
}

impl Mode {
    /// The word `GetStatus` reports, and the one the Storage preferences page
    /// will show beside the local usage figure.
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Spool => "spool",
            Mode::FsSnapshot(_) => "fs-snapshot",
        }
    }
}

/// Work out how this machine should protect changes locally.
///
/// Filesystem snapshots where they genuinely work, the spool otherwise. "Where
/// they genuinely work" is settled by taking one and removing it again rather
/// than by inspecting the filesystem type — see [`crate::snapshot`], where the
/// measured answer on a stock desktop is that a user service cannot snapshot
/// the subvolume `$HOME` lives in.
///
/// Called at startup and whenever the sources change. Cheap enough for that: at
/// worst one snapshot of a subvolume, created and immediately removed.
pub async fn detect(sources: &[PathBuf], snapshots_dir: &Path) -> Mode {
    let Some(first) = sources.first() else {
        return Mode::Spool;
    };
    if !crate::snapshot::is_btrfs(first) {
        return Mode::Spool;
    }
    let Some(subvolume) = crate::snapshot::containing_subvolume(first) else {
        tracing::debug!(
            source = %first.display(),
            "on btrfs, but no containing subvolume was found; using the spool"
        );
        return Mode::Spool;
    };
    // Every source must live in the same subvolume, or one snapshot cannot
    // stand for all of them.
    if !sources
        .iter()
        .all(|s| s.starts_with(&subvolume.root) && crate::snapshot::is_btrfs(s))
    {
        tracing::debug!("the backup sources span more than one filesystem; using the spool");
        return Mode::Spool;
    }
    if crate::snapshot::probe(&subvolume.root, snapshots_dir).await {
        tracing::info!(
            subvolume = %subvolume.root.display(),
            "filesystem snapshots are available for local protection"
        );
        return Mode::FsSnapshot(subvolume);
    }
    Mode::Spool
}

/// How long a locally-held snapshot is kept after the destination has caught
/// up.
///
/// Not arbitrary: what these still hold once a real backup has succeeded is the
/// *intermediate* versions from the offline window — the 10:00, 11:00 and 12:00
/// edits of a file that changed several times while away. A month is long
/// enough that somebody who realises on their return that they want the
/// mid-afternoon version can still have it, and short enough that a laptop does
/// not carry an offline week around forever.
pub const EXPIRE_AFTER_CATCH_UP: Duration = Duration::from_secs(30 * 24 * 3_600);

/// Whether a locally-held snapshot may be discarded at `now`.
///
/// Two rules, and which applies depends on what kind of snapshot it is:
///
/// - **Nothing expires until the destination has caught up.** An unmarked
///   archive is the only copy of the versions it holds, and discarding it would
///   be losing data to save disk.
/// - Once marked, a **spool** archive lives for [`EXPIRE_AFTER_CATCH_UP`],
///   while a **filesystem snapshot** also has its own daily retention and goes
///   at whichever comes first. Snapshots are near-free but they pin extents, so
///   yesterday's hourlies stop earning their keep quickly.
pub fn expired(row: &backtrack_core::index::LocalArchiveRow, now: SystemTime) -> bool {
    let Some(marked) = row.expirable_at else {
        return false;
    };
    let marked = UNIX_EPOCH + Duration::from_secs(marked.max(0) as u64);
    let due = marked + EXPIRE_AFTER_CATCH_UP;
    let due = if row.repo == backtrack_core::index::Repo::FsSnapshot.as_str() {
        let created = UNIX_EPOCH + Duration::from_secs(row.ts.max(0) as u64);
        due.min(crate::snapshot::expires_at(created, None))
    } else {
        due
    };
    due <= now
}

/// The archive name for a local snapshot taken at `now`.
///
/// `bt-local-` rather than the primary's `bt-{hostname}-`, so a glance at a
/// repository — or at a log line — says which kind of snapshot this is. The
/// timestamp format is shared with the primary naming, so both sort
/// lexicographically in chronological order.
pub fn local_archive_name(now: SystemTime) -> String {
    format!("bt-local-{}", crate::pipeline::iso8601_basic_at(now))
}

/// A local snapshot name that is not already taken, and the instant it names.
///
/// The name has one-second resolution, which reads well in a timeline and is
/// unique for any realistic cadence — offline ticks are an hour apart. It is
/// not unique for two snapshots taken inside the same second, and Borg does not
/// forgive that: `borg create` refuses with "Archive … already exists" and the
/// run fails outright.
///
/// That is reachable in three ways worth defending against: pressing "Back Up
/// Now" twice while away from the destination, the development interval
/// override set to a second or two, and a retry after a failure that was quick.
/// Rather than making the name ugly for everyone, a colliding snapshot is dated
/// to the next free second — off by under a second from when it was taken,
/// against a run that would otherwise not happen at all.
pub fn next_local_name(now: SystemTime, taken: &[String]) -> (String, SystemTime) {
    crate::pipeline::next_free_name(now, taken, local_archive_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    fn archives(n: usize) -> Vec<LocalArchive> {
        (1..=n)
            .map(|i| LocalArchive {
                seq: i as i64,
                name: format!("bt-local-{i}"),
            })
            .collect()
    }

    #[test]
    fn a_spool_well_inside_its_cap_evicts_nothing() {
        let plan = plan_cap(10 * GB, 2 * GB, 100 * 1024 * 1024, &archives(4));
        assert!(plan.evict.is_empty());
        assert!(plan.proceed);
        assert!(!plan.degraded);
    }

    #[test]
    fn the_oldest_local_snapshots_go_first() {
        // 10 archives sharing 9 GB, so about 900 MB each; a 2 GB delta against a
        // 10 GB cap needs roughly two of them gone.
        let plan = plan_cap(10 * GB, 9 * GB, 2 * GB, &archives(10));
        assert!(plan.proceed);
        assert!(!plan.evict.is_empty());
        assert_eq!(
            plan.evict.first().map(|a| a.seq),
            Some(1),
            "eviction starts at the oldest"
        );
        let seqs: Vec<i64> = plan.evict.iter().map(|a| a.seq).collect();
        let mut sorted = seqs.clone();
        sorted.sort_unstable();
        assert_eq!(seqs, sorted, "and works forwards in age order");
    }

    #[test]
    fn a_single_delta_larger_than_the_cap_is_taken_once_anyway() {
        // An offline afternoon spent editing video. Refusing to protect any of
        // it because the allowance is too small is the wrong answer; taking it
        // and admitting the spool is over its cap is the right one.
        let plan = plan_cap(GB, 0, 5 * GB, &[]);
        assert!(plan.proceed, "something protected beats nothing protected");
        assert!(plan.degraded, "and the user is told");
    }

    #[test]
    fn once_that_oversized_archive_is_held_the_spool_stops_growing() {
        // The other half: having taken it once, the next tick must not keep
        // piling on. Everything evictable is dropped, and it still does not fit.
        let plan = plan_cap(GB, 5 * GB, 5 * GB, &archives(1));
        assert_eq!(plan.evict.len(), 1, "it drops what it can");
        assert!(!plan.proceed, "and then holds rather than growing forever");
        assert!(plan.degraded);
    }

    #[test]
    fn a_cap_of_zero_means_no_limit() {
        let plan = plan_cap(0, 500 * GB, 500 * GB, &archives(3));
        assert!(plan.proceed);
        assert!(plan.evict.is_empty());
        assert!(!plan.degraded);
    }

    #[test]
    fn an_empty_spool_taking_a_normal_delta_just_proceeds() {
        let plan = plan_cap(10 * GB, 0, 50 * 1024 * 1024, &[]);
        assert!(plan.proceed);
        assert!(plan.evict.is_empty());
        assert!(!plan.degraded);
    }

    #[test]
    fn eviction_never_asks_for_more_archives_than_exist() {
        let existing = archives(3);
        let plan = plan_cap(GB, 100 * GB, GB, &existing);
        assert!(plan.evict.len() <= existing.len());
    }

    #[test]
    fn directory_bytes_adds_up_a_tree() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::write(dir.path().join("top"), vec![0u8; 100]).unwrap();
        std::fs::write(dir.path().join("a/mid"), vec![0u8; 200]).unwrap();
        std::fs::write(dir.path().join("a/b/deep"), vec![0u8; 300]).unwrap();
        assert_eq!(directory_bytes(dir.path()), 600);
    }

    #[test]
    fn a_spool_that_does_not_exist_yet_occupies_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(directory_bytes(&dir.path().join("not-there")), 0);
    }

    #[test]
    fn a_name_that_is_already_taken_moves_to_the_next_free_second() {
        // Borg refuses a duplicate archive name outright ("Archive … already
        // exists", exit 30), which fails the whole run. Two local snapshots
        // inside one second are reachable by pressing Back Up Now twice while
        // away, or by the development interval override.
        use std::time::UNIX_EPOCH;
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let first = local_archive_name(now);
        let (name, at) = next_local_name(now, std::slice::from_ref(&first));
        assert_ne!(name, first);
        assert_eq!(at, now + Duration::from_secs(1));
        assert_eq!(name, local_archive_name(now + Duration::from_secs(1)));
    }

    #[test]
    fn a_free_name_is_used_as_is() {
        use std::time::UNIX_EPOCH;
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let (name, at) = next_local_name(now, &["bt-local-somethingelse".to_string()]);
        assert_eq!(name, local_archive_name(now));
        assert_eq!(at, now);
    }

    #[test]
    fn several_collisions_in_a_row_still_terminate() {
        use std::time::UNIX_EPOCH;
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let taken: Vec<String> = (0..5)
            .map(|i| local_archive_name(now + Duration::from_secs(i)))
            .collect();
        let (name, at) = next_local_name(now, &taken);
        assert_eq!(at, now + Duration::from_secs(5));
        assert!(!taken.contains(&name));
    }

    #[test]
    fn local_snapshots_are_named_so_they_sort_chronologically() {
        use std::time::{Duration, UNIX_EPOCH};
        let earlier = local_archive_name(UNIX_EPOCH + Duration::from_secs(1_000));
        let later = local_archive_name(UNIX_EPOCH + Duration::from_secs(2_000));
        assert!(earlier < later, "{earlier} must sort before {later}");
        assert!(earlier.starts_with("bt-local-"));
        assert!(
            !earlier.contains(':'),
            "a colon is the one character borg reads specially in repo::archive"
        );
    }
}
