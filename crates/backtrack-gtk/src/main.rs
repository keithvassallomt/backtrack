// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Backtrack's window: browse the backups, step through time, restore.
//!
//! The launch contract is the one the file-manager plugins use:
//!
//! ```text
//! backtrack-gtk [--path DIR] [--select FILE] [--fix REASON]
//! ```
//!
//! `--fix` is how a notification opens the fix for what it was about: the
//! daemon sends the notification and, when it is clicked, runs this with the
//! health reason it named.
//!
//! Launching it again with a different `--path` re-points the window that is
//! already open rather than stacking up another one, because right-clicking
//! "Browse Backups of This Folder…" twice is a thing people do.

mod daemon;
mod index;
mod model;
mod path;
mod state;
mod ui;
mod window;

use std::path::PathBuf;

use clap::Parser;
use gtk4::gio::prelude::ApplicationCommandLineExt;
use gtk4::prelude::*;
use gtk4::{gio, glib};
use libadwaita as adw;
use tracing::{error, info, warn};

/// Freedesktop application ID (see stack.md / brief.md).
const APP_ID: &str = "tools.backtrack.Backtrack";

/// Where the window should open.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "backtrack-gtk",
    version,
    about = "Browse and restore your backups."
)]
struct Args {
    /// Folder to open the timeline on. Defaults to your home directory.
    #[arg(long, value_name = "DIR")]
    path: Option<PathBuf>,
    /// File to select once that folder is open.
    #[arg(long, value_name = "FILE")]
    select: Option<PathBuf>,
    /// Open the fix for a problem Backtrack has told you about.
    #[arg(long, value_name = "REASON")]
    fix: Option<String>,
    /// GApplication's own flag, passed when the session bus starts the window
    /// to answer an activation (S14-T2). GApplication acts on it; it is
    /// accepted here only so that parsing does not refuse it.
    #[arg(long = "gapplication-service", hide = true)]
    gapplication_service: bool,
}

/// What the window is being asked to show, in archive-relative paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub folder: String,
    pub select: Option<String>,
}

impl Target {
    /// Work out the folder and selection from the arguments, or `None` when
    /// they name neither and where to open is the catalogue's to say (see
    /// `window::backed_up_target`).
    ///
    /// `--select` naming a file implies its folder, so `--select` alone is
    /// enough: a file manager passing a file has already said where it is.
    fn resolve(args: &Args) -> Option<Target> {
        let select = args.select.as_deref().map(crate::path::to_archive);
        let folder = match (&args.path, &select) {
            (Some(dir), _) => crate::path::to_archive(dir),
            (None, Some(file)) => crate::path::parent(file).unwrap_or_default(),
            (None, None) => return None,
        };
        Some(Target { folder, select })
    }
}

