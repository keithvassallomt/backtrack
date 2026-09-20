// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The widgets, and the handful of things that apply to all of them.

pub mod breadcrumb;
pub mod calendar;
pub mod files;
pub mod menu;
pub mod preview;
pub mod sidebar;
pub mod strip;

use gtk4::prelude::*;
use gtk4::{gdk, glib};
use libadwaita as adw;
use tracing::{debug, warn};

/// Load the application stylesheet.
///
/// It is compiled into the binary rather than installed as a data file: there
/// is one stylesheet, it is small, and a missing one would leave the window
/// looking broken in a way that is tedious to diagnose.
pub fn install_stylesheet() {
    let provider = gtk4::CssProvider::new();
    provider.load_from_string(include_str!("../style.css"));
    let Some(display) = gdk::Display::default() else {
        warn!("no display, so the stylesheet was not installed");
        return;
    };
    gtk4::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// Honour `BACKTRACK_THEME=dark|light`, which forces the colour scheme for one
/// run.
///
/// The window has to be right in both themes, and checking that should not mean
/// changing the whole desktop's appearance and back again. Unset — which is
/// every real run — the system's own preference is followed, as it must be.
pub fn apply_theme_override() {
    let Some(choice) = std::env::var_os("BACKTRACK_THEME") else {
        return;
    };
    let scheme = match choice.to_string_lossy().to_ascii_lowercase().as_str() {
        "dark" => adw::ColorScheme::ForceDark,
        "light" => adw::ColorScheme::ForceLight,
        other => {
            warn!(value = other, "BACKTRACK_THEME must be 'dark' or 'light'");
            return;
        }
    };
    debug!(?scheme, "colour scheme forced for this run");
    adw::StyleManager::default().set_color_scheme(scheme);
}

/// The application icon if it is installed, and a stock stand-in if it is not.
///
/// Packaging installs the real icon (Stage 13); a development build run
/// straight out of the tree has no icon theme entry for it, and an unresolvable
/// icon name renders as a broken image.
pub fn app_icon_name(widget: &impl IsA<gtk4::Widget>) -> &'static str {
    let installed = gdk::Display::default()
        .map(|display| gtk4::IconTheme::for_display(&display))
        .is_some_and(|theme| theme.has_icon(crate::APP_ID));
    let _ = widget;
    if installed {
        crate::APP_ID
    } else {
        "document-open-recent-symbolic"
    }
}

/// Run `task` on the main loop. A thin alias so the intent reads at the call
/// site: this is work that must not block the frame, not a background thread.
pub fn spawn(task: impl std::future::Future<Output = ()> + 'static) {
    glib::spawn_future_local(task);
}
