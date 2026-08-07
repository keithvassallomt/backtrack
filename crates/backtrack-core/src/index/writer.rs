// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

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
use super::{open_connection, open_memory_connection, IndexError, Result};

/// The source of a listing could not produce the rest of it.
///
/// Deliberately carries no detail: the caller keeps its own error, and the index
/// needs to know only that what it received is not the whole archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingIncomplete;

/// What an [`IndexWriter::ingest_archive`] call did, for logging and tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IngestStats {
    /// The `seq` assigned to the newly-ingested archive.
    pub seq: i64,
    /// Number of listing items processed.
    pub items: usize,
    /// Version rows newly opened (new paths or changed content).
    pub new_versions: usize,
    /// Existing version intervals extended because the item was unchanged.
    pub extended: usize,
    /// Intervals on both sides of this archive rejoined into one, because the
    /// content matched across it. Only ever non-zero when catalogues are filled
    /// in out of order (backfill, reconciliation).
    pub merged: usize,
}

/// The status stored in `archives.status` for an archive whose row exists but
/// whose file list has not been read yet. `NULL` means catalogued.
pub const STATUS_PENDING: &str = "pending";

/// What [`IndexWriter::sync_archives`] changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Seqs of archives newly recorded as `pending`, newest first — the order
    /// they should be catalogued in, since the newest snapshot is the one the
    /// user wants to browse.
    pub pending: Vec<i64>,
    /// How many archives the repository no longer has, and the index dropped.
    pub removed: usize,
}

impl SyncReport {
    /// Whether the index already matched the repository.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty() && self.removed == 0
    }
}

/// The sole writer onto an index database.
pub struct IndexWriter {
    conn: Connection,
}

/// A version interval abutting the archive being ingested, on either side.
struct Neighbour {
    rowid: i64,
    kind: String,
    size: i64,
    mtime: i64,
    chunk_hash: Option<String>,
    /// Where the interval ends, needed when two intervals are rejoined across
    /// the archive being ingested.
    last_seq: i64,
}

impl Neighbour {
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

/// A locally-held snapshot, as the offline paths need it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalArchiveRow {
    pub seq: i64,
    pub name: String,
    /// When the snapshot was taken, epoch seconds.
    pub ts: i64,
    /// Which local store holds it — `spool` or `fs-snapshot`.
    pub repo: String,
    /// When it may be discarded; `None` while it is still the only copy of the
    /// versions it holds.
    pub expirable_at: Option<i64>,
}

/// One `archives` row, as [`IndexWriter::sync_archives`] needs it.
struct ArchiveRow {
    seq: i64,
    name: String,
    ts: i64,
    repo: String,
}

