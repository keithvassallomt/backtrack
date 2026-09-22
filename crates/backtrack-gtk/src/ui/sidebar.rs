// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The snapshot sidebar — the primary time control.
//!
//! The list is a tree rather than a flat run of rows with headings, because the
//! older groups have to *collapse*. A year of backups is a few thousand
//! entries, and the mockup's "May 2026 (31)" is not decoration: it is what
//! keeps the sidebar readable at the point where a flat list stops being one.
//! `GtkTreeExpander` also brings keyboard expansion with it, which a section
//! header cannot offer.
//!
//! Selection runs both ways. Clicking a row moves the timeline; moving the
//! timeline any other way — the stepping buttons, the calendar, the density
//! strip — moves the selection here, opening whichever group the backup is in.
//! Only clicking: the pointer passing over a row must never move the timeline,
//! which is why the list does not set `single-click-activate`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{
    gio, glib, Align, Box as GtkBox, Label, ListItem, ListView, Orientation, ScrolledWindow,
    SignalListItemFactory, SingleSelection, TreeExpander, TreeListModel, TreeListRow, Widget,
};

use crate::model::group::{self, Group, Row};
use crate::state::{AppState, Change};

/// A node of the sidebar tree: either a heading or a backup.
#[derive(Clone)]
enum Node {
    Group { index: usize, title: String },
    Snapshot(Row),
}

/// The sidebar and the bookkeeping that keeps it in step with the state.
pub struct Sidebar {
    scroller: ScrolledWindow,
    list: ListView,
    selection: SingleSelection,
    tree: TreeListModel,
    root: gio::ListStore,
    groups: RefCell<Rc<Vec<Group>>>,
    /// Set while the sidebar is selecting a row on the state's behalf, so the
    /// selection handler does not answer back and start a loop.
    syncing: Cell<bool>,
    state: Rc<AppState>,
}

/// Build the sidebar for `state`.
pub fn build(state: &Rc<AppState>) -> Rc<Sidebar> {
    let root = gio::ListStore::new::<glib::BoxedAnyObject>();
    let groups: Rc<RefCell<Rc<Vec<Group>>>> = Rc::new(RefCell::new(Rc::new(Vec::new())));

    let children = Rc::clone(&groups);
    let tree = TreeListModel::new(root.clone(), false, false, move |item| {
        let node = item
            .downcast_ref::<glib::BoxedAnyObject>()?
            .borrow::<Node>()
            .clone();
        let Node::Group { index, .. } = node else {
            return None;
        };
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        for row in &children.borrow().get(index)?.rows {
            store.append(&glib::BoxedAnyObject::new(Node::Snapshot(row.clone())));
        }
        Some(store.upcast())
    });

    let selection = SingleSelection::builder()
        .model(&tree)
        .autoselect(false)
        .can_unselect(true)
        .build();

    // Deliberately *not* `single_click_activate`. In GTK4 that property does
    // two things, and the second one is fatal here: it activates a row on one
    // click, and it selects whichever row the pointer is over. Selection is
    // what moves the timeline, so hovering the sidebar dragged the whole
    // window through time — and hovering a month heading toggled it open and
    // shut. A plain ListView already changes the selection on a single click,
    // which is the whole of what this list needs.
    let list = ListView::builder()
        .model(&selection)
        .factory(&factory())
        .build();
    list.add_css_class("navigation-sidebar");

    let scroller = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&list)
        .vexpand(true)
        .build();

    let sidebar = Rc::new(Sidebar {
        scroller,
        list: list.clone(),
        selection,
        tree,
        root,
        groups: RefCell::new(Rc::clone(&groups.borrow())),
        syncing: Cell::new(false),
        state: Rc::clone(state),
    });

    // `groups` is shared with the tree's child-model callback, so the sidebar
    // writes through that cell rather than keeping a second copy.
    let shared = Rc::clone(&groups);
    let this = Rc::clone(&sidebar);
    state.subscribe(move |view, change| match change {
        Change::Archives => {
            let tz = glib::TimeZone::local();
            let now = glib::DateTime::now_utc().map(|d| d.to_unix()).unwrap_or(0);
            let built = Rc::new(group::sidebar(&view.archives, now, &tz));
            *shared.borrow_mut() = Rc::clone(&built);
            *this.groups.borrow_mut() = Rc::clone(&built);
            this.rebuild();
            this.select(view.seq);
        }
        Change::Seq => this.select(view.seq),
        _ => {}
    });

    let clicked = Rc::clone(&sidebar);
    sidebar
        .selection
        .connect_selected_item_notify(move |selection| {
            if clicked.syncing.get() {
                return;
            }
            let Some(node) = node_at(selection.selected_item().as_ref()) else {
                return;
            };
            match node {
                // A heading is not a place in time; clicking one opens it.
                Node::Group { .. } => {
                    if let Some(row) = selection
                        .selected_item()
                        .and_then(|o| o.downcast::<TreeListRow>().ok())
                    {
                        row.set_expanded(!row.is_expanded());
                    }
                }
                Node::Snapshot(row) if row.catalogued => clicked.state.set_seq(row.seq),
                // Still being read; there is nothing to show yet.
                Node::Snapshot(_) => {}
            }
        });

    sidebar
}

