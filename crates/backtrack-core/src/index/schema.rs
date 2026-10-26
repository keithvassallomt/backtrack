// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Index schema definition and the forward-only migration runner.
//!
//! The schema is versioned by an integer stored in `meta.schema_version`. On
//! open, [`super::Index`] applies every migration whose target version exceeds
//! the recorded one, inside a single transaction per step. Migrations are
//! append-only: never edit a shipped migration, add a new one.

use rusqlite::{Connection, OptionalExtension};

use super::{IndexError, Result};

/// The schema version this build of the crate expects. Bump when appending a
/// migration.
pub const SCHEMA_VERSION: i64 = 4;

/// Ordered list of migrations. Index `i` migrates the database *to* version
/// `i + 1`. The runner applies only those beyond the current version.
const MIGRATIONS: &[&str] = &[
    // ── v1 ── initial schema: interval-encoded catalogue + FTS5 name search.
    r#"
    -- One row per Borg archive. `seq` is our own monotonic ordinal (Borg's
    -- archive order); `repo` distinguishes the primary repo from the offline
    -- spool and btrfs snapshots so unified queries and the "on this computer"
    -- badge fall out for free.
    CREATE TABLE archives(
        seq     INTEGER PRIMARY KEY,
        borg_id TEXT,
        name    TEXT,
        ts      INTEGER,
        repo    TEXT NOT NULL CHECK(repo IN ('primary','spool','fs-snapshot')),
        status  TEXT
    );

    -- Deduplicated path tree. Each row is one component; `parent_id = 0` marks a
    -- top-level component (there is no row for the virtual root). Uniqueness of
    -- (parent_id, name) makes path resolution an upsert.
    CREATE TABLE paths(
        id        INTEGER PRIMARY KEY,
        parent_id INTEGER NOT NULL,
        name      TEXT NOT NULL
    );
    CREATE UNIQUE INDEX paths_parent_name ON paths(parent_id, name);

    -- One row per *file version*, spanning the inclusive archive range
    -- first_seq..=last_seq over which it was unchanged (interval encoding).
    CREATE TABLE versions(
        path_id    INTEGER NOT NULL,
        first_seq  INTEGER NOT NULL,
        last_seq   INTEGER NOT NULL,
        size       INTEGER,
        mtime      INTEGER,
        mode       INTEGER,
        kind       TEXT NOT NULL,
        chunk_hash TEXT
    );
    CREATE INDEX versions_path_seq ON versions(path_id, first_seq, last_seq);

    -- Filename search. rowid is kept equal to paths.id so a hit maps straight
    -- back to a path (and thus its version history).
    CREATE VIRTUAL TABLE fts_names USING fts5(name);

    CREATE TABLE meta(key TEXT PRIMARY KEY, value);
    "#,
    // ── v2 ── when a locally-held snapshot may be discarded.
    r#"
    -- Set on spool and fs-snapshot archives once a backup to the real
    -- destination has succeeded: from that moment the current state of every
    -- file is safely off the machine, and what these still hold is only the
    -- intermediate versions made during the offline window. NULL means "still
    -- carrying the load" — nothing has caught up yet, so nothing may be
    -- discarded. Epoch seconds, like every other time in this schema.
    ALTER TABLE archives ADD COLUMN expirable_at INTEGER;
    "#,
    // ── v3 ── finding the versions that ended inside a range of archives.
    r#"
    -- What a local snapshot holds that the next backup at the destination does
    -- not is the versions that ended between the two. Without this, asking
    -- that reads every version in the catalogue, once per local snapshot.
    CREATE INDEX versions_last_seq ON versions(last_seq);
    "#,
    // ── v4 ── Snapper's snapshots as a kind of archive (Stage 17).
    r#"
    -- A CHECK cannot be altered, so the table is made again with the new value
    -- allowed and everything else as it was, and its rows copied across.
    CREATE TABLE archives_v4(
        seq          INTEGER PRIMARY KEY,
        borg_id      TEXT,
        name         TEXT,
        ts           INTEGER,
        repo         TEXT NOT NULL
                     CHECK(repo IN ('primary','spool','fs-snapshot','snapper')),
        status       TEXT,
        expirable_at INTEGER
    );
    INSERT INTO archives_v4(seq, borg_id, name, ts, repo, status, expirable_at)
      SELECT seq, borg_id, name, ts, repo, status, expirable_at FROM archives;
    DROP TABLE archives;
    ALTER TABLE archives_v4 RENAME TO archives;
    "#,
];

