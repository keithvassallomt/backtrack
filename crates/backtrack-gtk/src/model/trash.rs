// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The Trash's entries in the timeline (Stage 16).
//!
//! What the daemon lists (`ListTrashed`) becomes places in the timeline beside
//! the backups: one entry for each gap between two backups (and the gap after
//! the newest) in which something that no backup has was moved to the Trash,
//! placed at the latest such deletion. Listed for a folder only when such an
//! item was at or below it, and then timed at the latest deletion there.
//!
//! An entry shows the folder as it was in the backup before its gap, with
//! everything moved to the Trash during the gap put back in its place: with A
//! and B backed up at 12:00 and C made and trashed by 12:45, the folder at
//! 12:45 held A, B and C, and showing C alone would read as A and B deleted.
//! Inside a folder that went to the Trash whole, everything is the Trash's copy.
//!
//! An entry is not a backup, and has no sequence number of its own in the
//! catalogue. It takes a negative one, `-(base + 1)` where `base` is the backup
//! before its gap (0 when there is none), which no backup can have, and which
//! stays the same while the Trash changes, so the entry being viewed stays
//! selected.

use std::collections::BTreeMap;

use backtrack_core::dbus::TrashedItem;
use backtrack_core::index::{ArchiveSummary, Entry, Kind};

use crate::path;

/// The `repo` an entry carries, which is what the sidebar badges it by.
pub const REPO: &str = "trash";

/// The sequence number of the entry for the gap after backup `base`, or
/// before the first backup when `base` is None.
pub fn entry_seq(base: Option<i64>) -> i64 {
    -(base.unwrap_or(0) + 1)
}

/// Whether `seq` is an entry's rather than a backup's.
pub fn is_entry(seq: i64) -> bool {
    seq < 0
}

/// The backup before the gap of entry `seq`: Some(None) for the gap before
/// the first backup, None if `seq` is a backup's.
pub fn base_of(seq: i64) -> Option<Option<i64>> {
    if !is_entry(seq) {
        return None;
    }
    let base = -seq - 1;
    Some((base > 0).then_some(base))
}

/// The backup the gap containing `at` follows: the newest catalogued one
/// taken at or before it. `archives` newest first.
fn base_for(archives: &[ArchiveSummary], at: i64) -> Option<i64> {
    archives
        .iter()
        .find(|a| a.catalogued && a.ts <= at)
        .map(|a| a.seq)
}

/// The entry `item` belongs to.
pub fn entry_of(archives: &[ArchiveSummary], item: &TrashedItem) -> i64 {
    entry_seq(base_for(archives, item.deleted_at))
}

fn within(path: &str, folder: &str) -> bool {
    folder.is_empty() || path::is_within(path, folder)
}

/// The entries listed for `folder`, newest first: one for each gap in which
/// an item at or below `folder` that holds something no backup has was moved
/// to the Trash, timed at the latest such deletion.
pub fn entries_for(
    folder: &str,
    archives: &[ArchiveSummary],
    items: &[TrashedItem],
) -> Vec<ArchiveSummary> {
    let mut latest: BTreeMap<i64, i64> = BTreeMap::new();
    for item in items.iter().filter(|i| i.counts && within(&i.path, folder)) {
        let at = latest.entry(entry_of(archives, item)).or_insert(i64::MIN);
        *at = (*at).max(item.deleted_at);
    }
    let mut entries: Vec<ArchiveSummary> = latest
        .into_iter()
        .map(|(seq, ts)| ArchiveSummary {
            seq,
            borg_id: None,
            name: "the Trash".to_string(),
            ts,
            repo: REPO.to_string(),
            catalogued: true,
            caught_up: None,
            holds_intermediate: false,
        })
        .collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.ts));
    entries
}

/// A row of a folder at an entry: what the backup before the gap held, or
/// something moved to the Trash in the gap, with the archive name its bytes
/// are read through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub entry: Entry,
    /// `trash:<…>` for an item in the Trash; None for one from the backup.
    pub trash: Option<String>,
}

fn trashed_row(name: &str, item: &TrashedItem) -> Row {
    Row {
        entry: Entry {
            path_id: 0,
            name: name.to_string(),
            kind: Kind::from_token(&item.kind),
            size: item.size,
            mtime: item.mtime,
            deleted_after: false,
            changed_since: false,
        },
        trash: Some(item.archive.clone()),
    }
}

