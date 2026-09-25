// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Step 4, mockup 13: the passphrase, the recovery key, and Start.
//!
//! The order on this page is forced by what a recovery key is. It is the
//! repository's key, so the repository is created the first time Save or
//! Print is pressed, with the passphrase as typed, and the passphrase fields
//! lock from then on. Start stays unavailable until the key has been saved or
//! printed: [`Protect`] holds that rule, and this page only draws it.
//!
//! A second run that keeps the same destination has no passphrase to set and
//! no key it must keep: the passphrase fields are hidden, Save and Print stay
//! on offer for whoever wants another copy, and the last button saves the
//! changes rather than starting a first backup.

use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, CheckButton, Label, LevelBar, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::{info, warn};

use super::{explain, Wizard};
use crate::model::wizard::{self as model, Protect, Strength};
use crate::ui::schedule;

/// How the key is kept.
#[derive(Debug, Clone, Copy)]
enum Keep {
    Save,
    Print,
}

struct Page {
    wizard: Rc<Wizard>,
    secrets: GtkBox,
    passphrase: adw::PasswordEntryRow,
    confirm: adw::PasswordEntryRow,
    meter: LevelBar,
    meter_label: Label,
    remember: CheckButton,
    save: Button,
    print: Button,
    caption: Label,
    start: Button,
    /// Set while the repository is being created or the key fetched, so a
    /// second press cannot start a second creation.
    busy: std::cell::Cell<bool>,
}

pub fn build(wizard: &Rc<Wizard>) -> adw::NavigationPage {
    let step = super::step("protect", "Protect your backups", 4, "Start First Backup");

    let secrets = GtkBox::new(Orientation::Vertical, 12);
    let fields = adw::PreferencesGroup::new();
    let passphrase = adw::PasswordEntryRow::builder().title("Passphrase").build();
    let confirm = adw::PasswordEntryRow::builder().title("Confirm").build();
    fields.add(&passphrase);
    fields.add(&confirm);
    secrets.append(&fields);

    let strength = GtkBox::new(Orientation::Horizontal, 12);
    let meter = LevelBar::builder()
        .mode(gtk4::LevelBarMode::Discrete)
        .min_value(0.0)
        .max_value(f64::from(Strength::SEGMENTS))
        .hexpand(true)
        .valign(Align::Center)
        .build();
    meter.add_css_class("strength");
    let meter_label = Label::new(None);
    meter_label.set_width_chars(7);
    meter_label.set_xalign(0.0);
    meter_label.add_css_class("heading");
    strength.append(&meter);
    strength.append(&meter_label);
    secrets.append(&strength);

    let remember = CheckButton::with_label("Remember it so backups run automatically");
    remember.set_active(wizard.choices.borrow().remember);
    secrets.append(&remember);
    step.body.append(&secrets);

    step.body.append(&crate::ui::recovery::warning_card());

    let buttons = GtkBox::new(Orientation::Horizontal, 12);
    buttons.set_homogeneous(true);
    let save = icon_button("document-save-symbolic", "Save Recovery Key…");
    let print = icon_button("printer-symbolic", "Print…");
    buttons.append(&save);
    buttons.append(&print);
    step.body.append(&buttons);
    let caption = Label::new(Some("Required before continuing"));
    caption.add_css_class("dim-label");
    caption.add_css_class("caption");
    step.body.append(&caption);

    let advanced = gtk4::ListBox::new();
    advanced.set_selection_mode(gtk4::SelectionMode::None);
    advanced.add_css_class("boxed-list");
    advanced.append(&schedule_expander(wizard));
    step.body.append(&advanced);

    let page = Rc::new(Page {
        wizard: Rc::clone(wizard),
        secrets,
        passphrase,
        confirm,
        meter,
        meter_label,
        remember,
        save,
        print,
        caption,
        start: step.next.clone(),
        busy: std::cell::Cell::new(false),
    });

    for entry in [&page.passphrase, &page.confirm] {
        let this = Rc::clone(&page);
        entry.connect_changed(move |_| this.typed());
    }
    let this = Rc::clone(&page);
    page.remember.connect_toggled(move |check| {
        this.wizard.choices.borrow_mut().remember = check.is_active();
    });
    for (button, how) in [(&page.save, Keep::Save), (&page.print, Keep::Print)] {
        let this = Rc::clone(&page);
        button.connect_clicked(move |_| {
            let this = Rc::clone(&this);
            crate::ui::spawn(async move { this.keep(how).await });
        });
    }
    let this = Rc::clone(&page);
    page.start.connect_clicked(move |_| {
        let this = Rc::clone(&this);
        crate::ui::spawn(async move { this.finish().await });
    });
    // What the page asks for depends on the destination chosen a page ago,
    // which can change every time the person comes back through it.
    let this = Rc::clone(&page);
    step.page.connect_showing(move |_| this.refresh());

    step.page
}

fn icon_button(icon: &str, label: &str) -> Button {
    let content = adw::ButtonContent::builder()
        .icon_name(icon)
        .label(label)
        .use_underline(false)
        .build();
    Button::builder().child(&content).build()
}