/// Read the current schema version, treating a database with no `meta` table
/// (a brand-new file) as version 0.
pub(super) fn current_version(conn: &Connection) -> Result<i64> {
    let has_meta = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='meta'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !has_meta {
        return Ok(0);
    }
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key='schema_version'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(raw.and_then(|s| s.parse().ok()).unwrap_or(0))
}

/// Apply every migration beyond the recorded version, advancing
/// `meta.schema_version` in the same transaction as each step.
pub(super) fn migrate(conn: &mut Connection) -> Result<()> {
    let mut current = current_version(conn)?;
    while (current as usize) < MIGRATIONS.len() {
        let target = current + 1;
        let sql = MIGRATIONS[current as usize];
        let tx = conn.transaction()?;
        tx.execute_batch(sql)
            .map_err(|e| IndexError::Migration(format!("v{target}: {e}")))?;
        tx.execute(
            "INSERT INTO meta(key, value) VALUES('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [target.to_string()],
        )?;
        tx.commit()?;
        current = target;
    }
    Ok(())
}

/// Run SQLite's quick integrity check. Any failure — a malformed image or a file
/// that is not a database at all — maps to [`IndexError::Corrupt`].
pub(super) fn integrity_check(conn: &Connection) -> Result<()> {
    match conn.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0)) {
        Ok(msg) if msg == "ok" => Ok(()),
        Ok(msg) => Err(IndexError::Corrupt(msg)),
        Err(e) => Err(IndexError::Corrupt(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database as a build at `version` left it.
    fn at_version(version: usize) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..version] {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute(
            "INSERT INTO meta(key, value) VALUES('schema_version', ?1)",
            [version.to_string()],
        )
        .unwrap();
        conn
    }

    /// An archive row, every column.
    type Row = (
        i64,
        Option<String>,
        String,
        i64,
        String,
        Option<String>,
        Option<i64>,
    );

    fn columns(conn: &Connection) -> Vec<(String, String, bool, bool)> {
        let mut stmt = conn.prepare("PRAGMA table_info(archives)").unwrap();
        stmt.query_map([], |r| Ok((r.get(1)?, r.get(2)?, r.get(3)?, r.get(5)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn v4_keeps_every_archive_and_allows_snappers() {
        let mut conn = at_version(3);
        conn.execute_batch(
            "INSERT INTO archives(seq, borg_id, name, ts, repo, status, expirable_at) VALUES
               (1, 'ab12', 'bt-host-1', 1000, 'primary', NULL, NULL),
               (2, NULL, 'bt-local-2', 2000, 'spool', NULL, 5000),
               (3, NULL, 'bt-local-3', 3000, 'fs-snapshot', 'pending', NULL);
             INSERT INTO versions(path_id, first_seq, last_seq, size, mtime, mode, kind)
               VALUES (1, 1, 3, 5, 10, 420, 'file');",
        )
        .unwrap();
        let rows = |conn: &Connection| -> Vec<Row> {
            let mut stmt = conn
                .prepare("SELECT seq, borg_id, name, ts, repo, status, expirable_at FROM archives ORDER BY seq")
                .unwrap();
            stmt.query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
        };
        let before = rows(&conn);

        migrate(&mut conn).unwrap();

        assert_eq!(current_version(&conn).unwrap(), 4);
        assert_eq!(rows(&conn), before, "every archive as it was");
        let versions: i64 = conn
            .query_row("SELECT COUNT(*) FROM versions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(versions, 1, "the versions are not touched");
        conn.execute(
            "INSERT INTO archives(seq, name, ts, repo) VALUES (4, 'snapper:/home/.snapshots/7', 4000, 'snapper')",
            [],
        )
        .expect("a Snapper snapshot is a kind of archive now");
        assert!(
            conn.execute(
                "INSERT INTO archives(seq, name, ts, repo) VALUES (5, 'x', 5000, 'elsewhere')",
                [],
            )
            .is_err(),
            "and any other kind is still refused"
        );
        assert_eq!(
            columns(&conn),
            columns(&at_version(3)),
            "every column as it was, with its type, NOT NULL and key"
        );
        let leftover: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'archives_v4'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(leftover, 0);
    }
}