fn main() -> glib::ExitCode {
    let _log_guard = backtrack_core::logging::init("backtrack-gtk");

    // zbus is built with its Tokio backend, because the keyring library the
    // daemon uses turns that on for the whole workspace. The window's own
    // concurrency is GLib's — every future here is polled by the main loop —
    // but zbus still opens its socket and spawns its internal tasks through
    // Tokio, and those panic outside a runtime. So one runtime is created and
    // entered for the life of the process: it drives nothing of ours, it is
    // simply the context zbus insists on.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            error!(%error, "the D-Bus client runtime could not be started");
            return glib::ExitCode::FAILURE;
        }
    };
    let _runtime_guard = runtime.enter();

    // Parsed here purely so that `--help`, `--version` and a mistyped flag are
    // answered by the process the user ran, on the terminal they ran it from,
    // without starting a window or waking the daemon. The primary instance
    // parses the arguments again — see `connect_command_line` — because with a
    // single-instance application they may have come from a different process
    // than this one.
    let _validated = Args::parse();

    info!(
        version = backtrack_core::VERSION,
        app_id = APP_ID,
        "backtrack-gtk starting"
    );

    let app = adw::Application::builder()
        .application_id(APP_ID)
        // SEND_ENVIRONMENT is what makes `command_line.getenv` answer for a
        // launch that was forwarded to an instance already running. Without it
        // GApplication sends only the arguments, and the environment a second
        // `BACKTRACK_THEME=…` was set in never reaches the process that would
        // act on it.
        .flags(
            gio::ApplicationFlags::HANDLES_COMMAND_LINE | gio::ApplicationFlags::SEND_ENVIRONMENT,
        )
        .build();

    app.connect_startup(|_| {
        ui::install_stylesheet();
        refresh_flatpak_plugins();
    });

    app.connect_command_line(
        |app: &adw::Application, command_line: &gio::ApplicationCommandLine| -> glib::ExitCode {
            let args = match Args::try_parse_from(command_line.arguments()) {
                Ok(args) => args,
                // Unreachable in practice: the process the user ran parsed the
                // same arguments before forwarding them, and clap reported the
                // problem there, on their terminal. Refusing quietly here is the
                // right answer to an invalid forward from anywhere else.
                Err(error) => {
                    warn!(%error, "ignoring a launch with arguments that do not parse");
                    return glib::ExitCode::FAILURE;
                }
            };
            // From the shell that ran this, not from the process that answers
            // it: whenever a window is already open those are two different
            // processes, and the one with the variable set is the first.
            ui::apply_theme_override(command_line.getenv("BACKTRACK_THEME").as_deref());

            let target = Target::resolve(&args);
            let fix = args.fix.as_deref().and_then(parse_fix);
            show(app, target, fix);
            glib::ExitCode::SUCCESS
        },
    );

    // The same requests over the session bus, for a caller with no window of
    // its own to forward a command line from: the daemon's notifications and
    // the tray icon inside the Flatpak (S14-T2). A bare activation opens the
    // window as a launch with no arguments does.
    app.connect_activate(|app| show(app, None, None));
    let fix = gio::SimpleAction::new(
        backtrack_core::dbus::APP_ACTION_FIX,
        Some(glib::VariantTy::STRING),
    );
    let fixer = app.clone();
    fix.connect_activate(move |_, parameter| {
        let token = parameter.and_then(|token| token.str()).unwrap_or_default();
        show(&fixer, None, parse_fix(token));
    });
    app.add_action(&fix);
    let browse = gio::SimpleAction::new(
        backtrack_core::dbus::APP_ACTION_BROWSE,
        Some(glib::VariantTy::new("(ss)").expect("a valid variant type")),
    );
    let browser = app.clone();
    browse.connect_activate(move |_, parameter| {
        let Some((path, select)) = parameter.and_then(|value| value.get::<(String, String)>())
        else {
            return;
        };
        let given = |path: String| (!path.is_empty()).then(|| PathBuf::from(path));
        let args = Args {
            path: given(path),
            select: given(select),
            fix: None,
            gapplication_service: false,
        };
        show(&browser, Target::resolve(&args), None);
    });
    app.add_action(&browse);

    // The user's own argv, forwarded to whichever instance is primary: that is
    // what lets a second `--path` re-point the window already on screen.
    app.run_with_args(&std::env::args().collect::<Vec<String>>())
}

/// Show `target` and the fix for `fix`: in the window already open, or in a
/// first one.
fn show(app: &adw::Application, target: Option<Target>, fix: Option<backtrack_core::dbus::Reason>) {
    match app.active_window() {
        Some(existing) => {
            // Launched again with nothing to show: bring the window forward as
            // it is, rather than moving it somewhere.
            if let Some(target) = &target {
                window::retarget(&existing, target);
            }
            existing.present();
            if let Some(reason) = fix {
                window::fix(&existing, reason);
            }
        }
        None => open_first_window(app, target, fix),
    }
}

/// Inside a Flatpak, bring the file-manager integrations it installed up to
/// this version, so an update to the Flatpak updates them too (S14-T4).
fn refresh_flatpak_plugins() {
    if !backtrack_core::host::in_flatpak() {
        return;
    }
    let home = backtrack_core::paths::host_data_home();
    let roots = backtrack_core::paths::roots_file();
    for plugin in model::prefs::Plugin::ALL {
        match plugin.refresh_flatpak_copy(&home, &roots) {
            Ok(true) => info!(?plugin, "brought the Flatpak's integration up to date"),
            Ok(false) => {}
            Err(error) => warn!(%error, ?plugin, "the Flatpak's integration could not be updated"),
        }
    }
}

/// The fix a `--fix` token or a `fix` action names, if this build knows it.
fn parse_fix(token: &str) -> Option<backtrack_core::dbus::Reason> {
    let reason = backtrack_core::dbus::Reason::parse(token);
    if reason.is_none() {
        warn!(token, "ignoring a fix this build does not know");
    }
    reason
}

