// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>

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
pub const SCHEMA_VERSION: i64 = 1;

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
