// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The icon and its menu.
//!
//! The menu is the window's primary menu cut down to what makes sense from a
//! panel: how backups are doing, Back Up Now, Pause Backups and Resume
//! Backups with the same choices, and a way into the window. Nothing in it
//! switches backups off, for the reason the window's menu has nothing either.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use backtrack_core::dbus::Status;
use ksni::menu::{MenuItem, StandardItem, SubMenu};
use tokio::sync::Notify;
use tracing::{info, warn};

use crate::daemon::Daemon1Proxy;
use crate::pause;
use crate::text::{self, Attention};

pub struct Tray {
    daemon: Daemon1Proxy<'static>,
    /// Woken to read the status again.
    refresh: Arc<Notify>,
    /// Woken to take the icon away.
    quit: Arc<Notify>,
    /// The status as last read; `None` while the daemon is not running.
    status: Option<Status>,
    icon: String,
}

impl Tray {
    pub fn new(daemon: Daemon1Proxy<'static>, refresh: Arc<Notify>, quit: Arc<Notify>) -> Tray {
        Tray {
            daemon,
            refresh,
            quit,
            status: None,
            icon: icon(),
        }
    }

    /// Show `status` from now on.
    pub fn show(&mut self, status: Option<Status>) {
        self.status = status;
    }

    fn shown(&self) -> text::Shown {
        let now = glib::DateTime::now_utc()
            .map(|now| now.to_unix())
            .unwrap_or(0);
        text::shown(self.status.as_ref(), now, &glib::TimeZone::local())
    }

    /// Ask the daemon for something, then read the status again so the menu
    /// says what came of it.
    fn ask<F, Fut>(&self, what: &'static str, call: F)
    where
        F: FnOnce(Daemon1Proxy<'static>) -> Fut + Send + 'static,
        Fut: Future<Output = zbus::Result<()>> + Send,
    {
        let daemon = self.daemon.clone();
        let refresh = Arc::clone(&self.refresh);
        tokio::spawn(async move {
            match call(daemon).await {
                Ok(()) => info!(what, "asked for from the tray"),
                Err(error) => warn!(%error, what, "the daemon refused what the tray asked for"),
            }
            refresh.notify_one();
        });
    }

    fn pause(&self, option: &'static str) {
        let now = glib::DateTime::now_utc()
            .map(|now| now.to_unix())
            .unwrap_or(0);
        let Some(seconds) = pause::duration(option, now, &glib::TimeZone::local()) else {
            return;
        };
        let until = now.max(0) as u64 + seconds;
        self.ask(
            "pause",
            move |daemon| async move { daemon.pause(until).await },
        );
    }
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        backtrack_core::secret::APP_ID.to_string()
    }

    fn title(&self) -> String {
        "Backtrack".to_string()
    }

    fn status(&self) -> ksni::Status {
        match self.shown().attention {
            Some(_) => ksni::Status::NeedsAttention,
            None => ksni::Status::Active,
        }
    }

    fn icon_name(&self) -> String {
        self.icon.clone()
    }

    /// What a panel shows instead of the icon while it needs attention.
    fn attention_icon_name(&self) -> String {
        match self.shown().attention {
            Some(Attention::Warning) => "dialog-warning",
            Some(Attention::Error) => "dialog-error",
            None => "",
        }
        .to_string()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Backtrack".to_string(),
            description: self.shown().line,
            ..Default::default()
        }
    }

    /// A click on the icon itself opens the window, where whatever the icon is
    /// drawing attention to has its banner and its fix.
    fn activate(&mut self, _x: i32, _y: i32) {
        open_window();
    }

    /// Opening the menu reads the status again, which costs one call and
    /// catches anything the signals did not say.
    fn menu_about_to_show(&mut self) {
        self.refresh.notify_one();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let shown = self.shown();
        let pauses = pause::OPTIONS
            .into_iter()
            .map(|(option, label)| {
                StandardItem {
                    label: label.to_string(),
                    activate: Box::new(move |tray: &mut Self| tray.pause(option)),
                    ..Default::default()
                }
                .into()
            })
            .collect();
        vec![
            StandardItem {
                label: shown.line,
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Back Up Now".to_string(),
                enabled: shown.can_back_up,
                activate: Box::new(|tray: &mut Self| {
                    tray.ask("backup", |daemon| async move {
                        daemon.backup_now().await.map(drop)
                    });
                }),
                ..Default::default()
            }
            .into(),
            SubMenu {
                label: "Pause Backups".to_string(),
                enabled: shown.can_pause,
                submenu: pauses,
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Resume Backups".to_string(),
                visible: shown.paused,
                activate: Box::new(|tray: &mut Self| {
                    tray.ask("resume", |daemon| async move { daemon.resume().await });
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Open Backtrack".to_string(),
                activate: Box::new(|_: &mut Self| open_window()),
                ..Default::default()
            }
            .into(),
            // Not "Quit": backups carry on without the icon, and the label
            // should not suggest otherwise.
            StandardItem {
                label: "Quit Tray Icon".to_string(),
                activate: Box::new(|tray: &mut Self| tray.quit.notify_one()),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// The app's icon where a package has installed it, and a stock one where it
/// has not, as for a development build: a panel draws an icon name it cannot
/// find as a broken image.
fn icon() -> String {
    let app = backtrack_core::secret::APP_ID;
    let installed = std::iter::once(glib::user_data_dir())
        .chain(glib::system_data_dirs())
        .any(|dir| {
            dir.join("icons/hicolor/scalable/apps")
                .join(format!("{app}.svg"))
                .is_file()
        });
    if installed {
        app.to_string()
    } else {
        "document-open-recent".to_string()
    }
}

/// Open Backtrack's window: the one beside this program, so that a
/// development build opens the development window, else the one on `PATH`.
/// The window is a single application, so this brings forward one that is
/// already open.
fn open_window() {
    let window = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("backtrack-gtk")))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("backtrack-gtk"));
    // Not waited for here; Tokio collects it when it exits.
    match tokio::process::Command::new(&window).spawn() {
        Ok(_) => info!(path = %window.display(), "opening the window from the tray"),
        Err(error) => warn!(%error, path = %window.display(), "the window could not be opened"),
    }
}