/// Split every version interval that spans `seq` into the parts either side of
/// it, so nothing claims to know what an uncatalogued archive contained.
///
/// The rows to split are materialised into a temporary table first: inserting
/// into `versions` while selecting from it would let the statement see its own
/// output, and the halves it produces are themselves candidates for the next
/// insertion's split.
fn split_versions_around(tx: &Transaction, seq: i64) -> Result<()> {
    tx.execute(
        "CREATE TEMP TABLE spanning AS
           SELECT rowid AS rid, path_id, last_seq, size, mtime, mode, kind, chunk_hash
           FROM versions WHERE first_seq < ?1 AND last_seq > ?1",
        params![seq],
    )?;
    tx.execute(
        "INSERT INTO versions
           (path_id, first_seq, last_seq, size, mtime, mode, kind, chunk_hash)
         SELECT path_id, ?1, last_seq, size, mtime, mode, kind, chunk_hash FROM spanning",
        params![seq + 1],
    )?;
    tx.execute(
        "UPDATE versions SET last_seq = ?1 WHERE rowid IN (SELECT rid FROM spanning)",
        params![seq - 1],
    )?;
    tx.execute_batch("DROP TABLE spanning")?;
    Ok(())
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

    /// Append an archive to the end of the catalogue and ingest its listing.
    ///
    /// The straightforward path, and the only one a normal hourly backup takes:
    /// the archive that was just created is newer than everything already
    /// indexed, so it goes on the end. Out-of-order catalogues — reconciliation
    /// after a crash, first-run backfill — go through [`Self::sync_archives`]
    /// and [`Self::ingest_pending`] instead.
    pub fn ingest_archive(
        &mut self,
        meta: &ArchiveMeta,
        repo: Repo,
        items: impl Iterator<Item = BorgItem>,
    ) -> Result<IngestStats> {
        let seq = self.append_archive(meta, repo)?;
        self.ingest_pending(seq, items)
    }

    /// Record an archive at the end of the catalogue, not yet catalogued.
    /// Returns its `seq`.
    pub fn append_archive(&mut self, meta: &ArchiveMeta, repo: Repo) -> Result<i64> {
        let max: Option<i64> = self
            .conn
            .query_row("SELECT MAX(seq) FROM archives", [], |r| r.get(0))
            .optional()?
            .flatten();
        let seq = max.unwrap_or(0) + 1;
        self.conn.execute(
            "INSERT INTO archives(seq, borg_id, name, ts, repo, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                seq,
                meta.borg_id,
                meta.name,
                meta.ts,
                repo.as_str(),
                STATUS_PENDING
            ],
        )?;
        Ok(seq)
    }

    /// Ingest the file listing for the archive at `seq`, wherever it sits in the
    /// catalogue, and mark it browsable.
    ///
    /// Each item is compared against its neighbours on *both* sides — the
    /// version interval ending at `seq - 1` and the one starting at `seq + 1`:
    ///
    /// - matches both → the two intervals are one interval that was interrupted
    ///   by an archive nobody had read yet; they are rejoined.
    /// - matches the left only → that interval extends forward over `seq`.
    /// - matches the right only → that interval extends backward over `seq`.
    /// - matches neither → a new version row covering `seq` alone.
    ///
    /// The symmetry is what makes catalogue order irrelevant. Ingesting oldest
    /// to newest only ever finds a left neighbour, which is the original Stage 1
    /// behaviour; ingesting newest to oldest, as backfill does, only ever finds a
    /// right one; filling a hole left by a crash finds both. All three produce
    /// exactly the catalogue a from-scratch chronological ingest would.
    ///
    /// Relies on the invariant that no version interval *spans* an uncatalogued
    /// archive — [`Self::sync_archives`] splits any that would, so an interval
    /// never claims a file was unchanged across an archive nobody has read.
    pub fn ingest_pending(
        &mut self,
        seq: i64,
        items: impl Iterator<Item = BorgItem>,
    ) -> Result<IngestStats> {
        self.ingest_pending_fallible(seq, items.map(Ok))
    }

    /// [`Self::ingest_pending`], for a listing arriving from something that can
    /// fail part way through — which, when it is a pipe from a Borg subprocess,
    /// is every real ingest.
    ///
    /// A single [`ListingIncomplete`] rolls the whole transaction back and
    /// leaves the archive `pending`. Committing what arrived would be much
    /// worse than it sounds: the archive would be marked browsable while holding
    /// half its files, so the timeline would show a snapshot with the user's
    /// documents missing and no indication anything was wrong. Re-reading the
    /// listing later is cheap; a catalogue that quietly lies is not recoverable.
    pub fn ingest_pending_fallible(
        &mut self,
        seq: i64,
        items: impl Iterator<Item = std::result::Result<BorgItem, ListingIncomplete>>,
    ) -> Result<IngestStats> {
        self.ingest_inner(seq, items, false)
    }

    /// Ingest a **delta** listing: an archive that holds only what changed since
    /// the previous snapshot, which is what the offline spool writes.
    ///
    /// A full listing says "this is everything that existed". A delta says "this
    /// is what moved", and the catalogue has to read the rest — every file that
    /// did *not* change — as still being there. Ingesting a delta as though it
    /// were a full listing would be a disaster in the timeline: every unchanged
    /// file would appear deleted at the spool snapshot and restored at the next
    /// one, so somebody browsing an hour spent on a train would see their
    /// documents missing.
    ///
    /// So the listed items are folded in exactly as usual, and then every
    /// interval that ended at `seq - 1` **and was not mentioned** is extended
    /// across `seq`. The result is the state of the filesystem at that moment:
    /// what it was, plus what changed.
    ///
    /// **Deletions are not captured.** The delta can only contain files that
    /// still exist, so a file deleted during an offline window keeps its
    /// interval extended and goes on appearing in local snapshots until the
    /// next backup to the real destination records its absence. That is the
    /// honest trade named in offline-strategy.md — the spool exists to protect
    /// changed data, not to be a complete record — and erring this way shows a
    /// file that is gone rather than hiding one that is there.
    ///
    /// Assumes `seq` is the newest archive in the catalogue, which it always is:
    /// spool snapshots are appended as they are taken. There is therefore no
    /// right-hand neighbour to rejoin.
    pub fn ingest_delta(
        &mut self,
        seq: i64,
        items: impl Iterator<Item = std::result::Result<BorgItem, ListingIncomplete>>,
    ) -> Result<IngestStats> {
        self.ingest_inner(seq, items, true)
    }

    fn ingest_inner(
        &mut self,
        seq: i64,
        items: impl Iterator<Item = std::result::Result<BorgItem, ListingIncomplete>>,
        delta: bool,
    ) -> Result<IngestStats> {
        let tx = self.conn.transaction()?;
        if delta {
            tx.execute_batch("CREATE TEMP TABLE mentioned(path_id INTEGER PRIMARY KEY)")?;
        }
        let mut stats = IngestStats {
            seq,
            ..IngestStats::default()
        };
        {
            let mut resolver = PathResolver::new(&tx)?;
            let mut neighbour = tx.prepare(
                "SELECT rowid, kind, size, mtime, chunk_hash, last_seq
                 FROM versions WHERE path_id = ?1 AND last_seq = ?2",
            )?;
            let mut successor = tx.prepare(
                "SELECT rowid, kind, size, mtime, chunk_hash, last_seq
                 FROM versions WHERE path_id = ?1 AND first_seq = ?2",
            )?;
            let mut set_first =
                tx.prepare("UPDATE versions SET first_seq = ?2 WHERE rowid = ?1")?;
            let mut set_last = tx.prepare("UPDATE versions SET last_seq = ?2 WHERE rowid = ?1")?;
            let mut delete = tx.prepare("DELETE FROM versions WHERE rowid = ?1")?;
            let mut insert = tx.prepare(
                "INSERT INTO versions
                 (path_id, first_seq, last_seq, size, mtime, mode, kind, chunk_hash)
                 VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            // Prepared only for a delta, so an ordinary ingest — the hot path,
            // 200,000 items at a time — does not pay an extra insert per file.
            let mut mention = delta
                .then(|| tx.prepare("INSERT OR IGNORE INTO temp.mentioned(path_id) VALUES (?1)"))
                .transpose()?;

            let read = |r: &rusqlite::Row| -> rusqlite::Result<Neighbour> {
                Ok(Neighbour {
                    rowid: r.get(0)?,
                    kind: r.get(1)?,
                    size: r.get(2)?,
                    mtime: r.get(3)?,
                    chunk_hash: r.get(4)?,
                    last_seq: r.get(5)?,
                })
            };

            for item in items {
                // Dropping `tx` here rolls the transaction back, so a listing
                // cut short leaves the catalogue exactly as it was.
                let item = item.map_err(|_| IndexError::ListingIncomplete { seq })?;
                stats.items += 1;
                let path_id = resolver.resolve(&item.path)?;
                if let Some(mention) = mention.as_mut() {
                    mention.execute(params![path_id])?;
                }

                let left = neighbour
                    .query_row(params![path_id, seq - 1], read)
                    .optional()?
                    .filter(|n| n.matches(&item));
                let right = successor
                    .query_row(params![path_id, seq + 1], read)
                    .optional()?
                    .filter(|n| n.matches(&item));

                match (left, right) {
                    (Some(left), Some(right)) => {
                        set_last.execute(params![left.rowid, right.last_seq])?;
                        delete.execute([right.rowid])?;
                        stats.merged += 1;
                        stats.extended += 1;
                    }
                    (Some(left), None) => {
                        set_last.execute(params![left.rowid, seq])?;
                        stats.extended += 1;
                    }
                    (None, Some(right)) => {
                        set_first.execute(params![right.rowid, seq])?;
                        stats.extended += 1;
                    }
                    (None, None) => {
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
        if delta {
            // Everything the delta did not mention was, by construction,
            // unchanged — so its current version reaches across this snapshot.
            // Restricted to intervals ending exactly at `seq - 1`: an interval
            // that already ended earlier belongs to a file that was gone before
            // this window opened, and must stay gone.
            stats.extended += tx.execute(
                "UPDATE versions SET last_seq = ?1
                 WHERE last_seq = ?1 - 1
                   AND path_id NOT IN (SELECT path_id FROM temp.mentioned)",
                params![seq],
            )?;
            tx.execute_batch("DROP TABLE temp.mentioned")?;
        }
        tx.execute(
            "UPDATE archives SET status = NULL WHERE seq = ?1",
            params![seq],
        )?;
        tx.commit()?;
        Ok(stats)
    }

    /// Make the catalogue's archive list match the repository's.
    ///
    /// Archives the repository no longer has are removed; archives it has that
    /// the catalogue does not are inserted in chronological position and marked
    /// `pending`, to be catalogued by [`Self::ingest_pending`].
    ///
    /// Inserting into the middle of the catalogue means existing version
    /// intervals may span the new archive, claiming a file was unchanged across
    /// a snapshot nobody has read. Those intervals are split around it, which
    /// makes the file simply absent from that (pending) archive's view until it
    /// is catalogued, and lets the ingest rejoin them if the content did match.
    /// Guessing would be worse: an interval that spans an unread archive is the
    /// catalogue asserting something it does not know.
    ///
    /// `entries` is the repository's archive list, in any order. Only rows
    /// belonging to `repo` are reconciled, so the offline spool (Stage 5) can be
    /// synchronised separately without disturbing the primary catalogue.
    pub fn sync_archives(&mut self, repo: Repo, entries: &[ArchiveMeta]) -> Result<SyncReport> {
        let mut report = SyncReport::default();

        // Removals first, through the existing path: it renumbers densely and
        // repairs the intervals, so the insert step below sees a tidy catalogue.
        let existing = self.archive_rows()?;
        let wanted: std::collections::HashSet<&str> =
            entries.iter().map(|e| e.name.as_str()).collect();
        let gone: Vec<i64> = existing
            .iter()
            .filter(|row| row.repo == repo.as_str() && !wanted.contains(row.name.as_str()))
            .map(|row| row.seq)
            .collect();
        report.removed = gone.len();
        self.remove_archives(&gone)?;

        let existing = self.archive_rows()?;
        let known: std::collections::HashSet<&str> = existing
            .iter()
            .filter(|row| row.repo == repo.as_str())
            .map(|row| row.name.as_str())
            .collect();
        let missing: Vec<&ArchiveMeta> = entries
            .iter()
            .filter(|e| !known.contains(e.name.as_str()))
            .collect();
        if missing.is_empty() {
            return Ok(report);
        }

        // The catalogue's order, with the newcomers slotted in by timestamp.
        // Name breaks ties so two archives taken in the same second land in a
        // stable order rather than one that changes between runs.
        enum Slot<'a> {
            Existing(&'a ArchiveRow),
            New(&'a ArchiveMeta),
        }
        let mut order: Vec<(i64, &str, Slot)> = existing
            .iter()
            .map(|row| (row.ts, row.name.as_str(), Slot::Existing(row)))
            .chain(
                missing
                    .iter()
                    .map(|e| (e.ts, e.name.as_str(), Slot::New(e))),
            )
            .collect();
        order.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));

        let mut remap: Vec<(i64, i64)> = Vec::new();
        let mut inserted: Vec<(i64, &ArchiveMeta)> = Vec::new();
        for (index, (_, _, slot)) in order.iter().enumerate() {
            let new_seq = index as i64 + 1;
            match slot {
                Slot::Existing(row) => remap.push((row.seq, new_seq)),
                Slot::New(meta) => inserted.push((new_seq, meta)),
            }
        }

        let tx = self.conn.transaction()?;
        {
            // Shift the existing catalogue up to make room. Archive rows are
            // rewritten wholesale rather than updated in place: new seqs are
            // greater than old ones here, so an in-place UPDATE would collide
            // with a row it has not moved yet.
            tx.execute_batch(
                "CREATE TEMP TABLE seqmap(old_seq INTEGER PRIMARY KEY, new_seq INTEGER)",
            )?;
            {
                let mut ins = tx.prepare("INSERT INTO seqmap(old_seq, new_seq) VALUES (?1, ?2)")?;
                for (old, new) in &remap {
                    ins.execute(params![old, new])?;
                }
            }
            tx.execute(
                "UPDATE versions SET
                    first_seq = (SELECT new_seq FROM seqmap WHERE old_seq = versions.first_seq),
                    last_seq  = (SELECT new_seq FROM seqmap WHERE old_seq = versions.last_seq)",
                [],
            )?;
            tx.execute_batch(
                "CREATE TEMP TABLE restacked AS
                   SELECT (SELECT new_seq FROM seqmap WHERE old_seq = a.seq) AS seq,
                          borg_id, name, ts, repo, status
                   FROM archives a;
                 DELETE FROM archives;
                 INSERT INTO archives(seq, borg_id, name, ts, repo, status)
                   SELECT seq, borg_id, name, ts, repo, status FROM restacked;
                 DROP TABLE restacked;
                 DROP TABLE seqmap;",
            )?;

            let mut add = tx.prepare(
                "INSERT INTO archives(seq, borg_id, name, ts, repo, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for (seq, meta) in &inserted {
                add.execute(params![
                    seq,
                    meta.borg_id,
                    meta.name,
                    meta.ts,
                    repo.as_str(),
                    STATUS_PENDING
                ])?;
                split_versions_around(&tx, *seq)?;
            }
        }
        tx.commit()?;

        // Newest first: the snapshot somebody wants to browse is the last one
        // taken, so that is the one worth catalogued first.
        report.pending = inserted.iter().map(|(seq, _)| *seq).rev().collect();
        Ok(report)
    }

    /// Archives whose row exists but whose file list has never been read,
    /// **newest first**.
    ///
    /// The order is the whole point. An archive nobody has catalogued is one
    /// nobody can browse, and the snapshot somebody wants is almost always the
    /// most recent — so a first run on a repository with a year of history
    /// becomes useful in seconds rather than after the whole backfill.
    ///
    /// This is also what makes cataloguing resumable across a restart: the work
    /// still outstanding is a query, not something held in memory.
    pub fn pending_archives(&self) -> Result<Vec<(i64, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT seq, name FROM archives WHERE status IS NOT NULL ORDER BY seq DESC")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }

    /// Archives belonging to `repo`, oldest first.
    ///
    /// The order is what the spool's cap eviction needs: when local storage has
    /// to give, the oldest snapshot is the one to let go of. The timestamp is
    /// what snapshot expiry needs, which works on age rather than on size.
    pub fn archives_in(&self, repo: Repo) -> Result<Vec<LocalArchiveRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, name, ts, repo, expirable_at FROM archives
             WHERE repo = ?1 ORDER BY seq",
        )?;
        let rows = stmt
            .query_map([repo.as_str()], |r| {
                Ok(LocalArchiveRow {
                    seq: r.get(0)?,
                    name: r.get(1)?,
                    ts: r.get(2)?,
                    repo: r.get(3)?,
                    expirable_at: r.get(4)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }

    /// Every locally-held snapshot, of either kind, oldest first.
    pub fn local_archives(&self) -> Result<Vec<LocalArchiveRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, name, ts, repo, expirable_at FROM archives
             WHERE repo IN ('spool','fs-snapshot') ORDER BY seq",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(LocalArchiveRow {
                    seq: r.get(0)?,
                    name: r.get(1)?,
                    ts: r.get(2)?,
                    repo: r.get(3)?,
                    expirable_at: r.get(4)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }

    /// Mark every locally-held snapshot that is not already marked as
    /// discardable from `at`.
    ///
    /// Called when a backup to the real destination succeeds. From that moment
    /// the current state of every file is off the machine, so what these still
    /// hold is only the *intermediate* versions made while the destination was
    /// away — the 10:00, 11:00 and 12:00 edits of a file that changed several
    /// times. Those are worth keeping for a while, and not forever.
    ///
    /// Only unmarked rows are touched, so a snapshot's clock starts at the
    /// first catch-up and is not reset by every backup afterwards.
    ///
    /// BORG2: with `borg transfer`, these archives could be moved into the
    /// primary repository instead of expiring — the intermediate versions would
    /// survive rather than being discarded. Replace this and the expiry sweep
    /// when Borg 2 is a foundation worth standing on.
    pub fn mark_expirable(&mut self, at: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE archives SET expirable_at = ?1
             WHERE repo IN ('spool','fs-snapshot') AND expirable_at IS NULL",
            params![at],
        )?)
    }

    /// Every archive row, oldest first.
    fn archive_rows(&self) -> Result<Vec<ArchiveRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT seq, name, ts, repo FROM archives ORDER BY seq")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(ArchiveRow {
                    seq: r.get(0)?,
                    name: r.get(1)?,
                    ts: r.get(2)?,
                    repo: r.get(3)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }

    /// Remove archives from the index, leaving it exactly as if they had never
    /// been ingested: surviving archives are densely renumbered to `1..=k`,
    /// version intervals are clamped to the surviving seqs (versions that lived
    /// only in removed archives disappear), and intervals that removal made
    /// adjacent with identical content are merged back together.
    ///
    /// Used by prune (Stage 4) and spool expiry (Stage 5). A no-op for an empty
    /// `seqs`.
    pub fn remove_archives(&mut self, seqs: &[i64]) -> Result<()> {
        if seqs.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;

        // Stage the removed seqs, then drop those archives. Survivors keep their
        // old seqs until the remap below.
        tx.execute_batch("CREATE TEMP TABLE removed_seqs(seq INTEGER PRIMARY KEY)")?;
        {
            let mut ins = tx.prepare("INSERT OR IGNORE INTO removed_seqs(seq) VALUES (?1)")?;
            for &s in seqs {
                ins.execute([s])?;
            }
        }
        tx.execute(
            "DELETE FROM archives WHERE seq IN (SELECT seq FROM removed_seqs)",
            [],
        )?;

        // Dense old->new seq map over the survivors (chronological order).
        tx.execute_batch(
            "CREATE TEMP TABLE seqmap AS
               SELECT seq AS old_seq, ROW_NUMBER() OVER (ORDER BY seq) AS new_seq
               FROM archives;
             CREATE INDEX temp.seqmap_old ON seqmap(old_seq);",
        )?;

        // Versions living entirely inside removed archives vanish; the rest are
        // clamped/remapped onto the dense seqs. The SET subqueries read the row's
        // pre-update first_seq/last_seq, so MIN/MAX pick the surviving endpoints.
        tx.execute(
            "DELETE FROM versions WHERE NOT EXISTS
               (SELECT 1 FROM seqmap WHERE old_seq BETWEEN versions.first_seq AND versions.last_seq)",
            [],
        )?;
        tx.execute(
            "UPDATE versions SET
                first_seq = (SELECT MIN(new_seq) FROM seqmap
                             WHERE old_seq BETWEEN versions.first_seq AND versions.last_seq),
                last_seq  = (SELECT MAX(new_seq) FROM seqmap
                             WHERE old_seq BETWEEN versions.first_seq AND versions.last_seq)",
            [],
        )?;

        // Renumber the archive rows. Processed in ascending seq (rowid) order and
        // new_seq <= old_seq, so no transient primary-key collision.
        tx.execute(
            "UPDATE archives SET seq = (SELECT new_seq FROM seqmap WHERE old_seq = archives.seq)",
            [],
        )?;
        tx.execute_batch("DROP TABLE seqmap; DROP TABLE removed_seqs;")?;

        coalesce_versions(&tx)?;
        tx.commit()?;
        Ok(())
    }

    /// Borrow the connection (read-side queries and tests build on this).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }
}

/// Merge version intervals that became adjacent (`prev.last + 1 == next.first`)
/// with identical content — the case where the only archives between two equal
/// versions were removed. Coalescing chains left to right per path.
fn coalesce_versions(tx: &Transaction) -> Result<()> {
    struct Row {
        rowid: i64,
        path_id: i64,
        first: i64,
        last: i64,
        size: i64,
        mtime: i64,
        kind: String,
        hash: Option<String>,
    }
    let mut select = tx.prepare(
        "SELECT rowid, path_id, first_seq, last_seq, size, mtime, kind, chunk_hash
         FROM versions ORDER BY path_id, first_seq",
    )?;
    let rows: Vec<Row> = select
        .query_map([], |r| {
            Ok(Row {
                rowid: r.get(0)?,
                path_id: r.get(1)?,
                first: r.get(2)?,
                last: r.get(3)?,
                size: r.get(4)?,
                mtime: r.get(5)?,
                kind: r.get(6)?,
                hash: r.get(7)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    drop(select);

    let mut extend = tx.prepare("UPDATE versions SET last_seq = ?2 WHERE rowid = ?1")?;
    let mut delete = tx.prepare("DELETE FROM versions WHERE rowid = ?1")?;
    let mut i = 0;
    while i < rows.len() {
        let base = &rows[i];
        let mut new_last = base.last;
        let mut j = i + 1;
        while j < rows.len() {
            let next = &rows[j];
            let mergeable = next.path_id == base.path_id
                && next.first == new_last + 1
                && next.size == base.size
                && next.mtime == base.mtime
                && next.kind == base.kind
                && next.hash == base.hash;
            if !mergeable {
                break;
            }
            new_last = next.last;
            delete.execute([next.rowid])?;
            j += 1;
        }
        if new_last != base.last {
            extend.execute(params![base.rowid, new_last])?;
        }
        i = j;
    }
    Ok(())
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
    fn remove_middle_archive_clamps_intervals() {
        let mut w = IndexWriter::open_in_memory().unwrap();
        ingest(&mut w, "a1", vec![file("a", 1, 100)]);
        ingest(&mut w, "a2", vec![file("a", 1, 100)]);
        ingest(&mut w, "a3", vec![file("a", 2, 100)]); // a: [1,2] v1, [3,3] v2
        w.remove_archives(&[2]).unwrap();
        // survivors {1,3} -> {1,2}
        assert_eq!(spans(&w, "a"), vec![(1, 1), (2, 2)]);
        assert_eq!(scalar(&w, "SELECT COUNT(*) FROM archives"), 2);
        assert_eq!(scalar(&w, "SELECT MAX(seq) FROM archives"), 2);
    }

    #[test]
    fn remove_gap_archive_merges_identical_intervals() {
        let mut w = IndexWriter::open_in_memory().unwrap();
        ingest(&mut w, "a1", vec![file("a", 1, 100)]);
        ingest(&mut w, "a2", vec![]); // a absent
        ingest(&mut w, "a3", vec![file("a", 1, 100)]); // a: [1,1], [3,3] same content
        w.remove_archives(&[2]).unwrap();
        assert_eq!(spans(&w, "a"), vec![(1, 2)]); // gap removed -> merged
        assert_eq!(versions(&w), 1);
    }

    #[test]
    fn remove_renumbers_surviving_archives() {
        let mut w = IndexWriter::open_in_memory().unwrap();
        for i in 1..=5 {
            ingest(&mut w, &format!("a{i}"), vec![file("a", 1, 100)]);
        } // a: [1,5]
        w.remove_archives(&[2, 4]).unwrap();
        // survivors {1,3,5} -> {1,2,3}, still one unbroken interval
        assert_eq!(spans(&w, "a"), vec![(1, 3)]);
        assert_eq!(scalar(&w, "SELECT COUNT(*) FROM archives"), 3);
    }

    #[test]
    fn remove_deletes_versions_only_in_removed_archives() {
        let mut w = IndexWriter::open_in_memory().unwrap();
        ingest(&mut w, "a1", vec![file("a", 1, 100), file("keep", 1, 100)]);
        ingest(&mut w, "a2", vec![file("keep", 1, 100)]); // a gone; a:[1,1], keep:[1,2]
        w.remove_archives(&[1]).unwrap();
        // survivors {2} -> {1}: a lived only in the removed archive
        assert_eq!(spans(&w, "a"), Vec::<(i64, i64)>::new());
        assert_eq!(spans(&w, "keep"), vec![(1, 1)]);
    }

    #[test]
    fn remove_empty_is_a_noop() {
        let mut w = IndexWriter::open_in_memory().unwrap();
        ingest(&mut w, "a1", vec![file("a", 1, 100)]);
        w.remove_archives(&[]).unwrap();
        assert_eq!(spans(&w, "a"), vec![(1, 1)]);
        assert_eq!(scalar(&w, "SELECT COUNT(*) FROM archives"), 1);
    }

    #[test]
    fn remove_all_archives_empties_the_index() {
        let mut w = IndexWriter::open_in_memory().unwrap();
        ingest(&mut w, "a1", vec![file("a", 1, 100)]);
        ingest(&mut w, "a2", vec![file("a", 1, 100)]);
        w.remove_archives(&[1, 2]).unwrap();
        assert_eq!(versions(&w), 0);
        assert_eq!(scalar(&w, "SELECT COUNT(*) FROM archives"), 0);
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

/// Tests about the *shape* of the catalogue: what removal and out-of-order
/// cataloguing do to the interval encoding. The two oracles here are the ones
/// that keep the encoding honest — each says "however you got here, the answer
/// must equal a plain chronological ingest of what survives".
#[cfg(test)]
mod catalogue_tests {
    use super::*;
    use crate::index::Kind;
    use proptest::prelude::*;

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

    fn flat(name: &str, size: i64) -> BorgItem {
        BorgItem {
            path: name.to_string(),
            kind: Kind::File,
            size,
            mtime: 100,
            mode: 0o644,
            chunk_hash: None,
        }
    }

    /// Canonical dump of (path name, interval, content) for every live version.
    fn dump_versions(w: &IndexWriter) -> Vec<(String, i64, i64, i64, i64, String)> {
        let mut stmt = w
            .conn()
            .prepare(
                "SELECT p.name, v.first_seq, v.last_seq, v.size, v.mtime, v.kind
                 FROM versions v JOIN paths p ON p.id = v.path_id
                 ORDER BY p.name, v.first_seq",
            )
            .unwrap();
        stmt.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    }

    fn dump_archives(w: &IndexWriter) -> Vec<(i64, String, i64)> {
        let mut stmt = w
            .conn()
            .prepare("SELECT seq, name, ts FROM archives ORDER BY seq")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    proptest! {
        /// The acceptance oracle: ingesting N archives then removing a subset
        /// yields an index byte-identical (archives + versions) to a from-scratch
        /// ingest of only the surviving archives. Files a/b/c appear/change/vanish
        /// independently per archive; a random subset of archives is removed.
        #[test]
        fn remove_archives_equals_ingesting_only_survivors(
            plan in prop::collection::vec(
                (
                    prop::option::of(0i64..3),
                    prop::option::of(0i64..3),
                    prop::option::of(0i64..3),
                    any::<bool>(),
                ),
                1..7,
            )
        ) {
            let listing = |a: &Option<i64>, b: &Option<i64>, c: &Option<i64>| {
                let mut items = Vec::new();
                if let Some(s) = a { items.push(flat("a", *s)); }
                if let Some(s) = b { items.push(flat("b", *s)); }
                if let Some(s) = c { items.push(flat("c", *s)); }
                items
            };
            let name = |i: usize| format!("arch{i}");
            let ts = |i: usize| (i as i64 + 1) * 1000;

            // Full index: ingest everything, then remove the flagged archives.
            let mut full = IndexWriter::open_in_memory().unwrap();
            for (i, (a, b, c, _)) in plan.iter().enumerate() {
                full.ingest_archive(
                    &ArchiveMeta { borg_id: None, name: name(i), ts: ts(i) },
                    Repo::Primary,
                    listing(a, b, c).into_iter(),
                ).unwrap();
            }
            let removed: Vec<i64> = plan.iter().enumerate()
                .filter(|(_, (_, _, _, rm))| *rm)
                .map(|(i, _)| i as i64 + 1)
                .collect();
            full.remove_archives(&removed).unwrap();

            // Survivor index: ingest only the kept archives, same names/timestamps.
            let mut surv = IndexWriter::open_in_memory().unwrap();
            for (i, (a, b, c, rm)) in plan.iter().enumerate() {
                if *rm { continue; }
                surv.ingest_archive(
                    &ArchiveMeta { borg_id: None, name: name(i), ts: ts(i) },
                    Repo::Primary,
                    listing(a, b, c).into_iter(),
                ).unwrap();
            }

            prop_assert_eq!(dump_archives(&full), dump_archives(&surv));
            prop_assert_eq!(dump_versions(&full), dump_versions(&surv));
        }
    }

    proptest! {
        /// The oracle for out-of-order cataloguing: reserving every archive up
        /// front and reading their listings in an arbitrary order must produce
        /// exactly the catalogue a straight chronological ingest produces.
        ///
        /// This is what lets backfill work newest-first and lets reconciliation
        /// fill a hole a crash left in the middle, without either of them being
        /// a second implementation of the interval encoding.
        #[test]
        fn catalogue_order_does_not_change_the_catalogue(
            plan in prop::collection::vec(
                (
                    prop::option::of(0i64..3),
                    prop::option::of(0i64..3),
                    prop::option::of(0i64..3),
                ),
                1..7,
            ),
            shuffle in prop::collection::vec(0usize..100, 1..7),
        ) {
            let listing = |a: &Option<i64>, b: &Option<i64>, c: &Option<i64>| {
                let mut items = Vec::new();
                if let Some(s) = a { items.push(flat("a", *s)); }
                if let Some(s) = b { items.push(flat("b", *s)); }
                if let Some(s) = c { items.push(flat("c", *s)); }
                items
            };
            let entries: Vec<ArchiveMeta> = (0..plan.len())
                .map(|i| ArchiveMeta {
                    borg_id: None,
                    name: format!("arch{i}"),
                    ts: (i as i64 + 1) * 1000,
                })
                .collect();

            // Chronological, the ordinary hourly-backup path.
            let mut expected = IndexWriter::open_in_memory().unwrap();
            for (i, (a, b, c)) in plan.iter().enumerate() {
                expected.ingest_archive(
                    &entries[i], Repo::Primary, listing(a, b, c).into_iter(),
                ).unwrap();
            }

            // Reserved up front, then catalogued in a shuffled order. The
            // shuffle keys give proptest something to shrink; equal keys keep
            // their relative order, which is fine — any order must work.
            let mut actual = IndexWriter::open_in_memory().unwrap();
            let report = actual.sync_archives(Repo::Primary, &entries).unwrap();
            prop_assert_eq!(report.pending.len(), plan.len());
            prop_assert_eq!(report.removed, 0);

            let mut order: Vec<usize> = (0..plan.len()).collect();
            order.sort_by_key(|i| shuffle.get(*i).copied().unwrap_or(0));
            for i in order {
                let (a, b, c) = &plan[i];
                // Seqs are 1-based and chronological, so archive i is seq i+1.
                actual.ingest_pending(i as i64 + 1, listing(a, b, c).into_iter()).unwrap();
            }

            prop_assert_eq!(dump_archives(&actual), dump_archives(&expected));
            prop_assert_eq!(dump_versions(&actual), dump_versions(&expected));
        }
    }

    /// Every archive row's `status`, oldest first.
    fn dump_status(w: &IndexWriter) -> Vec<Option<String>> {
        let mut stmt = w
            .conn()
            .prepare("SELECT status FROM archives ORDER BY seq")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    fn entry(name: &str, ts: i64) -> ArchiveMeta {
        ArchiveMeta {
            borg_id: None,
            name: name.to_string(),
            ts,
        }
    }

    #[test]
    fn syncing_an_empty_index_records_every_archive_as_pending() {
        let mut w = IndexWriter::open_in_memory().unwrap();
        let entries = [entry("a", 100), entry("b", 200), entry("c", 300)];
        let report = w.sync_archives(Repo::Primary, &entries).unwrap();

        assert_eq!(
            report.pending,
            vec![3, 2, 1],
            "newest first: the snapshot somebody wants to browse is the last one taken"
        );
        assert_eq!(report.removed, 0);
        assert_eq!(dump_status(&w).len(), 3);
        assert!(dump_status(&w)
            .iter()
            .all(|s| s.as_deref() == Some("pending")));
    }

    #[test]
    fn cataloguing_an_archive_marks_it_browsable() {
        let mut w = IndexWriter::open_in_memory().unwrap();
        w.sync_archives(Repo::Primary, &[entry("a", 100)]).unwrap();
        w.ingest_pending(1, vec![flat("x", 1)].into_iter()).unwrap();
        assert_eq!(dump_status(&w), vec![None]);
    }

    #[test]
    fn a_repeated_sync_changes_nothing() {
        // Reconciliation runs on every startup and after every prune. It has to
        // be free when there is nothing to do, or it would churn the catalogue.
        let mut w = IndexWriter::open_in_memory().unwrap();
        let entries = [entry("a", 100), entry("b", 200)];
        w.sync_archives(Repo::Primary, &entries).unwrap();
        w.ingest_pending(1, vec![flat("x", 1)].into_iter()).unwrap();
        w.ingest_pending(2, vec![flat("x", 1)].into_iter()).unwrap();
        let before = dump_versions(&w);

        let report = w.sync_archives(Repo::Primary, &entries).unwrap();
        assert!(report.is_empty(), "nothing to do: {report:?}");
        assert_eq!(dump_versions(&w), before);
    }

    #[test]
    fn sync_drops_archives_the_repository_no_longer_has() {
        // What a prune leaves behind: the repository is the authority, and the
        // catalogue must not keep offering snapshots that are gone.
        let mut w = IndexWriter::open_in_memory().unwrap();
        for (i, name) in ["a", "b", "c"].iter().enumerate() {
            ingest(&mut w, name, vec![flat("x", i as i64)]);
        }
        let report = w
            .sync_archives(Repo::Primary, &[entry("b", 0), entry("c", 0)])
            .unwrap();

        assert_eq!(report.removed, 1);
        assert!(report.pending.is_empty());
        assert_eq!(
            dump_archives(&w)
                .iter()
                .map(|a| a.1.clone())
                .collect::<Vec<_>>(),
            vec!["b", "c"]
        );
    }

    #[test]
    fn an_archive_inserted_into_the_middle_splits_the_intervals_that_spanned_it() {
        // The crash case: a backup landed in the repository but was never
        // catalogued, and a later one was. The catalogue must not go on
        // claiming a file was unchanged across a snapshot nobody has read.
        let mut w = IndexWriter::open_in_memory().unwrap();
        ingest(&mut w, "a", vec![flat("x", 1)]);
        ingest(&mut w, "c", vec![flat("x", 1)]);
        assert_eq!(dump_versions(&w)[0].1..=dump_versions(&w)[0].2, 1..=2);

        // "b" belongs between them by timestamp.
        let entries = [entry("a", 100), entry("b", 200), entry("c", 300)];
        // Give the existing rows the timestamps the repository reports.
        w.conn()
            .execute_batch(
                "UPDATE archives SET ts = 100 WHERE name='a';
                            UPDATE archives SET ts = 300 WHERE name='c';",
            )
            .unwrap();
        let report = w.sync_archives(Repo::Primary, &entries).unwrap();
        assert_eq!(report.pending, vec![2]);

        let versions = dump_versions(&w);
        assert_eq!(
            versions.iter().map(|v| (v.1, v.2)).collect::<Vec<_>>(),
            vec![(1, 1), (3, 3)],
            "the interval is split around the archive nobody has read"
        );
    }

    #[test]
    fn cataloguing_that_archive_rejoins_the_intervals() {
        let mut w = IndexWriter::open_in_memory().unwrap();
        ingest(&mut w, "a", vec![flat("x", 1)]);
        ingest(&mut w, "c", vec![flat("x", 1)]);
        w.conn()
            .execute_batch(
                "UPDATE archives SET ts = 100 WHERE name='a';
                            UPDATE archives SET ts = 300 WHERE name='c';",
            )
            .unwrap();
        w.sync_archives(
            Repo::Primary,
            &[entry("a", 100), entry("b", 200), entry("c", 300)],
        )
        .unwrap();

        // The file was there all along, unchanged.
        let stats = w.ingest_pending(2, vec![flat("x", 1)].into_iter()).unwrap();
        assert_eq!(stats.merged, 1, "the two halves are one interval again");

        let versions = dump_versions(&w);
        assert_eq!(versions.len(), 1);
        assert_eq!((versions[0].1, versions[0].2), (1, 3));
    }

    #[test]
    fn cataloguing_a_hole_where_the_file_differed_keeps_three_intervals() {
        let mut w = IndexWriter::open_in_memory().unwrap();
        ingest(&mut w, "a", vec![flat("x", 1)]);
        ingest(&mut w, "c", vec![flat("x", 1)]);
        w.conn()
            .execute_batch(
                "UPDATE archives SET ts = 100 WHERE name='a';
                            UPDATE archives SET ts = 300 WHERE name='c';",
            )
            .unwrap();
        w.sync_archives(
            Repo::Primary,
            &[entry("a", 100), entry("b", 200), entry("c", 300)],
        )
        .unwrap();

        let stats = w
            .ingest_pending(2, vec![flat("x", 99)].into_iter())
            .unwrap();
        assert_eq!(stats.merged, 0);
        assert_eq!(stats.new_versions, 1);
        assert_eq!(
            dump_versions(&w)
                .iter()
                .map(|v| (v.1, v.2, v.3))
                .collect::<Vec<_>>(),
            vec![(1, 1, 1), (2, 2, 99), (3, 3, 1)],
            "the file really did change and change back"
        );
    }

    #[test]
    fn an_incomplete_listing_commits_nothing_and_leaves_the_archive_pending() {
        // Half a catalogue marked complete is worse than no catalogue: the
        // timeline would show a snapshot with files missing and nothing to
        // indicate anything was wrong.
        let mut w = IndexWriter::open_in_memory().unwrap();
        w.sync_archives(Repo::Primary, &[entry("a", 100)]).unwrap();

        let feed = vec![Ok(flat("x", 1)), Ok(flat("y", 2)), Err(ListingIncomplete)];
        let err = w
            .ingest_pending_fallible(1, feed.into_iter())
            .expect_err("an incomplete listing is an error");
        assert!(
            matches!(err, IndexError::ListingIncomplete { seq: 1 }),
            "got {err:?}"
        );

        assert!(dump_versions(&w).is_empty(), "the transaction rolled back");
        assert_eq!(
            dump_status(&w),
            vec![Some("pending".to_string())],
            "the archive is still waiting to be read"
        );
    }

    #[test]
    fn re_reading_that_listing_catalogues_it_properly() {
        let mut w = IndexWriter::open_in_memory().unwrap();
        w.sync_archives(Repo::Primary, &[entry("a", 100)]).unwrap();
        let _ = w.ingest_pending_fallible(
            1,
            vec![Ok(flat("x", 1)), Err(ListingIncomplete)].into_iter(),
        );

        let stats = w
            .ingest_pending(1, vec![flat("x", 1), flat("y", 2)].into_iter())
            .expect("the retry succeeds");
        assert_eq!(stats.items, 2);
        assert_eq!(
            stats.new_versions, 2,
            "no duplicates from the failed attempt"
        );
        assert_eq!(dump_status(&w), vec![None]);
    }

    #[test]
    fn pending_archives_are_reported_newest_first() {
        // Backfill order: the snapshot somebody wants to browse is the last one
        // taken, so a year of history becomes useful in seconds.
        let mut w = IndexWriter::open_in_memory().unwrap();
        w.sync_archives(
            Repo::Primary,
            &[entry("a", 100), entry("b", 200), entry("c", 300)],
        )
        .unwrap();
        let pending = w.pending_archives().unwrap();
        assert_eq!(
            pending.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>(),
            vec!["c", "b", "a"]
        );

        w.ingest_pending(3, vec![flat("x", 1)].into_iter()).unwrap();
        assert_eq!(w.pending_archives().unwrap().len(), 2);
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

    /// Ingest a delta listing at the end of the catalogue, the way a spool
    /// snapshot arrives.
    fn ingest_spool(w: &mut IndexWriter, name: &str, items: Vec<BorgItem>) -> IngestStats {
        let seq = w.append_archive(&meta(name), Repo::Spool).unwrap();
        w.ingest_delta(seq, items.into_iter().map(Ok)).unwrap()
    }

    #[test]
    fn a_delta_leaves_unchanged_files_present_rather_than_deleted() {
        // The failure this exists to prevent: ingesting a changed-files-only
        // archive as though it were a full listing makes every untouched file
        // look deleted at that snapshot. Somebody browsing an hour spent on a
        // train would find their documents missing.
        let mut w = IndexWriter::open_in_memory().unwrap();
        ingest(
            &mut w,
            "primary-1",
            vec![flat("a", 1), flat("b", 1), flat("c", 1)],
        );

        // Offline: only `b` changed.
        ingest_spool(&mut w, "spool-1", vec![flat("b", 2)]);

        assert_eq!(spans(&w, "a"), vec![(1, 2)], "untouched, and still there");
        assert_eq!(spans(&w, "c"), vec![(1, 2)], "untouched, and still there");
        assert_eq!(
            spans(&w, "b"),
            vec![(1, 1), (2, 2)],
            "the file that changed opens a new version"
        );
    }

    #[test]
    fn a_delta_snapshot_browses_as_the_whole_tree() {
        // The same thing stated the way the timeline asks it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        {
            let mut w = IndexWriter::open(&path).unwrap();
            ingest(
                &mut w,
                "primary-1",
                vec![
                    BorgItem {
                        path: "home/report.odt".into(),
                        ..flat("x", 10)
                    },
                    BorgItem {
                        path: "home/notes.txt".into(),
                        ..flat("x", 20)
                    },
                ],
            );
            ingest_spool(
                &mut w,
                "spool-1",
                vec![BorgItem {
                    path: "home/notes.txt".into(),
                    ..flat("x", 99)
                }],
            );
        }
        let reader = crate::index::IndexReader::open(&path).unwrap();
        let at_spool = reader.folder_at("home", 2).unwrap();
        assert_eq!(
            at_spool.len(),
            2,
            "both files are visible in the local snapshot, got {:?}",
            at_spool.iter().map(|e| &e.name).collect::<Vec<_>>()
        );
        let notes = at_spool.iter().find(|e| e.name == "notes.txt").unwrap();
        assert_eq!(notes.size, 99, "and the edited one shows its new contents");
    }

    #[test]
    fn a_delta_does_not_resurrect_a_file_that_was_already_gone() {
        // Only intervals ending at the immediately preceding archive are carried
        // across. A file deleted two snapshots ago must stay deleted.
        let mut w = IndexWriter::open_in_memory().unwrap();
        ingest(&mut w, "p1", vec![flat("a", 1), flat("gone", 1)]);
        ingest(&mut w, "p2", vec![flat("a", 1)]);
        ingest_spool(&mut w, "spool-1", vec![flat("a", 2)]);

        assert_eq!(spans(&w, "gone"), vec![(1, 1)], "still gone");
        assert_eq!(spans(&w, "a"), vec![(1, 2), (3, 3)]);
    }

    #[test]
    fn a_delta_records_a_file_that_did_not_exist_before() {
        let mut w = IndexWriter::open_in_memory().unwrap();
        ingest(&mut w, "p1", vec![flat("a", 1)]);
        let stats = ingest_spool(&mut w, "spool-1", vec![flat("new", 1)]);

        assert_eq!(stats.new_versions, 1);
        assert_eq!(spans(&w, "new"), vec![(2, 2)]);
        assert_eq!(spans(&w, "a"), vec![(1, 2)], "and the rest carried across");
    }

    #[test]
    fn an_empty_delta_carries_the_whole_tree_across() {
        // An offline hour in which nothing changed still produces a snapshot in
        // the timeline, and it must show the same files as the one before it.
        let mut w = IndexWriter::open_in_memory().unwrap();
        ingest(&mut w, "p1", vec![flat("a", 1), flat("b", 1)]);
        let stats = ingest_spool(&mut w, "spool-1", vec![]);

        assert_eq!(stats.items, 0);
        assert_eq!(stats.extended, 2);
        assert_eq!(spans(&w, "a"), vec![(1, 2)]);
        assert_eq!(spans(&w, "b"), vec![(1, 2)]);
    }

    #[test]
    fn an_ordinary_ingest_is_untouched_by_the_delta_path() {
        // The delta rule must not leak into normal backups, where an absent file
        // genuinely means the file was deleted.
        let mut w = IndexWriter::open_in_memory().unwrap();
        ingest(&mut w, "p1", vec![flat("a", 1), flat("deleted", 1)]);
        ingest(&mut w, "p2", vec![flat("a", 1)]);
        assert_eq!(
            spans(&w, "deleted"),
            vec![(1, 1)],
            "a full listing that omits a file means it is gone"
        );
    }

    #[test]
    fn archives_can_be_listed_per_repository_oldest_first() {
        let mut w = IndexWriter::open_in_memory().unwrap();
        ingest(&mut w, "p1", vec![flat("a", 1)]);
        ingest_spool(&mut w, "s1", vec![flat("a", 2)]);
        ingest_spool(&mut w, "s2", vec![flat("a", 3)]);

        let spool = w.archives_in(Repo::Spool).unwrap();
        assert_eq!(
            spool.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            vec!["s1", "s2"],
            "oldest first: the one to evict when local storage has to give"
        );
        assert_eq!(w.archives_in(Repo::Primary).unwrap().len(), 1);
    }

    #[test]
    fn the_delta_is_compared_against_the_newest_catalogued_archive() {
        // Not MAX(seq): an archive that exists but has not been read has no
        // versions recorded, so comparing against it would report every file on
        // the machine as new and spool the lot.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        let latest = || {
            crate::index::IndexReader::open(&path)
                .unwrap()
                .latest_catalogued_seq()
                .unwrap()
        };

        let mut w = IndexWriter::open(&path).unwrap();
        ingest(&mut w, "p1", vec![flat("a", 1)]);
        assert_eq!(latest(), Some(1));

        w.append_archive(&meta("p2"), Repo::Primary).unwrap();
        assert_eq!(
            latest(),
            Some(1),
            "a pending archive is not something to compare against"
        );

        w.ingest_pending(2, vec![flat("a", 1)].into_iter()).unwrap();
        assert_eq!(latest(), Some(2));
    }

    #[test]
    fn appending_records_the_archive_before_its_listing_is_read() {
        // The order the pipeline needs: the archive exists in the repository the
        // moment `borg create` finishes, so the catalogue records it before
        // spending minutes reading its file list. A crash in between leaves a
        // pending row, which is exactly what reconciliation looks for.
        let mut w = IndexWriter::open_in_memory().unwrap();
        let seq = w.append_archive(&meta("a"), Repo::Primary).unwrap();
        assert_eq!(seq, 1);
        assert_eq!(dump_status(&w), vec![Some("pending".to_string())]);

        w.ingest_pending(seq, vec![flat("x", 1)].into_iter())
            .unwrap();
        assert_eq!(dump_status(&w), vec![None]);
    }
}
