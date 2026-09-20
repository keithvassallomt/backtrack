// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The window's side of `org.backtrack.Daemon1`.
//!
//! Browsing does not go through here: the timeline reads the index directly,
//! which is what makes it instant and what makes it work with the daemon
//! stopped. The daemon is needed for the things only it can do — extracting a
//! file for the preview, starting a backup, pausing the schedule — and for
//! telling the window when the catalogue has grown.
//!
//! The connection is made through D-Bus activation, so there is no "is the
//! daemon running?" check anywhere in the app. Asking for it starts it.

use backtrack_core::dbus::{SearchResult, Status};
use tracing::warn;

/// The daemon, as the window calls it.
#[zbus::proxy(
    interface = "org.backtrack.Daemon1",
    default_path = "/org/backtrack/Daemon1"
)]
pub trait Daemon1 {
    fn backup_now(&self) -> zbus::Result<u64>;
    fn pause(&self, until: u64) -> zbus::Result<()>;
    fn resume(&self) -> zbus::Result<()>;
    fn get_status(&self) -> zbus::Result<Status>;
    /// A readable descriptor onto one file's contents as of `archive` (the
    /// archive's name, as the index records it).
    fn preview_file(&self, archive: &str, path: &str) -> zbus::Result<zbus::zvariant::OwnedFd>;
    fn search_files(&self, query: &str) -> zbus::Result<Vec<SearchResult>>;
    fn cancel_job(&self, id: u64) -> zbus::Result<()>;

    /// Progress of a running backup.
    #[zbus(signal)]
    fn backup_progress(&self, job: u64, phase: &str, current: u64, total: u64) -> zbus::Result<()>;

    /// Progress of catalogue ingest for one archive, as a percentage.
    #[zbus(signal)]
    fn indexing_progress(&self, archive: &str, pct: u32) -> zbus::Result<()>;

    /// The overall health state changed.
    #[zbus(signal)]
    fn status_changed(&self, state: &str) -> zbus::Result<()>;
}

/// Connect to the daemon, honoring `BACKTRACK_DEV` for the bus name.
///
/// Failing here is not fatal and must not be treated as such. Everything the
/// timeline shows comes from the index, so a window with no daemon behind it is
/// still a working browser — it simply cannot preview, back up or pause, and it
/// says so where those controls are.
pub async fn connect() -> Result<Daemon1Proxy<'static>, String> {
    let connection = zbus::Connection::session()
        .await
        .map_err(|e| format!("Backtrack could not reach the session bus: {e}"))?;
    let name = backtrack_core::dbus::bus_name();
    Daemon1Proxy::builder(&connection)
        .destination(name)
        .map_err(|e| e.to_string())?
        .build()
        .await
        .map_err(|e| explain(&e, name))
}

/// Turn a connection failure into something a person can act on.
fn explain(error: &zbus::Error, name: &str) -> String {
    match error {
        zbus::Error::MethodError(err_name, _, _)
            if err_name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" =>
        {
            warn!(bus_name = name, "no daemon, and the bus cannot start one");
            "The Backtrack service is not installed, so backups cannot be started from \
             here. Browsing and restoring still work."
                .to_string()
        }
        other => {
            warn!(bus_name = name, error = %other, "could not reach the daemon");
            format!("Backtrack could not reach its background service: {other}")
        }
    }
}
