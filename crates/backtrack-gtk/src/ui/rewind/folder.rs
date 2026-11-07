// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! A window of Rewind's: one backup's folder, as mockups 25 and 26 draw it
//! (S18-T4). Its header bar holds the folder's trail, when the backup was,
//! and the pair of buttons that shows it as icons or as the main window's
//! list.
//!
//! The window in front and the cards behind it are built here alike, so that
//! a card that comes to the front looks the same as the window it becomes.
//! The cards are never on screen as widgets: the stage lays each out once,
//! keeps its drawing, and lets it go (see `stage`). Only the window in front
//! is used: it walks into folders, selects, and switches between icons and
//! the list.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use backtrack_core::index::Kind;
use gtk4::prelude::*;
use gtk4::{
    gdk, gio, glib, Align, Box as GtkBox, ColumnView, GridView, Image, Label, ListItem,
    Orientation, Overlay, ScrolledWindow, SignalListItemFactory, SingleSelection, ToggleButton,
};

use crate::model::file_badge;
use crate::path;
use crate::ui::breadcrumb::{self, Go};
use crate::ui::files::{self, Row};

/// The header bar's height.
const HEADER: i32 = 46;
/// An icon's size, and the room left around it for its badge.
const ICON: i32 = 64;
const CELL_WIDTH: i32 = 124;

/// How a window shows its folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Icons,
    List,
}

/// What a window's body shows.
#[derive(Clone)]
pub enum Body {
    /// The catalogue has not read the backup yet, or it is being read.
    Waiting,
    Rows(Vec<Row>),
    /// Why there is nothing to show.
    Empty {
        title: String,
        description: String,
    },
}

/// What a window shows.
#[derive(Clone)]
pub struct Content {
    /// The folder, archive-relative, as its trail names it.
    pub folder: String,
    /// When this was: "Today (Now)", or the backup's date.
    pub when: String,
    pub body: Body,
}

/// The pictures already made for the files shown, by name and modification
/// time, which is to say by version: the desktop's thumbnails (see
/// `model::thumbnail`), and those made for the window in front.
pub type Pictures = Rc<RefCell<HashMap<(String, i64), gdk::Texture>>>;

/// The image each file's cell shows, by name, while it is bound: where a
/// picture made later goes.
pub type Cells = Rc<RefCell<HashMap<String, glib::WeakRef<Image>>>>;

/// What the window in front does when it is used. The cards behind are never
/// used.
#[derive(Clone)]
pub struct Actions {
    /// Go to a folder: a step of the trail, or a folder opened.
    pub go: Go,
    pub mode: Rc<dyn Fn(Mode)>,
    pub select: Rc<dyn Fn(Option<Row>)>,
    /// An entry came into sight, its picture not yet made: the window in
    /// front makes it then, and calls `picture` with it.
    pub shown: Shown,
}

/// What the window in front does with a file come into sight.
pub type Shown = Rc<dyn Fn(&Row)>;

