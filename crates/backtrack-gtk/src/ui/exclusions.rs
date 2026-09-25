// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The exclusion list, as rows that can be removed and a row that adds one.
//!
//! One editor for two places: the wizard's "Advanced: exclusions" expander,
//! which edits the wizard's own choices, and Preferences → Backup, which
//! writes each change straight to the daemon. What happens to a change is the
//! caller's; the editor only reports the new list.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, Button};
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::model::exclusions::{self, Row};

/// Where the rows go.
#[derive(Clone)]
pub enum Container {
    Expander(adw::ExpanderRow),
    Group(adw::PreferencesGroup),
}

impl Container {
    fn add(&self, row: &impl IsA<gtk4::Widget>) {
        match self {
            Container::Expander(expander) => expander.add_row(row),
            Container::Group(group) => group.add(row),
        }
    }

    fn remove(&self, row: &impl IsA<gtk4::Widget>) {
        match self {
            Container::Expander(expander) => expander.remove(row),
            Container::Group(group) => group.remove(row),
        }
    }
}

/// The list and its rows.
pub struct Editor {
    container: Container,
    patterns: RefCell<Vec<String>>,
    shown: RefCell<Vec<adw::ActionRow>>,
    add: adw::ButtonRow,
    changed: Changed,
}

/// Told the whole new list after every change.
type Changed = Box<dyn Fn(&[String])>;

impl Editor {
    /// Fill `container` with `patterns`, and call `changed` with the whole
    /// new list whenever a row is added or removed.
    pub fn new(
        container: Container,
        patterns: Vec<String>,
        changed: impl Fn(&[String]) + 'static,
    ) -> Rc<Editor> {
        let add = adw::ButtonRow::builder()
            .title("Add Exclusion…")
            .start_icon_name("list-add-symbolic")
            .build();
        let editor = Rc::new(Editor {
            container,
            patterns: RefCell::new(patterns),
            shown: RefCell::new(Vec::new()),
            add,
            changed: Box::new(changed),
        });
        let asker = Rc::clone(&editor);
        editor.add.connect_activated(move |row| asker.ask(row));
        editor.render();
        editor
    }

    /// Show a different list, as when the daemon's copy is re-read.
    pub fn set(self: &Rc<Self>, patterns: Vec<String>) {
        *self.patterns.borrow_mut() = patterns;
        self.render();
    }

    fn render(self: &Rc<Self>) {
        for row in self.shown.borrow_mut().drain(..) {
            self.container.remove(&row);
        }
        if self.add.parent().is_some() {
            self.container.remove(&self.add);
        }
        let patterns = self.patterns.borrow().clone();
        for entry in exclusions::rows(&patterns) {
            let row = self.row(entry);
            self.container.add(&row);
            self.shown.borrow_mut().push(row);
        }
        self.container.add(&self.add);
    }

    fn row(self: &Rc<Self>, entry: Row) -> adw::ActionRow {
        // A pattern is whatever somebody typed, and `&` or `<` in it is not
        // markup.
        let row = adw::ActionRow::builder()
            .title(&entry.label)
            .use_markup(false)
            .build();
        let remove = Button::from_icon_name("window-close-symbolic");
        remove.set_valign(Align::Center);
        remove.add_css_class("flat");
        remove.add_css_class("circular");
        let spoken = format!("Stop excluding {}", entry.label);
        remove.set_tooltip_text(Some(&spoken));
        remove.update_property(&[gtk4::accessible::Property::Label(&spoken)]);
        row.add_suffix(&remove);

        let editor = Rc::clone(self);
        remove.connect_clicked(move |_| {
            let left = exclusions::without(&editor.patterns.borrow(), &entry);
            editor.commit(left);
        });
        row
    }

    /// Ask for a new exclusion.
    fn ask(self: &Rc<Self>, anchor: &adw::ButtonRow) {
        let entry = gtk4::Entry::builder()
            .placeholder_text("node_modules, *.tmp or /home/you/Videos")
            .activates_default(true)
            .build();
        let dialog = adw::AlertDialog::builder()
            .heading("Add Exclusion")
            .body(
                "A name leaves out everything called that, anywhere. A pattern such as \
                 *.tmp leaves out every file that matches it. A folder's full path \
                 leaves out that folder.",
            )
            .extra_child(&entry)
            .default_response("add")
            .close_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", "Cancel"), ("add", "Add")]);
        dialog.set_response_appearance("add", adw::ResponseAppearance::Suggested);
        dialog.set_response_enabled("add", false);
        let enabler = dialog.clone();
        entry.connect_changed(move |entry| {
            enabler.set_response_enabled("add", exclusions::from_input(&entry.text()).is_some());
        });

        let editor = Rc::clone(self);
        dialog.connect_response(Some("add"), move |_, _| {
            let Some(pattern) = exclusions::from_input(&entry.text()) else {
                return;
            };
            let mut list = editor.patterns.borrow().clone();
            if !list.contains(&pattern) {
                list.push(pattern);
                editor.commit(list);
            }
        });
        dialog.present(Some(anchor));
    }

    fn commit(self: &Rc<Self>, patterns: Vec<String>) {
        *self.patterns.borrow_mut() = patterns;
        self.render();
        (self.changed)(&self.patterns.borrow());
    }
}
