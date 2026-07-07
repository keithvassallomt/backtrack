// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>

//! The single writer. All mutations to the index go through [`IndexWriter`];
//! the daemon owns exactly one. Readers open the database separately, read-only.
//!
//! Ingest is interval-encoded and streaming: each archive listing is diffed in
//! SQL against the immediately-preceding archive's still-open version rows, in
//! one transaction, holding only the path-resolution cache in memory.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{params, OptionalExtension};
use rusqlite::{Connection, Transaction};

use super::item::{ArchiveMeta, BorgItem, Repo};
use super::{open_connection, open_memory_connection, Result};

/// What an [`IndexWriter::ingest_archive`] call did, for logging and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IngestStats {
    /// The `seq` assigned to the newly-ingested archive.
    pub seq: i64,
    /// Number of listing items processed.
    pub items: usize,
    /// Version rows newly opened (new paths or changed content).
    pub new_versions: usize,
    /// Existing version intervals extended because the item was unchanged.
    pub extended: usize,
}

/// The sole writer onto an index database.
pub struct IndexWriter {
    conn: Connection,
}

/// The previous archive's open version for a path, used for change detection.
struct PrevVersion {
    rowid: i64,
    kind: String,
    size: i64,
    mtime: i64,
    chunk_hash: Option<String>,
}

impl PrevVersion {
    /// Whether `item` is unchanged from this version. Content identity is
    /// kind + size + mtime + chunk hash (the hash is usually absent, so size
    /// and mtime carry the decision — exactly as the architecture specifies).
    fn matches(&self, item: &BorgItem) -> bool {
        self.kind == item.kind.as_str()
            && self.size == item.size
            && self.mtime == item.mtime
            && self.chunk_hash == item.chunk_hash
    }
}

/// Resolves `/`-separated paths to `paths.id`, inserting missing components (and
/// their FTS rows) as it goes. A per-ingest cache keyed by (parent_id, name)
/// means each directory is touched once no matter how many children it has, so
/// resolution cost is bounded by the tree, not the listing length.
struct PathResolver<'a> {
    cache: HashMap<(i64, String), i64>,
    upsert: rusqlite::Statement<'a>,
    lookup: rusqlite::Statement<'a>,
    fts: rusqlite::Statement<'a>,
}

impl<'a> PathResolver<'a> {
    fn new(tx: &'a Transaction<'a>) -> Result<PathResolver<'a>> {
        Ok(PathResolver {
            cache: HashMap::new(),
            upsert: tx.prepare(
                "INSERT INTO paths(parent_id, name) VALUES (?1, ?2)
                 ON CONFLICT(parent_id, name) DO NOTHING",
            )?,
            lookup: tx.prepare("SELECT id FROM paths WHERE parent_id = ?1 AND name = ?2")?,
            fts: tx.prepare("INSERT INTO fts_names(rowid, name) VALUES (?1, ?2)")?,
        })
    }

    /// Resolve a full archive-relative path to its leaf `paths.id`.
    fn resolve(&mut self, path: &str) -> Result<i64> {
        // `parent_id = 0` is the virtual root; top-level components hang off it.
        let mut parent = 0i64;
        for name in path.split('/').filter(|s| !s.is_empty()) {
            parent = self.resolve_component(parent, name)?;
        }
        Ok(parent)
    }

    fn resolve_component(&mut self, parent: i64, name: &str) -> Result<i64> {
        if let Some(&id) = self.cache.get(&(parent, name.to_string())) {
            return Ok(id);
        }
        // Insert if absent; a freshly-created row is a path's first appearance,
        // so index its name for search at the same time.
        let inserted = self.upsert.execute(params![parent, name])? == 1;
        let id = self
            .lookup
            .query_row(params![parent, name], |r| r.get::<_, i64>(0))?;
        if inserted {
            self.fts.execute(params![id, name])?;
        }
        self.cache.insert((parent, name.to_string()), id);
        Ok(id)
    }
}

impl IndexWriter {
    /// Open (creating if absent) the index at `path` for writing.
    pub fn open(path: &Path) -> Result<IndexWriter> {
        Ok(IndexWriter {
            conn: open_connection(path)?,
        })
    }

    /// Open a private in-memory index for writing (tests, ephemeral tooling).
    pub fn open_in_memory() -> Result<IndexWriter> {
        Ok(IndexWriter {
            conn: open_memory_connection()?,
        })
    }

