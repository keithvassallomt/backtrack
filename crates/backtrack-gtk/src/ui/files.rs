// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The file pane: one folder, as it was at the selected backup.
//!
//! This is where the product's one idea shows up as behaviour. The pane
//! reloads when the folder changes and when the backup changes, and the two are
//! the same code path — stepping back through time is, to this pane, simply a
//! different answer to the same question. Files that were deleted later
//! reappear as you go back, which is what makes finding them a matter of
//! looking rather than searching.
//!
//! Both status badges come from the index, not from a comparison done here:
//! "deleted after this" and "changed since then" are columns of the query in
//! S01-T3, computed in SQL over the interval encoding.

use std::cell::Cell;
use std::rc::Rc;

use backtrack_core::index::{Entry, Kind};
use gtk4::prelude::*;
use gtk4::{
    gio, glib, Align, Box as GtkBox, ColumnView, ColumnViewColumn, Image, Label, ListItem,
    Orientation, ScrolledWindow, SignalListItemFactory, SingleSelection, Stack, Widget,
};
use libadwaita as adw;
use tracing::warn;

use crate::index::Index;
use crate::path;
use crate::state::{AppState, Change, Selected};

/// A row of the pane: the indexed entry plus the strings it is drawn with.
#[derive(Clone)]
struct RowData {
    entry: Entry,
    size: String,
    modified: String,
}

/// The pane and what it needs to refill itself.
pub struct Files {
    stack: Stack,
    rows: gio::ListStore,
    selection: SingleSelection,
    empty: adw::StatusPage,
    state: Rc<AppState>,
    index: Index,
    /// Which load is current. A load that finishes after a newer one started is
    /// dropped: arrow-keying through time fires these faster than they return,
    /// and the last answer to arrive must not win over the last one asked for.
    generation: Cell<u64>,
    syncing: Cell<bool>,
}

/// Build the file pane for `state`.
pub fn build(state: &Rc<AppState>, index: &Index) -> Rc<Files> {
    let rows = gio::ListStore::new::<glib::BoxedAnyObject>();
    let selection = SingleSelection::builder()
        .model(&rows)
        .autoselect(false)
        .can_unselect(true)
        .build();

    let view = ColumnView::builder()
        .model(&selection)
        .show_column_separators(false)
        .show_row_separators(true)
        .build();
    view.add_css_class("card");
    for column in columns() {
        view.append_column(&column);
    }

    let scroller = ScrolledWindow::builder()
        .child(&view)
        .hexpand(true)
        .vexpand(true)
        .build();

    let empty = adw::StatusPage::builder()
        .icon_name("folder-symbolic")
        .title("Nothing here")
        .build();
    empty.add_css_class("compact");

    let stack = Stack::new();
    stack.add_named(&scroller, Some("files"));
    stack.add_named(&empty, Some("empty"));

    let files = Rc::new(Files {
        stack,
        rows,
        selection,
        empty,
        state: Rc::clone(state),
        index: index.clone(),
        generation: Cell::new(0),
        syncing: Cell::new(false),
    });

    let watcher = Rc::clone(&files);
    state.subscribe(move |_, change| {
        if matches!(change, Change::Folder | Change::Seq | Change::Archives) {
            watcher.reload();
        }
    });

    let selector = Rc::clone(&files);
    files
        .selection
        .connect_selected_item_notify(move |selection| {
            if selector.syncing.get() {
                return;
            }
            let selected = row_of(selection.selected_item().as_ref()).map(|row| {
                let folder = selector.state.view().folder;
                Selected {
                    path: path::join(&folder, &row.entry.name),
                    name: row.entry.name.clone(),
                    is_dir: row.entry.kind == Kind::Dir,
                    size: row.entry.size,
                    mtime: row.entry.mtime,
                }
            });
            selector.state.set_selected(selected);
        });

    // Double-click, or Enter: a folder is somewhere to go, a file is something
    // to look at.
    let opener = Rc::clone(&files);
    view.connect_activate(move |view, position| {
        let Some(row) = view
            .model()
            .and_then(|model| model.item(position))
            .and_then(|item| row_of(Some(&item)))
        else {
            return;
        };
        if row.entry.kind == Kind::Dir {
            let folder = opener.state.view().folder;
            opener
                .state
                .set_folder(path::join(&folder, &row.entry.name));
        }
    });

    files.reload();
    files
}

