// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The primary menu, and the actions behind it.
//!
//! Actions first, then the app-level entries, per the GNOME convention and
//! mockup 14. Pause offers only self-expiring options: there is no "off" in
//! here, deliberately — switching backups off permanently belongs in
//! Preferences where it has to be looked for, not one click from the header
//! bar where it can be done by accident and forgotten about.

use gtk4::prelude::*;
use gtk4::{gio, glib, MenuButton};
use libadwaita as adw;
use tracing::warn;

use crate::model::status::INDEFINITE_PAUSE_THRESHOLD;

/// How long "For 1 hour" actually lasts.
///
/// One minute under `BACKTRACK_DEV`, because a self-expiring pause that takes
/// an hour to expire cannot be tested in a working session, and an untested
/// expiry is one that does not work.
pub fn hour_pause() -> u64 {
    if std::env::var_os("BACKTRACK_DEV").is_some() {
        60
    } else {
        3_600
    }
}

/// Seconds from `now` until the pause each menu option asks for lifts.
///
/// "Until I resume" has no natural end, but the daemon's `Pause` takes a time
/// and refuses one in the past — by design, so that "off forever" cannot be
/// set from a menu. It is expressed here as a date far enough out that nothing
/// will reach it, and the status line recognises that and says "until you
/// resume" rather than reading the date back.
pub fn pause_duration(option: &str, now: i64, tz: &glib::TimeZone) -> Option<u64> {
    match option {
        "hour" => Some(hour_pause()),
        "tomorrow" => tomorrow_morning(now, tz).map(|until| (until - now).max(60) as u64),
        "indefinite" => Some(INDEFINITE_PAUSE_THRESHOLD * 100),
        _ => None,
    }
}

/// 09:00 tomorrow, local — the start of the next working day rather than
/// midnight, which is when "until tomorrow" would otherwise lift while nobody
/// is there to notice.
fn tomorrow_morning(now: i64, tz: &glib::TimeZone) -> Option<i64> {
    let today = glib::DateTime::from_unix_local(now)
        .ok()?
        .to_timezone(tz)
        .ok()?;
    let tomorrow = today.add_days(1).ok()?;
    glib::DateTime::new(
        tz,
        tomorrow.year(),
        tomorrow.month(),
        tomorrow.day_of_month(),
        9,
        0,
        0.0,
    )
    .ok()
    .map(|dt| dt.to_unix())
}

/// The menu model, in mockup 14's order.
pub fn model() -> gio::Menu {
    let menu = gio::Menu::new();

    let actions = gio::Menu::new();
    actions.append(Some("Back Up Now"), Some("win.backup-now"));

    let pause = gio::Menu::new();
    pause.append(Some("For 1 hour"), Some("win.pause::hour"));
    pause.append(Some("Until tomorrow"), Some("win.pause::tomorrow"));
    pause.append(Some("Until I resume"), Some("win.pause::indefinite"));
    actions.append_submenu(Some("Pause Backups"), &pause);
    actions.append(Some("Resume Backups"), Some("win.resume"));
    menu.append_section(None, &actions);

    let stash = gio::Menu::new();
    stash.append(
        Some("Recently Replaced Files"),
        Some("win.recently-replaced"),
    );
    menu.append_section(None, &stash);

    let app = gio::Menu::new();
    app.append(Some("Preferences"), Some("win.preferences"));
    app.append(Some("Keyboard Shortcuts"), Some("win.shortcuts"));
    app.append(Some("Help"), Some("win.help"));
    app.append(Some("About Backtrack"), Some("win.about"));
    menu.append_section(None, &app);

    menu
}

/// The header-bar button that opens it.
pub fn button() -> MenuButton {
    let button = MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&model())
        .tooltip_text("Main menu")
        .primary(true)
        .build();
    button.update_property(&[gtk4::accessible::Property::Label("Main menu")]);
    button
}

/// The shortcuts dialog, listing the accelerators that are actually installed.
///
/// Built from the same `win.` action names the accelerators are attached to,
/// so an entry here cannot drift from what the key does — `AdwShortcutsItem`
/// takes an action and finds its accelerator itself. The previous version of
/// this listed a key that was never wired to anything, which is the failure
/// mode a hand-written list invites.
pub fn shortcuts_dialog() -> adw::ShortcutsDialog {
    let dialog = adw::ShortcutsDialog::new();

    let time = adw::ShortcutsSection::new(Some("Moving through time"));
    time.add(adw::ShortcutsItem::from_action(
        "One backup older",
        "win.older",
    ));
    time.add(adw::ShortcutsItem::from_action(
        "One backup newer",
        "win.newer",
    ));
    dialog.add(time);

    let files = adw::ShortcutsSection::new(Some("Files"));
    files.add(adw::ShortcutsItem::from_action(
        "Go to the parent folder",
        "win.parent",
    ));
    files.add(adw::ShortcutsItem::from_action(
        "Restore the selected item",
        "win.restore",
    ));
    files.add(adw::ShortcutsItem::from_action(
        "Restore it into a folder you choose",
        "win.restore-to",
    ));
    files.add(adw::ShortcutsItem::from_action(
        "Compare it with the file on your computer",
        "win.compare",
    ));
    // The one key that belongs to the list widget rather than to an action.
    files.add(adw::ShortcutsItem::new(
        "Open the selected folder",
        "Return",
    ));
    dialog.add(files);

    let general = adw::ShortcutsSection::new(Some("General"));
    general.add(adw::ShortcutsItem::from_action(
        "Back up now",
        "win.backup-now",
    ));
    general.add(adw::ShortcutsItem::from_action(
        "Keyboard shortcuts",
        "win.shortcuts",
    ));
    general.add(adw::ShortcutsItem::from_action(
        "Close the window",
        "window.close",
    ));
    dialog.add(general);

    dialog
}

