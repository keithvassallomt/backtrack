// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Step 3, mockup 12: where backups go.
//!
//! Drives are found rather than asked for: GIO knows what is plugged in and
//! how much room it has, and the list follows drives coming and going while
//! the page is open. A network folder is mounted through GIO and then used as
//! the folder GIO makes of it, which is what "network folder" means to Borg.
//! An SSH server is tested before it is accepted, by the daemon, since the
//! daemon is what will be connecting to it every hour.
//!
//! Nothing is created here. Continue asks the daemon what is at the chosen
//! place first, so that a drive already holding this computer's backups leads
//! to them instead of to a second repository on top.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{gio, glib, Align, Box as GtkBox, Button, CheckButton, Label, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::{info, warn};

use super::{explain, Import, Wizard};
use crate::model::wizard::{self as model, Destination, Fit};

const OFFLINE_NOTE: &str = "If this location isn't reachable, Backtrack keeps protecting your \
                            changes on this computer and catches up when it reconnects.";

/// A removable drive GIO has mounted.
struct Drive {
    mount: PathBuf,
    name: String,
    free: Option<u64>,
}

struct Page {
    wizard: Rc<Wizard>,
    list: gtk4::ListBox,
    /// The group every row's radio belongs to. Invisible: a group needs a
    /// member that is never on screen so that none of the visible ones has
    /// to be chosen before the person has chosen.
    group: CheckButton,
    current: Option<(adw::ActionRow, CheckButton)>,
    /// Every row the drive list put on screen, the placeholder included.
    drive_rows: RefCell<Vec<adw::ActionRow>>,
    drives: RefCell<Vec<(CheckButton, Drive)>>,
    network: adw::ActionRow,
    network_radio: CheckButton,
    ssh: adw::ActionRow,
    ssh_radio: CheckButton,
    space: GtkBox,
    space_icon: gtk4::Image,
    space_label: Label,
    next: Button,
    /// Kept so GIO keeps telling us about drives.
    monitor: gio::VolumeMonitor,
}

