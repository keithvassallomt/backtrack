// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The widgets, and the handful of things that apply to all of them.

pub mod breadcrumb;
pub mod calendar;
pub mod conflict;
pub mod files;
pub mod menu;
pub mod preview;
pub mod restore;
pub mod sidebar;
pub mod strip;
pub mod summary;

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

/// Which colour scheme `BACKTRACK_THEME` asks for.
///
/// Unset — which is every real run — means the system's own preference, as it
/// must. An unrecognised value means the same, loudly: silently keeping the
/// last launch's scheme would be worse than ignoring a typo.
pub fn color_scheme_for(value: Option<&str>) -> adw::ColorScheme {
    match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        None | Some("") => adw::ColorScheme::Default,
        Some("dark") => adw::ColorScheme::ForceDark,
        Some("light") => adw::ColorScheme::ForceLight,
        Some(other) => {
            warn!(value = other, "BACKTRACK_THEME must be 'dark' or 'light'");
            adw::ColorScheme::Default
        }
    }
}

/// Apply `BACKTRACK_THEME` for this launch.
///
/// This sets libadwaita's colour *scheme*. It does not, and cannot, decide the
/// colours: a `~/.config/gtk-4.0/gtk.css` that redefines the libadwaita
/// palette — a common desktop customisation — loads above the theme stylesheet
/// and wins, so the window stays whatever colour that file says whichever
/// scheme is selected. That is correct behaviour, and it is why Backtrack's
/// own stylesheet names colours rather than spelling them out. To see the
/// window in stock Adwaita, run it with `XDG_CONFIG_HOME` pointed somewhere
/// empty; `just theme-check` does that.
///
/// Read per launch rather than once at startup, and from the environment of
/// the process that was *run* rather than this one's. The window is
/// single-instance: launching it a second time hands the arguments to the
/// instance already open and never reaches `startup`, so a scheme applied
/// there could only ever be the first launch's. Checking the two themes means
/// running the command twice, which is exactly the case that would not work.
pub fn apply_theme_override(from: Option<&str>) {
    let scheme = color_scheme_for(from);
    let manager = adw::StyleManager::default();
    manager.set_color_scheme(scheme);
    // The resulting `dark`, not just the scheme asked for: the two are not the
    // same question, and only the second one is the answer.
    debug!(
        ?scheme,
        dark = manager.is_dark(),
        "colour scheme for this launch"
    );

    // Forcing a scheme is a request to *see* it. If something else will decide
    // the colours, that is worth a word.
    if scheme != adw::ColorScheme::Default {
        warn_if_outranked();
    }
}

/// The libadwaita colours the window is built on. A user stylesheet that
/// redefines any of these outranks whatever colour scheme is selected.
const PALETTE: &[&str] = &[
    "window_bg_color",
    "view_bg_color",
    "card_bg_color",
    "headerbar_bg_color",
];

/// Say so when the scheme just forced will not be what appears on screen.
///
/// Silence is the problem being solved here. The scheme changes, the window
/// does not, and there is nothing anywhere to suggest why — which is a long
/// way to travel before arriving at "GTK loads the user's stylesheet above the
/// theme's".
fn warn_if_outranked() {
    let path = user_stylesheet();
    let Ok(stylesheet) = std::fs::read_to_string(&path) else {
        return;
    };
    if !overrides_palette(&stylesheet) {
        return;
    }
    warn!(
        stylesheet = %path.display(),
        "the forced colour scheme will not be visible: this stylesheet redefines the \
         libadwaita palette, and GTK loads it above the theme's. `just run-app` with \
         BACKTRACK_THEME set runs with an empty XDG_CONFIG_HOME to get around it."
    );
}

/// Where GTK looks for the user's own stylesheet.
fn user_stylesheet() -> std::path::PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| glib::home_dir().join(".config"))
        .join("gtk-4.0")
        .join("gtk.css")
}

/// Whether a stylesheet redefines any of the colours the window is built on.
fn overrides_palette(stylesheet: &str) -> bool {
    stylesheet.lines().any(|line| {
        line.trim()
            .strip_prefix("@define-color")
            .and_then(|rest| rest.split_whitespace().next())
            .is_some_and(|name| PALETTE.contains(&name))
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_override_reads_what_it_is_given() {
        assert_eq!(color_scheme_for(Some("dark")), adw::ColorScheme::ForceDark);
        assert_eq!(
            color_scheme_for(Some("light")),
            adw::ColorScheme::ForceLight
        );
        // Case and stray whitespace come with copying a command out of a README.
        assert_eq!(
            color_scheme_for(Some(" Dark ")),
            adw::ColorScheme::ForceDark
        );
    }

    #[test]
    fn no_override_follows_the_system() {
        assert_eq!(color_scheme_for(None), adw::ColorScheme::Default);
        assert_eq!(color_scheme_for(Some("")), adw::ColorScheme::Default);
    }

    #[test]
    fn a_stylesheet_that_repaints_the_palette_is_recognised() {
        assert!(overrides_palette("@define-color window_bg_color #0f0d12;"));
        assert!(overrides_palette(
            "  @define-color card_bg_color  @window_bg_color;"
        ));
    }

    #[test]
    fn a_stylesheet_that_leaves_the_palette_alone_is_not_flagged() {
        // Plenty of people customise GTK without touching these.
        assert!(!overrides_palette("headerbar { min-height: 32px; }"));
        assert!(!overrides_palette("@define-color my_own_color #123456;"));
        // A longer name that merely starts like one of ours.
        assert!(!overrides_palette(
            "@define-color window_bg_color_alt #123456;"
        ));
        assert!(!overrides_palette(""));
    }

    #[test]
    fn a_typo_falls_back_rather_than_keeping_the_last_launch_s_scheme() {
        // The bug this whole function exists to avoid is a second launch
        // silently showing the first launch's theme.
        assert_eq!(color_scheme_for(Some("drak")), adw::ColorScheme::Default);
    }
}