/// "Advanced: schedule & retention", collapsed to the mockup's one-line
/// summary, open to the same controls Preferences has.
fn schedule_expander(wizard: &Rc<Wizard>) -> adw::ExpanderRow {
    let choices = wizard.choices.borrow().clone();
    // Plain text: row titles are markup by default, and the ampersand in
    // the mockup's wording is not an entity.
    let expander = adw::ExpanderRow::builder()
        .title("Advanced: schedule & retention")
        .use_markup(false)
        .subtitle(model::schedule_summary(
            choices.frequency,
            &choices.retention,
        ))
        .build();
    let summary = expander.clone();
    let this = Rc::clone(wizard);
    let refresh = Rc::new(move || {
        let choices = this.choices.borrow();
        summary.set_subtitle(&model::schedule_summary(
            choices.frequency,
            &choices.retention,
        ));
    });

    let rows = schedule::Rows::build(
        &schedule::Values {
            frequency: choices.frequency,
            on_battery: choices.on_battery,
            on_metered: choices.on_metered,
            retention: choices.retention,
        },
        {
            let wizard = Rc::clone(wizard);
            let refresh = Rc::clone(&refresh);
            move |values: &schedule::Values| {
                let mut choices = wizard.choices.borrow_mut();
                choices.frequency = values.frequency;
                choices.on_battery = values.on_battery;
                choices.on_metered = values.on_metered;
                choices.retention = values.retention;
                drop(choices);
                refresh();
            }
        },
    );
    for row in rows.all() {
        expander.add_row(&row);
    }
    expander
}

impl Page {
    /// Whether this run creates a repository, and so has a passphrase to set
    /// and a key that must be kept.
    fn creating(&self) -> bool {
        self.wizard.before.is_none() || self.wizard.destination_changed()
    }

    fn repository(&self) -> Option<String> {
        self.wizard
            .destination
            .borrow()
            .as_ref()
            .map(|destination| destination.repository(&self.wizard.host))
    }

    fn typed(&self) {
        {
            let mut protect = self.wizard.protect.borrow_mut();
            protect.passphrase = self.passphrase.text().to_string();
            protect.confirm = self.confirm.text().to_string();
        }
        let strength = model::strength(&self.passphrase.text(), &[&self.wizard.host]);
        self.meter.set_value(f64::from(strength.level));
        self.meter_label.set_label(strength.label);
        for label in ["weak", "fair", "good", "strong"] {
            self.meter.remove_css_class(label);
            self.meter_label.remove_css_class(label);
        }
        if !strength.label.is_empty() {
            self.meter.add_css_class(strength.label);
            self.meter_label.add_css_class(strength.label);
        }
        self.refresh();
    }

    /// Bring every control into line with [`Protect`] and the destination.
    fn refresh(&self) {
        let creating = self.creating();
        let protect: Protect = self.wizard.protect.borrow().clone();
        self.secrets.set_visible(creating);
        for entry in [&self.passphrase, &self.confirm] {
            entry.set_sensitive(protect.editable());
        }
        self.remember.set_sensitive(protect.editable());

        let busy = self.busy.get();
        let can_keep = !creating || protect.can_keep_key();
        self.save.set_sensitive(can_keep && !busy);
        self.print.set_sensitive(can_keep && !busy);

        if creating {
            self.caption.set_visible(!protect.can_start());
            self.start.set_label("Start First Backup");
            self.start.set_sensitive(protect.can_start() && !busy);
        } else {
            self.caption.set_visible(false);
            self.start.set_label("Save Changes");
            self.start.set_sensitive(!busy);
        }
    }

    /// Save or print the key, creating the repository first if this is the
    /// first time either has been asked for.
    async fn keep(self: &Rc<Self>, how: Keep) {
        let Some(repository) = self.repository() else {
            return;
        };
        self.busy.set(true);
        self.refresh();
        let outcome = self.keep_key(how, &repository).await;
        self.busy.set(false);
        match outcome {
            Ok(true) => {
                self.wizard.protect.borrow_mut().kept();
                self.wizard.toast(match how {
                    Keep::Save => "Recovery key saved",
                    Keep::Print => "Recovery key sent to the printer",
                });
            }
            Ok(false) => {}
            Err(message) => self.wizard.toast(&message),
        }
        self.refresh();
    }

    async fn keep_key(&self, how: Keep, repository: &str) -> Result<bool, String> {
        let needs_creating =
            self.creating() && self.wizard.protect.borrow().created.as_deref() != Some(repository);
        if needs_creating {
            self.create(repository).await?;
        }
        let key = self
            .wizard
            .daemon
            .export_recovery_key()
            .await
            .map_err(|e| explain(&e))?;
        let window = self.wizard.window.upcast_ref::<gtk4::Window>();
        match how {
            Keep::Save => crate::ui::recovery::save(window, &key, &self.wizard.host).await,
            Keep::Print => {
                crate::ui::recovery::print(window, &key, repository, &self.wizard.host).await
            }
        }
    }

    /// Create the repository, with everything chosen so far written first so
    /// that a wizard abandoned from here on leaves a working setup behind.
    async fn create(&self, repository: &str) -> Result<(), String> {
        let passphrase = self.wizard.protect.borrow().passphrase.clone();
        self.wizard.apply_choices().await?;
        info!(repository, "creating the repository");
        self.wizard
            .daemon
            .setup_repo(repository, &passphrase)
            .await
            .map_err(|e| {
                warn!(error = %e, repository, "the repository could not be created");
                explain(&e)
            })?;
        self.wizard.protect.borrow_mut().created(repository);
        Ok(())
    }

    /// The last button: start the first backup, or on a second run that
    /// keeps its destination, save and leave.
    async fn finish(self: &Rc<Self>) {
        self.busy.set(true);
        self.refresh();
        let applied = self.wizard.apply_choices().await;
        self.busy.set(false);
        if let Err(message) = applied {
            self.wizard.toast(&message);
            self.refresh();
            return;
        }
        if !self.creating() {
            info!("second run finished; settings saved");
            self.wizard.window.close();
            return;
        }
        super::first_backup::start(&self.wizard).await;
        self.refresh();
    }
}
