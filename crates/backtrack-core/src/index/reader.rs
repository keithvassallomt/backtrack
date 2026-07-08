// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The read side. [`IndexReader`] opens an existing index read-only (the GUI
//! opens one directly; the daemon's writer stays separate) and answers the
//! timeline questions the UI is built on — all as indexed SQL against the
//! interval encoding, instant and offline.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};

use super::item::Kind;
use super::schema::{self, SCHEMA_VERSION};
use super::{IndexError, Result};

/// One row in the file pane: a direct child of the folder being viewed, as it
/// existed at the selected archive, with the two timeline status badges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path_id: i64,
    pub name: String,
    pub kind: Kind,
    pub size: i64,
    pub mtime: i64,
    /// The entry existed at the selected archive but is gone in the latest one
    /// ("deleted after this").
    pub deleted_after: bool,
    /// A newer version of this entry exists after the selected archive.
    pub changed_since: bool,
}

/// One version interval in a file's history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionSpan {
    pub first_seq: i64,
    pub last_seq: i64,
    pub size: i64,
    pub mtime: i64,
    pub kind: Kind,
    pub chunk_hash: Option<String>,
    /// Archive timestamps at the interval's ends (epoch seconds).
    pub first_ts: i64,
    pub last_ts: i64,
}

/// One search result, aggregated across all of a path's versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub path_id: i64,
    /// Full archive-relative path, reconstructed from the tree.
    pub path: String,
    pub name: String,
    /// Kind of the most recent version.
    pub kind: Kind,
    /// Lifespan across every version: earliest first_seq .. latest last_seq.
    pub first_seq: i64,
    pub last_seq: i64,
    pub first_ts: i64,
    pub last_ts: i64,
    /// How many distinct versions the path has had.
    pub version_count: i64,
    /// Whether the path is present in the latest archive.
    pub exists_today: bool,
}

/// A single archive, for the snapshot sidebar. The GUI buckets these by
/// day/week/month; `repo` drives the "on this computer" (spool/fs-snapshot)
/// badge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveSummary {
    pub seq: i64,
    pub borg_id: Option<String>,
    pub name: String,
    pub ts: i64,
    pub repo: String,
}

/// Which way [`IndexReader::next_change`] steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Newer,
    Older,
}

/// One live filesystem entry, as produced by a walker for
/// [`IndexReader::changed_since`]. `path` is archive-relative (matching how Borg
/// stores paths); `mtime` is epoch **microseconds**, truncated to Borg's
/// resolution so it compares equal to an unchanged indexed version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveEntry {
    pub path: PathBuf,
    pub size: i64,
    pub mtime: i64,
}

/// Read-only handle onto an index database.
pub struct IndexReader {
    conn: Connection,
}

/// Turn user input into a safe FTS5 MATCH expression. The core term is wrapped
/// in double quotes (so punctuation and FTS operators are treated literally),
/// and a trailing `*` is preserved as a prefix match. Empty input yields `None`.
fn fts_match_query(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (core, prefix) = match trimmed.strip_suffix('*') {
        Some(c) => (c.trim_end(), true),
        None => (trimmed, false),
    };
    if core.is_empty() {
        return None;
    }
    let escaped = core.replace('"', "\"\"");
    Some(if prefix {
        format!("\"{escaped}\"*")
    } else {
        format!("\"{escaped}\"")
    })
}

