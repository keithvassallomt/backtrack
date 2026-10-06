// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The tray's side of `org.backtrack.Daemon1`: the calls its menu makes and
//! the signals that say the status line is out of date.

use backtrack_core::dbus::Status;
use tracing::debug;
use zbus::proxy::{CacheProperties, MethodFlags};

#[zbus::proxy(
    interface = "org.backtrack.Daemon1",
    default_path = "/org/backtrack/Daemon1"
)]
pub trait Daemon1 {
    fn backup_now(&self) -> zbus::Result<u64>;
    fn pause(&self, until: u64) -> zbus::Result<()>;
    fn resume(&self) -> zbus::Result<()>;
    fn get_status(&self) -> zbus::Result<Status>;

    #[zbus(signal)]
    fn backup_progress(&self, job: u64, phase: &str, current: u64, total: u64) -> zbus::Result<()>;
    #[zbus(signal)]
    fn status_changed(&self, state: &str, reason: &str) -> zbus::Result<()>;
    #[zbus(signal)]
    fn job_finished(&self, job: u64, kind: &str, outcome: &str) -> zbus::Result<()>;
}

/// The daemon on `connection`, honoring `BACKTRACK_DEV` for the bus name.
///
/// No properties are cached: fetching them would start a daemon that is not
/// running, which is the one thing [`status`] is careful not to do.
pub async fn proxy(connection: &zbus::Connection) -> zbus::Result<Daemon1Proxy<'static>> {
    Daemon1Proxy::builder(connection)
        .destination(backtrack_core::dbus::bus_name())?
        .cache_properties(CacheProperties::No)
        .build()
        .await
}

/// What the daemon says, or `None` when it is not running.
///
/// Asked without starting it. With "Run in background" off, the daemon leaves
/// once nobody needs it, and a tray that started it again to ask how it was
/// doing would keep it running all day on the tray's account. The menu's own
/// actions do start it: those are somebody asking for a backup.
pub async fn status(daemon: &Daemon1Proxy<'_>) -> Option<Status> {
    match daemon
        .inner()
        .call_with_flags::<_, _, Status>("GetStatus", MethodFlags::NoAutoStart.into(), &())
        .await
    {
        Ok(status) => status,
        Err(error) => {
            debug!(%error, "the daemon did not answer");
            None
        }
    }
}