/// What `folder` shows at entry `seq`: `base` (its entries in the backup
/// before the gap, empty before the first backup) with every item moved to
/// the Trash in the gap from directly inside `folder` put back in its place,
/// taking the place of the backup's entry of the same name. An item further
/// down brings the folder that leads to it, if the backup lacks it.
pub fn overlay(
    folder: &str,
    seq: i64,
    archives: &[ArchiveSummary],
    items: &[TrashedItem],
    base: Vec<Entry>,
) -> Vec<Row> {
    let in_gap: Vec<&TrashedItem> = items
        .iter()
        .filter(|i| entry_of(archives, i) == seq)
        .collect();
    let mut rows: Vec<Row> = base
        .into_iter()
        .map(|entry| Row { entry, trash: None })
        .collect();
    for item in &in_gap {
        let Some(below) = child_towards(folder, &item.path) else {
            continue;
        };
        if path::join(folder, below) == item.path {
            match rows.iter_mut().find(|r| r.entry.name == below) {
                Some(row) => *row = trashed_row(below, item),
                None => rows.push(trashed_row(below, item)),
            }
        } else if !rows.iter().any(|r| r.entry.name == below) {
            // The folder leading down to it, which the backup does not have.
            rows.push(Row {
                entry: Entry {
                    path_id: 0,
                    name: below.to_string(),
                    kind: Kind::Dir,
                    size: 0,
                    mtime: 0,
                    deleted_after: false,
                    changed_since: false,
                },
                trash: None,
            });
        }
    }
    rows.sort_by(|a, b| a.entry.name.cmp(&b.entry.name));
    rows
}

/// The name of the entry directly inside `folder` on the way to `path`, if
/// `path` is below `folder`.
fn child_towards<'a>(folder: &str, path: &'a str) -> Option<&'a str> {
    let rest = if folder.is_empty() {
        path
    } else {
        path.strip_prefix(folder)?.strip_prefix('/')?
    };
    rest.split('/').next().filter(|name| !name.is_empty())
}