impl IndexReader {
    /// Open an existing, already-migrated index for reading. The connection is
    /// put into `query_only` mode so a reader can never mutate the catalogue.
    pub fn open(path: &Path) -> Result<IndexReader> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        schema::integrity_check(&conn)?;
        let version = schema::current_version(&conn)?;
        if version != SCHEMA_VERSION {
            return Err(IndexError::Migration(format!(
                "index is schema v{version}, this build expects v{SCHEMA_VERSION}"
            )));
        }
        conn.pragma_update(None, "query_only", true)?;
        Ok(IndexReader { conn })
    }

    /// List the direct children of `folder` as they existed at archive `seq`,
    /// each carrying its `deleted_after` / `changed_since` badges. An empty or
    /// `/` folder means the tree root.
    pub fn folder_at(&self, folder: &str, seq: i64) -> Result<Vec<Entry>> {
        let Some(folder_id) = self.folder_id(folder)? else {
            return Ok(Vec::new());
        };
        let max = self.max_seq()?.unwrap_or(seq);

        // ?1 folder_id, ?2 selected seq, ?3 latest seq. The two correlated
        // subqueries are indexed lookups on versions(path_id, first_seq, last_seq):
        //   deleted_after — no version is valid in the latest archive;
        //   changed_since — a newer version opens after the selected archive.
        let mut stmt = self.conn.prepare_cached(
            "SELECT p.id, p.name, v.kind, v.size, v.mtime,
                NOT EXISTS(SELECT 1 FROM versions vl
                           WHERE vl.path_id = p.id AND vl.first_seq <= ?3 AND vl.last_seq >= ?3),
                EXISTS(SELECT 1 FROM versions vc
                       WHERE vc.path_id = p.id AND vc.first_seq > ?2)
             FROM versions v JOIN paths p ON p.id = v.path_id
             WHERE p.parent_id = ?1 AND v.first_seq <= ?2 AND v.last_seq >= ?2
             ORDER BY p.name",
        )?;
        let rows = stmt.query_map(params![folder_id, seq, max], |r| {
            Ok(Entry {
                path_id: r.get(0)?,
                name: r.get(1)?,
                kind: Kind::from_token(&r.get::<_, String>(2)?),
                size: r.get(3)?,
                mtime: r.get(4)?,
                deleted_after: r.get(5)?,
                changed_since: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The full version history of a single path, oldest interval first.
    pub fn file_history(&self, path: &str) -> Result<Vec<VersionSpan>> {
        let Some(path_id) = self.resolve_path(path)? else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT v.first_seq, v.last_seq, v.size, v.mtime, v.kind, v.chunk_hash,
                    a1.ts, a2.ts
             FROM versions v
             JOIN archives a1 ON a1.seq = v.first_seq
             JOIN archives a2 ON a2.seq = v.last_seq
             WHERE v.path_id = ?1
             ORDER BY v.first_seq",
        )?;
        let rows = stmt.query_map(params![path_id], |r| {
            Ok(VersionSpan {
                first_seq: r.get(0)?,
                last_seq: r.get(1)?,
                size: r.get(2)?,
                mtime: r.get(3)?,
                kind: Kind::from_token(&r.get::<_, String>(4)?),
                chunk_hash: r.get(5)?,
                first_ts: r.get(6)?,
                last_ts: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Every archive, newest first, for the snapshot sidebar.
    pub fn archives_overview(&self) -> Result<Vec<ArchiveSummary>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT seq, borg_id, name, ts, repo FROM archives ORDER BY seq DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ArchiveSummary {
                seq: r.get(0)?,
                borg_id: r.get(1)?,
                name: r.get(2)?,
                ts: r.get(3)?,
                repo: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The archive `seq` to jump to for the next change to `path` relative to
    /// `from_seq`, in `direction`. `None` if there is no further change.
    ///
    /// When `path` exists at `from_seq`, its interval ends exactly where its
    /// content next changes (or it is deleted), so the boundary is the adjacent
    /// archive past the interval. When it is absent (in a gap), the next change
    /// is its nearest (re)appearance or last disappearance in that direction.
    pub fn next_change(
        &self,
        path: &str,
        from_seq: i64,
        direction: Direction,
    ) -> Result<Option<i64>> {
        let Some(path_id) = self.resolve_path(path)? else {
            return Ok(None);
        };
        let current: Option<(i64, i64)> = self
            .conn
            .query_row(
                "SELECT first_seq, last_seq FROM versions
                 WHERE path_id = ?1 AND first_seq <= ?2 AND last_seq >= ?2",
                params![path_id, from_seq],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;

        let (sql, arg) = match (direction, current) {
            (Direction::Newer, Some((_, last))) => {
                ("SELECT MIN(seq) FROM archives WHERE seq > ?2", last)
            }
            (Direction::Newer, None) => (
                "SELECT MIN(first_seq) FROM versions WHERE path_id = ?1 AND first_seq > ?2",
                from_seq,
            ),
            (Direction::Older, Some((first, _))) => {
                ("SELECT MAX(seq) FROM archives WHERE seq < ?2", first)
            }
            (Direction::Older, None) => (
                "SELECT MAX(last_seq) FROM versions WHERE path_id = ?1 AND last_seq < ?2",
                from_seq,
            ),
        };
        Ok(self
            .conn
            .query_row(sql, params![path_id, arg], |r| r.get::<_, Option<i64>>(0))?)
    }

    /// Files whose current on-disk state differs from archive `seq` — the set
    /// the offline spool must capture. Given a walker's live entries (path +
    /// size + mtime), a path is "changed" when it is not present at `seq` (a new
    /// file) or its size/mtime differ from the version that was. Paths present at
    /// `seq` but absent from the walk (deletions) are not returned: the spool can
    /// only archive files that still exist. Results are sorted for determinism.
    ///
    /// The filesystem walk is intentionally the caller's job, so this is testable
    /// with a synthetic entry list and has no I/O of its own.
    pub fn changed_since(
        &self,
        seq: i64,
        live: impl IntoIterator<Item = LiveEntry>,
    ) -> Result<Vec<PathBuf>> {
        let mut at_seq = self.conn.prepare_cached(
            "SELECT size, mtime FROM versions
             WHERE path_id = ?1 AND first_seq <= ?2 AND last_seq >= ?2",
        )?;
        let mut changed = Vec::new();
        for entry in live {
            let rel = entry.path.to_string_lossy();
            let indexed = match self.resolve_path(&rel)? {
                Some(path_id) => at_seq
                    .query_row(params![path_id, seq], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
                    })
                    .optional()?,
                None => None,
            };
            let unchanged = matches!(indexed, Some((size, mtime)) if size == entry.size && mtime == entry.mtime);
            if !unchanged {
                changed.push(entry.path);
            }
        }
        changed.sort();
        Ok(changed)
    }

    /// Cross-snapshot filename search over FTS5. Each hit aggregates a path's
    /// whole history: lifespan, version count, and whether it still exists.
    /// Results are ranked deleted-first, then by most-recent existence, then by
    /// FTS relevance (bm25) — so a file you deleted surfaces above ones you
    /// still have.
    pub fn search(&self, query: &str) -> Result<Vec<SearchHit>> {
        let Some(match_query) = fts_match_query(query) else {
            return Ok(Vec::new());
        };
        let global_max = self.max_seq()?.unwrap_or(0);

        // Matching paths with their FTS relevance. Each path has exactly one
        // fts_names row, so the match (and its bm25 rank) is per file.
        let mut fts = self.conn.prepare_cached(
            "SELECT rowid, bm25(fts_names) FROM fts_names WHERE fts_names MATCH ?1",
        )?;
        let matches: Vec<(i64, f64)> = fts
            .query_map(params![match_query], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;

        let mut agg = self.conn.prepare_cached(
            "SELECT p.name,
                    MIN(v.first_seq), MAX(v.last_seq), COUNT(*),
                    (SELECT kind FROM versions WHERE path_id = p.id ORDER BY last_seq DESC LIMIT 1)
             FROM paths p JOIN versions v ON v.path_id = p.id
             WHERE p.id = ?1",
        )?;

        let mut hits = Vec::with_capacity(matches.len());
        for (path_id, bm25) in matches {
            // A matched name might have no versions only if the index is
            // inconsistent; skip defensively.
            let row = agg
                .query_row(params![path_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                })
                .optional()?;
            let Some((name, first_seq, last_seq, version_count, kind)) = row else {
                continue;
            };
            hits.push((
                bm25,
                SearchHit {
                    path_id,
                    path: self.full_path(path_id)?,
                    name,
                    kind: Kind::from_token(&kind),
                    first_seq,
                    last_seq,
                    first_ts: self.archive_ts(first_seq)?,
                    last_ts: self.archive_ts(last_seq)?,
                    version_count,
                    exists_today: last_seq == global_max,
                },
            ));
        }

        // Rank: deleted first, then most-recent existence, then FTS relevance
        // (lower bm25 = better match).
        hits.sort_by(|(a_rank, a), (b_rank, b)| {
            a.exists_today
                .cmp(&b.exists_today)
                .then(b.last_seq.cmp(&a.last_seq))
                .then(a_rank.total_cmp(b_rank))
        });
        Ok(hits.into_iter().map(|(_, h)| h).collect())
    }

    /// The creation timestamp (epoch seconds) of the archive with this `seq`.
    fn archive_ts(&self, seq: i64) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT ts FROM archives WHERE seq = ?1",
            params![seq],
            |r| r.get(0),
        )?)
    }

    /// Reconstruct a path's full `/`-separated string by walking to the root.
    fn full_path(&self, mut id: i64) -> Result<String> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT parent_id, name FROM paths WHERE id = ?1")?;
        let mut parts = Vec::new();
        while id != 0 {
            let (parent, name): (i64, String) =
                stmt.query_row(params![id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            parts.push(name);
            id = parent;
        }
        parts.reverse();
        Ok(parts.join("/"))
    }

    /// Resolve a folder path to its `paths.id`, treating an empty or `/` path as
    /// the virtual root (`0`). `None` if the folder does not exist.
    fn folder_id(&self, folder: &str) -> Result<Option<i64>> {
        if folder.trim_matches('/').is_empty() {
            return Ok(Some(0));
        }
        self.resolve_path(folder)
    }

    /// Resolve a `/`-separated path to its leaf `paths.id`, or `None` if any
    /// component is missing.
    fn resolve_path(&self, path: &str) -> Result<Option<i64>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id FROM paths WHERE parent_id = ?1 AND name = ?2")?;
        let mut parent = 0i64;
        for name in path.split('/').filter(|s| !s.is_empty()) {
            match stmt
                .query_row(params![parent, name], |r| r.get::<_, i64>(0))
                .optional()?
            {
                Some(id) => parent = id,
                None => return Ok(None),
            }
        }
        Ok(Some(parent))
    }

    fn max_seq(&self) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row("SELECT MAX(seq) FROM archives", [], |r| {
                r.get::<_, Option<i64>>(0)
            })?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{ArchiveMeta, BorgItem, IndexWriter, Repo};
    use std::time::Instant;

    fn item(path: &str, kind: Kind, size: i64) -> BorgItem {
        BorgItem {
            path: path.to_string(),
            kind,
            size,
            mtime: 100,
            mode: if kind == Kind::Dir { 0o755 } else { 0o644 },
            chunk_hash: None,
        }
    }
    fn dir(path: &str) -> BorgItem {
        item(path, Kind::Dir, 0)
    }
    fn file(path: &str, size: i64) -> BorgItem {
        item(path, Kind::File, size)
    }
    fn meta(name: &str, ts: i64) -> ArchiveMeta {
        ArchiveMeta {
            borg_id: Some(format!("id-{name}")),
            name: name.to_string(),
            ts,
        }
    }

    /// A scripted four-archive history with known answers:
    ///   a1,a2: home/report(v1), home/old/data(v1)
    ///   a3:    home/report(v2); old/ and old/data deleted
    ///   a4:    home/report(v2), home/new/file(v1)
    fn scripted() -> (tempfile::TempDir, IndexReader) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("index.db");
        {
            let mut w = IndexWriter::open(&path).unwrap();
            let base = vec![
                dir("home"),
                dir("home/old"),
                file("home/report", 1),
                file("home/old/data", 1),
            ];
            w.ingest_archive(&meta("a1", 1_000), Repo::Primary, base.clone().into_iter())
                .unwrap();
            w.ingest_archive(&meta("a2", 2_000), Repo::Primary, base.into_iter())
                .unwrap();
            w.ingest_archive(
                &meta("a3", 3_000),
                Repo::Primary,
                vec![dir("home"), file("home/report", 2)].into_iter(),
            )
            .unwrap();
            w.ingest_archive(
                &meta("a4", 4_000),
                Repo::Primary,
                vec![
                    dir("home"),
                    dir("home/new"),
                    file("home/report", 2),
                    file("home/new/file", 1),
                ]
                .into_iter(),
            )
            .unwrap();
        }
        let reader = IndexReader::open(&path).unwrap();
        (tmp, reader)
    }

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }
    fn find<'a>(entries: &'a [Entry], name: &str) -> &'a Entry {
        entries.iter().find(|e| e.name == name).unwrap()
    }

    #[test]
    fn folder_at_root_lists_top_level() {
        let (_t, r) = scripted();
        let root = r.folder_at("", 1).unwrap();
        assert_eq!(names(&root), vec!["home"]);
        assert_eq!(find(&root, "home").kind, Kind::Dir);
    }

    #[test]
    fn folder_at_home_at_first_archive_flags_deletions_and_changes() {
        let (_t, r) = scripted();
        let entries = r.folder_at("home", 1).unwrap();
        assert_eq!(names(&entries), vec!["old", "report"]); // sorted by name

        let report = find(&entries, "report");
        assert_eq!(report.kind, Kind::File);
        assert_eq!(report.size, 1); // v1 at seq 1
        assert!(!report.deleted_after, "report still exists in latest");
        assert!(report.changed_since, "report changed after a1");

        let old = find(&entries, "old");
        assert!(old.deleted_after, "old/ is gone by the latest archive");
        assert!(
            !old.changed_since,
            "old/ has no newer version, only deletion"
        );
    }

    #[test]
    fn folder_at_home_at_latest_has_new_folder_and_no_badges() {
        let (_t, r) = scripted();
        let entries = r.folder_at("home", 4).unwrap();
        assert_eq!(names(&entries), vec!["new", "report"]);
        for e in &entries {
            assert!(!e.deleted_after);
            assert!(!e.changed_since);
        }
    }

    #[test]
    fn folder_at_subfolder_shows_deleted_child() {
        let (_t, r) = scripted();
        let entries = r.folder_at("home/old", 1).unwrap();
        assert_eq!(names(&entries), vec!["data"]);
        assert!(find(&entries, "data").deleted_after);
    }

    #[test]
    fn file_history_returns_all_intervals_with_timestamps() {
        let (_t, r) = scripted();
        let hist = r.file_history("home/report").unwrap();
        assert_eq!(hist.len(), 2);
        assert_eq!((hist[0].first_seq, hist[0].last_seq), (1, 2));
        assert_eq!(hist[0].size, 1);
        assert_eq!((hist[0].first_ts, hist[0].last_ts), (1_000, 2_000));
        assert_eq!((hist[1].first_seq, hist[1].last_seq), (3, 4));
        assert_eq!(hist[1].size, 2);
        assert_eq!((hist[1].first_ts, hist[1].last_ts), (3_000, 4_000));
    }

    #[test]
    fn file_history_of_unknown_path_is_empty() {
        let (_t, r) = scripted();
        assert!(r.file_history("home/nope").unwrap().is_empty());
    }

    #[test]
    fn archives_overview_is_newest_first() {
        let (_t, r) = scripted();
        let all = r.archives_overview().unwrap();
        assert_eq!(
            all.iter().map(|a| a.seq).collect::<Vec<_>>(),
            vec![4, 3, 2, 1]
        );
        assert_eq!(all[0].ts, 4_000);
        assert!(all.iter().all(|a| a.repo == "primary"));
    }

    #[test]
    fn next_change_steps_to_boundaries() {
        let (_t, r) = scripted();
        let nc = |p: &str, from, dir| r.next_change(p, from, dir).unwrap();

        // report: versions [1,2] then [3,4].
        assert_eq!(nc("home/report", 1, Direction::Newer), Some(3));
        assert_eq!(nc("home/report", 2, Direction::Newer), Some(3));
        assert_eq!(nc("home/report", 3, Direction::Newer), None); // last version
        assert_eq!(nc("home/report", 4, Direction::Older), Some(2));
        assert_eq!(nc("home/report", 1, Direction::Older), None); // first version

        // deleted file: home/old/data, versions [1,2].
        assert_eq!(nc("home/old/data", 2, Direction::Newer), Some(3)); // the deletion
        assert_eq!(nc("home/old/data", 3, Direction::Newer), None); // absent, never returns
        assert_eq!(nc("home/old/data", 3, Direction::Older), Some(2)); // last existence
    }

    /// The "Charlie" fixture: 50 archives. `home/keep.txt` and
    /// `home/container.txt` live throughout; `home/contract.pdf` exists only in
    /// archives 10..=40 and is gone since. Archive N has ts = N * 1000.
    fn charlie_open() -> (tempfile::TempDir, IndexReader) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("index.db");
        {
            let mut w = IndexWriter::open(&path).unwrap();
            for seq in 1..=50 {
                let mut listing = vec![
                    dir("home"),
                    file("home/keep.txt", 1),
                    file("home/container.txt", 3),
                ];
                if (10..=40).contains(&seq) {
                    listing.push(file("home/contract.pdf", 5));
                }
                w.ingest_archive(
                    &meta(&format!("a{seq}"), seq * 1000),
                    Repo::Primary,
                    listing.into_iter(),
                )
                .unwrap();
            }
        }
        let reader = IndexReader::open(&path).unwrap();
        (tmp, reader)
    }

    #[test]
    fn search_finds_deleted_file_with_correct_lifespan() {
        let (_t, r) = charlie_open();
        let hits = r.search("contract").unwrap();
        assert_eq!(hits.len(), 1);
        let h = &hits[0];
        assert_eq!(h.path, "home/contract.pdf");
        assert_eq!(h.kind, Kind::File);
        assert!(!h.exists_today, "contract.pdf was deleted after archive 40");
        assert_eq!((h.first_seq, h.last_seq), (10, 40));
        assert_eq!((h.first_ts, h.last_ts), (10_000, 40_000));
        assert_eq!(h.version_count, 1); // unchanged across its life
    }

    #[test]
    fn search_prefix_matches_and_ranks_deleted_first() {
        let (_t, r) = charlie_open();
        // "cont*" prefix-matches both contract.pdf (deleted) and container.txt
        // (still present). Deleted must rank first.
        let hits = r.search("cont*").unwrap();
        let paths: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(paths, vec!["home/contract.pdf", "home/container.txt"]);
        assert!(!hits[0].exists_today);
        assert!(hits[1].exists_today);
    }

    #[test]
    fn search_exact_term_does_not_leak_across_tokens() {
        let (_t, r) = charlie_open();
        // "contract" must not match "container".
        let hits = r.search("contract").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "home/contract.pdf");
    }

    #[test]
    fn search_present_file_flagged_exists_today() {
        let (_t, r) = charlie_open();
        let hits = r.search("keep").unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].exists_today);
        assert_eq!((hits[0].first_seq, hits[0].last_seq), (1, 50));
    }

    #[test]
    fn search_empty_query_returns_nothing() {
        let (_t, r) = charlie_open();
        assert!(r.search("").unwrap().is_empty());
    }

    #[test]
    fn fts_query_quotes_and_escapes() {
        assert_eq!(fts_match_query("contract").as_deref(), Some("\"contract\""));
        assert_eq!(fts_match_query("cont*").as_deref(), Some("\"cont\"*"));
        // Quotes are doubled so a crafted name can't break out of the phrase.
        assert_eq!(fts_match_query(r#"a"b"#).as_deref(), Some("\"a\"\"b\""));
        assert_eq!(fts_match_query("   "), None);
        assert_eq!(fts_match_query("*"), None);
    }

    fn live(path: &str, size: i64, mtime: i64) -> LiveEntry {
        LiveEntry {
            path: PathBuf::from(path),
            size,
            mtime,
        }
    }

    /// One archive (seq 1): home/{a=10, b=20, c=30, e=40}, all mtime 100.
    fn changed_fixture() -> (tempfile::TempDir, IndexReader) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("index.db");
        {
            let mut w = IndexWriter::open(&path).unwrap();
            w.ingest_archive(
                &meta("a1", 1000),
                Repo::Primary,
                vec![
                    dir("home"),
                    file("home/a", 10),
                    file("home/b", 20),
                    file("home/c", 30),
                    file("home/e", 40),
                ]
                .into_iter(),
            )
            .unwrap();
        }
        (tmp, IndexReader::open(&path).unwrap())
    }

    #[test]
    fn changed_since_reports_added_modified_and_touched_only() {
        let (_t, r) = changed_fixture();
        let walk = vec![
            live("home/a", 10, 100), // unchanged
            live("home/b", 99, 100), // modified: size
            live("home/e", 40, 999), // touched: mtime only
            live("home/d", 1, 100),  // added: not in index
                                     // home/c is deleted (absent from the walk)
        ];
        let changed = r.changed_since(1, walk).unwrap();
        assert_eq!(
            changed,
            vec![
                PathBuf::from("home/b"),
                PathBuf::from("home/d"),
                PathBuf::from("home/e"),
            ]
        );
    }

    #[test]
    fn changed_since_empty_walk_is_empty() {
        let (_t, r) = changed_fixture();
        assert!(r.changed_since(1, Vec::new()).unwrap().is_empty());
    }

    #[test]
    fn folder_at_is_fast_on_a_large_index() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("index.db");
        {
            let mut w = IndexWriter::open(&path).unwrap();
            // 200k files spread over 500 dirs -> ~400 children per dir.
            let items: Vec<BorgItem> = (0..200_000)
                .map(|i| file(&format!("home/dir{}/file{i}", i % 500), i as i64))
                .collect();
            w.ingest_archive(&meta("big", 1), Repo::Primary, items.into_iter())
                .unwrap();
        }
        let r = IndexReader::open(&path).unwrap();

        let start = Instant::now();
        let entries = r.folder_at("home/dir0", 1).unwrap();
        let elapsed = start.elapsed();

        assert_eq!(entries.len(), 400);
        assert!(
            elapsed.as_millis() < 10,
            "folder_at took {elapsed:?}, budget 10ms"
        );
    }
}
