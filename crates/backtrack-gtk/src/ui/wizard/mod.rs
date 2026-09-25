// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The onboarding wizard: Screen 9, mockups 10 to 13.
//!
//! Four steps, and a fifth page that watches the first backup. Nothing is
//! written until the person commits to it: choices are held in [`Choices`]
//! until the repository is created or, on a second run, until the end. A
//! wizard closed halfway leaves the settings as it found them, which on a
//! second run is the difference between "reconfigure" and "break".
//!
//! One thing has to happen early. The recovery key is the repository's key
//! and does not exist until the repository does, so the repository is created
//! the moment the person asks to save or print the key, on the last page, with
//! everything chosen so far written first. A wizard abandoned after that
//! point leaves a working setup behind rather than half of one.

mod destination;
mod first_backup;

pub use destination::display;
mod import;
mod protect;
mod welcome;
mod what;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use backtrack_core::config::Config;
use gtk4::prelude::*;
use gtk4::{gio, glib, Align, Box as GtkBox, Button, Label, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::{info, warn};

use crate::daemon::Daemon1Proxy;
use crate::model::wizard::{self as model, Choices, Destination, Protect};

/// A repository about to be adopted rather than created.
#[derive(Debug, Clone)]
struct Import {
    repository: String,
    /// Chosen from step 3 on a computer that is setting itself up, so the
    /// choices from step 2 apply to it. Otherwise it came from the Welcome
    /// page: somebody's backups arriving on a new computer, where nothing has
    /// been chosen and restoring comes before backing anything up.
    adopting: bool,
}

pub struct Wizard {
    app: adw::Application,
    window: adw::ApplicationWindow,
    nav: adw::NavigationView,
    toasts: adw::ToastOverlay,
    daemon: Daemon1Proxy<'static>,
    /// On a second run, the configuration it started from.
    before: Option<Config>,
    host: String,
    personal: Vec<PathBuf>,
    choices: RefCell<Choices>,
    destination: RefCell<Option<Destination>>,
    /// The size of what is selected, once measured.
    estimate: Cell<Option<u64>>,
    /// Trips the measurement in flight when the selection changes under it.
    measuring: RefCell<Option<Arc<AtomicBool>>>,
    estimate_watchers: RefCell<Vec<EstimateWatcher>>,
    protect: RefCell<Protect>,
    import: RefCell<Option<Import>>,
}

/// Told the size of the selection whenever it is measured again.
type EstimateWatcher = Box<dyn Fn(Option<u64>)>;

/// Open the wizard.
///
/// `before` is the configuration a second run starts from (Preferences → Run
/// Welcome Wizard Again), and `parent` the window it was opened from; a
/// first run has neither. `start` opens it at one of its pages rather than
/// at the Welcome page, as Storage → Change… does with "where".
pub fn present(
    app: &adw::Application,
    daemon: Daemon1Proxy<'static>,
    before: Option<Config>,
    parent: Option<&gtk4::Window>,
    start: Option<&'static str>,
) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Backtrack")
        .default_width(720)
        .default_height(680)
        .width_request(360)
        .height_request(480)
        .build();
    if let Some(parent) = parent {
        window.set_transient_for(Some(parent));
        window.set_modal(true);
    }

    let personal = personal_folders();
    let choices = match &before {
        Some(config) => Choices::from_config(config, &personal),
        None => Choices::fresh(),
    };
    let destination = before
        .as_ref()
        .and_then(|config| config.storage.repository.clone())
        .map(|repository| Destination::Current { repository });

    let nav = adw::NavigationView::new();
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&nav));
    window.set_content(Some(&toasts));

    let wizard = Rc::new(Wizard {
        app: app.clone(),
        window: window.clone(),
        nav: nav.clone(),
        toasts,
        daemon,
        before,
        host: glib::host_name().to_string(),
        personal,
        choices: RefCell::new(choices),
        destination: RefCell::new(destination),
        estimate: Cell::new(None),
        measuring: RefCell::new(None),
        estimate_watchers: RefCell::new(Vec::new()),
        protect: RefCell::new(Protect::default()),
        import: RefCell::new(None),
    });

    nav.add(&welcome::build(&wizard));
    nav.add(&what::build(&wizard));
    nav.add(&destination::build(&wizard));
    nav.add(&protect::build(&wizard));
    for page in import::build(&wizard) {
        nav.add(&page);
    }
    if let Some(tag) = start {
        nav.push_by_tag(tag);
    }

    // Stop measuring when the window goes: a walk of somebody's home folder
    // is not worth finishing for a wizard nobody is looking at.
    let closing = Rc::clone(&wizard);
    window.connect_close_request(move |_| {
        if let Some(flag) = closing.measuring.borrow_mut().take() {
            flag.store(true, Ordering::Relaxed);
        }
        // The pages' watchers hold the wizard, and the wizard holds them.
        closing.estimate_watchers.borrow_mut().clear();
        glib::Propagation::Proceed
    });

    info!(again = wizard.before.is_some(), "welcome wizard opened");
    window.present();
    wizard.remeasure();
}

