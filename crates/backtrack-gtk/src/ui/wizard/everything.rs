// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! "Welcome back. Restore this computer?": Screen 12, mockup 21.
//!
//! Offered once somebody's backups have been imported onto a computer from
//! the Welcome page, when the newest backup can be browsed. Three ways on:
//! everything, as of a backup chosen from the dropdown; only some of its
//! folders; or none of it for now, and the timeline. The first two hand over
//! to the daemon and to the progress window, and the wizard is done.

use std::cell::RefCell;
use std::rc::Rc;

use backtrack_core::index::IndexReader;
use backtrack_core::recovery;
use gtk4::prelude::*;
use gtk4::{gio, glib, Align, Box as GtkBox, Button, CheckButton, Label, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::{info, warn};

use super::{explain, Wizard};
use crate::model::everything::{self as copy, Snapshot};

/// What the page offers, read from the catalogue.
struct Offer {
    /// Backups that can be restored from, newest first.
    snapshots: Vec<Snapshot>,
    /// Every backup at the destination, browsable yet or not.
    count: usize,
    user: Option<String>,
    host: Option<String>,
    /// What restoring the newest would bring back.
    bytes: u64,
    /// Room on the disk holding the home folder.
    free: Option<u64>,
}

/// Offer to restore this computer, or go straight to the timeline when there
/// is nothing in the backups to offer.
pub async fn offer(wizard: &Rc<Wizard>) {
    let home = glib::home_dir();
    let user = glib::user_name().to_string_lossy().into_owned();
    let found = gio::spawn_blocking(move || read_offer(&home, &user))
        .await
        .ok()
        .flatten();
    match found {
        Some(offer) => {
            info!(
                snapshots = offer.snapshots.len(),
                bytes = offer.bytes,
                "offering to restore this computer"
            );
            wizard.nav.push(&page(wizard, Rc::new(offer)));
        }
        None => {
            info!("nothing in the backups to offer; opening the timeline");
            wizard.open_timeline().await;
        }
    }
}

fn read_offer(home: &std::path::Path, user: &str) -> Option<Offer> {
    let reader = IndexReader::open(&backtrack_core::paths::index_db())
        .map_err(|error| warn!(%error, "the catalogue could not be read"))
        .ok()?;
    let (snapshots, count) = restorable(&reader)?;
    let latest = snapshots.first()?;
    let layout = recovery::layout(&reader, latest.seq, home, user).ok()??;
    if layout.steps.is_empty() {
        return None;
    }
    Some(Offer {
        free: backtrack_core::restore::free_space(home).ok(),
        host: copy::host_of(&latest.name),
        user: copy::user_of(&layout.source),
        bytes: layout.bytes(),
        count,
        snapshots,
    })
}

/// The backups that can be restored from, newest first, and how many there
/// are at the destination altogether.
fn restorable(reader: &IndexReader) -> Option<(Vec<Snapshot>, usize)> {
    let archives: Vec<_> = reader
        .archives_overview()
        .ok()?
        .into_iter()
        .filter(|a| a.repo == "primary")
        .collect();
    let snapshots = archives
        .iter()
        .filter(|a| a.catalogued)
        .map(|a| Snapshot {
            seq: a.seq,
            name: a.name.clone(),
            ts: a.ts,
        })
        .collect();
    Some((snapshots, archives.len()))
}

/// Add each backup to the dropdown as it becomes restorable.
///
/// The page appears as soon as the newest backup is catalogued, which on a
/// repository of any age is long before the rest are; a dropdown holding one
/// backup under a line saying there are forty-seven would read as forty-six
/// lost. What is chosen stays chosen as the list grows.
fn keep_up(
    dropdown: &gtk4::DropDown,
    model: &gtk4::StringList,
    snapshots: &Rc<RefCell<Vec<Snapshot>>>,
    count: usize,
) {
    if snapshots.borrow().len() >= count {
        return;
    }
    let dropdown = dropdown.downgrade();
    let model = model.clone();
    let snapshots = Rc::clone(snapshots);
    let reading = Rc::new(std::cell::Cell::new(false));
    glib::timeout_add_seconds_local(2, move || {
        let Some(dropdown) = dropdown.upgrade() else {
            return glib::ControlFlow::Break;
        };
        if snapshots.borrow().len() >= count {
            return glib::ControlFlow::Break;
        }
        if reading.replace(true) {
            return glib::ControlFlow::Continue;
        }
        let model = model.clone();
        let snapshots = Rc::clone(&snapshots);
        let reading = Rc::clone(&reading);
        crate::ui::spawn(async move {
            let found = gio::spawn_blocking(|| {
                let reader = IndexReader::open(&backtrack_core::paths::index_db()).ok()?;
                restorable(&reader).map(|(snapshots, _)| snapshots)
            })
            .await
            .ok()
            .flatten();
            reading.set(false);
            let Some(found) = found.filter(|f| f.len() > snapshots.borrow().len()) else {
                return;
            };
            let chosen = snapshots
                .borrow()
                .get(dropdown.selected() as usize)
                .map(|s| s.name.clone());
            let now = glib::DateTime::now_local()
                .map(|t| t.to_unix())
                .unwrap_or_default();
            let tz = glib::TimeZone::local();
            let labels: Vec<String> = found.iter().map(|s| copy::when(s.ts, now, &tz)).collect();
            let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
            model.splice(0, model.n_items(), &labels);
            let keep = found
                .iter()
                .position(|s| Some(&s.name) == chosen.as_ref())
                .unwrap_or(0);
            dropdown.set_selected(keep as u32);
            *snapshots.borrow_mut() = found;
        });
        glib::ControlFlow::Continue
    });
}

/// Which way on the person has chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Way {
    Everything,
    Selected,
    Browse,
}