/// Open the timeline, or the welcome wizard if nothing is set up yet.
///
/// With no `target`, the timeline opens over the top of the newest backup,
/// which for backups of the usual folders is the home folder anyway.
///
/// Asked of the daemon rather than read from `config.toml`: the daemon is what
/// backs up, so its answer is the one that counts. With no daemon to ask, the
/// timeline opens anyway. Browsing needs only the catalogue, and the wizard
/// could not create anything without the daemon behind it.
fn open_first_window(
    app: &adw::Application,
    target: Option<Target>,
    fix: Option<backtrack_core::dbus::Reason>,
) {
    // Nothing is on screen while the daemon is asked, and an application with
    // no window and nothing holding it would quit in the meantime.
    let hold = app.hold();
    let app = app.clone();
    glib::spawn_future_local(async move {
        let daemon = daemon::connect().await.ok();
        let status = match &daemon {
            Some(daemon) => daemon.get_status().await.ok(),
            None => None,
        };
        // A restore of this computer under way, or waiting for an answer, is
        // what a plain launch comes back to: the window that was watching it
        // may have been closed hours ago. A launch that asked for something
        // else still gets it.
        let recovering = match &daemon {
            Some(daemon) if target.is_none() && fix.is_none() => daemon
                .get_recovery()
                .await
                .ok()
                .filter(ui::everything::wants_attention),
            _ => None,
        };
        match (daemon, status) {
            (Some(daemon), Some(status)) if !status.configured => {
                info!("nothing is set up yet; opening the welcome wizard");
                ui::wizard::present(&app, daemon, None, None, None);
            }
            (Some(daemon), _) if recovering.is_some() => {
                info!("a restore of this computer is under way; reopening it");
                ui::everything::present(&app, daemon);
            }
            _ => {
                let target = match target {
                    Some(target) => target,
                    None => window::backed_up_target().await,
                };
                let window = window::Window::build(&app, &target);
                window.present();
                if let Some(reason) = fix {
                    window::fix(window.upcast_ref(), reason);
                }
            }
        }
        drop(hold);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(path: Option<&str>, select: Option<&str>) -> Args {
        Args {
            path: path.map(PathBuf::from),
            select: select.map(PathBuf::from),
            fix: None,
            gapplication_service: false,
        }
    }

    #[test]
    fn a_start_by_the_session_bus_is_not_refused() {
        // The session bus starts the window with GApplication's own flag; a
        // parse that refused it would exit before GApplication could answer
        // the activation that started it.
        let args = Args::try_parse_from(["backtrack-gtk", "--gapplication-service"]).unwrap();
        assert!(args.gapplication_service);
        assert_eq!(Target::resolve(&args), None);
    }

    #[test]
    fn a_notification_s_click_is_read_as_the_fix_it_names() {
        // The other half of the daemon's `notify::command`: what it passes is
        // what this parses, for every reason a notification can be about.
        for reason in backtrack_core::dbus::Reason::ALL {
            let parsed = Args::try_parse_from(["backtrack-gtk", "--fix", reason.as_str()]).unwrap();
            assert_eq!(
                parsed
                    .fix
                    .as_deref()
                    .and_then(backtrack_core::dbus::Reason::parse),
                Some(reason)
            );
        }
        let plain = Args::try_parse_from(["backtrack-gtk"]).unwrap();
        assert_eq!(plain.fix, None);
    }

    #[test]
    fn no_arguments_leave_it_to_the_catalogue() {
        // Not the home folder: Borg records nothing above the folders it was
        // asked for, so for a backup of one folder deep in the tree the home
        // folder is an empty view.
        assert_eq!(Target::resolve(&args(None, None)), None);
    }

    #[test]
    fn a_path_opens_that_folder() {
        let target = Target::resolve(&args(Some("/srv/data"), None)).unwrap();
        assert_eq!(target.folder, "srv/data");
    }

    #[test]
    fn a_selected_file_implies_the_folder_it_is_in() {
        // "Restore Previous Version…" on a file passes only the file.
        let target = Target::resolve(&args(None, Some("/home/keith/report.odt"))).unwrap();
        assert_eq!(target.folder, "home/keith");
        assert_eq!(target.select, Some("home/keith/report.odt".to_string()));
    }

    #[test]
    fn an_explicit_path_wins_over_the_selected_file_s_own_folder() {
        let target = Target::resolve(&args(
            Some("/home/keith/Documents"),
            Some("/home/keith/report.odt"),
        ))
        .unwrap();
        assert_eq!(target.folder, "home/keith/Documents");
        assert_eq!(target.select, Some("home/keith/report.odt".to_string()));
    }
}
