// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Backtrack's window: browse the backups, step through time, restore.
//!
//! The launch contract is the one the file-manager plugins use:
//!
//! ```text
//! backtrack-gtk [--path DIR] [--select FILE]
//! ```
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
const APP_ID: &str = "io.github.keithvassallomt.Backtrack";

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

    app.connect_startup(|_| ui::install_stylesheet());

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

            match app.active_window() {
                Some(existing) => {
                    // Launched again with nothing to show: bring the window
                    // forward as it is, rather than moving it somewhere.
                    if let Some(target) = &target {
                        window::retarget(&existing, target);
                    }
                    existing.present();
                }
                None => open_first_window(app, target),
            }
            glib::ExitCode::SUCCESS
        },
    );

    // The user's own argv, forwarded to whichever instance is primary: that is
    // what lets a second `--path` re-point the window already on screen.
    app.run_with_args(&std::env::args().collect::<Vec<String>>())
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
fn open_first_window(app: &adw::Application, target: Option<Target>) {
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
        match (daemon, status) {
            (Some(daemon), Some(status)) if !status.configured => {
                info!("nothing is set up yet; opening the welcome wizard");
                ui::wizard::present(&app, daemon, None, None, None);
            }
            _ => {
                let target = match target {
                    Some(target) => target,
                    None => window::backed_up_target().await,
                };
                window::Window::build(&app, &target).present();
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
        }
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