fn page(wizard: &Rc<Wizard>, offer: Rc<Offer>) -> adw::NavigationPage {
    let body = GtkBox::new(Orientation::Vertical, 18);
    body.set_margin_top(24);
    body.set_margin_bottom(24);
    body.set_margin_start(12);
    body.set_margin_end(12);

    let title = Label::builder()
        .label(copy::WELCOME)
        .wrap(true)
        .justify(gtk4::Justification::Center)
        .build();
    title.add_css_class("title-1");
    body.append(&title);
    let now = glib::DateTime::now_local()
        .map(|t| t.to_unix())
        .unwrap_or_default();
    let tz = glib::TimeZone::local();
    let found = Label::builder()
        .label(copy::found_line(
            offer.user.as_deref(),
            offer.host.as_deref(),
            offer.snapshots[0].ts,
            offer.bytes,
            offer.count,
            now,
            &tz,
        ))
        .wrap(true)
        .justify(gtk4::Justification::Center)
        .build();
    found.add_css_class("dim-label");
    body.append(&found);
    if let Some(warning) = offer
        .free
        .and_then(|free| copy::room_warning(offer.bytes, free))
    {
        let room = Label::builder()
            .label(warning)
            .wrap(true)
            .justify(gtk4::Justification::Center)
            .build();
        room.add_css_class("warning");
        body.append(&room);
    }

    let list = gtk4::ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::None);
    list.add_css_class("boxed-list-separate");
    list.set_margin_top(6);

    // Restore everything, as of a backup chosen here.
    let everything = CheckButton::new();
    everything.set_active(true);
    everything.set_valign(Align::Start);
    let first = GtkBox::new(Orientation::Horizontal, 18);
    first.append(&everything);
    let text = GtkBox::new(Orientation::Vertical, 12);
    let heading = GtkBox::new(Orientation::Horizontal, 12);
    heading.append(&choice_label(copy::EVERYTHING));
    let badge = Label::new(Some("Recommended"));
    badge.add_css_class("badge");
    badge.add_css_class("recommended");
    badge.set_valign(Align::Center);
    heading.append(&badge);
    text.append(&heading);
    let when = GtkBox::new(Orientation::Horizontal, 12);
    let as_of = Label::new(Some("Home folder as of"));
    as_of.add_css_class("dim-label");
    when.append(&as_of);
    let labels: Vec<String> = offer
        .snapshots
        .iter()
        .map(|s| copy::when(s.ts, now, &tz))
        .collect();
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    let model = gtk4::StringList::new(&labels);
    let snapshot = gtk4::DropDown::new(Some(model.clone()), gtk4::Expression::NONE);
    snapshot.set_valign(Align::Center);
    let snapshots = Rc::new(RefCell::new(offer.snapshots.clone()));
    keep_up(&snapshot, &model, &snapshots, offer.count);
    when.append(&snapshot);
    text.append(&when);
    first.append(&text);
    list.append(&card(&first));
    // The backup chosen here is the one "Restore everything" restores, and
    // only that: it sits in that choice's card, so it reads as part of it, and
    // greys out when another choice is made. The folder checklist has a
    // dropdown of its own.
    everything
        .bind_property("active", &when, "sensitive")
        .sync_create()
        .build();

    let selected = CheckButton::new();
    selected.set_group(Some(&everything));
    list.append(&card(&choice(&selected, copy::SELECTED)));
    let browse = CheckButton::new();
    browse.set_group(Some(&everything));
    list.append(&card(&choice(&browse, copy::BROWSE)));
    let radios = [everything.clone(), selected.clone(), browse.clone()];
    list.connect_row_activated(move |_, row| {
        if let Some(radio) = radios.get(row.index() as usize) {
            radio.set_active(true);
        }
    });
    body.append(&list);

    let promises = GtkBox::new(Orientation::Horizontal, 12);
    promises.set_margin_top(6);
    let info = gtk4::Image::from_icon_name("dialog-information-symbolic");
    info.set_valign(Align::Start);
    promises.append(&info);
    let promise = Label::builder()
        .label(copy::PROMISES)
        .xalign(0.0)
        .wrap(true)
        .build();
    promise.add_css_class("dim-label");
    promises.append(&promise);
    body.append(&promises);

    let clamp = adw::Clamp::builder().maximum_size(720).child(&body).build();
    let scroller = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(&clamp)
        .build();

    let back = Button::with_label("Back");
    back.set_action_name(Some("navigation.pop"));
    let start = Button::with_label("Start Restore");
    start.add_css_class("suggested-action");
    let bar = GtkBox::new(Orientation::Horizontal, 12);
    bar.set_margin_top(12);
    bar.set_margin_bottom(12);
    bar.set_margin_start(18);
    bar.set_margin_end(18);
    bar.append(&back);
    let spacer = GtkBox::new(Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    bar.append(&spacer);
    bar.append(&start);

    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&scroller));
    view.add_bottom_bar(&bar);
    let page = adw::NavigationPage::builder()
        .title("Backtrack")
        .tag("everything")
        .child(&view)
        .build();

    let way = Rc::new(RefCell::new(Way::Everything));
    for (radio, chosen, label) in [
        (&everything, Way::Everything, "Start Restore"),
        (&selected, Way::Selected, "Choose Folders…"),
        (&browse, Way::Browse, "Browse Backups"),
    ] {
        let way = Rc::clone(&way);
        let start = start.clone();
        radio.connect_toggled(move |radio| {
            if radio.is_active() {
                *way.borrow_mut() = chosen;
                start.set_label(label);
            }
        });
    }

    let this = Rc::clone(wizard);
    let count = offer.count;
    start.connect_clicked(move |button| {
        let snapshots = Rc::clone(&snapshots);
        let Some(chosen) = snapshots
            .borrow()
            .get(snapshot.selected() as usize)
            .cloned()
        else {
            return;
        };
        let this = Rc::clone(&this);
        let button = button.clone();
        let way = *way.borrow();
        crate::ui::spawn(async move {
            match way {
                Way::Browse => this.open_timeline().await,
                Way::Selected => {
                    // From the newest backup, whatever the dropdown above says:
                    // that dropdown is "Restore everything"'s.
                    let newest = snapshots.borrow().clone();
                    let Some(seq) = newest.first().map(|s| s.seq) else {
                        return;
                    };
                    let steps = load_folders(seq).await;
                    if steps.is_empty() {
                        this.toast("That backup has no folders to choose from");
                        return;
                    }
                    this.nav.push(&folders_page(&this, newest, count, steps));
                }
                Way::Everything => {
                    button.set_sensitive(false);
                    let started = this.daemon.restore_everything(&chosen.name, "ask").await;
                    button.set_sensitive(true);
                    match started {
                        Ok(job) => {
                            info!(job, archive = chosen.name, "restoring this computer");
                            this.hand_over();
                        }
                        Err(error) => {
                            warn!(%error, "the restore could not start");
                            this.toast(&explain(&error));
                        }
                    }
                }
            }
        });
    });
    page
}

