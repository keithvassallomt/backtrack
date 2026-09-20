// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The path trail in the header bar.
//!
//! It is the window's title as well as its navigation: the folder is the thing
//! that stays put while time moves, so it is the thing the title bar names.

use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{glib, Align, Box as GtkBox, Button, Label, Orientation};

use crate::path::{self, Crumb};
use crate::state::{AppState, Change};

/// Build the trail and keep it in step with the folder being viewed.
pub fn build(state: &Rc<AppState>) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 0);
    row.add_css_class("breadcrumb");
    row.set_valign(Align::Center);

    let home = home_archive_path();
    render(&row, &state.view().folder, home.as_deref(), state);

    let watched = row.clone();
    let owner = Rc::clone(state);
    state.subscribe(move |view, change| {
        if change == Change::Folder {
            render(&watched, &view.folder, home.as_deref(), &owner);
        }
    });
    row
}

/// Replace the trail's buttons with the ones `folder` calls for.
fn render(row: &GtkBox, folder: &str, home: Option<&str>, state: &Rc<AppState>) {
    while let Some(child) = row.first_child() {
        row.remove(&child);
    }

    let crumbs = path::breadcrumbs(folder, home);
    let last = crumbs.len().saturating_sub(1);
    for (position, crumb) in crumbs.iter().enumerate() {
        if position > 0 {
            let separator = Label::new(Some("›"));
            separator.add_css_class("separator");
            row.append(&separator);
        }
        row.append(&button(crumb, position == last, state));
    }
}

/// One step of the trail. The last one is the folder you are already in, so it
/// is shown as the current position rather than as somewhere to go.
fn button(crumb: &Crumb, is_current: bool, state: &Rc<AppState>) -> Button {
    let button = Button::new();
    button.add_css_class("flat");

    let content = GtkBox::new(Orientation::Horizontal, 6);
    if crumb.is_home {
        content.append(&gtk4::Image::from_icon_name("user-home-symbolic"));
    }
    let label = Label::new(Some(&crumb.label));
    if is_current {
        label.add_css_class("heading");
    }
    content.append(&label);
    button.set_child(Some(&content));

    // Spoken as the folder's name, not as "button, Home, image".
    button.update_property(&[gtk4::accessible::Property::Label(&crumb.label)]);
    button.set_tooltip_text(Some(&path::to_filesystem(&crumb.path).to_string_lossy()));

    if is_current {
        button.set_sensitive(false);
        // Insensitive would normally hide it from assistive technology, but
        // this crumb is the answer to "where am I?".
        button.set_can_focus(false);
    } else {
        let target = crumb.path.clone();
        let state = Rc::clone(state);
        button.connect_clicked(move |_| state.set_folder(target.clone()));
    }
    button
}

/// The user's home directory in archive form, which is what turns
/// `home/keith/Documents` into `Home ▸ Documents`.
pub fn home_archive_path() -> Option<String> {
    let home = glib::home_dir();
    let archive = path::to_archive(&home);
    (!archive.is_empty()).then_some(archive)
}