pub fn build(wizard: &Rc<Wizard>) -> adw::NavigationPage {
    let step = super::step("where", "Where should backups go?", 3, "Continue");

    let list = gtk4::ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::None);
    list.add_css_class("boxed-list");
    step.body.append(&list);

    let group = CheckButton::new();
    let current = wizard.before.as_ref().and_then(|before| {
        let repository = before.storage.repository.clone()?;
        let radio = radio(&group);
        let row = adw::ActionRow::builder()
            .title("Keep the current location")
            .subtitle(glib::markup_escape_text(&display(&repository)))
            .activatable(true)
            .build();
        row.add_prefix(&super::tile("emblem-system-symbolic"));
        row.add_suffix(&radio);
        list.append(&row);
        Some((row, radio))
    });

    let network_radio = radio(&group);
    network_radio.set_visible(false);
    let network = adw::ActionRow::builder()
        .title("Network folder")
        .subtitle("SMB or NFS share")
        .activatable(true)
        .build();
    network.add_prefix(&super::tile("folder-remote-symbolic"));
    network.add_suffix(&network_radio);
    network.add_suffix(&gtk4::Image::from_icon_name("go-next-symbolic"));

    let ssh_radio = radio(&group);
    ssh_radio.set_visible(false);
    let ssh = adw::ActionRow::builder()
        .title("SSH server")
        .subtitle("Any Linux box, NAS, or BorgBase account")
        .activatable(true)
        .build();
    ssh.add_prefix(&super::tile("network-server-symbolic"));
    ssh.add_suffix(&ssh_radio);
    ssh.add_suffix(&gtk4::Image::from_icon_name("go-next-symbolic"));

    let cloud = adw::ActionRow::builder()
        .title("Cloud storage")
        .subtitle("Coming later")
        .sensitive(false)
        .build();
    cloud.add_prefix(&super::tile("weather-overcast-symbolic"));
    cloud.add_suffix(&gtk4::Image::from_icon_name("go-next-symbolic"));

    list.append(&network);
    list.append(&ssh);
    list.append(&cloud);

    let space = GtkBox::new(Orientation::Horizontal, 12);
    space.set_margin_start(12);
    let space_icon = gtk4::Image::new();
    let space_label = Label::builder().xalign(0.0).wrap(true).build();
    space.append(&space_icon);
    space.append(&space_label);
    space.set_visible(false);
    step.body.append(&space);

    let note = GtkBox::new(Orientation::Horizontal, 12);
    note.set_margin_start(12);
    let info = gtk4::Image::from_icon_name("dialog-information-symbolic");
    info.set_valign(Align::Start);
    note.append(&info);
    let text = Label::builder()
        .label(OFFLINE_NOTE)
        .xalign(0.0)
        .wrap(true)
        .hexpand(true)
        .build();
    note.append(&text);
    step.body.append(&note);

    let page = Rc::new(Page {
        wizard: Rc::clone(wizard),
        list,
        group,
        current,
        drive_rows: RefCell::new(Vec::new()),
        drives: RefCell::new(Vec::new()),
        network,
        network_radio,
        ssh,
        ssh_radio,
        space,
        space_icon,
        space_label,
        next: step.next.clone(),
        monitor: gio::VolumeMonitor::get(),
    });

    page.show_drives();
    let watcher = Rc::clone(&page);
    page.monitor
        .connect_mount_added(move |_, _| watcher.show_drives());
    let watcher = Rc::clone(&page);
    page.monitor
        .connect_mount_removed(move |_, _| watcher.show_drives());
    let watcher = Rc::clone(&page);
    page.monitor
        .connect_mount_changed(move |_, _| watcher.show_drives());

    if let Some((row, radio)) = &page.current {
        radio.set_active(true);
        let chooser = Rc::clone(&page);
        let keep = page.wizard.destination.borrow().clone();
        row.connect_activated(move |_| {
            if let Some(Destination::Current { repository }) = &keep {
                chooser.choose(Destination::Current {
                    repository: repository.clone(),
                });
            }
        });
    }
    let networker = Rc::clone(&page);
    page.network
        .connect_activated(move |_| networker.wizard.nav.push_by_tag("network"));
    let sshed = Rc::clone(&page);
    page.ssh
        .connect_activated(move |_| sshed.wizard.nav.push_by_tag("ssh"));

    let sizes = Rc::clone(&page);
    wizard.on_estimate(move |_| sizes.show_space());

    let onward = Rc::clone(&page);
    step.next.connect_clicked(move |_| {
        let page = Rc::clone(&onward);
        crate::ui::spawn(async move { page.go_on().await });
    });

    wizard.nav.add(&network_page(&page));
    wizard.nav.add(&ssh_page(&page));
    page.refresh();
    step.page
}

/// A radio in the page's group that shows the row's state; the row is what
/// takes the click.
fn radio(group: &CheckButton) -> CheckButton {
    let radio = CheckButton::new();
    radio.set_group(Some(group));
    radio.set_can_target(false);
    radio.set_focusable(false);
    radio.set_valign(Align::Center);
    radio
}

/// A repository location as a person would recognise it: a network share's
/// address rather than the folder GIO mounted it on.
pub fn display(repository: &str) -> String {
    let path = std::path::Path::new(repository);
    if path.is_absolute() {
        let uri = gio::File::for_path(path).uri().to_string();
        if !uri.starts_with("file://") {
            return uri;
        }
    }
    repository.to_string()
}

/// The removable drives mounted right now, with the room each one has.
fn removable_drives(monitor: &gio::VolumeMonitor) -> Vec<Drive> {
    monitor
        .mounts()
        .into_iter()
        .filter(|mount| !mount.is_shadowed())
        .filter(|mount| {
            mount
                .drive()
                .is_some_and(|drive| drive.is_removable() || drive.is_media_removable())
        })
        .filter_map(|mount| {
            let root = mount.root();
            let path = root.path()?;
            Some(Drive {
                free: free_space(&root),
                name: mount.name().to_string(),
                mount: path,
            })
        })
        .collect()
}

fn free_space(file: &gio::File) -> Option<u64> {
    file.query_filesystem_info(gio::FILE_ATTRIBUTE_FILESYSTEM_FREE, gio::Cancellable::NONE)
        .ok()
        .map(|info| info.attribute_uint64(gio::FILE_ATTRIBUTE_FILESYSTEM_FREE))
}