impl Sidebar {
    /// The widget to put in the window.
    pub fn widget(&self) -> Widget {
        self.scroller.clone().upcast()
    }

    /// Replace the tree with the current groups, opening the recent ones.
    fn rebuild(&self) {
        self.syncing.set(true);
        self.root.remove_all();
        let groups = Rc::clone(&self.groups.borrow());
        for (index, group) in groups.iter().enumerate() {
            self.root.append(&glib::BoxedAnyObject::new(Node::Group {
                index,
                title: group.title.clone(),
            }));
        }
        for (index, group) in groups.iter().enumerate() {
            if group.expanded {
                if let Some(row) = self.tree.child_row(index as u32) {
                    row.set_expanded(true);
                }
            }
        }
        self.syncing.set(false);
    }

    /// Select the row for `seq`, opening its group if it is closed.
    fn select(&self, seq: Option<i64>) {
        let Some(seq) = seq else {
            self.syncing.set(true);
            self.selection.set_selected(gtk4::INVALID_LIST_POSITION);
            self.syncing.set(false);
            return;
        };
        let groups = Rc::clone(&self.groups.borrow());
        let Some(group_index) = groups
            .iter()
            .position(|g| g.rows.iter().any(|r| r.seq == seq))
        else {
            return;
        };

        self.syncing.set(true);
        if let Some(parent) = self.tree.child_row(group_index as u32) {
            parent.set_expanded(true);
        }
        // Positions shift as groups open, so the row is found after expanding.
        if let Some(position) = self.position_of(seq) {
            self.selection.set_selected(position);
            // A jump from the calendar or the density strip can land in a group
            // that is scrolled out of sight, and a selection you cannot see is
            // not an answer.
            self.list
                .scroll_to(position, gtk4::ListScrollFlags::NONE, None);
        }
        self.syncing.set(false);
    }

    /// Where `seq` sits in the flattened tree, if its group is open.
    fn position_of(&self, seq: i64) -> Option<u32> {
        (0..self.tree.n_items()).find(|index| {
            matches!(
                node_at(self.tree.item(*index).as_ref()),
                Some(Node::Snapshot(row)) if row.seq == seq
            )
        })
    }
}

/// The node behind a list item, which is a `GtkTreeListRow` wrapping it.
fn node_at(item: Option<&glib::Object>) -> Option<Node> {
    let row = item?.downcast_ref::<TreeListRow>()?;
    let boxed = row.item()?.downcast::<glib::BoxedAnyObject>().ok()?;
    let node = boxed.borrow::<Node>().clone();
    Some(node)
}

/// How a row is built and filled.
fn factory() -> SignalListItemFactory {
    let factory = SignalListItemFactory::new();

    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<ListItem>() else {
            return;
        };
        let content = GtkBox::new(Orientation::Horizontal, 6);
        let label = Label::new(None);
        label.set_halign(Align::Start);
        label.set_hexpand(true);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        content.append(&label);

        let badge = Label::new(None);
        badge.add_css_class("badge");
        badge.set_visible(false);
        content.append(&badge);

        let expander = TreeExpander::new();
        expander.set_child(Some(&content));
        item.set_child(Some(&expander));
    });

    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<ListItem>() else {
            return;
        };
        let Some(expander) = item.child().and_then(|c| c.downcast::<TreeExpander>().ok()) else {
            return;
        };
        let Some(row) = item.item().and_then(|o| o.downcast::<TreeListRow>().ok()) else {
            return;
        };
        expander.set_list_row(Some(&row));

        let Some(content) = expander.child().and_then(|c| c.downcast::<GtkBox>().ok()) else {
            return;
        };
        let Some(label) = content
            .first_child()
            .and_then(|c| c.downcast::<Label>().ok())
        else {
            return;
        };
        let Some(badge) = content
            .last_child()
            .and_then(|c| c.downcast::<Label>().ok())
        else {
            return;
        };

        match node_at(item.item().as_ref()) {
            Some(Node::Group { title, .. }) => {
                label.set_text(&title);
                label.add_css_class("heading");
                badge.set_visible(false);
                item.set_selectable(true);
            }
            Some(Node::Snapshot(snapshot)) => {
                label.set_text(&snapshot.label);
                label.remove_css_class("heading");
                label.set_sensitive(snapshot.catalogued);
                describe(&label, &snapshot);
                if !snapshot.catalogued {
                    badge.set_text("cataloguing…");
                    badge.remove_css_class("local");
                    badge.set_visible(true);
                } else if snapshot.local_only {
                    badge.set_text("on this computer");
                    badge.add_css_class("local");
                    badge.set_visible(true);
                } else {
                    badge.set_visible(false);
                }
                item.set_selectable(snapshot.catalogued);
            }
            None => {}
        }
    });

    factory
}

/// What a screen reader says for a row, which has to carry what the badge says
/// as well as the time.
fn describe(label: &Label, snapshot: &Row) {
    let spoken = match (snapshot.catalogued, snapshot.local_only) {
        (false, _) => format!("{}, still being catalogued", snapshot.label),
        (true, true) => format!("{}, kept on this computer", snapshot.label),
        (true, false) => snapshot.label.clone(),
    };
    label.update_property(&[gtk4::accessible::Property::Label(&spoken)]);
}