/// The folders of backup `seq`, and what each would bring back.
async fn load_folders(seq: i64) -> Vec<recovery::Step> {
    let home = glib::home_dir();
    let user = glib::user_name().to_string_lossy().into_owned();
    gio::spawn_blocking(move || {
        let reader = IndexReader::open(&backtrack_core::paths::index_db()).ok()?;
        Some(recovery::layout(&reader, seq, &home, &user).ok()??.steps)
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_default()
}

/// "Restore selected folders…": a checklist of the folders in a backup, and a
/// dropdown of its own for which backup that is.
struct Folders {
    snapshots: Rc<RefCell<Vec<Snapshot>>>,
    dropdown: gtk4::DropDown,
    /// Holds the checklist, which is rebuilt when another backup is chosen.
    rows: GtkBox,
    ticks: RefCell<Vec<(String, CheckButton)>>,
    /// Folders the person has unticked, kept unticked across a change of
    /// backup: choosing an older one should not quietly put Videos back.
    unticked: RefCell<std::collections::HashSet<String>>,
    start: Button,
    /// Which reading of the catalogue is the latest, so a slow one for a
    /// backup no longer chosen cannot overwrite a newer one.
    reading: std::cell::Cell<u64>,
}

fn folders_page(
    wizard: &Rc<Wizard>,
    snapshots: Vec<Snapshot>,
    count: usize,
    steps: Vec<recovery::Step>,
) -> adw::NavigationPage {
    let body = GtkBox::new(Orientation::Vertical, 18);
    body.set_margin_top(24);
    body.set_margin_bottom(24);
    body.set_margin_start(12);
    body.set_margin_end(12);
    let title = Label::builder()
        .label("Choose what to restore")
        .xalign(0.0)
        .wrap(true)
        .build();
    title.add_css_class("title-1");
    body.append(&title);
    let promise = Label::builder()
        .label("Files already on this computer are never overwritten without asking.")
        .xalign(0.0)
        .wrap(true)
        .build();
    promise.add_css_class("dim-label");
    body.append(&promise);

    let now = glib::DateTime::now_local()
        .map(|t| t.to_unix())
        .unwrap_or_default();
    let tz = glib::TimeZone::local();
    let labels: Vec<String> = snapshots
        .iter()
        .map(|s| copy::when(s.ts, now, &tz))
        .collect();
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    let model = gtk4::StringList::new(&labels);
    let dropdown = gtk4::DropDown::new(Some(model.clone()), gtk4::Expression::NONE);
    dropdown.set_valign(Align::Center);
    let when = GtkBox::new(Orientation::Horizontal, 12);
    let as_of = Label::new(Some("Folders as of"));
    as_of.add_css_class("dim-label");
    when.append(&as_of);
    when.append(&dropdown);
    body.append(&when);
    let snapshots = Rc::new(RefCell::new(snapshots));
    keep_up(&dropdown, &model, &snapshots, count);

    let rows = GtkBox::new(Orientation::Vertical, 0);
    body.append(&rows);

    let clamp = adw::Clamp::builder().maximum_size(640).child(&body).build();
    let scroller = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(&clamp)
        .build();
    let back = Button::with_label("Back");
    back.set_action_name(Some("navigation.pop"));
    let start = Button::with_label("Start Restore");
    start.add_css_class("suggested-action");
    let bar = GtkBox::new(Orientation::Horizontal, 12);
    bar.set_margin_top(12);
    bar.set_margin_bottom(12);
    bar.set_margin_start(18);
    bar.set_margin_end(18);
    bar.append(&back);
    let spacer = GtkBox::new(Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    bar.append(&spacer);
    bar.append(&start);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&scroller));
    view.add_bottom_bar(&bar);

    let page = Rc::new(Folders {
        snapshots,
        dropdown,
        rows,
        ticks: RefCell::new(Vec::new()),
        unticked: RefCell::new(std::collections::HashSet::new()),
        start,
        reading: std::cell::Cell::new(0),
    });
    page.show(&steps);

    let this = Rc::clone(&page);
    page.dropdown
        .connect_selected_notify(move |_| this.reload());

    let this = Rc::clone(&page);
    let wizard = Rc::clone(wizard);
    page.start.connect_clicked(move |button| {
        let Some(chosen) = this
            .snapshots
            .borrow()
            .get(this.dropdown.selected() as usize)
            .cloned()
        else {
            return;
        };
        let keys: Vec<String> = this
            .ticks
            .borrow()
            .iter()
            .filter(|(_, t)| t.is_active())
            .map(|(key, _)| key.clone())
            .collect();
        let wizard = Rc::clone(&wizard);
        let button = button.clone();
        crate::ui::spawn(async move {
            button.set_sensitive(false);
            let started = wizard
                .daemon
                .restore_folders(&chosen.name, &keys, "ask")
                .await;
            button.set_sensitive(true);
            match started {
                Ok(job) => {
                    info!(job, archive = chosen.name, folders = ?keys, "restoring some of this computer");
                    wizard.hand_over();
                }
                Err(error) => {
                    warn!(%error, "the restore could not start");
                    wizard.toast(&explain(&error));
                }
            }
        });
    });

    adw::NavigationPage::builder()
        .title("Backtrack")
        .tag("everything-folders")
        .child(&view)
        .build()
}

impl Folders {
    /// Read the folders of the backup now chosen, and show them.
    fn reload(self: &Rc<Self>) {
        let Some(seq) = self
            .snapshots
            .borrow()
            .get(self.dropdown.selected() as usize)
            .map(|s| s.seq)
        else {
            return;
        };
        let reading = self.reading.get() + 1;
        self.reading.set(reading);
        self.start.set_sensitive(false);
        let this = Rc::clone(self);
        crate::ui::spawn(async move {
            let steps = load_folders(seq).await;
            if this.reading.get() == reading {
                this.show(&steps);
            }
        });
    }

    /// The checklist for `steps`, ticked except where the person has
    /// unticked a folder of the same name.
    fn show(self: &Rc<Self>, steps: &[recovery::Step]) {
        while let Some(child) = self.rows.first_child() {
            self.rows.remove(&child);
        }
        let group = adw::PreferencesGroup::new();
        let mut ticks = Vec::with_capacity(steps.len());
        for step in steps {
            let tick = CheckButton::new();
            tick.set_active(!self.unticked.borrow().contains(&step.key));
            tick.set_valign(Align::Center);
            let row = adw::ActionRow::builder()
                .title(copy::step_name(&step.key))
                .subtitle(if step.key == copy::THE_REST {
                    format!(
                        "Hidden folders and the files beside your folders · {}",
                        copy::size(step.bytes)
                    )
                } else {
                    copy::size(step.bytes)
                })
                .activatable_widget(&tick)
                .build();
            row.add_prefix(&tick);
            group.add(&row);
            let this = Rc::downgrade(self);
            let key = step.key.clone();
            tick.connect_toggled(move |tick| {
                let Some(this) = this.upgrade() else { return };
                if tick.is_active() {
                    this.unticked.borrow_mut().remove(&key);
                } else {
                    this.unticked.borrow_mut().insert(key.clone());
                }
                this.update_start();
            });
            ticks.push((step.key.clone(), tick));
        }
        self.rows.append(&group);
        *self.ticks.borrow_mut() = ticks;
        self.update_start();
    }

    fn update_start(&self) {
        self.start
            .set_sensitive(self.ticks.borrow().iter().any(|(_, t)| t.is_active()));
    }
}

/// A choice's own words, in the size the mockup sets them.
fn choice_label(text: &str) -> Label {
    let label = Label::builder().label(text).xalign(0.0).wrap(true).build();
    label.add_css_class("title-4");
    label
}

fn choice(radio: &CheckButton, text: &str) -> GtkBox {
    radio.set_valign(Align::Center);
    let row = GtkBox::new(Orientation::Horizontal, 18);
    row.append(radio);
    row.append(&choice_label(text));
    row
}

/// One of the three separate cards the mockup lays the choices out as.
fn card(content: &GtkBox) -> gtk4::ListBoxRow {
    content.set_margin_top(18);
    content.set_margin_bottom(18);
    content.set_margin_start(18);
    content.set_margin_end(18);
    gtk4::ListBoxRow::builder()
        .child(content)
        .activatable(true)
        .build()
}

impl Wizard {
    /// The restore is the daemon's now: watch it in its own window, and close
    /// this one.
    fn hand_over(&self) {
        crate::ui::everything::present(&self.app, self.daemon.clone());
        self.window.close();
    }
}