impl Page {
    /// Rebuild the drive rows from what is plugged in now.
    fn show_drives(self: &Rc<Self>) {
        for row in self.drive_rows.borrow_mut().drain(..) {
            self.list.remove(&row);
        }
        self.drives.borrow_mut().clear();
        let chosen = self.wizard.destination.borrow().clone();
        let first = i32::from(self.current.is_some());
        let drives = removable_drives(&self.monitor);
        if drives.is_empty() {
            let row = adw::ActionRow::builder()
                .title("External drive")
                .subtitle("Plug in a drive to use it")
                .sensitive(false)
                .build();
            row.add_prefix(&super::tile("drive-harddisk-usb-symbolic"));
            self.list.insert(&row, first);
            self.drive_rows.borrow_mut().push(row);
        }
        for (position, drive) in (first..).zip(drives) {
            let radio = radio(&self.group);
            let free = drive
                .free
                .map(|bytes| format!(" — {} free", glib::format_size(bytes)))
                .unwrap_or_default();
            let row = adw::ActionRow::builder()
                .title("External drive")
                .subtitle(glib::markup_escape_text(&format!("{}{free}", drive.name)))
                .activatable(true)
                .build();
            row.add_prefix(&super::tile("drive-harddisk-usb-symbolic"));
            let badge = Label::new(Some("Detected"));
            badge.add_css_class("badge");
            badge.add_css_class("recommended");
            badge.set_valign(Align::Center);
            row.add_suffix(&badge);
            row.add_suffix(&radio);
            let this = Rc::clone(self);
            let destination = Destination::Drive {
                mount: drive.mount.clone(),
                name: drive.name.clone(),
            };
            if chosen.as_ref() == Some(&destination) {
                radio.set_active(true);
            }
            row.connect_activated(move |_| this.choose(destination.clone()));
            self.list.insert(&row, position);
            self.drive_rows.borrow_mut().push(row);
            self.drives.borrow_mut().push((radio, drive));
        }
        // A drive that was chosen and has since been unplugged is no longer
        // a choice.
        if let Some(Destination::Drive { mount, .. }) = &chosen {
            if !self.drives.borrow().iter().any(|(_, d)| &d.mount == mount) {
                *self.wizard.destination.borrow_mut() = None;
                self.group.set_active(true);
            }
        }
        self.refresh();
    }

    fn choose(self: &Rc<Self>, destination: Destination) {
        info!(?destination, "destination chosen");
        *self.wizard.destination.borrow_mut() = Some(destination);
        self.refresh();
    }

    /// Bring the radios, the addresses and the space line into step with the
    /// chosen destination.
    fn refresh(&self) {
        let chosen = self.wizard.destination.borrow().clone();
        match &chosen {
            Some(Destination::Current { .. }) => {
                if let Some((_, radio)) = &self.current {
                    radio.set_active(true);
                }
            }
            Some(Destination::Drive { mount, .. }) => {
                if let Some((radio, _)) = self
                    .drives
                    .borrow()
                    .iter()
                    .find(|(_, drive)| &drive.mount == mount)
                {
                    radio.set_active(true);
                }
            }
            Some(Destination::Network { address, .. }) => {
                self.network_radio.set_visible(true);
                self.network_radio.set_active(true);
                self.network
                    .set_subtitle(&glib::markup_escape_text(address));
            }
            Some(Destination::Ssh { address }) => {
                self.ssh_radio.set_visible(true);
                self.ssh_radio.set_active(true);
                self.ssh.set_subtitle(&glib::markup_escape_text(address));
            }
            None => self.group.set_active(true),
        }
        self.next.set_sensitive(chosen.is_some());
        self.show_space();
    }

    /// The space check: what is needed against what the destination has.
    /// Shown only when both are known; a remote server's free space is not
    /// something this side can find out, and a guess would be worse than
    /// silence.
    fn show_space(&self) {
        let chosen = self.wizard.destination.borrow().clone();
        let free = chosen.as_ref().and_then(|destination| match destination {
            Destination::Drive { mount, .. } => self
                .drives
                .borrow()
                .iter()
                .find(|(_, drive)| &drive.mount == mount)
                .and_then(|(_, drive)| drive.free),
            other => other.local_root().and_then(|root| {
                let existing = root.ancestors().find(|p| p.exists())?;
                free_space(&gio::File::for_path(existing))
            }),
        });
        let (Some(needed), Some(free)) = (self.wizard.estimate.get(), free) else {
            self.space.set_visible(false);
            return;
        };
        let (fit, lead, numbers) = model::space_check(needed, free);
        let (icon, class) = match fit {
            Fit::Plenty => ("object-select-symbolic", "success"),
            Fit::Short => ("dialog-warning-symbolic", "warning"),
        };
        self.space_icon.set_icon_name(Some(icon));
        for label in [
            self.space_label.upcast_ref::<gtk4::Widget>(),
            self.space_icon.upcast_ref(),
        ] {
            label.remove_css_class("success");
            label.remove_css_class("warning");
            label.add_css_class(class);
        }
        self.space_label.set_markup(&format!(
            "<b>{}</b> {}",
            glib::markup_escape_text(lead),
            glib::markup_escape_text(&numbers)
        ));
        self.space.set_visible(true);
    }

