// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The published D-Bus contract: the names that identify Backtrack on the
//! session bus, and the payload types its methods return.
//!
//! The names live here — the daemon implements the interface and the
//! clients call it, but both have to agree on what to call, so the strings sit
//! in the one crate they both depend on. There is no zbus dependency in this
//! module.
//!
//! The *interface* and *object path* are fixed: they are the published API from
//! `stack.md` §2. The *bus name* is not, quite — under `BACKTRACK_DEV` it gains
//! a `.Dev` suffix, so a development daemon and an installed one can run side by
//! side without either serving the other's clients. That matters more than it
//! sounds: dev mode already redirects the data directory, so a dev daemon
//! answering the real GUI would show an empty timeline for a machine that is in
//! fact fully backed up.
//!
//! The payload types live here for the same reason: the daemon marshals them
//! and every client unmarshals them, so a single definition is the only way the
//! two cannot drift apart.

use serde::{Deserialize, Serialize};
use zvariant::Type;

/// What `GetStatus` answers with.
///
/// A struct rather than a loose dictionary: the shape is part of the published
/// interface, so it belongs in the introspection output where a client can see
/// it and a snapshot test can pin it.
///
/// Times are seconds since the epoch, with `0` meaning "never" or "not
/// applicable" — D-Bus has no null, and inventing one per field would be worse
/// than one convention applied everywhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Status {
    /// One of health.md's states, as its documented spelling.
    pub state: String,
    /// When the last backup succeeded.
    pub last_backup: u64,
    /// When the next scheduled backup is due; 0 when manual or paused.
    pub next_backup: u64,
    /// Whether the destination answered the last time it was probed.
    pub destination_reachable: bool,
    /// Bytes the offline spool is currently holding.
    pub spool_bytes: u64,
    /// The job currently running, or 0 when idle.
    pub active_job: u64,
    /// When the current pause lifts; 0 when not paused.
    pub paused_until: u64,
    /// Whether a repository has been configured at all.
    pub configured: bool,
}

/// One result row from `SearchFiles`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SearchResult {
    /// Archive-relative path.
    pub path: String,
    pub name: String,
    /// "file", "dir", "symlink", or "other".
    pub kind: String,
    /// First and last archive sequence the path appears in.
    pub first_seq: i64,
    pub last_seq: i64,
    /// Timestamps of those archives, seconds since the epoch.
    pub first_ts: i64,
    pub last_ts: i64,
    /// How many distinct versions the path has had.
    pub versions: u32,
    /// Whether the file still exists in the newest archive. A false here is what
    /// drives the "deleted after this" badge.
    pub exists_today: bool,
}

/// The well-known bus name of an installed daemon.
pub const BUS_NAME: &str = "org.backtrack.Daemon1";

/// The bus name of a development daemon (`BACKTRACK_DEV` set).
pub const BUS_NAME_DEV: &str = "org.backtrack.Daemon1.Dev";

/// The interface every client talks. Fixed in both modes — only the bus name
/// varies, so introspection output is identical either way.
pub const INTERFACE: &str = "org.backtrack.Daemon1";

/// The single object the daemon exports.
pub const OBJECT_PATH: &str = "/org/backtrack/Daemon1";

/// The prefix for the daemon's named D-Bus errors, which mirror the engine's
/// error taxonomy (e.g. `org.backtrack.Error.PassphraseMissing`).
pub const ERROR_PREFIX: &str = "org.backtrack.Error";

/// The bus name to own (daemon) or call (clients), honoring `BACKTRACK_DEV`.
pub fn bus_name() -> &'static str {
    bus_name_for(std::env::var_os("BACKTRACK_DEV").is_some())
}

/// Pure selection, split out so it can be tested without touching the process
/// environment.
fn bus_name_for(dev: bool) -> &'static str {
    if dev {
        BUS_NAME_DEV
    } else {
        BUS_NAME
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_mode_uses_a_distinct_bus_name() {
        assert_eq!(bus_name_for(false), BUS_NAME);
        assert_eq!(bus_name_for(true), BUS_NAME_DEV);
        assert_ne!(BUS_NAME, BUS_NAME_DEV);
    }

    #[test]
    fn the_published_interface_does_not_vary() {
        // Clients introspect the same interface in both modes; only the name
        // they connect through differs.
        assert_eq!(INTERFACE, "org.backtrack.Daemon1");
        assert_eq!(OBJECT_PATH, "/org/backtrack/Daemon1");
    }
}
