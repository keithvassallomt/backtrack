// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! "Already have backups? Import…": find them, open them, and browse.
//!
//! Two ways in. From the Welcome page, somebody's backups arriving on a new
//! computer: nothing has been chosen to back up there yet, and nothing is,
//! because restoring comes first (the guided restore is Stage 11; until then
//! this lands on the timeline). From step 3, a destination that turned out to
//! hold backups already: the choices made on step 2 apply, and backing up
//! carries on into the same repository.
//!
//! Either way the import returns once the newest backup can be browsed, and
//! the rest are catalogued behind it, newest first.

use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, CheckButton, Label, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::{info, warn};

use super::{explain, Import, Wizard};
use crate::model::wizard as model;

pub fn build(wizard: &Rc<Wizard>) -> Vec<adw::NavigationPage> {
    vec![location_page(wizard), passphrase_page(wizard)]
}

/// A plain page: a heading, a line of explanation, and the page's own rows.
fn page(tag: &str, heading: &str, explanation: &str) -> (adw::NavigationPage, GtkBox) {
    let body = GtkBox::new(Orientation::Vertical, 18);
    body.set_margin_top(12);
    body.set_margin_bottom(24);
    body.set_margin_start(12);
    body.set_margin_end(12);
    let title = Label::builder()
        .label(heading)
        .xalign(0.0)
        .wrap(true)
        .build();
    title.add_css_class("title-1");
    body.append(&title);
    let text = Label::builder()
        .label(explanation)
        .xalign(0.0)
        .wrap(true)
        .build();
    body.append(&text);

    let clamp = adw::Clamp::builder().maximum_size(640).child(&body).build();
    let scroller = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&clamp)
        .build();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&scroller));
    let page = adw::NavigationPage::builder()
        .title("Backtrack")
        .tag(tag)
        .child(&view)
        .build();
    (page, body)
}

fn location_page(wizard: &Rc<Wizard>) -> adw::NavigationPage {
    let (page, body) = page(
        "import",
        "Where are your backups?",
        "Choose the folder that holds them, on a drive or a network share, or give the \
         address of the SSH server they are on.",
    );

    let list = gtk4::ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::None);
    list.add_css_class("boxed-list");
    let folder = adw::ActionRow::builder()
        .title("Choose Folder…")
        .subtitle("On a drive, or a network share")
        .activatable(true)
        .build();
    folder.add_prefix(&super::tile("folder-symbolic"));
    folder.add_suffix(&gtk4::Image::from_icon_name("go-next-symbolic"));
    list.append(&folder);
    let ssh = adw::EntryRow::builder()
        .title("SSH server address")
        .show_apply_button(true)
        .build();
    ssh.add_prefix(&super::tile("network-server-symbolic"));
    list.append(&ssh);
    body.append(&list);

    let this = Rc::clone(wizard);
    folder.connect_activated(move |_| {
        let this = Rc::clone(&this);
        crate::ui::spawn(async move {
            let dialog = gtk4::FileDialog::builder()
                .title("Choose the Folder Holding Your Backups")
                .accept_label("Choose")
                .modal(true)
                .build();
            let Ok(chosen) = dialog.select_folder_future(Some(&this.window)).await else {
                return;
            };
            match chosen.path() {
                Some(path) => found(&this, path.to_string_lossy().into_owned()).await,
                None => this.toast("That folder cannot be opened directly"),
            }
        });
    });
    let this = Rc::clone(wizard);
    ssh.connect_apply(move |entry| {
        let this = Rc::clone(&this);
        let typed = entry.text().to_string();
        crate::ui::spawn(async move {
            match model::parse_ssh(&typed) {
                Some(address) => found(&this, address).await,
                None => this.toast("That is not an SSH address. It looks like user@host:path."),
            }
        });
    });
    page
}

/// Check that `repository` really holds backups, and ask for its passphrase.
async fn found(wizard: &Rc<Wizard>, repository: String) {
    match wizard
        .daemon
        .inspect_destination(&repository)
        .await
        .as_deref()
    {
        Ok("existing") => {
            *wizard.import.borrow_mut() = Some(Import {
                repository,
                adopting: false,
            });
            wizard.nav.push_by_tag("passphrase");
        }
        Ok(_) => wizard.toast("No backups were found there. Choose the folder that holds them."),
        Err(error) => {
            warn!(%error, repository, "the backups could not be looked for");
            wizard.toast(&explain(error));
        }
    }
}

fn passphrase_page(wizard: &Rc<Wizard>) -> adw::NavigationPage {
    let (page, body) = page(
        "passphrase",
        "Enter your passphrase",
        "The passphrase these backups were protected with when they were set up.",
    );
    let group = adw::PreferencesGroup::new();
    let entry = adw::PasswordEntryRow::builder().title("Passphrase").build();
    group.add(&entry);
    body.append(&group);
    let remember = CheckButton::with_label("Remember it so backups run automatically");
    remember.set_active(wizard.choices.borrow().remember);
    body.append(&remember);
    let problem = Label::builder()
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .build();
    problem.add_css_class("error");
    body.append(&problem);
    let open = Button::with_label("Open Backups");
    open.add_css_class("pill");
    open.add_css_class("suggested-action");
    open.set_halign(Align::Center);
    open.set_sensitive(false);
    body.append(&open);

    let enabler = open.clone();
    entry.connect_changed(move |entry| enabler.set_sensitive(!entry.text().is_empty()));
    let button = open.clone();
    entry.connect_entry_activated(move |_| button.emit_clicked());
    let this = Rc::clone(wizard);
    remember.connect_toggled(move |check| {
        this.choices.borrow_mut().remember = check.is_active();
    });

    let this = Rc::clone(wizard);
    open.connect_clicked(move |button| {
        let this = Rc::clone(&this);
        let passphrase = entry.text().to_string();
        let button = button.clone();
        let problem = problem.clone();
        crate::ui::spawn(async move {
            button.set_sensitive(false);
            button.set_label("Opening…");
            problem.set_visible(false);
            let outcome = open_backups(&this, &passphrase).await;
            button.set_label("Open Backups");
            button.set_sensitive(true);
            match outcome {
                Ok(()) => this.open_timeline().await,
                Err(message) => {
                    problem.set_label(&message);
                    problem.set_visible(true);
                }
            }
        });
    });
    page
}

/// Adopt the chosen repository with `passphrase`.
async fn open_backups(wizard: &Rc<Wizard>, passphrase: &str) -> Result<(), String> {
    let Some(import) = wizard.import.borrow().clone() else {
        return Err("Choose where your backups are first.".to_string());
    };
    // Where the passphrase is kept is settled before it is handed over, so it
    // lands in the keyring or in memory as the box says.
    let remember = wizard.choices.borrow().remember;
    wizard
        .daemon
        .set_config("security.remember_passphrase", &remember.to_string())
        .await
        .map_err(|e| explain(&e))?;
    info!(repository = import.repository, "importing backups");
    wizard
        .daemon
        .import_repo(&import.repository, passphrase)
        .await
        .map_err(|e| explain(&e))?;
    if import.adopting {
        wizard.apply_choices().await?;
    }
    Ok(())
}