    /// Continue: find out what is at the chosen place, then go the way that
    /// answer leads.
    async fn go_on(self: &Rc<Self>) {
        let Some(destination) = self.wizard.destination.borrow().clone() else {
            return;
        };
        if let Destination::Current { .. } = destination {
            self.wizard.nav.push_by_tag("protect");
            return;
        }
        let repository = destination.repository(&self.wizard.host);
        self.next.set_sensitive(false);
        let answer = self.wizard.daemon.inspect_destination(&repository).await;
        self.next.set_sensitive(true);
        match answer.as_deref() {
            Ok("empty") => {
                if self.wizard.before.is_some() && !self.confirm_move().await {
                    return;
                }
                self.wizard.protect.borrow_mut().retarget(&repository);
                self.wizard.nav.push_by_tag("protect");
            }
            Ok("existing") => {
                if self.offer_existing().await {
                    *self.wizard.import.borrow_mut() = Some(Import {
                        repository,
                        adopting: true,
                    });
                    self.wizard.nav.push_by_tag("passphrase");
                }
            }
            Ok("occupied") => self.wizard.toast(
                "That folder already holds other files. Choose an empty folder or another place.",
            ),
            Ok("unwritable") => self
                .wizard
                .toast("Backtrack is not allowed to write there. Choose another place."),
            Ok(other) => warn!(answer = other, "unexpected answer about a destination"),
            Err(error) => {
                warn!(%error, repository, "the destination could not be inspected");
                self.wizard.toast(&explain(error));
            }
        }
    }

    /// Offer the backups found at the chosen place, in the wireframe's words.
    async fn offer_existing(&self) -> bool {
        let dialog = adw::AlertDialog::builder()
            .heading("Existing Backtrack/Borg backup found — use it?")
            .body(
                "There are already backups at this location. Backtrack can open them and \
                 carry on backing up there, or you can choose another place.",
            )
            .default_response("use")
            .close_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", "Choose Another"), ("use", "Use It")]);
        dialog.set_response_appearance("use", adw::ResponseAppearance::Suggested);
        crate::ui::prefer_wide_responses(&dialog);
        dialog.choose_future(Some(&self.wizard.window)).await == "use"
    }

    /// Moving the backups on a second run: the new place starts empty, and
    /// the old backups stay where they are. Said before anything is created,
    /// with the old place named.
    async fn confirm_move(&self) -> bool {
        let old = self
            .wizard
            .before
            .as_ref()
            .and_then(|before| before.storage.repository.clone())
            .map(|repository| display(&repository))
            .unwrap_or_default();
        let dialog = adw::AlertDialog::builder()
            .heading("Start Backing Up Somewhere New?")
            .body(format!(
                "Backups at the new location start fresh, with a new passphrase and \
                 recovery key. The backups you already have stay at {old}, untouched, and \
                 you can open them again at any time with Import."
            ))
            .default_response("cancel")
            .close_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", "Cancel"), ("move", "Use New Location")]);
        dialog.set_response_appearance("move", adw::ResponseAppearance::Suggested);
        crate::ui::prefer_wide_responses(&dialog);
        dialog.choose_future(Some(&self.wizard.window)).await == "move"
    }
}

