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
    let owner = Rc::clone(state);
    let go: Go = Rc::new(move |folder| owner.set_folder(folder));
    render(&row, &state.view().folder, home.as_deref(), &go);

    let watched = row.clone();
    state.subscribe(move |view, change| {
        if change == Change::Folder {
            render(&watched, &view.folder, home.as_deref(), &go);
        }
    });
    row
}

/// What clicking a step of the trail does with the folder it stands for.
pub type Go = Rc<dyn Fn(String)>;

/// A trail for `folder` that stays as it is, each step handing its folder to
/// `go`: a window of Rewind's (S18-T4).
pub fn trail(folder: &str, go: &Go) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 0);
    row.add_css_class("breadcrumb");
    row.set_valign(Align::Center);
    render(&row, folder, home_archive_path().as_deref(), go);
    row
}

/// Replace the trail's buttons with the ones `folder` calls for.
fn render(row: &GtkBox, folder: &str, home: Option<&str>, go: &Go) {
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
        row.append(&self::crumb(crumb, position == last, go));
    }
}

/// One step of the trail.
///
/// The folder being viewed is a label rather than a disabled button. Drawn as
/// a button it has to be insensitive — there is nowhere to go — and an
/// insensitive button is dimmed, which made the one thing the title bar exists
/// to say the faintest thing in it.
fn crumb(crumb: &Crumb, is_current: bool, go: &Go) -> gtk4::Widget {
    let content = GtkBox::new(Orientation::Horizontal, 6);
    if crumb.is_home {
        content.append(&gtk4::Image::from_icon_name("user-home-symbolic"));
    }
    let label = Label::new(Some(&crumb.label));
    content.append(&label);

    if is_current {
        label.add_css_class("heading");
        content.add_css_class("breadcrumb-current");
        content.set_valign(Align::Center);
        // Still announced, because "where am I?" is a question assistive
        // technology has to be able to answer.
        content.update_property(&[gtk4::accessible::Property::Label(&crumb.label)]);
        content.set_tooltip_text(Some(&path::to_filesystem(&crumb.path).to_string_lossy()));
        return content.upcast();
    }

    let button = Button::new();
    button.add_css_class("flat");
    button.set_child(Some(&content));
    button.update_property(&[gtk4::accessible::Property::Label(&crumb.label)]);
    button.set_tooltip_text(Some(&path::to_filesystem(&crumb.path).to_string_lossy()));
    let target = crumb.path.clone();
    let go = Rc::clone(go);
    button.connect_clicked(move |_| go(target.clone()));
    button.upcast()
}

/// The user's home directory in archive form, which is what turns
/// `home/keith/Documents` into `Home ▸ Documents`.
pub fn home_archive_path() -> Option<String> {
    let home = glib::home_dir();
    let archive = path::to_archive(&home);
    (!archive.is_empty()).then_some(archive)
}
