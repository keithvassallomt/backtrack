// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! "Recently Replaced Files": the safety stash, where a person can reach it.
//!
//! The toast's Undo covers the ten seconds after a restore. This covers the
//! thirty days after it, which is the timescale on which people actually
//! notice — the file was replaced on Tuesday, and what was wrong about that is
//! only obvious on Friday.
//!
//! Grouped by restore rather than listed flat, because that is how the memory
//! works: somebody is looking for the moment they lost the file, not for its
//! name. They remember restoring a folder; they do not remember which of the
//! four hundred files in it they are now missing.

use std::cell::RefCell;
use std::rc::Rc;

use backtrack_core::dbus::ReplacedFile;
use gtk4::prelude::*;
use gtk4::{glib, Align, Button, Label};
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::{info, warn};

use crate::daemon::Daemon1Proxy;
use crate::model::restore as copy;

/// How many entries to ask for. A restore of a large folder replaces as many
/// files as it touches, and this window is "recently replaced", not "every
/// file Backtrack has ever held".
const HOW_MANY: u32 = 500;

/// The daemon, as this window holds it.
type Service = Rc<RefCell<Option<Daemon1Proxy<'static>>>>;

/// Open the window, reading the stash as it goes.
pub fn present(parent: &adw::ApplicationWindow, daemon: &Service) {
    let window = adw::Window::builder()
        .transient_for(parent)
        .title("Recently Replaced Files")
        .default_width(680)
        .default_height(580)
        .build();

    let toasts = adw::ToastOverlay::new();
    let content = adw::ToolbarView::new();
    content.add_top_bar(&adw::HeaderBar::new());
    content.set_content(Some(&toasts));
    window.set_content(Some(&content));

    window.present();
    reload(&toasts, daemon);
}

/// Read the stash and put what it says on the screen.
///
/// Re-read rather than patched after a put-back: the stash is on disk and the
/// daemon owns it, so asking again is the only answer that cannot drift from
/// what is actually there. The page is rebuilt each time for the same reason,
/// and because `AdwPreferencesPage` offers no way to enumerate the groups it
/// is holding — so a page kept across reads is a page that accumulates.
fn reload(toasts: &adw::ToastOverlay, daemon: &Service) {
    let Some(proxy) = daemon.borrow().clone() else {
        show(toasts, daemon, Vec::new());
        return;
    };
    let toasts = toasts.clone();
    let daemon = Rc::clone(daemon);
    crate::ui::spawn(async move {
        match proxy.list_replaced(HOW_MANY).await {
            Ok(entries) => show(&toasts, &daemon, entries),
            Err(error) => {
                warn!(%error, "the safety stash could not be read");
                toasts.add_toast(crate::ui::toast("The safety stash could not be read"));
            }
        }
    });
}

/// Build the page, grouped by the restore that made each entry.
fn show(toasts: &adw::ToastOverlay, daemon: &Service, entries: Vec<ReplacedFile>) {
    if entries.is_empty() {
        // Not an error and not an apology: an empty stash means nothing has
        // been overwritten, which is the normal state of affairs.
        toasts.set_child(Some(
            &adw::StatusPage::builder()
                .icon_name("edit-undo-symbolic")
                .title(copy::NOTHING_REPLACED)
                .description(copy::NOTHING_REPLACED_BODY)
                .build(),
        ));
        return;
    }

    let page = adw::PreferencesPage::new();
    let tz = glib::TimeZone::local();
    let mut group: Option<adw::PreferencesGroup> = None;
    let mut current = i64::MIN;

    for entry in entries {
        if entry.replaced_at != current {
            current = entry.replaced_at;
            let made = adw::PreferencesGroup::builder()
                .title(copy::stash_group_title(entry.replaced_at, &tz))
                .build();
            page.add(&made);
            group = Some(made);
        }
        let Some(group) = group.as_ref() else {
            continue;
        };
        group.add(&row(&entry, &tz, toasts, daemon));
    }

    let note = adw::PreferencesGroup::new();
    let label = Label::new(Some(copy::STASH_NOTE));
    label.add_css_class("dim-label");
    label.add_css_class("caption");
    label.set_wrap(true);
    label.set_halign(Align::Start);
    note.add(&label);
    page.add(&note);

    toasts.set_child(Some(&page));
}

/// One replaced file, with the button that takes it back.
fn row(
    entry: &ReplacedFile,
    tz: &glib::TimeZone,
    toasts: &adw::ToastOverlay,
    daemon: &Service,
) -> adw::ActionRow {
    let name = crate::path::name(&entry.original).to_string();
    // Plain text: a file name is not markup, and one with `&` in it would
    // otherwise leave the row blank.
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .title(&name)
        .subtitle(copy::stash_row_subtitle(
            &entry.original,
            entry.size,
            entry.mtime,
            &glib::home_dir().to_string_lossy(),
            tz,
        ))
        .build();
    // One line, because the path is already shortened to fit on one. Allowing
    // two invites the prefix back in to fill them.
    row.set_subtitle_lines(1);

    let button = Button::with_label("Put Back");
    button.set_valign(Align::Center);
    let stashed = entry.stashed.clone();
    let toasts = toasts.clone();
    let daemon = Rc::clone(daemon);
    button.connect_clicked(move |button| {
        let Some(proxy) = daemon.borrow().clone() else {
            return;
        };
        // One press only. The page is rebuilt when the answer arrives, so a
        // second press would be against a row that is on its way out.
        button.set_sensitive(false);
        let stashed = stashed.clone();
        let name = name.clone();
        let toasts = toasts.clone();
        let daemon = Rc::clone(&daemon);
        let button = button.clone();
        crate::ui::spawn(async move {
            match proxy.put_back_replaced(&stashed).await {
                Ok(displaced) => {
                    info!(stashed, displaced, "a replaced file was put back");
                    toasts.add_toast(crate::ui::toast(&copy::put_back_toast(&name)));
                    reload(&toasts, &daemon);
                }
                Err(error) => {
                    warn!(%error, stashed, "the file could not be put back");
                    button.set_sensitive(true);
                    toasts.add_toast(crate::ui::toast(&clean(&error.to_string())));
                }
            }
        });
    });
    row.add_suffix(&button);
    row.set_activatable_widget(Some(&button));
    row
}

/// Strip the D-Bus error-name prefix, which is addressed to programs.
fn clean(message: &str) -> String {
    message
        .rsplit_once(": ")
        .map_or(message, |(_, tail)| tail)
        .to_string()
}
