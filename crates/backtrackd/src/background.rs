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
//! Neither half is allowed to fail anything else. A desktop without a systemd
//! user manager simply keeps the old behaviour: started on demand.
//!
//! Inside a Flatpak there is no unit to enable. There the Background portal
//! grants running in the background and, when wanted, starts the daemon at
//! login, and being a sandboxed app running without a window is also what
//! lists Backtrack under GNOME's quick settings → background apps: GNOME
//! lists only Flatpak instances there (xdg-desktop-portal 1.22,
//! `background.c`), so outside one Backtrack is never listed, whatever it
//! asks for.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tracing::{debug, info, warn};

use crate::service::Shared;

/// Inside a Flatpak, start the tray icon, on desktops that have a tray.
///
/// Packages start `backtrack-tray` at login from `/etc/xdg/autostart`, which
/// a Flatpak cannot export, and the Background portal gives an application one
/// autostart entry, which is this daemon's. So the daemon starts the tray,
/// except on GNOME, which has no tray (the rule the autostart file states with
/// `NotShowIn=GNOME;`). A tray already running holds its bus name and a
/// second one leaves, so a restarted daemon starting another changes nothing
/// (S14-T3).
pub fn start_tray() {
    if !ashpd::is_sandboxed() || on_gnome(std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref()) {
        return;
    }
    let tray = std::env::current_exe()
        .map(|daemon| daemon.with_file_name("backtrack-tray"))
        .unwrap_or_else(|_| "backtrack-tray".into());
    match tokio::process::Command::new(&tray).spawn() {
        Ok(mut child) => {
            info!(path = %tray.display(), "started the tray icon");
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
        }
        Err(error) => warn!(%error, path = %tray.display(), "the tray icon could not be started"),
    }
}

/// Inside a Flatpak, keep the line GNOME's Background Apps shows under
/// Backtrack in step with what the daemon is doing (S14-T10): the tray's
/// first line, sent through the Background portal's `SetStatus` whenever it
/// changes. Outside one there is nothing to send it to, since GNOME lists
/// only Flatpak instances there.
pub fn publish_status_line(shared: &Arc<Shared>) {
    if !ashpd::is_sandboxed() {
        return;
    }
    let mut line = shared.status_line();
    tokio::spawn(async move {
        let portal = match ashpd::desktop::background::BackgroundProxy::new().await {
            Ok(portal) => portal,
            Err(error) => {
                warn!(%error, "the Background portal is not there to show a status line");
                return;
            }
        };
        // Version 2 added SetStatus. An older portal shows no line, which is
        // all there is to it.
        if portal.version() < 2 {
            debug!(
                version = portal.version(),
                "the Background portal cannot show a status line"
            );
            return;
        }
        loop {
            let message = line.borrow_and_update().clone();
            if !message.is_empty() {
                let options =
                    ashpd::desktop::background::SetStatusOptions::default().set_message(&message);
                match portal.set_status(options).await {
                    Ok(()) => debug!(message, "status line sent to the Background portal"),
                    Err(error) => {
                        warn!(%error, "the Background portal did not take the status line")
                    }
                }
            }
            if line.changed().await.is_err() {
                return;
            }
        }
    });
}

/// Whether `XDG_CURRENT_DESKTOP` (a colon-separated list) names GNOME.
fn on_gnome(desktop: Option<&str>) -> bool {
    desktop.is_some_and(|desktop| {
        desktop
            .split(':')
            .any(|name| name.eq_ignore_ascii_case("GNOME"))
    })
}

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

/// What the Background portal shows when it asks whether Backtrack may run
/// in the background.
const REASON: &str = "Hourly backups";

/// Enable or disable starting at login, to match `wanted`.
///
/// Compared against what systemd reports rather than applied blindly, so the
/// common case (nothing has changed) writes nothing, and a unit the user has
/// deliberately masked is reported rather than fought with.
pub async fn apply(connection: &zbus::Connection, wanted: bool) {
    if ashpd::is_sandboxed() {
        // The portal may ask on screen first, and nothing that changed a
        // setting should wait for somebody to answer it.
        tokio::spawn(ask_portal(wanted));
        return;
    }
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

/// Take away the link that starts the daemon at login, once the unit it
/// points at has gone with the package.
///
/// The daemon made the link (`EnableUnitFiles`, when "Run in background" is
/// on), and an uninstall leaves it behind in the home folder, pointing at
/// nothing. Only a link that points at nothing is removed: one that still
/// works belongs to an install that is still there.
pub fn forget_login_link() {
    forget_login_link_in(&backtrack_core::paths::systemd_user_dir());
}

fn forget_login_link_in(systemd_user: &std::path::Path) {
    let link = systemd_user.join("default.target.wants").join(UNIT);
    let dangling = link
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
        && !link.exists();
    if !dangling {
        return;
    }
    match std::fs::remove_file(&link) {
        Ok(()) => info!(link = %link.display(), "removed the link that started backups at login"),
        Err(error) => warn!(%error, link = %link.display(), "the login link could not be removed"),
    }
}

/// Ask the Background portal to let the daemon run in the background, and to
/// start it at login if `wanted`. Asked every time, rather than compared
/// first as the unit is: the portal says what it granted, and asking again is
/// how a change of `wanted` reaches it.
async fn ask_portal(wanted: bool) {
    let answer = ashpd::desktop::background::Background::request()
        .reason(REASON)
        .auto_start(wanted)
        .command(["backtrackd", "--supervise"])
        .dbus_activatable(false)
        .send()
        .await
        .and_then(|request| request.response());
    match answer {
        Ok(granted) => info!(
            background = granted.run_in_background(),
            at_login = granted.auto_start(),
            "the Background portal answered"
        ),
        Err(error) => warn!(%error, "the Background portal did not answer"),
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
    fn only_a_login_link_that_points_at_nothing_is_taken_away() {
        let dir = tempfile::tempdir().unwrap();
        let wants = dir.path().join("default.target.wants");
        std::fs::create_dir_all(&wants).unwrap();
        let link = wants.join(UNIT);

        // The package's unit is still there: the link is an install's.
        let unit = dir.path().join("unit");
        std::fs::write(&unit, "[Unit]\n").unwrap();
        std::os::unix::fs::symlink(&unit, &link).unwrap();
        forget_login_link_in(dir.path());
        assert!(link.symlink_metadata().is_ok(), "a working link is kept");

        // The package removed: the link points at nothing.
        std::fs::remove_file(&unit).unwrap();
        forget_login_link_in(dir.path());
        assert!(
            link.symlink_metadata().is_err(),
            "a dangling link is removed"
        );

        // Not a link at all: somebody's own file, left alone.
        std::fs::write(&link, "mine").unwrap();
        forget_login_link_in(dir.path());
        assert!(link.exists(), "a plain file is not ours to remove");
    }

    #[test]
    fn the_tray_is_left_out_on_gnome_alone() {
        assert!(on_gnome(Some("GNOME")));
        assert!(on_gnome(Some("ubuntu:GNOME")));
        assert!(!on_gnome(Some("KDE")));
        assert!(!on_gnome(Some("Hyprland")));
        assert!(!on_gnome(None));
    }

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