/// "My personal files" on this computer.
fn personal_folders() -> Vec<PathBuf> {
    let home = glib::home_dir();
    let special: Vec<Option<PathBuf>> = [
        glib::UserDirectory::Documents,
        glib::UserDirectory::Pictures,
        glib::UserDirectory::Music,
        glib::UserDirectory::Downloads,
        glib::UserDirectory::Desktop,
    ]
    .into_iter()
    .map(glib::user_special_dir)
    .collect();
    model::personal_sources(&home, &special, |path| path.is_dir())
}

/// The parts of a numbered step page that its module fills in.
struct Step {
    page: adw::NavigationPage,
    body: GtkBox,
    next: Button,
}

/// A numbered step: heading and "Step N of 4" across the top, the page's own
/// content, and Back and the way forward along the bottom, as mockups 11 to
/// 13 lay them out.
fn step(tag: &str, heading: &str, number: u32, next: &str) -> Step {
    let body = GtkBox::new(Orientation::Vertical, 18);
    body.set_margin_top(12);
    body.set_margin_bottom(24);
    body.set_margin_start(12);
    body.set_margin_end(12);

    let top = GtkBox::new(Orientation::Horizontal, 12);
    let title = Label::builder()
        .label(heading)
        .halign(Align::Start)
        .hexpand(true)
        .wrap(true)
        .xalign(0.0)
        .build();
    title.add_css_class("title-1");
    top.append(&title);
    let count = Label::new(Some(&format!("Step {number} of 4")));
    count.add_css_class("dim-label");
    count.set_valign(Align::Center);
    top.append(&count);
    body.append(&top);

    let clamp = adw::Clamp::builder().maximum_size(640).child(&body).build();
    let scroller = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(&clamp)
        .build();

    let back = Button::with_label("Back");
    back.set_action_name(Some("navigation.pop"));
    let forward = Button::with_label(next);
    forward.add_css_class("suggested-action");
    let bar = GtkBox::new(Orientation::Horizontal, 12);
    bar.set_margin_top(12);
    bar.set_margin_bottom(12);
    bar.set_margin_start(18);
    bar.set_margin_end(18);
    bar.append(&back);
    let spacer = GtkBox::new(Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    bar.append(&spacer);
    bar.append(&forward);

    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&scroller));
    view.add_bottom_bar(&bar);

    let page = adw::NavigationPage::builder()
        .title("Backtrack")
        .tag(tag)
        .child(&view)
        .build();
    Step {
        page,
        body,
        next: forward,
    }
}

/// An icon in the rounded tile the mockups set row icons in.
fn tile(icon: &str) -> gtk4::Image {
    let image = gtk4::Image::from_icon_name(icon);
    image.set_pixel_size(32);
    image.add_css_class("wizard-tile");
    image.set_valign(Align::Center);
    image
}

impl Wizard {
    /// Say something in passing.
    fn toast(&self, message: &str) {
        self.toasts.add_toast(crate::ui::toast(message));
    }

    /// Whether this run is moving the backups somewhere new.
    fn destination_changed(&self) -> bool {
        self.before.is_some()
            && !matches!(
                *self.destination.borrow(),
                Some(Destination::Current { .. }) | None
            )
    }

    /// Be told when the size estimate changes.
    fn on_estimate(&self, watcher: impl Fn(Option<u64>) + 'static) {
        watcher(self.estimate.get());
        self.estimate_watchers.borrow_mut().push(Box::new(watcher));
    }