    /// Ingest one archive listing, assigning it the next `seq`. Items are
    /// diffed against the previous archive: unchanged entries extend their
    /// interval, changed/new entries open a fresh version row, and entries that
    /// have vanished simply stop being extended (their interval already ends at
    /// the previous archive).
    ///
    /// Archives must be ingested in chronological order — each call diffs
    /// against the current maximum `seq`. Newest-first backfill (Stage 4) will
    /// need a different entry point.
    pub fn ingest_archive(
        &mut self,
        meta: &ArchiveMeta,
        repo: Repo,
        items: impl Iterator<Item = BorgItem>,
    ) -> Result<IngestStats> {
        let prev_seq: Option<i64> = self
            .conn
            .query_row("SELECT MAX(seq) FROM archives", [], |r| r.get(0))
            .optional()?
            .flatten();
        let seq = prev_seq.unwrap_or(0) + 1;

        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO archives(seq, borg_id, name, ts, repo, status)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
            params![seq, meta.borg_id, meta.name, meta.ts, repo.as_str()],
        )?;

        let mut stats = IngestStats {
            seq,
            items: 0,
            new_versions: 0,
            extended: 0,
        };
        {
            let mut resolver = PathResolver::new(&tx)?;
            let mut prev = tx.prepare(
                "SELECT rowid, kind, size, mtime, chunk_hash
                 FROM versions WHERE path_id = ?1 AND last_seq = ?2",
            )?;
            let mut extend = tx.prepare("UPDATE versions SET last_seq = ?2 WHERE rowid = ?1")?;
            let mut insert = tx.prepare(
                "INSERT INTO versions
                 (path_id, first_seq, last_seq, size, mtime, mode, kind, chunk_hash)
                 VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;

            for item in items {
                stats.items += 1;
                let path_id = resolver.resolve(&item.path)?;

                // The still-open version at the previous archive, if any.
                let existing = match prev_seq {
                    Some(ps) => prev
                        .query_row(params![path_id, ps], |r| {
                            Ok(PrevVersion {
                                rowid: r.get(0)?,
                                kind: r.get(1)?,
                                size: r.get(2)?,
                                mtime: r.get(3)?,
                                chunk_hash: r.get(4)?,
                            })
                        })
                        .optional()?,
                    None => None,
                };

                match existing {
                    Some(p) if p.matches(&item) => {
                        extend.execute(params![p.rowid, seq])?;
                        stats.extended += 1;
                    }
                    _ => {
                        insert.execute(params![
                            path_id,
                            seq,
                            item.size,
                            item.mtime,
                            item.mode,
                            item.kind.as_str(),
                            item.chunk_hash,
                        ])?;
                        stats.new_versions += 1;
                    }
                }
            }
        }
        tx.commit()?;
        Ok(stats)
    }

    /// Borrow the connection (read-side queries and tests build on this).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Kind;
    use std::time::Instant;

    fn file(path: &str, size: i64, mtime: i64) -> BorgItem {
        BorgItem {
            path: path.to_string(),
            kind: Kind::File,
            size,
            mtime,
            mode: 0o644,
            chunk_hash: None,
        }
    }

    fn meta(name: &str) -> ArchiveMeta {
        ArchiveMeta {
            borg_id: None,
            name: name.to_string(),
            ts: 0,
        }
    }

    fn ingest(w: &mut IndexWriter, name: &str, items: Vec<BorgItem>) -> IngestStats {
        w.ingest_archive(&meta(name), Repo::Primary, items.into_iter())
            .unwrap()
    }

