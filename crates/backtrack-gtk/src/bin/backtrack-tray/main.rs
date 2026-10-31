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
//! (measured on Arch, 2026-10-06). It shares the window's pause module,
//! included by path, and its status line with the daemon through
//! `backtrack_core::status_line`; it links GLib and no GTK.

#[path = "../../model/pause.rs"]
mod pause;

mod daemon;
mod icon;
mod text;
mod tray;

use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use backtrack_core::host::Replaced;

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
        "tools.backtrack.Backtrack.Tray.Dev"
    } else {
        "tools.backtrack.Backtrack.Tray"
    }
}

/// How the tray ends.
enum Ending {
    Exit(ExitCode),
    /// Backtrack has been upgraded: the new tray, `program`, takes over.
    Restart(PathBuf),
}

fn main() -> ExitCode {
    let log_guard = backtrack_core::logging::init("backtrack-tray");
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
    match runtime.block_on(run()) {
        Ending::Exit(code) => code,
        Ending::Restart(program) => {
            info!(program = %program.display(), "Backtrack has been upgraded; the tray starts afresh");
            drop(runtime);
            // The log file written out before the program is replaced; the
            // journal still hears of a failure to replace it.
            drop(log_guard);
            let error = std::process::Command::new(&program)
                .args(std::env::args_os().skip(1))
                .exec();
            error!(%error, "the upgraded tray could not be started");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Ending {
    let connection = match zbus::Connection::session().await {
        Ok(connection) => connection,
        Err(error) => {
            error!(%error, "the session bus could not be reached");
            return Ending::Exit(ExitCode::FAILURE);
        }
    };
    match connection
        .request_name_with_flags(bus_name(), RequestNameFlags::DoNotQueue.into())
        .await
    {
        Ok(RequestNameReply::PrimaryOwner) => {}
        Ok(_) | Err(zbus::Error::NameTaken) => {
            info!("the tray icon is already showing");
            return Ending::Exit(ExitCode::SUCCESS);
        }
        Err(error) => {
            error!(%error, "the tray's bus name could not be claimed");
            return Ending::Exit(ExitCode::FAILURE);
        }
    }
    let daemon = match daemon::proxy(&connection).await {
        Ok(daemon) => daemon,
        Err(error) => {
            error!(%error, "the daemon's interface could not be set up");
            return Ending::Exit(ExitCode::FAILURE);
        }
    };

    let refresh = Arc::new(Notify::new());
    let quit = Arc::new(Notify::new());
    let tray = tray::Tray::new(daemon.clone(), Arc::clone(&refresh), Arc::clone(&quit));
    // Started at login, the tray can be ready before the panel that shows it
    // is; it waits for the panel rather than giving up.
    // Inside a Flatpak, without the per-process `org.kde.StatusNotifierItem-…`
    // name, which the sandbox cannot own; ksni asks this of sandboxed programs,
    // and registers the icon by its unique name instead (S14-T3).
    let handle = match tray
        .assume_sni_available(true)
        .disable_dbus_name(backtrack_core::host::in_flatpak())
        .spawn()
        .await
    {
        Ok(handle) => handle,
        Err(error) => {
            error!(%error, "the tray icon could not be shown");
            return Ending::Exit(ExitCode::FAILURE);
        }
    };
    info!("tray icon showing");

    let busy = Arc::new(AtomicBool::new(false));
    // How far the backup under way has got, as its progress signals say it.
    let (lines, mut progress) = tokio::sync::watch::channel(None::<String>);
    tokio::spawn(follow(
        daemon.clone(),
        Arc::clone(&refresh),
        Arc::clone(&busy),
        lines,
    ));
    let replaced = Arc::new(Mutex::new(None));
    let program = std::env::current_exe().ok();
    if let Some(program) = &program {
        tokio::spawn(watch_program(
            program.clone(),
            Arc::clone(&replaced),
            Arc::clone(&quit),
        ));
    }
    refresh.notify_one();
    loop {
        tokio::select! {
            () = refresh.notified() => {
                let status = daemon::status(&daemon).await;
                let running = status.status().is_some_and(|status| status.active_job != 0);
                busy.store(running, Ordering::Relaxed);
                // Asked, not waited for: a tray started part way through a
                // backup, or a backup in a phase that says little, still
                // shows how far it has got.
                let progress = match running {
                    true => daemon::progress(&daemon).await,
                    false => None,
                };
                if handle.update(|tray| tray.show(status, progress)).await.is_none() {
                    break;
                }
            }
            Ok(()) = progress.changed() => {
                let line = progress.borrow_and_update().clone();
                if handle.update(|tray| tray.show_progress(line)).await.is_none() {
                    break;
                }
            }
            () = quit.notified() => break,
        }
    }
    handle.shutdown().await;
    info!("tray icon closed");
    let upgraded = *replaced.lock().unwrap() == Some(Replaced::Upgraded);
    match program {
        Some(program) if upgraded => {
            // Let go of first, or the new tray would find it taken and leave.
            if let Err(error) = connection.release_name(bus_name()).await {
                warn!(%error, "the tray's bus name could not be let go of");
            }
            if backtrack_core::host::in_flatpak() {
                // In a sandbox of its own: this one runs the old version.
                let args: Vec<String> = std::env::args().skip(1).collect();
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                match backtrack_core::upgrade::start_latest(&program, &args).await {
                    Ok(pid) => {
                        info!(
                            pid,
                            "the new version of the tray is starting; this one leaves"
                        );
                        return Ending::Exit(ExitCode::SUCCESS);
                    }
                    // This version again, rather than no icon.
                    Err(error) => warn!(%error, "the new version of the tray could not be started"),
                }
            }
            Ending::Restart(program)
        }
        _ => Ending::Exit(ExitCode::SUCCESS),
    }
}

/// Leave once `program`, this tray's own, is uninstalled or upgraded under
/// it, saying which in `replaced`.
///
/// Nothing else ends a tray left running by an uninstall. Its autostart file
/// goes with the package, and with it the unit that would have stopped the
/// tray with the session, so after logging out and straight back in the tray
/// ran on with no icon and no program behind it until the user's systemd
/// stopped. After an upgrade, it ran on as the old version and could not read
/// the new daemon's status (S18-T13): the new tray starts in its place.
async fn watch_program(
    program: PathBuf,
    replaced: Arc<Mutex<Option<Replaced>>>,
    quit: Arc<Notify>,
) {
    let what = backtrack_core::upgrade::replaced().await;
    if what == Replaced::Uninstalled {
        info!(
            program = %program.display(),
            "Backtrack has been uninstalled; the tray icon leaves with it"
        );
    }
    *replaced.lock().unwrap() = Some(what);
    quit.notify_one();
}

/// Wake the tray whenever what it says may have changed: the health state,
/// a job ending, a backup starting (its first progress, while none was
/// known to be running), and the daemon starting or leaving. It starts at
/// login, perhaps after the tray, and with "Run in background" off it leaves
/// once nobody needs it.
///
/// A backup's progress also goes to `lines`, in words (S06-T12): once a
/// second at most, since the menu and the tooltip are redrawn for each, and
/// at once when the backup moves to another phase.
async fn follow(
    daemon: daemon::Daemon1Proxy<'static>,
    refresh: Arc<Notify>,
    busy: Arc<AtomicBool>,
    lines: tokio::sync::watch::Sender<Option<String>>,
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
    let starting = Arc::clone(&refresh);
    tokio::spawn(async move {
        let mut progress = progress;
        let mut said: Option<(std::time::Instant, String)> = None;
        while let Some(signal) = progress.next().await {
            if !busy.load(Ordering::Relaxed) {
                // A backup has started that the tray did not know of.
                starting.notify_one();
            }
            let Ok(args) = signal.args() else { continue };
            let due = said.as_ref().is_none_or(|(at, phase)| {
                phase != args.phase || at.elapsed() >= std::time::Duration::from_secs(1)
            });
            if due {
                lines.send_replace(Some(backtrack_core::progress::line(
                    args.phase,
                    args.current,
                    args.total,
                )));
                said = Some((std::time::Instant::now(), args.phase.to_string()));
            }
        }
    });
    let mut events = stream::select_all([
        changed.map(drop).boxed(),
        finished.map(drop).boxed(),
        owners.map(drop).boxed(),
    ]);
    while events.next().await.is_some() {
        refresh.notify_one();
    }
}
