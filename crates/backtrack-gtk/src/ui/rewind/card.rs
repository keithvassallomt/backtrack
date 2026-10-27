// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! A card of Rewind's stack: one backup's folder, as a window of icons, as
//! mockups 25 and 26 draw it.
//!
//! A card is a real widget, styled by the theme (and by whatever palette the
//! person has given GTK), but it is never on screen as one: the stage lays it
//! out once, draws it into a texture, and lets it go. Every card in the stack
//! is such a texture, so moving the stack moves pictures and nothing is laid
//! out while it moves.

use backtrack_core::index::Entry;
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Label, Orientation};

/// The header bar's height.
const HEADER: i32 = 46;
/// One cell of the grid, and the space around the grid.
const CELL_WIDTH: i32 = 124;
const CELL_HEIGHT: i32 = 128;
const PADDING: i32 = 18;

/// What a card shows.
#[derive(Debug, Clone)]
pub struct Content {
    /// The folder's own name.
    pub folder: String,
    /// When this was: "Today (Now)", or the backup's date.
    pub when: String,
    /// What was in the folder, folders first; None while the catalogue has
    /// not read the backup yet.
    pub entries: Option<Vec<Entry>>,
}

/// A card `width` by `height` showing `content`. Only as many entries as fit
/// are made: the card is never scrolled, and the rest would only be clipped.
pub fn build(content: &Content, width: i32, height: i32) -> GtkBox {
    let card = GtkBox::new(Orientation::Vertical, 0);
    card.add_css_class("rewind-card");
    card.set_overflow(gtk4::Overflow::Hidden);
    card.set_can_target(false);
    card.set_size_request(width, height);

    let header = GtkBox::new(Orientation::Horizontal, 12);
    header.add_css_class("rewind-card-header");
    header.set_size_request(-1, HEADER);
    let folder = Label::new(Some(&content.folder));
    folder.add_css_class("title");
    folder.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    let when = Label::new(Some(&content.when));
    when.add_css_class("when");
    when.set_hexpand(true);
    when.set_halign(Align::End);
    header.append(&folder);
    header.append(&when);
    card.append(&header);

    // A scrolled window that never scrolls: it lets the grid be taller than
    // the card without the card asking for more room than it is given.
    let body = gtk4::ScrolledWindow::new();
    body.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::External);
    body.set_vexpand(true);
    body.add_css_class("rewind-card-body");
    card.append(&body);

    let Some(entries) = &content.entries else {
        body.set_child(Some(&notice("Cataloguing this backup…")));
        return card;
    };
    if entries.is_empty() {
        body.set_child(Some(&notice("Nothing here")));
        return card;
    }
    let columns = ((width - 2 * PADDING) / CELL_WIDTH).max(1);
    let rows = ((height - HEADER - 2 * PADDING) / CELL_HEIGHT + 1).max(1);
    let grid = gtk4::Grid::new();
    grid.add_css_class("rewind-card-grid");
    grid.set_column_homogeneous(true);
    grid.set_valign(Align::Start);
    for (i, entry) in entries.iter().take((columns * rows) as usize).enumerate() {
        let i = i as i32;
        grid.attach(&item(entry), i % columns, i / columns, 1, 1);
    }
    body.set_child(Some(&grid));
    card
}

fn item(entry: &Entry) -> GtkBox {
    let item = GtkBox::new(Orientation::Vertical, 6);
    item.add_css_class("rewind-card-item");
    let icon = gtk4::Image::from_gicon(&crate::ui::files::icon_for(entry));
    icon.set_pixel_size(64);
    let name = Label::new(Some(&entry.name));
    name.set_wrap(true);
    name.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
    name.set_lines(2);
    name.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    name.set_justify(gtk4::Justification::Center);
    name.set_max_width_chars(14);
    name.set_width_chars(14);
    name.set_valign(Align::Start);
    item.append(&icon);
    item.append(&name);
    item
}

fn notice(text: &str) -> Label {
    let label = Label::new(Some(text));
    label.add_css_class("dim-label");
    label.set_vexpand(true);
    label.set_valign(Align::Center);
    label
}
