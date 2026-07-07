// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>

//! The Backtrack index: a SQLite catalogue of every file version across every
//! Borg archive.
//!
//! The index is what makes the UI fast and fully offline. Rather than one row
//! per file×archive, versions are *interval-encoded*: a single row spans the
//! contiguous range of archives (`first_seq..=last_seq`) over which a file
//! version was unchanged. See [`schema`] for the layout.
//!
//! Access is split by capability:
//! - [`Index::open`] performs migrations and an integrity check, then hands out
//!   an owned connection.
//! - Writes go through a single writer (the daemon owns it) — Stage 1 T2+.
//! - Reads open the database read-only and use [`reader`] — Stage 1 T3+.

mod item;
mod reader;
mod schema;
mod writer;

use std::path::Path;

use rusqlite::Connection;

pub use item::{parse_borg_mtime, ArchiveMeta, BorgItem, ItemParseError, Kind, Repo};
pub use reader::{
    ArchiveSummary, Direction, Entry, IndexReader, LiveEntry, SearchHit, VersionSpan,
};
pub use schema::SCHEMA_VERSION;
pub use writer::{IndexWriter, IngestStats};

/// Errors surfaced by the index layer.
#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    /// The on-disk database failed its integrity check (or is not a database).
    /// Surfaced to the health model so the daemon can offer a rebuild.
    #[error("index database is corrupt: {0}")]
    Corrupt(String),

    /// A schema migration could not be applied.
    #[error("index schema migration failed: {0}")]
    Migration(String),

    /// An underlying SQLite error not otherwise classified.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

/// A convenience result alias for the index layer.
pub type Result<T> = std::result::Result<T, IndexError>;

/// An opened index database. Wraps a single [`rusqlite::Connection`] configured
/// for WAL mode with `synchronous=NORMAL`, with the schema migrated forward to
/// [`SCHEMA_VERSION`].
#[derive(Debug)]
pub struct Index {
    conn: Connection,
}

/// Configure a freshly-opened connection: integrity check, WAL + NORMAL
/// pragmas, then migrate the schema forward. Shared by every opener so the
/// writer and readers see identical setup.
fn configure(mut conn: Connection) -> Result<Connection> {
    // Integrity check first: refuse to migrate (or otherwise write to) a
    // database we already know is damaged.
    schema::integrity_check(&conn)?;
    // WAL with synchronous=NORMAL: concurrent readers (the GUI) while the single
    // writer (the daemon) ingests, without fsync-per-commit cost.
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
    schema::migrate(&mut conn)?;
    Ok(conn)
}

fn open_connection(path: &Path) -> Result<Connection> {
    configure(Connection::open(path)?)
}

fn open_memory_connection() -> Result<Connection> {
    configure(Connection::open_in_memory()?)
}

impl Index {
    /// Open (creating if absent) the index at `path`, run an integrity check,
    /// and migrate the schema forward to the current version.
    pub fn open(path: &Path) -> Result<Index> {
        Ok(Index {
            conn: open_connection(path)?,
        })
    }

    /// Open a private in-memory index. Used by tests and ephemeral tooling.
    pub fn open_in_memory() -> Result<Index> {
        Ok(Index {
            conn: open_memory_connection()?,
        })
    }

    /// The schema version recorded in `meta`.
    pub fn schema_version(&self) -> Result<i64> {
        schema::current_version(&self.conn)
    }

    /// Borrow the underlying connection. The read-side queries (T3+) and the
    /// writer are built on top of this; for now only the schema tests read it,
    /// so it is dead code in non-test builds.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};

    /// A table exists in the current schema.
    fn has_table(index: &Index, name: &str) -> bool {
        index
            .conn()
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type IN ('table','view') AND name = ?1",
                [name],
                |_| Ok(()),
            )
            .is_ok()
    }

    /// An index (by name) exists in the current schema.
    fn has_sqlite_index(index: &Index, name: &str) -> bool {
        index
            .conn()
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1",
                [name],
                |_| Ok(()),
            )
            .is_ok()
    }

    #[test]
    fn open_fresh_creates_current_schema() {
        let index = Index::open_in_memory().unwrap();

        assert_eq!(index.schema_version().unwrap(), SCHEMA_VERSION);
        for table in ["archives", "paths", "versions", "fts_names", "meta"] {
            assert!(has_table(&index, table), "missing table {table}");
        }
    }

    #[test]
    fn creates_required_indexes() {
        let index = Index::open_in_memory().unwrap();
        assert!(
            has_sqlite_index(&index, "versions_path_seq"),
            "missing versions(path_id, first_seq, last_seq) index"
        );
        assert!(
            has_sqlite_index(&index, "paths_parent_name"),
            "missing unique paths(parent_id, name) index"
        );
    }

    #[test]
    fn wal_mode_is_enabled_on_file_backed_db() {
        let dir = tempfile::tempdir().unwrap();
        let index = Index::open(&dir.path().join("index.db")).unwrap();
        let mode: String = index
            .conn()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    #[test]
    fn reopen_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");

        let first = Index::open(&path).unwrap().schema_version().unwrap();
        // Dropping the first handle and reopening must not re-run migrations or
        // error — the version is unchanged and the schema is intact.
        let second = Index::open(&path).unwrap().schema_version().unwrap();

        assert_eq!(first, SCHEMA_VERSION);
        assert_eq!(second, SCHEMA_VERSION);
    }

    #[test]
    fn corrupt_file_yields_index_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        drop(Index::open(&path).unwrap());

        // Overwrite the b-tree pages (everything past the first page header) with
        // garbage, leaving the SQLite magic intact so it is recognised as a
        // database that then fails its integrity check.
        {
            let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            let len = file.metadata().unwrap().len();
            file.seek(SeekFrom::Start(len.min(100))).unwrap();
            file.write_all(&vec![0xFF; (len.saturating_sub(100)) as usize + 512])
                .unwrap();
        }

        match Index::open(&path) {
            Err(IndexError::Corrupt(_)) => {}
            other => panic!("expected IndexError::Corrupt, got {other:?}"),
        }
    }

    #[test]
    fn non_database_file_yields_index_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        std::fs::write(&path, b"this is definitely not a sqlite database").unwrap();

        match Index::open(&path) {
            Err(IndexError::Corrupt(_)) => {}
            other => panic!("expected IndexError::Corrupt, got {other:?}"),
        }
    }
}