/// The sub-page for a network folder: an address, mounted through GIO.
fn network_page(page: &Rc<Page>) -> adw::NavigationPage {
    let (nav_page, entry, button, problem) = address_page(
        "network",
        "Network folder",
        "The address of a shared folder, such as smb://nas.local/backups or \
         nfs://nas.local/backups. A folder on this computer works too.",
        "Connect",
    );
    let this = Rc::clone(page);
    let field = entry.clone();
    button.connect_clicked(move |button| {
        let this = Rc::clone(&this);
        let address = field.text().trim().to_string();
        let button = button.clone();
        let problem = problem.clone();
        crate::ui::spawn(async move {
            button.set_sensitive(false);
            problem.set_visible(false);
            match mount(&this.wizard.window, &address).await {
                Ok(path) => {
                    info!(address, path = %path.display(), "network folder ready");
                    this.choose(Destination::Network { address, path });
                    this.wizard.nav.pop();
                }
                Err(message) => {
                    problem.set_label(&message);
                    problem.set_visible(true);
                }
            }
            button.set_sensitive(true);
        });
    });
    nav_page
}

/// Mount `address` if it needs mounting, and hand back the local folder it
/// becomes.
async fn mount(window: &adw::ApplicationWindow, address: &str) -> Result<PathBuf, String> {
    if address.is_empty() {
        return Err("Type the folder's address first.".to_string());
    }
    let file = if address.starts_with('/') {
        gio::File::for_path(address)
    } else {
        gio::File::for_uri(address)
    };
    if let Some(path) = file.path().filter(|path| path.is_dir()) {
        return Ok(path);
    }
    let operation = gtk4::MountOperation::new(Some(window));
    if let Err(error) = file
        .mount_enclosing_volume_future(gio::MountMountFlags::NONE, Some(&operation))
        .await
    {
        if !error.matches(gio::IOErrorEnum::AlreadyMounted) {
            warn!(%error, address, "the network folder could not be mounted");
            return Err(format!(
                "Backtrack could not connect to that folder: {error}"
            ));
        }
    }
    match file.path() {
        Some(path) if path.is_dir() => Ok(path),
        _ => Err("That location cannot be used as a folder on this computer.".to_string()),
    }
}

/// The sub-page for an SSH server: an address, tested by the daemon.
fn ssh_page(page: &Rc<Page>) -> adw::NavigationPage {
    let (nav_page, entry, button, problem) = address_page(
        "ssh",
        "SSH server",
        "user@host:path, or the full ssh:// address a service such as BorgBase gives \
         you. Backtrack signs in with your SSH key, so it needs to be set up for this \
         server already.",
        "Test Connection",
    );
    let this = Rc::clone(page);
    let field = entry.clone();
    button.connect_clicked(move |button| {
        let this = Rc::clone(&this);
        let typed = field.text().to_string();
        let button = button.clone();
        let problem = problem.clone();
        crate::ui::spawn(async move {
            problem.set_visible(false);
            let Some(address) = model::parse_ssh(&typed) else {
                problem.set_label("That is not an SSH address. It looks like user@host:path.");
                problem.set_visible(true);
                return;
            };
            button.set_sensitive(false);
            let answer = this.wizard.daemon.inspect_destination(&address).await;
            button.set_sensitive(true);
            match answer {
                Ok(_) => {
                    info!(address, "SSH server answered");
                    this.choose(Destination::Ssh { address });
                    this.wizard.nav.pop();
                }
                Err(error) => {
                    warn!(%error, address, "the SSH server did not answer");
                    problem.set_label(&explain(&error));
                    problem.set_visible(true);
                }
            }
        });
    });
    nav_page
}

/// An address to type and a button to try it, for the two kinds of
/// destination that are not simply plugged in.
fn address_page(
    tag: &str,
    title: &str,
    description: &str,
    action: &str,
) -> (adw::NavigationPage, adw::EntryRow, Button, Label) {
    let entry = adw::EntryRow::builder().title("Address").build();
    let group = adw::PreferencesGroup::builder()
        .description(description)
        .build();
    group.add(&entry);

    let problem = Label::builder()
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .build();
    problem.add_css_class("error");
    let button = Button::with_label(action);
    button.add_css_class("pill");
    button.add_css_class("suggested-action");
    button.set_halign(Align::Center);
    let button_for_entry = button.clone();
    entry.connect_entry_activated(move |_| button_for_entry.emit_clicked());

    let body = GtkBox::new(Orientation::Vertical, 18);
    body.set_margin_top(24);
    body.set_margin_bottom(24);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.append(&group);
    body.append(&problem);
    body.append(&button);
    let clamp = adw::Clamp::builder().maximum_size(560).child(&body).build();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&clamp));
    let page = adw::NavigationPage::builder()
        .title(title)
        .tag(tag)
        .child(&view)
        .build();
    (page, entry, button, problem)
}