/// A window `width` by `height` showing `content` as `mode` shows it,
/// `selected` (a name in it) selected, and its cells. With `actions`, it is
/// the window in front, and can be used; without, a card, which cannot.
pub fn build(
    content: &Content,
    (width, height): (i32, i32),
    mode: Mode,
    selected: Option<&str>,
    pictures: &Pictures,
    actions: Option<&Actions>,
) -> Built {
    let cells = Cells::default();
    let window = GtkBox::new(Orientation::Vertical, 0);
    window.add_css_class("rewind-card");
    window.set_overflow(gtk4::Overflow::Hidden);
    window.set_size_request(width, height);
    if actions.is_none() {
        window.set_can_target(false);
        window.set_can_focus(false);
    }

    let nowhere: Go = Rc::new(|_| {});
    let go = actions.map_or(&nowhere, |a| &a.go);
    let header = GtkBox::new(Orientation::Horizontal, 12);
    header.add_css_class("rewind-card-header");
    header.set_size_request(-1, HEADER);
    let trail = breadcrumb::trail(&content.folder, go);
    let when = Label::new(Some(&content.when));
    when.add_css_class("when");
    when.set_hexpand(true);
    when.set_halign(Align::End);
    header.append(&trail);
    header.append(&when);
    header.append(&modes(mode, actions));
    window.append(&header);

    let body = ScrolledWindow::new();
    // The cards behind never scroll: a scrolled window that does not lets
    // the folder be taller than the card without the card asking for more
    // room than it is given.
    let scrolls = if actions.is_some() {
        gtk4::PolicyType::Automatic
    } else {
        gtk4::PolicyType::External
    };
    body.set_policy(gtk4::PolicyType::Never, scrolls);
    body.set_vexpand(true);
    body.add_css_class("rewind-card-body");
    window.append(&body);

    let rows = match &content.body {
        Body::Waiting => {
            body.set_child(Some(&notice("Cataloguing this backup…", None)));
            return Built {
                window,
                cells,
                view: None,
            };
        }
        Body::Empty { title, description } => {
            body.set_child(Some(&notice(title, Some(description))));
            return Built {
                window,
                cells,
                view: None,
            };
        }
        Body::Rows(rows) if rows.is_empty() => {
            body.set_child(Some(&notice("Only hidden files here", None)));
            return Built {
                window,
                cells,
                view: None,
            };
        }
        Body::Rows(rows) => rows,
    };

    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    for row in rows {
        store.append(&glib::BoxedAnyObject::new(row.clone()));
    }
    let selection = SingleSelection::builder()
        .model(&store)
        .autoselect(false)
        .can_unselect(true)
        .build();
    let at = selected.and_then(|name| rows.iter().position(|r| r.entry.name == name));
    selection.set_selected(at.map_or(gtk4::INVALID_LIST_POSITION, |i| i as u32));

    let activate = actions.map(|actions| {
        let (go, folder) = (Rc::clone(&actions.go), content.folder.clone());
        move |model: Option<gtk4::SelectionModel>, position: u32| {
            let row = model
                .and_then(|m| m.item(position))
                .and_then(|item| files::row_of(Some(&item)));
            if let Some(row) = row.filter(|r| r.entry.kind == Kind::Dir) {
                go(path::join(&folder, &row.entry.name));
            }
        }
    });
    let view = match mode {
        Mode::Icons => {
            let shown = actions.map(|a| Rc::clone(&a.shown));
            let factory = icon_factory(pictures, &cells, shown);
            let grid = GridView::new(Some(selection.clone()), Some(factory));
            grid.add_css_class("rewind-card-grid");
            grid.set_max_columns(64);
            if let Some(activate) = activate {
                grid.connect_activate(move |grid, position| activate(grid.model(), position));
            }
            body.set_child(Some(&grid));
            grid.upcast::<gtk4::Widget>()
        }
        Mode::List => {
            let list = ColumnView::builder()
                .model(&selection)
                .show_column_separators(false)
                .show_row_separators(true)
                .build();
            list.add_css_class("rewind-card-list");
            for column in files::columns() {
                list.append_column(&column);
            }
            if let Some(activate) = activate {
                list.connect_activate(move |list, position| activate(list.model(), position));
            }
            body.set_child(Some(&list));
            list.upcast::<gtk4::Widget>()
        }
    };
    if let Some(actions) = actions {
        let select = Rc::clone(&actions.select);
        selection.connect_selected_item_notify(move |selection| {
            select(files::row_of(selection.selected_item().as_ref()));
        });
    }
    Built {
        window,
        cells,
        view: Some(view),
    }
}

/// A window, as built.
pub struct Built {
    pub window: GtkBox,
    pub cells: Cells,
    /// Its icons or its list, which take the keyboard; none for a notice.
    pub view: Option<gtk4::Widget>,
}

/// The pair of buttons that shows the folder as icons or as a list.
fn modes(mode: Mode, actions: Option<&Actions>) -> GtkBox {
    let pair = GtkBox::new(Orientation::Horizontal, 0);
    pair.add_css_class("linked");
    pair.set_valign(Align::Center);
    let icons = ToggleButton::builder()
        .icon_name("view-grid-symbolic")
        .tooltip_text("Icons")
        .active(mode == Mode::Icons)
        .build();
    let list = ToggleButton::builder()
        .icon_name("view-list-symbolic")
        .tooltip_text("List")
        .active(mode == Mode::List)
        .group(&icons)
        .build();
    // Their names for a screen reader, which reads no icon (S18-T8).
    icons.update_property(&[gtk4::accessible::Property::Label("Show as icons")]);
    list.update_property(&[gtk4::accessible::Property::Label("Show as a list")]);
    if let Some(actions) = actions {
        for (button, chosen) in [(&icons, Mode::Icons), (&list, Mode::List)] {
            let change = Rc::clone(&actions.mode);
            button.connect_toggled(move |button| {
                if button.is_active() && chosen != mode {
                    change(chosen);
                }
            });
        }
    }
    pair.append(&icons);
    pair.append(&list);
    pair
}