    /// Measure what the current choices would back up, abandoning any
    /// measurement already running for choices that no longer apply.
    fn remeasure(self: &Rc<Self>) {
        if let Some(previous) = self.measuring.borrow_mut().take() {
            previous.store(true, Ordering::Relaxed);
        }
        self.set_estimate(None);

        let choices = self.choices.borrow().clone();
        let spec = backtrack_core::walk::WalkSpec {
            sources: choices.include(&self.personal),
            excludes: backtrack_core::pattern::ExcludeSet::compile(&choices.exclude),
            one_file_system: true,
            never: Vec::new(),
            include_dirs: false,
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        *self.measuring.borrow_mut() = Some(Arc::clone(&cancelled));

        let (answer, measured) = async_channel::bounded(1);
        std::thread::Builder::new()
            .name("backtrack-measure".into())
            .spawn(move || {
                let _ = answer.send_blocking(backtrack_core::walk::measure(&spec, &cancelled));
            })
            .map_err(|error| warn!(%error, "the size of the selection cannot be measured"))
            .ok();

        let this = Rc::clone(self);
        crate::ui::spawn(async move {
            if let Ok(Some(found)) = measured.recv().await {
                info!(
                    bytes = found.bytes,
                    files = found.files,
                    "selection measured"
                );
                this.set_estimate(Some(found.bytes));
            }
        });
    }

    fn set_estimate(&self, bytes: Option<u64>) {
        self.estimate.set(bytes);
        for watcher in self.estimate_watchers.borrow().iter() {
            watcher(bytes);
        }
    }

    /// Write whatever the person has chosen that differs from what the daemon
    /// holds now, one setting at a time.
    async fn apply_choices(&self) -> Result<(), String> {
        let text = self.daemon.get_config().await.map_err(|e| explain(&e))?;
        let (current, _) = Config::parse(&text)
            .map_err(|e| format!("The current settings could not be read: {e}"))?;
        let choices = self.choices.borrow().clone();
        let writes = model::changes(&current, &choices, &self.personal)
            .map_err(|e| format!("The settings could not be prepared: {e}"))?;
        for (key, value) in writes {
            info!(key, "writing a wizard setting");
            self.daemon
                .set_config(key, &value)
                .await
                .map_err(|e| explain(&e))?;
        }
        Ok(())
    }

    /// Leave the wizard for the timeline, opened where the backups can be
    /// seen.
    async fn open_timeline(self: &Rc<Self>) {
        let target = timeline_target().await;
        if !crate::window::refresh_all(&target) {
            crate::window::Window::build(&self.app, &target).present();
        }
        self.window.close();
    }
}

/// Where the timeline should open: over the top of the newest backup, which
/// for a new computer's own backups is the home folder and for somebody's
/// imported ones may be anywhere at all.
async fn timeline_target() -> crate::Target {
    let found = gio::spawn_blocking(|| {
        let reader =
            backtrack_core::index::IndexReader::open(&backtrack_core::paths::index_db()).ok()?;
        let seq = reader.latest_catalogued_seq().ok()??;
        let roots = reader.backed_up_roots(seq).ok()?;
        crate::path::common_parent(&roots)
    })
    .await
    .ok()
    .flatten();
    crate::Target {
        folder: found.unwrap_or_else(|| crate::path::to_archive(&glib::home_dir())),
        select: None,
    }
}

/// A failed call to the daemon, in words for the person using the wizard.
///
/// Matched on the error's name, which is the contract; the daemon's own
/// message, meant for logs and bug reports, is the fallback.
pub fn explain(error: &zbus::Error) -> String {
    let (name, message) = match error {
        zbus::Error::MethodError(name, message, _) => (
            name.as_str()
                .rsplit('.')
                .next()
                .unwrap_or_default()
                .to_string(),
            message.clone().unwrap_or_default(),
        ),
        other => (String::new(), other.to_string()),
    };
    match name.as_str() {
        "RepoUnreachable" => "Backtrack could not reach that location.".to_string(),
        "AuthFailed" => "The server did not let Backtrack sign in. Backtrack signs in with \
                         your SSH key, which needs to be set up for this server first."
            .to_string(),
        "PassphraseWrong" => "That passphrase does not open these backups.".to_string(),
        "BorgMissing" => {
            "Borg, the backup engine Backtrack uses, is not installed on this computer.".to_string()
        }
        "DestinationFull" => "There is not enough space at that location.".to_string(),
        _ if !message.is_empty() => capitalise(&message),
        _ => "Something went wrong. The logs say what.".to_string(),
    }
}

fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => {
            let mut out: String = first.to_uppercase().collect();
            out.push_str(chars.as_str());
            if !out.ends_with('.') {
                out.push('.');
            }
            out
        }
        None => String::new(),
    }
}
