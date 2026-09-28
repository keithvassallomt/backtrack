// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The fixes: one for each problem in health.md's failure catalogue, opened
//! from a banner's Fix…, from a notification, or from `--fix`.
//!
//! health.md's third principle is that every alert names its fix. These are
//! the fixes. Each one does the work through the daemon and then leaves the
//! banner to say whether it worked: the daemon clears a problem when it has
//! proof it is gone, and the window believes the daemon.

mod passphrase;
mod repair;
mod signin;
mod space;

use backtrack_core::config::Config;
use backtrack_core::dbus::Reason;
use futures::StreamExt;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::warn;

use crate::daemon::Daemon1Proxy;
use crate::model::health;

/// What every fix needs: the window it belongs to, the daemon that carries it
/// out, and somewhere to say how it went.
#[derive(Clone)]
pub struct Context {
    pub app: adw::Application,
    pub window: adw::ApplicationWindow,
    pub daemon: Daemon1Proxy<'static>,
    pub toasts: adw::ToastOverlay,
}

impl Context {
    fn toast(&self, text: &str) {
        self.toasts.add_toast(crate::ui::toast(text));
    }

    /// Back up now, through the window's own action, which says how it went.
    fn back_up(&self) {
        let _ = WidgetExt::activate_action(&self.window, "win.backup-now", None);
    }

    /// The configuration as the daemon holds it.
    async fn config(&self) -> Option<Config> {
        let text = self
            .daemon
            .get_config()
            .await
            .map_err(|error| warn!(%error, "the settings could not be read"))
            .ok()?;
        Config::parse(&text).map(|(config, _)| config).ok()
    }

    /// The welcome wizard, over this window, starting at `step`.
    async fn wizard(&self, step: &'static str) {
        let Some(config) = self.config().await else {
            return;
        };
        crate::ui::wizard::present(
            &self.app,
            self.daemon.clone(),
            Some(config),
            Some(self.window.upcast_ref()),
            Some(step),
        );
    }
}

/// Open the fix for `reason`.
pub fn open(context: &Context, reason: Reason) {
    match reason {
        Reason::PassphraseMissing => passphrase::present(context, passphrase::Why::Missing),
        Reason::PassphraseWrong => passphrase::present(context, passphrase::Why::Wrong),
        Reason::AuthFailed => signin::present(context),
        Reason::DestinationFull => space::destination(context),
        Reason::LocalDiskFull => space::local(context),
        Reason::RepoCorrupt => repair::present(context),
        Reason::BorgMissing => engine_missing(context),
        Reason::NoRecentBackup => at_risk(context),
        Reason::NotYetBrowsable => not_browsable(context),
        // Waited for, or not problems at all: nothing to open.
        Reason::CatalogueRebuilding | Reason::DestinationAway | Reason::Paused => {}
    }
}

/// Start a job and wait for it to end, answering whether it completed.
///
/// Listening begins before the job is asked for, so a quick one cannot finish
/// unheard.
async fn finish<F>(daemon: &Daemon1Proxy<'static>, start: F) -> zbus::Result<bool>
where
    F: std::future::Future<Output = zbus::Result<u64>>,
{
    let mut finished = daemon.receive_job_finished().await?;
    let job = start.await?;
    while let Some(signal) = finished.next().await {
        let Ok(args) = signal.args() else { continue };
        if args.job == job {
            return Ok(args.outcome == "completed");
        }
    }
    Ok(false)
}

/// The last component of a named D-Bus error, which is the one the daemon
/// chose: `PassphraseWrong`, `RepoUnreachable`.
fn error_name(error: &zbus::Error) -> &str {
    match error {
        zbus::Error::MethodError(name, _, _) => name.as_str().rsplit('.').next().unwrap_or(""),
        _ => "",
    }
}

/// An alert with a Cancel and one way forward.
fn alert(heading: &str, body: &str, go: &str, suggested: bool) -> adw::AlertDialog {
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(body)
        .default_response("go")
        .close_response("cancel")
        .build();
    dialog.add_responses(&[("cancel", "Cancel"), ("go", go)]);
    if suggested {
        dialog.set_response_appearance("go", adw::ResponseAppearance::Suggested);
    }
    crate::ui::prefer_wide_responses(&dialog);
    dialog
}

/// health.md's "Backtrack's backup engine is missing.": the install command
/// for this distribution, and a way to look again once it is installed.
fn engine_missing(context: &Context) {
    let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let how = match health::install_command(&release) {
        Some(command) => format!("Install it with:\n\n{command}"),
        None => "Install BorgBackup with your distribution's software tool.".to_string(),
    };
    let heading = backtrack_core::engine::HealthFailure::BorgMissing.copy("");
    let dialog = alert(
        &heading,
        &format!(
            "Backtrack backs up with BorgBackup, which is not installed on this computer or \
             is older than version 1.2. {how}"
        ),
        "Check Again",
        true,
    );
    let backer = context.clone();
    dialog.connect_response(Some("go"), move |_, _| backer.back_up());
    dialog.present(Some(&context.window));
}

/// `AT_RISK`: what is known about why nothing has been backed up, and a way to
/// try now.
fn at_risk(context: &Context) {
    let context = context.clone();
    crate::ui::spawn(async move {
        let (Ok(status), report) = (
            context.daemon.get_status().await,
            context.daemon.get_health().await,
        ) else {
            return;
        };
        let last_error = report.ok().and_then(|report| {
            report
                .errors
                .into_iter()
                .find(|(subsystem, ..)| subsystem == "backup")
                .map(|(_, at, _, message)| (at as i64, message))
        });
        let now = gtk4::glib::DateTime::now_utc()
            .map(|d| d.to_unix())
            .unwrap_or(0);
        let detail = health::at_risk_detail(
            &status,
            last_error.as_ref().map(|(at, m)| (*at, m.as_str())),
            now,
            &gtk4::glib::TimeZone::local(),
        );
        let dialog = alert(&health::at_risk(&status, now), &detail, "Back Up Now", true);
        let backer = context.clone();
        dialog.connect_response(Some("go"), move |_, _| backer.back_up());
        dialog.present(Some(&context.window));
    });
}

/// "Snapshot taken but indexing failed": the catalogue retries by itself, and
/// this is the manual fallback health.md asks for.
fn not_browsable(context: &Context) {
    let dialog = alert(
        "Rebuild the Catalogue?",
        "Some backups are safely stored but cannot be browsed yet, because reading their \
         file lists did not finish. Backtrack keeps trying by itself. Rebuilding reads \
         every backup's file list again, which can take a while.",
        "Rebuild",
        true,
    );
    let context_for_response = context.clone();
    dialog.connect_response(Some("go"), move |_, _| {
        let context = context_for_response.clone();
        crate::ui::spawn(async move {
            let daemon = context.daemon.clone();
            context.toast("Rebuilding the catalogue…");
            match finish(&daemon, daemon.rebuild_catalogue()).await {
                Ok(true) => context.toast("The catalogue has been rebuilt"),
                Ok(false) => context.toast("The catalogue could not be rebuilt. The logs say why."),
                Err(error) => context.toast(&crate::ui::wizard::explain(&error)),
            }
        });
    });
    dialog.present(Some(&context.window));
}