/// The About dialog. The version is read from the crate metadata, never typed
/// in: a version string that has to be kept in step by hand is one that is
/// wrong by the second release.
pub fn about_dialog() -> adw::AboutDialog {
    let about = adw::AboutDialog::builder()
        .application_name("Backtrack")
        .application_icon(crate::APP_ID)
        .version(backtrack_core::VERSION)
        .developer_name("Keith Vassallo")
        .license_type(gtk4::License::Gpl30)
        .website(env!("CARGO_PKG_REPOSITORY"))
        .issue_url(format!("{}/issues", env!("CARGO_PKG_REPOSITORY")))
        .comments("Browse your backups as if they were folders, and put back what you lost.")
        .build();
    about.add_credit_section(Some("Built on"), &["Borg Backup https://borgbackup.org"]);
    about
}

/// Open the documentation in the user's browser.
pub fn open_help(parent: &impl IsA<gtk4::Window>) {
    let launcher = gtk4::UriLauncher::new(env!("CARGO_PKG_REPOSITORY"));
    launcher.launch(
        Some(parent),
        gio::Cancellable::NONE,
        |result: Result<(), glib::Error>| {
            if let Err(error) = result {
                warn!(%error, "the help page could not be opened");
            }
        },
    );
}

/// Enable or disable a window action by name.
pub fn set_enabled(window: &impl IsA<gio::ActionMap>, name: &str, enabled: bool) {
    if let Some(action) = window.as_ref().lookup_action(name) {
        if let Ok(action) = action.downcast::<gio::SimpleAction>() {
            action.set_enabled(enabled);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc() -> glib::TimeZone {
        glib::TimeZone::utc()
    }

    /// Wednesday 2026-06-10 12:00:00 UTC.
    const NOW: i64 = 1_781_092_800;

    #[test]
    fn until_tomorrow_lands_on_tomorrow_morning_not_on_midnight() {
        let seconds = pause_duration("tomorrow", NOW, &utc()).unwrap();
        // 12:00 today to 09:00 tomorrow is 21 hours.
        assert_eq!(seconds, 21 * 3_600);
    }

    #[test]
    fn until_tomorrow_is_still_in_the_future_late_at_night() {
        // 23:30, when "tomorrow at 09:00" is only nine and a half hours away —
        // and, more to the point, has not already gone.
        let late = NOW + 11 * 3_600 + 1_800;
        let seconds = pause_duration("tomorrow", late, &utc()).unwrap();
        assert_eq!(seconds, 9 * 3_600 + 1_800);
    }

    #[test]
    fn until_i_resume_is_far_enough_out_to_read_as_indefinite() {
        let seconds = pause_duration("indefinite", NOW, &utc()).unwrap();
        assert!(
            seconds > INDEFINITE_PAUSE_THRESHOLD,
            "the status line has to recognise it as open-ended"
        );
    }

    #[test]
    fn an_unknown_option_pauses_nothing() {
        assert_eq!(pause_duration("forever", NOW, &utc()), None);
    }

    #[test]
    fn the_menu_has_no_way_to_switch_backups_off() {
        // The one thing this menu must never grow. Preferences is where a
        // permanent change belongs, because it has to be looked for.
        let menu = model();
        let mut labels = Vec::new();
        for section in 0..menu.n_items() {
            if let Some(items) = menu.item_link(section, gio::MENU_LINK_SECTION) {
                for index in 0..items.n_items() {
                    if let Some(label) = items
                        .item_attribute_value(index, gio::MENU_ATTRIBUTE_LABEL, None)
                        .and_then(|v| v.get::<String>())
                    {
                        labels.push(label.to_lowercase());
                    }
                }
            }
        }
        assert!(labels.iter().any(|l| l == "back up now"));
        assert!(labels.iter().any(|l| l == "pause backups"));
        assert!(
            !labels
                .iter()
                .any(|l| l.contains("turn off") || l.contains("disable")),
            "found something that switches backups off: {labels:?}"
        );
    }
}