impl Files {
    /// The widget to put in the window.
    pub fn widget(&self) -> Widget {
        self.stack.clone().upcast()
    }

    /// Re-read the folder at the current backup.
    fn reload(self: &Rc<Self>) {
        let view = self.state.view();
        let Some(seq) = view.seq else {
            self.show_empty(
                "No backups yet",
                "Backtrack has not catalogued any backups.",
            );
            return;
        };
        let folder = view.folder.clone();
        let wanted = self.generation.get() + 1;
        self.generation.set(wanted);

        let this = Rc::clone(self);
        let query_folder = folder.clone();
        crate::ui::spawn(async move {
            let answer = this
                .index
                .query(move |reader| reader.folder_at(&query_folder, seq))
                .await;
            if this.generation.get() != wanted {
                // Superseded while the query was in flight.
                return;
            }
            match answer {
                Ok(entries) => this.show(entries, &folder),
                Err(error) => {
                    warn!(%error, folder, seq, "the folder could not be read");
                    this.show_empty("This folder could not be read", &error);
                }
            }
        });
    }

    /// Put `entries` on screen, keeping the selection if it survived the change.
    fn show(self: &Rc<Self>, entries: Vec<Entry>, folder: &str) {
        if entries.is_empty() {
            let name = path::name(folder);
            self.show_empty(
                "Not in this backup",
                &format!("“{name}” has nothing in it at this point in time. Step forward, or pick a more recent backup."),
            );
            return;
        }

        let previously = self.state.view().selected.map(|s| s.name);

        self.syncing.set(true);
        self.rows.remove_all();
        let tz = glib::TimeZone::local();
        let mut sorted = entries;
        // Folders first, then by name, the way every file manager does it. The
        // index returns plain name order, which mixes them.
        sorted.sort_by(|a, b| {
            (b.kind == Kind::Dir)
                .cmp(&(a.kind == Kind::Dir))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        for entry in sorted {
            let data = RowData {
                size: crate::model::format::size(entry.size, entry.kind),
                modified: crate::model::format::modified(entry.mtime, &tz),
                entry,
            };
            self.rows.append(&glib::BoxedAnyObject::new(data));
        }
        self.stack.set_visible_child_name("files");
        self.syncing.set(false);

        // Stepping through time keeps you on the file you were looking at, so
        // long as it existed then.
        match previously.and_then(|name| self.position_of(&name)) {
            Some(position) => self.selection.set_selected(position),
            None => {
                self.selection.set_selected(gtk4::INVALID_LIST_POSITION);
                self.state.set_selected(None);
            }
        }
    }

    fn show_empty(&self, title: &str, description: &str) {
        self.rows.remove_all();
        self.empty.set_title(title);
        self.empty.set_description(Some(description));
        self.stack.set_visible_child_name("empty");
    }

    fn position_of(&self, name: &str) -> Option<u32> {
        (0..self.rows.n_items()).find(|index| {
            row_of(self.rows.item(*index).as_ref()).is_some_and(|row| row.entry.name == name)
        })
    }
}

/// The row behind a list item.
fn row_of(item: Option<&glib::Object>) -> Option<RowData> {
    let boxed = item?.downcast_ref::<glib::BoxedAnyObject>()?;
    let row = boxed.borrow::<RowData>().clone();
    Some(row)
}

/// Name, Size, Modified, Status — the mockup's columns, in its order.
fn columns() -> Vec<ColumnViewColumn> {
    vec![
        ColumnViewColumn::builder()
            .title("Name")
            .expand(true)
            .resizable(true)
            .factory(&name_factory())
            .build(),
        ColumnViewColumn::builder()
            .title("Size")
            .fixed_width(96)
            .factory(&text_factory(|row| row.size.clone(), Align::End))
            .build(),
        ColumnViewColumn::builder()
            .title("Modified")
            .fixed_width(168)
            .factory(&text_factory(|row| row.modified.clone(), Align::Start))
            .build(),
        ColumnViewColumn::builder()
            .title("Status")
            .fixed_width(168)
            .factory(&status_factory())
            .build(),
    ]
}

/// An icon and the entry's name.
fn name_factory() -> SignalListItemFactory {
    let factory = SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<ListItem>() else {
            return;
        };
        let content = GtkBox::new(Orientation::Horizontal, 10);
        content.append(&Image::new());
        let label = Label::new(None);
        label.set_halign(Align::Start);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        content.append(&label);
        item.set_child(Some(&content));
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<ListItem>() else {
            return;
        };
        let (Some(content), Some(row)) = (
            item.child().and_then(|c| c.downcast::<GtkBox>().ok()),
            row_of(item.item().as_ref()),
        ) else {
            return;
        };
        if let Some(image) = content
            .first_child()
            .and_then(|c| c.downcast::<Image>().ok())
        {
            image.set_from_gicon(&icon_for(&row.entry));
        }
        if let Some(label) = content
            .last_child()
            .and_then(|c| c.downcast::<Label>().ok())
        {
            label.set_text(&row.entry.name);
        }
    });
    factory
}

