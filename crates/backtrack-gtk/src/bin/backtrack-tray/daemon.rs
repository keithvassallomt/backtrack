// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The tray's side of `tools.backtrack.Backtrack.Daemon1`: the calls its menu makes and
//! the signals that say the status line is out of date.

use backtrack_core::dbus::Status;
use tracing::debug;
use zbus::proxy::{CacheProperties, MethodFlags};

#[zbus::proxy(
    interface = "tools.backtrack.Backtrack.Daemon1",
    default_path = "/tools/backtrack/Backtrack/Daemon1"
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

/// How far the backup under way has got, in words, or `None` with none under
/// way or no daemon to ask. Asked without starting it, as [`status`] is.
pub async fn progress(daemon: &Daemon1Proxy<'_>) -> Option<String> {
    match daemon
        .inner()
        .call_with_flags::<_, _, (u64, String, u64, u64)>(
            "GetBackupProgress",
            MethodFlags::NoAutoStart.into(),
            &(),
        )
        .await
    {
        Ok(Some((job, phase, current, total))) if job != 0 => {
            Some(backtrack_core::progress::line(&phase, current, total))
        }
        Ok(_) => None,
        Err(error) => {
            debug!(%error, "the daemon did not say how far the backup has got");
            None
        }
    }
}

/// What the daemon said when asked how it is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Status(Status),
    /// Nothing: it is not running.
    NotRunning,
    /// A status this tray cannot read: the daemon is of another version, as
    /// in the moments after an upgrade, before the older of the two has
    /// restarted (S18-T13).
    Unreadable,
}

impl Answer {
    pub fn status(&self) -> Option<&Status> {
        match self {
            Answer::Status(status) => Some(status),
            Answer::NotRunning | Answer::Unreadable => None,
        }
    }
}

/// What the daemon says.
///
/// Asked without starting it. With "Run in background" off, the daemon leaves
/// once nobody needs it, and a tray that started it again to ask how it was
/// doing would keep it running all day on the tray's account. The menu's own
/// actions do start it: those are somebody asking for a backup.
pub async fn status(daemon: &Daemon1Proxy<'_>) -> Answer {
    answer_of(
        daemon
            .inner()
            .call_with_flags::<_, _, Status>("GetStatus", MethodFlags::NoAutoStart.into(), &())
            .await,
    )
}

/// A status that came back in another shape is one from another version;
/// any other failure is a daemon that did not answer.
fn answer_of(reply: zbus::Result<Option<Status>>) -> Answer {
    match reply {
        Ok(Some(status)) => Answer::Status(status),
        Ok(None) => Answer::NotRunning,
        Err(error @ zbus::Error::Variant(_)) => {
            debug!(%error, "the daemon's status could not be read");
            Answer::Unreadable
        }
        Err(error) => {
            debug!(%error, "the daemon did not answer");
            Answer::NotRunning
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_from_another_version_is_told_from_no_daemon() {
        // The daemon's status before S18-T10 added `held_up` to it, as a
        // daemon of that version still running after an upgrade sends it.
        let call = zbus::Message::method_call("/tools/backtrack/Backtrack/Daemon1", "GetStatus")
            .unwrap()
            .build(&())
            .unwrap();
        let older = (
            "PROTECTED",
            1u64,
            2u64,
            true,
            0u64,
            "spool",
            0u32,
            0u32,
            0u64,
            0u64,
            true,
            "",
            0u64,
            0u64,
        );
        let reply = zbus::Message::method_return(&call.header())
            .unwrap()
            .build(&(older,))
            .unwrap();
        let read = reply.body().deserialize::<Status>().map(Some);
        assert_eq!(answer_of(read), Answer::Unreadable);
        assert_eq!(
            answer_of(Err(zbus::Error::InterfaceNotFound)),
            Answer::NotRunning
        );
    }
}
