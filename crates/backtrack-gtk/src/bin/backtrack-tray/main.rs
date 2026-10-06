// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Backtrack's icon in the system tray, for the desktops that have one:
//! Plasma, Xfce, Cinnamon and the rest. GNOME has no tray, and the autostart
//! file leaves it out; there, quick settings' background apps list Backtrack
//! instead.
//!
//! The icon follows the health state, and its menu says how backups are doing
//! and offers Back Up Now, the pauses, and the window. It talks to the daemon
//! over D-Bus and to nothing else.
//!
//! A program of its own rather than a mode of the window, because it runs for
//! the whole session: started, this release build peaks at 9 MB, and the
//! window's, which links GTK, at 60 to 95 MB before it opens anything
//! (measured on Arch, 2026-10-06). It shares two GLib modules with the
//! window, included by path, and links GLib and no GTK.

#[path = "../../model/format.rs"]
// The window uses more of it than the tray does.
#[allow(dead_code)]
mod format;
#[path = "../../model/pause.rs"]
mod pause;

mod daemon;
mod text;
mod tray;

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::Parser;
use futures::stream::{self, StreamExt};
use ksni::TrayMethods;
use tokio::sync::Notify;
use tracing::{error, info, warn};
use zbus::fdo::{RequestNameFlags, RequestNameReply};

#[derive(Debug, Parser)]
#[command(
    name = "backtrack-tray",
    version,
    about = "Show how Backtrack's backups are doing in the system tray."
)]
struct Args {}

/// Claimed on the session bus so that a second copy started in the same
/// session leaves, rather than adding a second icon.
fn bus_name() -> &'static str {
    if std::env::var_os("BACKTRACK_DEV").is_some() {
        "org.backtrack.Tray.Dev"
    } else {
        "org.backtrack.Tray"
    }
}

fn main() -> ExitCode {
    let _log_guard = backtrack_core::logging::init("backtrack-tray");
    let _args = Args::parse();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            error!(%error, "the tray's runtime could not be started");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(run())
}

async fn run() -> ExitCode {
    let connection = match zbus::Connection::session().await {
        Ok(connection) => connection,
        Err(error) => {
            error!(%error, "the session bus could not be reached");
            return ExitCode::FAILURE;
        }
    };
    match connection
        .request_name_with_flags(bus_name(), RequestNameFlags::DoNotQueue.into())
        .await
    {
        Ok(RequestNameReply::PrimaryOwner) => {}
        Ok(_) | Err(zbus::Error::NameTaken) => {
            info!("the tray icon is already showing");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            error!(%error, "the tray's bus name could not be claimed");
            return ExitCode::FAILURE;
        }
    }
    let daemon = match daemon::proxy(&connection).await {
        Ok(daemon) => daemon,
        Err(error) => {
            error!(%error, "the daemon's interface could not be set up");
            return ExitCode::FAILURE;
        }
    };

    let refresh = Arc::new(Notify::new());
    let quit = Arc::new(Notify::new());
    let tray = tray::Tray::new(daemon.clone(), Arc::clone(&refresh), Arc::clone(&quit));
    // Started at login, the tray can be ready before the panel that shows it
    // is; it waits for the panel rather than giving up.
    let handle = match tray.assume_sni_available(true).spawn().await {
        Ok(handle) => handle,
        Err(error) => {
            error!(%error, "the tray icon could not be shown");
            return ExitCode::FAILURE;
        }
    };
    info!("tray icon showing");

    let busy = Arc::new(AtomicBool::new(false));
    tokio::spawn(follow(
        daemon.clone(),
        Arc::clone(&refresh),
        Arc::clone(&busy),
    ));
    refresh.notify_one();
    loop {
        tokio::select! {
            () = refresh.notified() => {
                let status = daemon::status(&daemon).await;
                busy.store(
                    status.as_ref().is_some_and(|status| status.active_job != 0),
                    Ordering::Relaxed,
                );
                if handle.update(|tray| tray.show(status)).await.is_none() {
                    break;
                }
            }
            () = quit.notified() => break,
        }
    }
    handle.shutdown().await;
    info!("tray icon closed");
    ExitCode::SUCCESS
}

/// Wake the tray whenever what it says may have changed: the health state,
/// a job ending, a backup starting (its first progress, while none was
/// known to be running), and the daemon starting or leaving. It starts at
/// login, perhaps after the tray, and with "Run in background" off it leaves
/// once nobody needs it.
async fn follow(
    daemon: daemon::Daemon1Proxy<'static>,
    refresh: Arc<Notify>,
    busy: Arc<AtomicBool>,
) {
    let streams = async {
        let changed = daemon.receive_status_changed().await?;
        let finished = daemon.receive_job_finished().await?;
        let progress = daemon.receive_backup_progress().await?;
        let owners = zbus::fdo::DBusProxy::new(daemon.inner().connection())
            .await?
            .receive_name_owner_changed_with_args(&[(0, backtrack_core::dbus::bus_name())])
            .await?;
        zbus::Result::Ok((changed, finished, progress, owners))
    };
    let (changed, finished, progress, owners) = match streams.await {
        Ok(streams) => streams,
        Err(error) => {
            warn!(%error, "the tray will not hear of changes, only read them when opened");
            return;
        }
    };
    let progress = progress.filter(move |_| std::future::ready(!busy.load(Ordering::Relaxed)));
    let mut events = stream::select_all([
        changed.map(drop).boxed(),
        finished.map(drop).boxed(),
        progress.map(drop).boxed(),
        owners.map(drop).boxed(),
    ]);
    while events.next().await.is_some() {
        refresh.notify_one();
    }
}