/// A plain text column.
fn text_factory(text: fn(&RowData) -> String, align: Align) -> SignalListItemFactory {
    let factory = SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<ListItem>() else {
            return;
        };
        let label = Label::new(None);
        label.set_halign(align);
        item.set_child(Some(&label));
    });
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<ListItem>() else {
            return;
        };
        let (Some(label), Some(row)) = (
            item.child().and_then(|c| c.downcast::<Label>().ok()),
            row_of(item.item().as_ref()),
        ) else {
            return;
        };
        label.set_text(&text(&row));
    });
    factory
}

/// The badges. Both are words first and colour second: the pane has to work in
/// a screenshot printed in black and white.
fn status_factory() -> SignalListItemFactory {
    let factory = SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<ListItem>() else {
            return;
        };
        let label = Label::new(None);
        label.set_halign(Align::Start);
        label.add_css_class("badge");
        item.set_child(Some(&label));
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<ListItem>() else {
            return;
        };
        let (Some(label), Some(row)) = (
            item.child().and_then(|c| c.downcast::<Label>().ok()),
            row_of(item.item().as_ref()),
        ) else {
            return;
        };
        label.remove_css_class("deleted");
        label.remove_css_class("changed");
        if row.entry.deleted_after {
            label.set_text("deleted after this");
            label.add_css_class("deleted");
            label.set_visible(true);
        } else if row.entry.changed_since {
            label.set_text("changed since then");
            label.add_css_class("changed");
            label.set_visible(true);
        } else {
            // An em dash rather than nothing, so the column reads as answered.
            label.set_text("—");
            label.set_visible(true);
        }
    });
    factory
}

/// The themed icon for an entry, chosen from its name the way a file manager
/// does — the index records no content type, and guessing from the name is what
/// gives a PDF a PDF icon.
fn icon_for(entry: &Entry) -> gio::Icon {
    match entry.kind {
        Kind::Dir => gio::ThemedIcon::new("folder").upcast(),
        Kind::Symlink => gio::ThemedIcon::new("emblem-symbolic-link").upcast(),
        _ => {
            let (content_type, _) = gio::content_type_guess(Some(&entry.name), None);
            gio::content_type_get_icon(&content_type)
        }
    }
}
