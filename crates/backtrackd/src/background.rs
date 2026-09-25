// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Preferences → General → "Run in background".
//!
//! The setting promises that hourly backups continue while the window is
//! closed, so it has two halves. Switched on, the daemon starts at login: the
//! systemd user unit is enabled. Switched off, the unit is disabled, and a
//! daemon that something else started (the window, the command line) leaves
//! once nothing needs it, rather than carrying on backing up until logout,
//! which is the thing the person switched off.
//!
//! Neither half is allowed to fail anything else. Under Flatpak there is no
//! unit to enable (the Background portal is Stage 12's), and a desktop without
//! a systemd user manager simply keeps the old behaviour: started on demand.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tracing::{debug, info, warn};

use crate::service::Shared;

/// The unit that `packaging/systemd` installs, by file name.
const UNIT: &str = "backtrackd.service";

/// The application's own bus name. GApplication claims it for as long as a
/// Backtrack window process is running, which makes it the cheapest honest
/// answer to "is anybody looking at this?".
const APP_BUS_NAME: &str = backtrack_core::secret::APP_ID;

/// How often a daemon that is not meant to run in the background checks
/// whether it is still needed. A minute: exiting a little late costs nothing,
/// and exiting while somebody is still reading the window would restart the
/// daemon on their next click.
const IDLE_CHECK: Duration = Duration::from_secs(60);

/// What systemd reports it changed: kind, symlink, target, per file.
type UnitChanges = Vec<(String, String, String)>;

#[zbus::proxy(
    interface = "org.freedesktop.systemd1.Manager",
    default_service = "org.freedesktop.systemd1",
    default_path = "/org/freedesktop/systemd1"
)]
trait Manager {
    fn get_unit_file_state(&self, file: &str) -> zbus::Result<String>;
    fn enable_unit_files(
        &self,
        files: &[&str],
        runtime: bool,
        force: bool,
    ) -> zbus::Result<(bool, UnitChanges)>;
    fn disable_unit_files(&self, files: &[&str], runtime: bool) -> zbus::Result<UnitChanges>;
    fn reload(&self) -> zbus::Result<()>;
}

/// How long systemd gets to answer. It answers in milliseconds when it is
/// there at all; this bounds the case where something else is on the other
/// end of the connection and never will.
const PATIENCE: Duration = Duration::from_secs(5);

/// Enable or disable starting at login, to match `wanted`.
///
/// Compared against what systemd reports rather than applied blindly, so the
/// common case (nothing has changed) writes nothing, and a unit the user has
/// deliberately masked is reported rather than fought with.
pub async fn apply(connection: &zbus::Connection, wanted: bool) {
    if tokio::time::timeout(PATIENCE, reconcile(connection, wanted))
        .await
        .is_err()
    {
        debug!("systemd did not answer in time; starting at login left as it was");
    }
}

async fn reconcile(connection: &zbus::Connection, wanted: bool) {
    let manager = match ManagerProxy::new(connection).await {
        Ok(manager) => manager,
        Err(error) => {
            debug!(%error, "no systemd user manager to ask");
            return;
        }
    };
    let state = match manager.get_unit_file_state(UNIT).await {
        Ok(state) => state,
        Err(error) => {
            // Not installed as a unit at all: a Flatpak, or a development
            // build nobody ran `just install-units` for.
            debug!(%error, unit = UNIT, "starting at login is not managed here");
            return;
        }
    };
    if (state == "enabled") == wanted {
        return;
    }
    if wanted && state != "disabled" {
        // "masked", "static", "linked": somebody has made a decision about
        // this unit by hand, and it is theirs.
        warn!(
            unit = UNIT,
            state, "not enabling the daemon at login: the unit's state was set by hand"
        );
        return;
    }

    let changed = if wanted {
        manager
            .enable_unit_files(&[UNIT], false, false)
            .await
            .map(|_| ())
    } else {
        manager.disable_unit_files(&[UNIT], false).await.map(|_| ())
    };
    match changed {
        Ok(()) => {
            // What `systemctl enable` does next, so the manager's idea of the
            // unit agrees with the files it just wrote.
            if let Err(error) = manager.reload().await {
                debug!(%error, "systemd did not reload after the unit changed");
            }
            info!(
                unit = UNIT,
                at_login = wanted,
                "changed whether backups start at login"
            );
        }
        Err(error) => warn!(%error, unit = UNIT, "could not change starting at login"),
    }
}

/// Whether a daemon that is not wanted in the background should leave now.
///
/// It stays while a window is open, while any job is running or queued, and
/// until it has been idle for two checks in a row: long enough that a window
/// closed and reopened a moment later does not cost a restart.
pub fn should_leave(
    run_in_background: bool,
    window_open: bool,
    busy: bool,
    idle_checks: u32,
) -> bool {
    !run_in_background && !window_open && !busy && idle_checks >= 2
}

/// Watch for the moment a daemon that is not wanted in the background stops
/// being needed, and say so through `leave`.
pub async fn watch_for_idle(shared: Arc<Shared>, connection: zbus::Connection, leave: Arc<Notify>) {
    let dbus = match zbus::fdo::DBusProxy::new(&connection).await {
        Ok(dbus) => dbus,
        Err(error) => {
            debug!(%error, "cannot ask the bus who is listening; staying up");
            return;
        }
    };
    let mut idle_checks = 0u32;
    loop {
        tokio::time::sleep(IDLE_CHECK).await;
        let run_in_background = shared.run_in_background();
        let window_open = match APP_BUS_NAME.try_into() {
            Ok(name) => dbus.name_has_owner(name).await.unwrap_or(true),
            Err(_) => true,
        };
        let busy = !shared.idle();
        idle_checks = if run_in_background || window_open || busy {
            0
        } else {
            idle_checks + 1
        };
        if should_leave(run_in_background, window_open, busy, idle_checks) {
            info!("backups are not set to run in the background and nothing needs them; leaving");
            leave.notify_one();
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_in_background_never_leaves() {
        assert!(!should_leave(true, false, false, 100));
    }

    #[test]
    fn an_open_window_or_a_running_job_keeps_it() {
        assert!(!should_leave(false, true, false, 100));
        assert!(
            !should_leave(false, false, true, 100),
            "leaving mid-backup would abandon the backup"
        );
    }

    #[test]
    fn it_leaves_only_after_being_idle_for_a_while() {
        assert!(!should_leave(false, false, false, 0));
        assert!(!should_leave(false, false, false, 1));
        assert!(should_leave(false, false, false, 2));
    }
}