    fn scalar(w: &IndexWriter, sql: &str) -> i64 {
        w.conn().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn versions(w: &IndexWriter) -> i64 {
        scalar(w, "SELECT COUNT(*) FROM versions")
    }

    /// All (first_seq, last_seq) spans for a path's leaf name, ordered.
    fn spans(w: &IndexWriter, name: &str) -> Vec<(i64, i64)> {
        let mut stmt = w
            .conn()
            .prepare(
                "SELECT v.first_seq, v.last_seq FROM versions v
                 JOIN paths p ON p.id = v.path_id
                 WHERE p.name = ?1 ORDER BY v.first_seq",
            )
            .unwrap();
        stmt.query_map([name], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    #[test]
    fn first_ingest_opens_one_version_per_item() {
        let mut w = IndexWriter::open_in_memory().unwrap();
        let stats = ingest(&mut w, "a1", vec![file("a", 1, 100), file("b", 2, 100)]);

        assert_eq!(stats.seq, 1);
        assert_eq!(stats.items, 2);
        assert_eq!(stats.new_versions, 2);
        assert_eq!(stats.extended, 0);
        assert_eq!(versions(&w), 2);
        assert_eq!(spans(&w, "a"), vec![(1, 1)]);
        assert_eq!(scalar(&w, "SELECT COUNT(*) FROM archives"), 1);
    }

    #[test]
    fn unchanged_item_extends_its_interval() {
        let mut w = IndexWriter::open_in_memory().unwrap();
        ingest(&mut w, "a1", vec![file("a", 1, 100)]);
        let stats = ingest(&mut w, "a2", vec![file("a", 1, 100)]);

        assert_eq!(stats.extended, 1);
        assert_eq!(stats.new_versions, 0);
        assert_eq!(versions(&w), 1);
        assert_eq!(spans(&w, "a"), vec![(1, 2)]);
    }

    #[test]
    fn changed_item_opens_a_new_version() {
        let mut w = IndexWriter::open_in_memory().unwrap();
        ingest(&mut w, "a1", vec![file("a", 1, 100)]);
        let stats = ingest(&mut w, "a2", vec![file("a", 2, 100)]);

        assert_eq!(stats.new_versions, 1);
        assert_eq!(stats.extended, 0);
        assert_eq!(spans(&w, "a"), vec![(1, 1), (2, 2)]);
    }

    #[test]
    fn deleted_item_interval_stays_closed() {
        let mut w = IndexWriter::open_in_memory().unwrap();
        ingest(&mut w, "a1", vec![file("a", 1, 100), file("g", 1, 100)]);
        ingest(&mut w, "a2", vec![file("a", 1, 100)]);

        assert_eq!(spans(&w, "a"), vec![(1, 2)]);
        assert_eq!(spans(&w, "g"), vec![(1, 1)]); // not extended into a2
    }

    #[test]
    fn reappearing_file_gets_a_fresh_interval() {
        let mut w = IndexWriter::open_in_memory().unwrap();
        ingest(&mut w, "a1", vec![file("a", 1, 100)]);
        ingest(&mut w, "a2", vec![]); // a vanishes
        ingest(&mut w, "a3", vec![file("a", 1, 100)]); // and returns

        assert_eq!(spans(&w, "a"), vec![(1, 1), (3, 3)]);
    }

    #[test]
    fn nested_paths_are_deduplicated() {
        let mut w = IndexWriter::open_in_memory().unwrap();
        ingest(
            &mut w,
            "a1",
            vec![
                file("home/user/a", 1, 100),
                file("home/user/b", 1, 100),
                file("home/other/c", 1, 100),
            ],
        );

        // Distinct components: home, user, a, b, other, c = 6 path rows.
        assert_eq!(scalar(&w, "SELECT COUNT(*) FROM paths"), 6);
        assert_eq!(
            scalar(&w, "SELECT COUNT(*) FROM paths WHERE name='home'"),
            1
        );
    }

    #[test]
    fn fts_has_one_row_per_path_keyed_by_id() {
        let mut w = IndexWriter::open_in_memory().unwrap();
        ingest(&mut w, "a1", vec![file("home/report", 1, 100)]);
        ingest(&mut w, "a2", vec![file("home/report", 2, 100)]); // change: still one path

        assert_eq!(
            scalar(&w, "SELECT COUNT(*) FROM fts_names"),
            scalar(&w, "SELECT COUNT(*) FROM paths")
        );
        // rowid == path_id mapping holds.
        assert_eq!(
            scalar(
                &w,
                "SELECT COUNT(*) FROM fts_names f JOIN paths p ON p.id = f.rowid
                 WHERE f.name <> p.name"
            ),
            0
        );
    }

    #[test]
    fn scripted_churn_has_exact_row_counts() {
        let mut w = IndexWriter::open_in_memory().unwrap();
        ingest(
            &mut w,
            "a1",
            vec![file("a", 1, 100), file("b", 1, 100), file("c", 1, 100)],
        );
        ingest(
            &mut w,
            "a2",
            vec![file("a", 2, 100), file("b", 1, 100), file("c", 1, 100)],
        ); // a changed
        ingest(&mut w, "a3", vec![file("a", 2, 100), file("c", 1, 100)]); // b deleted
        ingest(
            &mut w,
            "a4",
            vec![
                file("a", 2, 100),
                file("b", 9, 100),
                file("c", 1, 100),
                file("d", 1, 100),
            ],
        ); // b returns changed, d new

        assert_eq!(spans(&w, "a"), vec![(1, 1), (2, 4)]);
        assert_eq!(spans(&w, "b"), vec![(1, 2), (4, 4)]);
        assert_eq!(spans(&w, "c"), vec![(1, 4)]);
        assert_eq!(spans(&w, "d"), vec![(4, 4)]);
        assert_eq!(versions(&w), 6);
        assert_eq!(scalar(&w, "SELECT COUNT(*) FROM paths"), 4);
    }

    #[test]
    fn ingests_the_checked_in_small_fixture() {
        // A real `borg list --json-lines` capture (100 files, 5 dirs, 1 symlink)
        // exercises the parse -> ingest path against genuine Borg output.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/small-listing.jsonl");
        let text = std::fs::read_to_string(path).unwrap();
        let items: Vec<BorgItem> = text
            .lines()
            .map(|l| BorgItem::from_json_line(l).unwrap())
            .collect();
        let n = items.len();

        let mut w = IndexWriter::open_in_memory().unwrap();
        let stats = ingest(&mut w, "snapshot-1", items);

        assert_eq!(stats.items, n);
        assert_eq!(stats.new_versions, n); // first archive: everything is new
        assert_eq!(versions(&w), n as i64);
        assert_eq!(
            scalar(&w, "SELECT COUNT(*) FROM versions WHERE kind='file'"),
            100
        );
        assert_eq!(
            scalar(&w, "SELECT COUNT(*) FROM versions WHERE kind='dir'"),
            5
        );
        assert_eq!(
            scalar(&w, "SELECT COUNT(*) FROM versions WHERE kind='symlink'"),
            1
        );
    }

    #[test]
    fn large_ingest_is_fast_enough() {
        const N: usize = 200_000;
        let build = |bump: i64| -> Vec<BorgItem> {
            (0..N)
                .map(|i| {
                    file(
                        &format!("home/user/dir{}/file{i}", i % 500),
                        i as i64 + bump,
                        100,
                    )
                })
                .collect()
        };

        let mut w = IndexWriter::open_in_memory().unwrap();

        // First ingest: every item is new (no diff lookups).
        let start = Instant::now();
        let first = w
            .ingest_archive(&meta("big-1"), Repo::Primary, build(0).into_iter())
            .unwrap();
        let first_elapsed = start.elapsed();
        assert_eq!(first.new_versions, N);
        assert_eq!(versions(&w), N as i64);

        // Second identical ingest: exercises the per-item previous-version diff
        // lookup for all 200k paths — the hot path at real scale — and must
        // extend every interval, adding no rows.
        let start = Instant::now();
        let second = w
            .ingest_archive(&meta("big-2"), Repo::Primary, build(0).into_iter())
            .unwrap();
        let second_elapsed = start.elapsed();
        assert_eq!(second.extended, N);
        assert_eq!(second.new_versions, 0);
        assert_eq!(versions(&w), N as i64);

        assert!(
            first_elapsed.as_secs() < 15 && second_elapsed.as_secs() < 15,
            "200k ingest too slow: first {first_elapsed:?}, diff {second_elapsed:?}, budget 15s each"
        );
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use crate::index::Kind;
    use proptest::prelude::*;

    prop_compose! {
        fn a_file()(
            path in "[a-d](/[a-d]){0,2}",
            size in 0i64..5,
            mtime in 100i64..105,
        ) -> BorgItem {
            BorgItem { path, kind: Kind::File, size, mtime, mode: 0o644, chunk_hash: None }
        }
    }

    proptest! {
        /// Ingesting the same listing as two consecutive archives changes
        /// nothing structurally: one version row per distinct path, each
        /// spanning both archives. No spurious new intervals.
        #[test]
        fn ingesting_the_same_listing_twice_changes_nothing(
            items in prop::collection::vec(a_file(), 0..30)
        ) {
            // Borg listings hold each path once; dedupe by path.
            let mut seen = std::collections::HashSet::new();
            let listing: Vec<BorgItem> =
                items.into_iter().filter(|i| seen.insert(i.path.clone())).collect();
            let distinct = listing.len() as i64;

            let mut w = IndexWriter::open_in_memory().unwrap();
            w.ingest_archive(
                &ArchiveMeta { borg_id: None, name: "a1".into(), ts: 0 },
                Repo::Primary,
                listing.clone().into_iter(),
            ).unwrap();
            let stats = w.ingest_archive(
                &ArchiveMeta { borg_id: None, name: "a2".into(), ts: 0 },
                Repo::Primary,
                listing.into_iter(),
            ).unwrap();

            let rows: i64 = w.conn()
                .query_row("SELECT COUNT(*) FROM versions", [], |r| r.get(0)).unwrap();
            prop_assert_eq!(rows, distinct);
            prop_assert_eq!(stats.new_versions, 0);
            prop_assert_eq!(stats.extended as i64, distinct);
            // Every interval spans exactly a1..=a2.
            let open_at_2: i64 = w.conn()
                .query_row("SELECT COUNT(*) FROM versions WHERE first_seq=1 AND last_seq=2",
                    [], |r| r.get(0)).unwrap();
            prop_assert_eq!(open_at_2, distinct);
        }
    }
}
