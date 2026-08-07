// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The client side of `org.backtrack.Daemon1`.
//!
//! Generated from the same names and payload types the daemon serves, so the
//! two cannot drift: [`backtrack_core::dbus`] holds both.

use backtrack_core::dbus::{SearchResult, Status};

/// The daemon, as seen from a client.
#[zbus::proxy(
    interface = "org.backtrack.Daemon1",
    default_path = "/org/backtrack/Daemon1"
)]
pub trait Daemon1 {
    fn backup_now(&self) -> zbus::Result<u64>;
    fn pause(&self, until: u64) -> zbus::Result<()>;
    fn resume(&self) -> zbus::Result<()>;
    fn get_status(&self) -> zbus::Result<Status>;
    fn restore_files(
        &self,
        archive: &str,
        paths: &[String],
        dest: &str,
        policy: &str,
    ) -> zbus::Result<u64>;
    fn compare_file(&self, archive: &str, path: &str) -> zbus::Result<u64>;
    fn search_files(&self, query: &str) -> zbus::Result<Vec<SearchResult>>;
    fn restore_everything(&self, archive: &str, policy: &str) -> zbus::Result<u64>;
    fn prune(&self) -> zbus::Result<u64>;
    fn verify(&self) -> zbus::Result<u64>;
    fn compact(&self) -> zbus::Result<u64>;
    fn cancel_job(&self, id: u64) -> zbus::Result<()>;
    fn pause_job(&self, id: u64) -> zbus::Result<()>;
    fn resume_job(&self, id: u64) -> zbus::Result<()>;
    fn get_config(&self) -> zbus::Result<String>;
    fn get_config_key(&self, key: &str) -> zbus::Result<String>;
    fn set_config(&self, key: &str, value: &str) -> zbus::Result<()>;
    fn setup_repo(&self, path: &str, passphrase: &str) -> zbus::Result<()>;
    fn import_repo(&self, path: &str, passphrase: &str) -> zbus::Result<()>;
}

/// Connect to the daemon, honoring `BACKTRACK_DEV` for the bus name.
///
/// D-Bus activation means this starts the daemon if it is not already running,
/// so there is no "is it up?" check to get wrong — provided the units are
/// installed. When they are not, the error says so rather than leaving the user
/// with a bare `ServiceUnknown`.
pub async fn connect() -> Result<Daemon1Proxy<'static>, crate::CliError> {
    let connection = zbus::Connection::session()
        .await
        .map_err(|e| crate::CliError::NoSessionBus(e.to_string()))?;
    let name = backtrack_core::dbus::bus_name();
    Daemon1Proxy::builder(&connection)
        .destination(name)
        .map_err(|e| crate::CliError::Bus(e.to_string()))?
        .build()
        .await
        .map_err(|e| classify(e, name))
}

/// Turn a connection failure into something a person can act on.
fn classify(error: zbus::Error, name: &str) -> crate::CliError {
    match &error {
        zbus::Error::MethodError(err_name, _, _)
            if err_name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" =>
        {
            crate::CliError::DaemonUnavailable(format!(
                "no daemon is running on {name}, and the session bus does not know \
                 how to start one.\nInstall the service units (`just install-units` \
                 for a development build), or start the daemon yourself."
            ))
        }
        _ => crate::CliError::Bus(error.to_string()),
    }
}