/// The trashed folder `folder` is, or is inside, at entry `seq`, if any:
/// there, everything is the Trash's copy, listed through `ListTrashedFolder`.
pub fn trashed_folder<'a>(
    folder: &str,
    seq: i64,
    archives: &[ArchiveSummary],
    items: &'a [TrashedItem],
) -> Option<&'a TrashedItem> {
    if folder.is_empty() {
        return None;
    }
    items.iter().find(|i| {
        i.kind == "dir" && entry_of(archives, i) == seq && path::is_within(folder, &i.path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backup(seq: i64, ts: i64) -> ArchiveSummary {
        ArchiveSummary {
            seq,
            borg_id: None,
            name: format!("a{seq}"),
            ts,
            repo: "primary".to_string(),
            catalogued: true,
            caught_up: None,
            holds_intermediate: false,
        }
    }

    /// Backups at 12:00 (seq 1) and 13:00 (seq 2), newest first.
    fn noon_and_one() -> Vec<ArchiveSummary> {
        vec![backup(2, 13 * 3600), backup(1, 12 * 3600)]
    }

    fn trashed(path: &str, at: i64, counts: bool) -> TrashedItem {
        TrashedItem {
            archive: format!("trash:/t/info/{}.trashinfo", path::name(path)),
            path: path.to_string(),
            deleted_at: at,
            kind: "file".to_string(),
            size: 7,
            mtime: at * 1_000_000,
            counts,
        }
    }

    fn file(name: &str) -> Entry {
        Entry {
            path_id: 1,
            name: name.to_string(),
            kind: Kind::File,
            size: 1,
            mtime: 1,
            deleted_after: false,
            changed_since: false,
        }
    }

    const QUARTER_TO_ONE: i64 = 12 * 3600 + 45 * 60;

    #[test]
    fn an_entry_number_names_its_gap_and_never_a_backup() {
        assert_eq!(entry_seq(Some(1)), -2);
        assert_eq!(entry_seq(None), -1);
        assert_eq!(base_of(-2), Some(Some(1)));
        assert_eq!(base_of(-1), Some(None));
        assert_eq!(base_of(3), None);
    }

    #[test]
    fn items_fall_into_the_gap_after_the_backup_before_them() {
        let archives = noon_and_one();
        let items = [
            trashed("home/k/Docs/C", QUARTER_TO_ONE, true),
            trashed("home/k/Docs/D", 12 * 3600 + 10 * 60, true),
            trashed("home/k/Docs/E", 13 * 3600 + 30 * 60, true),
            trashed("home/k/Docs/F", 11 * 3600, true),
        ];
        let entries = entries_for("home/k/Docs", &archives, &items);
        let found: Vec<(i64, i64)> = entries.iter().map(|e| (e.seq, e.ts)).collect();
        assert_eq!(
            found,
            [
                (entry_seq(Some(2)), 13 * 3600 + 30 * 60),
                (entry_seq(Some(1)), QUARTER_TO_ONE),
                (entry_seq(None), 11 * 3600),
            ],
            "one entry per gap, newest first, at the latest deletion in it; \
             one before the first backup too"
        );
        assert!(entries.iter().all(|e| e.repo == REPO));
    }

    #[test]
    fn only_items_holding_something_new_make_an_entry() {
        let items = [trashed("home/k/Docs/A", QUARTER_TO_ONE, false)];
        assert!(entries_for("home/k/Docs", &noon_and_one(), &items).is_empty());
    }

    #[test]
    fn an_entry_is_listed_for_a_folder_at_the_latest_deletion_at_or_below_it() {
        let archives = noon_and_one();
        let items = [
            trashed("home/k/Docs/C", 12 * 3600 + 20 * 60, true),
            trashed("home/k/Pictures/P", QUARTER_TO_ONE, true),
        ];
        let docs = entries_for("home/k/Docs", &archives, &items);
        assert_eq!(docs.len(), 1);
        assert_eq!(
            docs[0].ts,
            12 * 3600 + 20 * 60,
            "Docs' own deletion, not Pictures'"
        );
        assert_eq!(
            entries_for("home/k", &archives, &items)[0].ts,
            QUARTER_TO_ONE
        );
        assert_eq!(entries_for("", &archives, &items)[0].ts, QUARTER_TO_ONE);
        assert!(entries_for("home/k/Music", &archives, &items).is_empty());
    }

    #[test]
    fn the_folder_at_an_entry_is_the_backup_before_it_with_the_trash_put_back() {
        // A and B backed up at 12:00; C made and trashed by 12:45; B edited
        // and trashed too, so the Trash's B replaces the backup's.
        let archives = noon_and_one();
        let items = [
            trashed("home/k/Docs/C", QUARTER_TO_ONE, true),
            trashed("home/k/Docs/B", QUARTER_TO_ONE, true),
        ];
        let rows = overlay(
            "home/k/Docs",
            entry_seq(Some(1)),
            &archives,
            &items,
            vec![file("A"), file("B")],
        );
        let shown: Vec<(&str, bool)> = rows
            .iter()
            .map(|r| (r.entry.name.as_str(), r.trash.is_some()))
            .collect();
        assert_eq!(shown, [("A", false), ("B", true), ("C", true)]);
        assert_eq!(rows[2].entry.size, 7, "C as it was trashed");
    }

    #[test]
    fn an_item_trashed_in_another_gap_is_not_put_back_here() {
        let items = [trashed("home/k/Docs/C", 13 * 3600 + 30 * 60, true)];
        let rows = overlay(
            "home/k/Docs",
            entry_seq(Some(1)),
            &noon_and_one(),
            &items,
            vec![file("A")],
        );
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn an_item_further_down_brings_the_folder_leading_to_it() {
        let items = [trashed("home/k/Docs/new/C", QUARTER_TO_ONE, true)];
        let rows = overlay(
            "home/k/Docs",
            entry_seq(Some(1)),
            &noon_and_one(),
            &items,
            vec![file("A")],
        );
        let shown: Vec<(&str, Kind)> = rows
            .iter()
            .map(|r| (r.entry.name.as_str(), r.entry.kind))
            .collect();
        assert_eq!(shown, [("A", Kind::File), ("new", Kind::Dir)]);
    }

    #[test]
    fn before_the_first_backup_the_trashed_items_stand_alone() {
        let items = [trashed("home/k/Docs/C", 11 * 3600, true)];
        let rows = overlay(
            "home/k/Docs",
            entry_seq(None),
            &noon_and_one(),
            &items,
            Vec::new(),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].entry.name, "C");
    }

    #[test]
    fn inside_a_trashed_folder_everything_is_the_trashs_copy() {
        let mut project = trashed("home/k/Docs/project", QUARTER_TO_ONE, true);
        project.kind = "dir".to_string();
        let items = [project.clone()];
        let archives = noon_and_one();
        let seq = entry_seq(Some(1));
        assert_eq!(
            trashed_folder("home/k/Docs/project", seq, &archives, &items),
            Some(&project)
        );
        assert_eq!(
            trashed_folder("home/k/Docs/project/src", seq, &archives, &items),
            Some(&project)
        );
        assert_eq!(trashed_folder("home/k/Docs", seq, &archives, &items), None);
        assert_eq!(
            trashed_folder("home/k/Docs/project", entry_seq(Some(2)), &archives, &items),
            None,
            "only at its own entry"
        );
    }
}