/// An icon view's cell: the icon, or the file's picture, with its badge
/// pinned to its foot, and the name under it on two lines, never more, so
/// that every cell is the same height.
fn icon_factory(pictures: &Pictures, cells: &Cells, shown: Option<Shown>) -> SignalListItemFactory {
    let factory = SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<ListItem>() else {
            return;
        };
        let cell = GtkBox::new(Orientation::Vertical, 4);
        cell.add_css_class("rewind-card-item");
        cell.set_size_request(CELL_WIDTH, -1);
        let image = Image::new();
        image.set_pixel_size(ICON);
        image.add_css_class("rewind-icon");
        let badge = Label::new(None);
        badge.add_css_class("badge");
        // A badge's colour is a tint, which a picture under it would show
        // through; it sits on a pill of the card's own colour instead.
        let base = GtkBox::new(Orientation::Horizontal, 0);
        base.add_css_class("rewind-badge");
        base.set_halign(Align::Center);
        base.set_valign(Align::End);
        base.append(&badge);
        let overlay = Overlay::new();
        overlay.set_child(Some(&image));
        overlay.add_overlay(&base);
        overlay.set_size_request(-1, ICON + 8);
        let name = Label::new(None);
        name.add_css_class("rewind-name");
        name.set_wrap(true);
        name.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
        name.set_lines(2);
        name.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        name.set_justify(gtk4::Justification::Center);
        name.set_max_width_chars(14);
        name.set_width_chars(14);
        name.set_valign(Align::Start);
        // Room for two lines whatever the name, measured in the label's own
        // font once it has one.
        name.connect_realize(|name| {
            let (_, two_lines) = name.create_pango_layout(Some("Ag\nAg")).pixel_size();
            name.set_size_request(-1, two_lines);
        });
        cell.append(&overlay);
        cell.append(&name);
        item.set_child(Some(&cell));
    });
    // A cell let go of no longer shows its file.
    let unbound = Rc::clone(cells);
    factory.connect_unbind(move |_, item| {
        let Some(item) = item.downcast_ref::<ListItem>() else {
            return;
        };
        if let Some(row) = files::row_of(item.item().as_ref()) {
            unbound.borrow_mut().remove(&row.entry.name);
        }
    });
    let (pictures, bound) = (Rc::clone(pictures), Rc::clone(cells));
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<ListItem>() else {
            return;
        };
        let (Some(cell), Some(row)) = (
            item.child().and_then(|c| c.downcast::<GtkBox>().ok()),
            files::row_of(item.item().as_ref()),
        ) else {
            return;
        };
        let Some(overlay) = cell.first_child().and_downcast::<Overlay>() else {
            return;
        };
        if let Some(image) = overlay.child().and_downcast::<Image>() {
            bound
                .borrow_mut()
                .insert(row.entry.name.clone(), image.downgrade());
            let picture = pictures
                .borrow()
                .get(&(row.entry.name.clone(), row.entry.mtime))
                .cloned();
            match picture {
                Some(texture) => image.set_paintable(Some(&texture)),
                None => {
                    image.set_from_gicon(&files::icon_for(&row.entry));
                    if let Some(shown) = &shown {
                        shown(&row);
                    }
                }
            }
        }
        let base = overlay.last_child();
        if let Some(badge) = base
            .as_ref()
            .and_then(|b| b.first_child())
            .and_downcast::<Label>()
        {
            for class in file_badge::CLASSES {
                badge.remove_css_class(class);
            }
            let chosen = row.badge();
            if let Some(base) = &base {
                base.set_visible(chosen.class.is_some());
            }
            // What a screen reader says of the cell: its name and its badge.
            item.set_accessible_label(&spoken(&row.entry.name, chosen.class.map(|_| chosen.text)));
            badge.set_text(chosen.text);
            if let Some(class) = chosen.class {
                badge.add_css_class(class);
            }
            badge.set_tooltip_text(chosen.tooltip);
        }
        if let Some(name) = cell.last_child().and_downcast::<Label>() {
            name.set_text(&row.entry.name);
            name.set_tooltip_text(Some(&row.entry.name));
        }
    });
    factory
}

/// A cell as a screen reader says it: the file's name, and its badge's word
/// if it has one ("report.odt, changed").
fn spoken(name: &str, badge: Option<&str>) -> String {
    match badge {
        Some(word) if !word.is_empty() => format!("{name}, {word}"),
        _ => name.to_string(),
    }
}

/// A picture made for `name` after its cell was bound, put in the cell if it
/// still shows it.
pub fn show_picture(cells: &Cells, name: &str, texture: &gdk::Texture) {
    let image = cells.borrow().get(name).and_then(glib::WeakRef::upgrade);
    if let Some(image) = image {
        image.set_paintable(Some(texture));
    }
}

fn notice(title: &str, description: Option<&str>) -> GtkBox {
    let notice = GtkBox::new(Orientation::Vertical, 6);
    notice.set_vexpand(true);
    notice.set_valign(Align::Center);
    let label = Label::new(Some(title));
    label.add_css_class("title-4");
    label.add_css_class("dim-label");
    notice.append(&label);
    if let Some(description) = description {
        let more = Label::new(Some(description));
        more.add_css_class("dim-label");
        more.set_wrap(true);
        more.set_justify(gtk4::Justification::Center);
        more.set_max_width_chars(50);
        notice.append(&more);
    }
    notice
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_is_spoken_as_its_name_and_badge() {
        assert_eq!(spoken("report.odt", Some("changed")), "report.odt, changed");
        assert_eq!(spoken("notes.txt", None), "notes.txt");
        assert_eq!(spoken("notes.txt", Some("")), "notes.txt");
    }
}
