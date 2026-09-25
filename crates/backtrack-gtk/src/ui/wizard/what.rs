// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Step 2, mockup 11: personal files or a choice of folders, with the size of
//! the selection measured as it changes, and the exclusions behind an
//! expander.

use std::path::PathBuf;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{gio, glib, Align, CheckButton, Label};
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::info;

use super::Wizard;
use crate::model::wizard::Sources;
use crate::ui::exclusions::Editor;

/// The mockup's description of "My personal files".
const PERSONAL: &str = "Documents, pictures, music, downloads, desktop";
const CHOOSE: &str = "Pick exactly which folders to protect";

struct Page {
    wizard: Rc<Wizard>,
    personal: adw::ActionRow,
    personal_radio: CheckButton,
    chosen: adw::ActionRow,
    chosen_radio: CheckButton,
}

pub fn build(wizard: &Rc<Wizard>) -> adw::NavigationPage {
    let step = super::step("what", "What should be backed up?", 2, "Continue");

    let list = gtk4::ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::None);
    list.add_css_class("boxed-list-separate");

    let personal_radio = radio();
    let personal = adw::ActionRow::builder()
        .title("My personal files")
        .subtitle(PERSONAL)
        .activatable(true)
        .build();
    personal.add_prefix(&super::tile("user-home-symbolic"));
    let badge = Label::new(Some("Recommended"));
    badge.add_css_class("badge");
    badge.add_css_class("recommended");
    badge.set_valign(Align::Center);
    personal.add_suffix(&badge);
    personal.add_suffix(&personal_radio);
    list.append(&personal);

    let chosen_radio = radio();
    chosen_radio.set_group(Some(&personal_radio));
    let chosen = adw::ActionRow::builder()
        .title("Let me choose folders…")
        .subtitle(CHOOSE)
        .activatable(true)
        .build();
    chosen.add_prefix(&super::tile("folder-symbolic"));
    chosen.add_suffix(&chosen_radio);
    list.append(&chosen);
    step.body.append(&list);

    let advanced = gtk4::ListBox::new();
    advanced.set_selection_mode(gtk4::SelectionMode::None);
    advanced.add_css_class("boxed-list");
    let expander = adw::ExpanderRow::builder()
        .title("Advanced: exclusions")
        .subtitle("Trash, caches, node_modules and VM images are excluded by default")
        .build();
    expander.add_prefix(&gtk4::Image::from_icon_name("emblem-system-symbolic"));
    advanced.append(&expander);
    step.body.append(&advanced);
    let editing = Rc::clone(wizard);
    Editor::new(
        expander,
        wizard.choices.borrow().exclude.clone(),
        move |patterns| {
            editing.choices.borrow_mut().exclude = patterns.to_vec();
            editing.remeasure();
        },
    );

    let page = Rc::new(Page {
        wizard: Rc::clone(wizard),
        personal,
        personal_radio,
        chosen,
        chosen_radio,
    });
    page.show_sources();

    let picker = Rc::clone(&page);
    page.personal
        .connect_activated(move |_| picker.use_personal());
    let chooser = Rc::clone(&page);
    page.chosen
        .connect_activated(move |_| chooser.choose_folders());
    let sizes = Rc::clone(&page);
    wizard.on_estimate(move |_| sizes.show_sources());

    let onward = Rc::clone(wizard);
    step.next.connect_clicked(move |_| {
        if onward.choices.borrow().include(&onward.personal).is_empty() {
            onward.toast("Choose at least one folder to back up");
            return;
        }
        onward.nav.push_by_tag("where");
    });
    step.page
}

/// A radio button that reports the row's state rather than taking clicks of
/// its own, so the whole row is the target and there is one way to choose.
fn radio() -> CheckButton {
    let radio = CheckButton::new();
    radio.set_can_target(false);
    radio.set_focusable(false);
    radio.set_valign(Align::Center);
    radio
}

impl Page {
    fn use_personal(&self) {
        self.wizard.choices.borrow_mut().sources = Sources::Personal;
        self.show_sources();
        self.wizard.remeasure();
    }

    /// Ask which folders, and switch to them if any were chosen. Cancelling
    /// leaves the previous choice exactly as it was.
    fn choose_folders(self: &Rc<Self>) {
        let this = Rc::clone(self);
        crate::ui::spawn(async move {
            let dialog = gtk4::FileDialog::builder()
                .title("Choose Folders to Back Up")
                .accept_label("Choose")
                .modal(true)
                .build();
            let Ok(chosen) = dialog
                .select_multiple_folders_future(Some(&this.wizard.window))
                .await
            else {
                return;
            };
            let folders: Vec<PathBuf> = (0..chosen.n_items())
                .filter_map(|i| chosen.item(i))
                .filter_map(|item| item.downcast::<gio::File>().ok())
                .filter_map(|file| file.path())
                .collect();
            if folders.is_empty() {
                return;
            }
            info!(count = folders.len(), "folders chosen to back up");
            this.wizard.choices.borrow_mut().sources = Sources::Chosen(folders);
            this.show_sources();
            this.wizard.remeasure();
        });
    }

    /// Put the choice, and the size of it once known, on screen.
    fn show_sources(&self) {
        let size = self
            .wizard
            .estimate
            .get()
            .map(|bytes| format!(" — {}", glib::format_size(bytes)))
            .unwrap_or_default();
        match &self.wizard.choices.borrow().sources {
            Sources::Personal => {
                self.personal_radio.set_active(true);
                self.personal.set_subtitle(&format!("{PERSONAL}{size}"));
                self.chosen.set_subtitle(CHOOSE);
            }
            Sources::Chosen(folders) => {
                self.chosen_radio.set_active(true);
                self.personal.set_subtitle(PERSONAL);
                let names: Vec<String> = folders
                    .iter()
                    .map(|folder| {
                        folder
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| folder.display().to_string())
                    })
                    .collect();
                self.chosen.set_subtitle(&glib::markup_escape_text(&format!(
                    "{}{size}",
                    names.join(", ")
                )));
            }
        }
    }
}
