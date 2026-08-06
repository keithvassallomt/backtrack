// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The names that identify Backtrack on the session bus.
//!
//! Only the names live here — the daemon implements the interface and the
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
