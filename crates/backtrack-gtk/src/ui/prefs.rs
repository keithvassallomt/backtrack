// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Preferences: Screen 10, mockups 15 to 19.
//!
//! A window of its own with the pages down a sidebar, as the mockups draw it.
//! `AdwPreferencesWindow`, which the stage file names, has been deprecated
//! since libadwaita 1.6, and its replacement puts the pages in a switcher
//! across the top; a split view with libadwaita's view-switcher sidebar is how
//! the mockups' layout is built now.
//!
//! Every control writes its setting to the daemon the moment it changes. There
//! is no Apply button because there is nothing to apply: `config.toml` has one
//! writer, and it is the daemon. Controls are bound through [`Setting`], and
//! the window checks as it is built that every setting has exactly one.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use backtrack_core::config::{self, Config};
use backtrack_core::dbus::{Status, StorageInfo};
use futures::StreamExt;
use gtk4::prelude::*;
use gtk4::{gio, glib, Align, Box as GtkBox, Button, Label, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;
use serde::Serialize;
use tracing::{error, info, warn};

use crate::daemon::Daemon1Proxy;
use crate::model::prefs::{self as model, Setting};
use crate::ui::exclusions::{Container, Editor};
use crate::ui::schedule;
use crate::ui::wizard::display;

thread_local! {
    /// The one Preferences window, so a second "Preferences" raises it
    /// rather than opening another that could disagree with it.
    static OPEN: RefCell<Option<adw::Window>> = const { RefCell::new(None) };
}

type Show = Box<dyn Fn(&Config)>;
type ShowStatus = Box<dyn Fn(&Status)>;
type ShowStorage = Box<dyn Fn(Result<&StorageInfo, &str>)>;

struct Prefs {
    app: adw::Application,
    parent: adw::ApplicationWindow,
    window: adw::Window,
    toasts: adw::ToastOverlay,
    /// The content side, which pushes the Home folder's list as a page.
    nav: adw::NavigationView,
    daemon: Daemon1Proxy<'static>,
    config: RefCell<Config>,
    /// Set while controls are being filled in from the configuration, so that
    /// filling them in is not mistaken for somebody changing them.
    loading: Cell<bool>,
    shows: RefCell<Vec<Show>>,
    status_shows: RefCell<Vec<ShowStatus>>,
    storage_shows: RefCell<Vec<ShowStorage>>,
    bound: RefCell<Vec<Setting>>,
}

/// Open Preferences over `parent`, or raise it if it is already open.
pub fn present(
    app: &adw::Application,
    parent: &adw::ApplicationWindow,
    daemon: Daemon1Proxy<'static>,
) {
    if let Some(open) = OPEN.with(|open| open.borrow().clone()) {
        open.present();
        return;
    }
    let app = app.clone();
    let parent = parent.clone();
    crate::ui::spawn(async move {
        let config = match read_config(&daemon).await {
            Ok(config) => config,
            Err(message) => {
                warn!(message, "Preferences could not read the settings");
                return;
            }
        };
        build(app, parent, daemon, config).present();
    });
}

async fn read_config(daemon: &Daemon1Proxy<'static>) -> Result<Config, String> {
    let text = daemon.get_config().await.map_err(|e| e.to_string())?;
    Config::parse(&text).map(|(config, _)| config)
}

fn build(
    app: adw::Application,
    parent: adw::ApplicationWindow,
    daemon: Daemon1Proxy<'static>,
    config: Config,
) -> adw::Window {
    let window = adw::Window::builder()
        .title("Preferences")
        .transient_for(&parent)
        .default_width(900)
        .default_height(680)
        .width_request(360)
        .height_request(400)
        .build();
    let toasts = adw::ToastOverlay::new();
    let nav = adw::NavigationView::new();
    let prefs = Rc::new(Prefs {
        app,
        parent,
        window: window.clone(),
        toasts: toasts.clone(),
        nav: nav.clone(),
        daemon,
        config: RefCell::new(config),
        loading: Cell::new(false),
        shows: RefCell::new(Vec::new()),
        status_shows: RefCell::new(Vec::new()),
        storage_shows: RefCell::new(Vec::new()),
        bound: RefCell::new(Vec::new()),
    });

    let stack = adw::ViewStack::new();
    for (name, title, icon, page) in [
        (
            "general",
            "General",
            "preferences-system-symbolic",
            prefs.general(),
        ),
        (
            "backup",
            "Backup",
            "document-open-recent-symbolic",
            prefs.backup(),
        ),
        (
            "storage",
            "Storage",
            "drive-harddisk-symbolic",
            prefs.storage(),
        ),
        (
            "security",
            "Security",
            "security-high-symbolic",
            prefs.security(),
        ),
        (
            "advanced",
            "Advanced",
            "applications-engineering-symbolic",
            prefs.advanced(),
        ),
    ] {
        stack.add_titled_with_icon(&page, Some(name), title, icon);
    }
    prefs.check_bindings();

    let sidebar = adw::ViewSwitcherSidebar::builder().stack(&stack).build();
    let sidebar_scroller = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&sidebar)
        .build();
    let sidebar_view = adw::ToolbarView::new();
    sidebar_view.add_top_bar(&adw::HeaderBar::new());
    sidebar_view.set_content(Some(&sidebar_scroller));
    let sidebar_page = adw::NavigationPage::builder()
        .title("Preferences")
        .child(&sidebar_view)
        .build();

    let pages_view = adw::ToolbarView::new();
    pages_view.add_top_bar(&adw::HeaderBar::new());
    pages_view.set_content(Some(&stack));
    let pages = adw::NavigationPage::builder()
        .title("General")
        .tag("pages")
        .child(&pages_view)
        .build();
    let titled = pages.clone();
    stack.connect_visible_child_notify(move |stack| {
        if let Some(child) = stack.visible_child() {
            if let Some(title) = stack.page(&child).title() {
                titled.set_title(&title);
            }
        }
    });
    nav.add(&pages);
    let content = adw::NavigationPage::builder()
        .title("Preferences")
        .child(&nav)
        .build();

    let split = adw::NavigationSplitView::builder()
        .sidebar(&sidebar_page)
        .content(&content)
        .min_sidebar_width(200.0)
        .max_sidebar_width(260.0)
        .build();
    let shower = split.clone();
    sidebar.connect_activated(move |_| shower.set_show_content(true));
    let narrow = adw::Breakpoint::new(
        adw::BreakpointCondition::parse("max-width: 560sp").expect("a valid breakpoint"),
    );
    narrow.add_setter(&split, "collapsed", Some(&true.to_value()));
    window.add_breakpoint(narrow);

    toasts.set_child(Some(&split));
    window.set_content(Some(&toasts));

    // Coming back to the window after the wizard, or after changing
    // something elsewhere, shows what is true now.
    let refresher = Rc::clone(&prefs);
    window.connect_is_active_notify(move |window| {
        if window.is_active() {
            let refresher = Rc::clone(&refresher);
            crate::ui::spawn(async move { refresher.reload().await });
        }
    });
    window.connect_close_request(|_| {
        OPEN.with(|open| *open.borrow_mut() = None);
        glib::Propagation::Proceed
    });
    OPEN.with(|open| *open.borrow_mut() = Some(window.clone()));

    let first = Rc::clone(&prefs);
    crate::ui::spawn(async move {
        first.refresh_status().await;
        first.refresh_storage().await;
    });
    info!("preferences opened");
    window
}

impl Prefs {
    fn toast(&self, message: &str) {
        self.toasts.add_toast(crate::ui::toast(message));
    }

    /// Record that `setting` now has a control.
    fn bind(&self, setting: Setting) {
        self.bound.borrow_mut().push(setting);
    }

    /// The other half of the settings cross-check: every setting has a
    /// control, and none has two. The first half is a test against the
    /// configuration schema; this half can only be checked by building the
    /// window, so it is checked every time the window is built.
    fn check_bindings(&self) {
        let bound = self.bound.borrow();
        for setting in Setting::ALL {
            let times = bound.iter().filter(|b| **b == setting).count();
            if times != 1 {
                error!(
                    key = setting.key(),
                    times, "a setting does not have exactly one control"
                );
            }
            debug_assert_eq!(times, 1, "{} has {times} controls", setting.key());
        }
    }

    /// Keep something on screen in step with the configuration.
    fn show(&self, show: impl Fn(&Config) + 'static) {
        self.loading.set(true);
        show(&self.config.borrow());
        self.loading.set(false);
        self.shows.borrow_mut().push(Box::new(show));
    }

    fn show_status(&self, show: impl Fn(&Status) + 'static) {
        self.status_shows.borrow_mut().push(Box::new(show));
    }

    fn show_storage(&self, show: impl Fn(Result<&StorageInfo, &str>) + 'static) {
        self.storage_shows.borrow_mut().push(Box::new(show));
    }

    /// Read the configuration again and put it on screen.
    async fn reload(&self) {
        match read_config(&self.daemon).await {
            Ok(config) => {
                *self.config.borrow_mut() = config;
                self.loading.set(true);
                for show in self.shows.borrow().iter() {
                    show(&self.config.borrow());
                }
                self.loading.set(false);
            }
            Err(message) => warn!(message, "the settings could not be read again"),
        }
        self.refresh_status().await;
    }

    async fn refresh_status(&self) {
        match self.daemon.get_status().await {
            Ok(status) => {
                for show in self.status_shows.borrow().iter() {
                    show(&status);
                }
            }
            Err(error) => warn!(%error, "the status could not be read"),
        }
    }

    async fn refresh_storage(&self) {
        let answer = self.daemon.get_storage_info().await;
        let reason = match &answer {
            Ok(_) => String::new(),
            Err(zbus::Error::MethodError(name, _, _))
                if name.as_str().ends_with("LockedByOther") =>
            {
                "Available once the current backup has finished".to_string()
            }
            Err(zbus::Error::MethodError(name, _, _))
                if name.as_str().ends_with("NotConfigured") =>
            {
                "Not set up yet".to_string()
            }
            Err(error) => {
                warn!(%error, "the storage figures could not be read");
                "Not available right now".to_string()
            }
        };
        for show in self.storage_shows.borrow().iter() {
            show(answer.as_ref().map_err(|_| reason.as_str()));
        }
    }

    /// Write one setting, and show what the daemon now holds either way: the
    /// new value if it took, or the old one back if it did not.
    fn write<T: Serialize + ?Sized>(self: &Rc<Self>, setting: Setting, value: &T) {
        let literal = match config::toml_literal(value) {
            Ok(literal) => literal,
            Err(error) => {
                warn!(%error, key = setting.key(), "a setting could not be written out");
                return;
            }
        };
        let this = Rc::clone(self);
        crate::ui::spawn(async move {
            match this.daemon.set_config(setting.key(), &literal).await {
                Ok(()) => info!(key = setting.key(), "setting changed"),
                Err(error) => {
                    warn!(%error, key = setting.key(), "the setting was not changed");
                    this.toast(&crate::ui::wizard::explain(&error));
                }
            }
            this.reload().await;
        });
    }

    fn switch(self: &Rc<Self>, setting: Setting, row: &adw::SwitchRow, get: fn(&Config) -> bool) {
        self.bind(setting);
        let shown = row.clone();
        self.show(move |config| shown.set_active(get(config)));
        let this = Rc::clone(self);
        row.connect_active_notify(move |row| {
            if !this.loading.get() {
                this.write(setting, &row.is_active());
            }
        });
    }

    fn combo<T: Copy + PartialEq + Serialize + 'static>(
        self: &Rc<Self>,
        setting: Setting,
        row: &adw::ComboRow,
        options: &'static [(T, &'static str)],
        get: fn(&Config) -> T,
    ) {
        self.bind(setting);
        let labels: Vec<&str> = options.iter().map(|(_, label)| *label).collect();
        row.set_model(Some(&gtk4::StringList::new(&labels)));
        let shown = row.clone();
        self.show(move |config| {
            let current = get(config);
            if let Some(index) = options.iter().position(|(value, _)| *value == current) {
                shown.set_selected(index as u32);
            }
        });
        let this = Rc::clone(self);
        row.connect_selected_notify(move |row| {
            if this.loading.get() {
                return;
            }
            if let Some((value, _)) = options.get(row.selected() as usize) {
                this.write(setting, value);
            }
        });
    }

    fn spin(self: &Rc<Self>, setting: Setting, row: &adw::SpinRow, get: fn(&Config) -> u32) {
        self.bind(setting);
        let shown = row.clone();
        self.show(move |config| shown.set_value(f64::from(get(config))));
        let this = Rc::clone(self);
        row.connect_value_notify(move |row| {
            if !this.loading.get() {
                this.write(setting, &(row.value().max(0.0) as u32));
            }
        });
    }

    /// Start a daemon job and say how it went, in toasts. Listening begins
    /// before the job is asked for, so a quick one cannot finish unheard.
    fn run_job<F>(
        self: &Rc<Self>,
        start: F,
        started: &'static str,
        done: &'static str,
        failed: &'static str,
    ) where
        F: std::future::Future<Output = zbus::Result<u64>> + 'static,
    {
        let this = Rc::clone(self);
        let daemon = self.daemon.clone();
        crate::ui::spawn(async move {
            let finished = daemon.receive_job_finished().await;
            let job = match start.await {
                Ok(job) => job,
                Err(error) => {
                    this.toast(&crate::ui::wizard::explain(&error));
                    return;
                }
            };
            this.toast(started);
            let Ok(mut finished) = finished else { return };
            while let Some(signal) = StreamExt::next(&mut finished).await {
                let Ok(args) = signal.args() else { continue };
                if args.job == job {
                    this.toast(match args.outcome {
                        "completed" => done,
                        "cancelled" => "Stopped",
                        _ => failed,
                    });
                    this.reload().await;
                    this.refresh_storage().await;
                    return;
                }
            }
        });
    }

    fn group(title: &str) -> adw::PreferencesGroup {
        adw::PreferencesGroup::builder().title(title).build()
    }

    fn general(self: &Rc<Self>) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::new();

        let behaviour = Self::group("Behaviour");
        let background = adw::SwitchRow::builder()
            .title("Run in background")
            .subtitle("Hourly backups continue while the window is closed")
            .build();
        self.switch(Setting::RunInBackground, &background, |c| {
            c.general.run_in_background
        });
        behaviour.add(&background);
        let notifications = adw::ComboRow::builder().title("Notifications").build();
        self.combo(
            Setting::Notifications,
            &notifications,
            &model::NOTIFICATIONS,
            |c| c.general.notifications,
        );
        behaviour.add(&notifications);
        page.add(&behaviour);

        // Whether each plugin is installed, and the Install… hint when it is
        // not, is Stage 12's: the plugins are what it ships.
        let integration = Self::group("File manager integration");
        let nautilus = adw::SwitchRow::builder()
            .title("GNOME Files (Nautilus)")
            .build();
        self.switch(Setting::NautilusIntegration, &nautilus, |c| {
            c.general.nautilus_integration
        });
        integration.add(&nautilus);
        let dolphin = adw::SwitchRow::builder().title("Dolphin").build();
        self.switch(Setting::DolphinIntegration, &dolphin, |c| {
            c.general.dolphin_integration
        });
        integration.add(&dolphin);
        page.add(&integration);

        let setup = Self::group("Setup");
        let again = adw::ActionRow::builder()
            .title("Run Welcome Wizard Again…")
            .subtitle("Reconfigure from the start — your backups are kept")
            .activatable(true)
            .build();
        again.add_suffix(&gtk4::Image::from_icon_name("go-next-symbolic"));
        let this = Rc::clone(self);
        again.connect_activated(move |_| this.run_wizard(None));
        setup.add(&again);
        page.add(&setup);
        page
    }

    /// The wizard again, starting from what is configured now, over this
    /// window. `start` skips to one of its pages.
    fn run_wizard(&self, start: Option<&'static str>) {
        crate::ui::wizard::present(
            &self.app,
            self.daemon.clone(),
            Some(self.config.borrow().clone()),
            Some(self.window.upcast_ref()),
            start,
        );
    }

    fn backup(self: &Rc<Self>) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::new();

        let schedule_group = Self::group("Schedule");
        let config = self.config.borrow().clone();
        let frequency = schedule::frequency_row(config.backup.frequency);
        self.combo(
            Setting::Frequency,
            &frequency,
            &schedule::FREQUENCIES,
            |c| c.backup.frequency,
        );
        schedule_group.add(&frequency);
        let battery = schedule::battery_row(config.backup.on_battery);
        self.switch(Setting::OnBattery, &battery, |c| c.backup.on_battery);
        schedule_group.add(&battery);
        let metered = schedule::metered_row(config.backup.on_metered);
        self.switch(Setting::OnMetered, &metered, |c| c.backup.on_metered);
        schedule_group.add(&metered);
        let next = adw::ActionRow::builder().title("").build();
        next.add_css_class("dim-label");
        let shown = next.clone();
        self.show_status(move |status| {
            let now = glib::DateTime::now_utc().map(|d| d.to_unix()).unwrap_or(0);
            shown.set_title(&model::next_backup(status, now, &glib::TimeZone::local()));
        });
        schedule_group.add(&next);
        page.add(&schedule_group);

        page.add(&self.folders_group());

        let exclusions = Self::group("Exclusions");
        self.bind(Setting::Exclude);
        let this = Rc::clone(self);
        let editor = Editor::new(
            Container::Group(exclusions.clone()),
            config.backup.exclude.clone(),
            move |patterns| this.write(Setting::Exclude, patterns),
        );
        self.show(move |config| editor.set(config.backup.exclude.clone()));
        page.add(&exclusions);
        page
    }

    /// "What to back up": the folders inside the home folder under one Home
    /// row, anything else on a row of its own, and Add Folder….
    fn folders_group(self: &Rc<Self>) -> adw::PreferencesGroup {
        self.bind(Setting::Include);
        let group = Self::group("What to back up");
        let rows: Rc<RefCell<Vec<gtk4::Widget>>> = Rc::default();
        let add = adw::ButtonRow::builder()
            .title("Add Folder…")
            .start_icon_name("list-add-symbolic")
            .build();
        let this = Rc::clone(self);
        add.connect_activated(move |_| this.add_folders());

        let this = Rc::clone(self);
        let shown = group.clone();
        self.show(move |config| {
            for row in rows.borrow_mut().drain(..) {
                shown.remove(&row);
            }
            if add.parent().is_some() {
                shown.remove(&add);
            }
            let home = glib::home_dir();
            let found = model::folders(&config.backup.include, &home);
            if !found.in_home.is_empty() {
                let names: Vec<String> = found
                    .in_home
                    .iter()
                    .map(|p| model::folder_name(p))
                    .collect();
                let row = adw::ActionRow::builder()
                    .title("Home")
                    .subtitle(names.join(", "))
                    .use_markup(false)
                    .activatable(true)
                    .build();
                row.add_prefix(&gtk4::Image::from_icon_name("user-home-symbolic"));
                row.add_suffix(&gtk4::Image::from_icon_name("go-next-symbolic"));
                let opener = Rc::clone(&this);
                row.connect_activated(move |_| opener.show_home_folders());
                shown.add(&row);
                rows.borrow_mut().push(row.upcast());
            }
            for path in found.elsewhere {
                let row = this.folder_row(&path);
                shown.add(&row);
                rows.borrow_mut().push(row.upcast());
            }
            shown.add(&add);
        });
        group
    }

    fn folder_row(self: &Rc<Self>, path: &std::path::Path) -> adw::ActionRow {
        let row = adw::ActionRow::builder()
            .title(model::folder_name(path))
            .subtitle(path.display().to_string())
            .use_markup(false)
            .build();
        row.add_prefix(&gtk4::Image::from_icon_name("folder-symbolic"));
        let remove = Button::from_icon_name("window-close-symbolic");
        remove.set_valign(Align::Center);
        remove.add_css_class("flat");
        remove.add_css_class("circular");
        let spoken = format!("Stop backing up {}", model::folder_name(path));
        remove.set_tooltip_text(Some(&spoken));
        remove.update_property(&[gtk4::accessible::Property::Label(&spoken)]);
        row.add_suffix(&remove);
        let this = Rc::clone(self);
        let path = path.to_path_buf();
        let gone = row.clone();
        remove.connect_clicked(move |_| {
            let include: Vec<PathBuf> = this
                .config
                .borrow()
                .backup
                .include
                .iter()
                .filter(|p| **p != path)
                .cloned()
                .collect();
            gone.set_visible(false);
            this.write(Setting::Include, &include);
        });
        row
    }

    /// The folders under Home, as a page of their own.
    fn show_home_folders(self: &Rc<Self>) {
        let group = adw::PreferencesGroup::new();
        let home = glib::home_dir();
        for path in model::folders(&self.config.borrow().backup.include, &home).in_home {
            group.add(&self.folder_row(&path));
        }
        let page = adw::PreferencesPage::new();
        page.add(&group);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.set_content(Some(&page));
        self.nav.push(
            &adw::NavigationPage::builder()
                .title("Home")
                .child(&view)
                .build(),
        );
    }

    fn add_folders(self: &Rc<Self>) {
        let this = Rc::clone(self);
        crate::ui::spawn(async move {
            let dialog = gtk4::FileDialog::builder()
                .title("Choose Folders to Back Up")
                .accept_label("Add")
                .modal(true)
                .build();
            let Ok(chosen) = dialog
                .select_multiple_folders_future(Some(&this.window))
                .await
            else {
                return;
            };
            let added: Vec<PathBuf> = (0..chosen.n_items())
                .filter_map(|i| chosen.item(i))
                .filter_map(|item| item.downcast::<gio::File>().ok())
                .filter_map(|file| file.path())
                .collect();
            let include = model::with_folders(&this.config.borrow().backup.include, &added);
            this.write(Setting::Include, &include);
        });
    }

    fn storage(self: &Rc<Self>) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::new();

        let destination = Self::group("Destination");
        self.bind(Setting::Repository);
        let repository = adw::ActionRow::builder()
            .title("Repository")
            .use_markup(false)
            .build();
        repository.add_prefix(&gtk4::Image::from_icon_name("drive-multidisk-symbolic"));
        let change = Button::with_label("Change…");
        change.set_valign(Align::Center);
        let this = Rc::clone(self);
        change.connect_clicked(move |_| this.run_wizard(Some("where")));
        repository.add_suffix(&change);
        let shown = repository.clone();
        self.show(move |config| {
            shown.set_subtitle(
                &config
                    .storage
                    .repository
                    .as_deref()
                    .map(display)
                    .unwrap_or_else(|| "Not set up".to_string()),
            );
        });
        destination.add(&repository);

        let (used, used_text, used_bar) = bar_row("Space used", "drive-harddisk-symbolic");
        self.show_storage(move |info| match info {
            Ok(info) => {
                let (text, fraction) = model::space_used(info);
                used_text.set_label(&text);
                used_bar.set_visible(fraction.is_some());
                used_bar.set_fraction(fraction.unwrap_or(0.0));
            }
            Err(reason) => {
                used_text.set_label(reason);
                used_bar.set_visible(false);
            }
        });
        destination.add(&used);

        let last = adw::ActionRow::builder().title("Last backup").build();
        last.add_prefix(&gtk4::Image::from_icon_name(
            "document-open-recent-symbolic",
        ));
        let state = gtk4::Image::new();
        last.add_suffix(&state);
        let shown = last.clone();
        self.show_status(move |status| {
            let now = glib::DateTime::now_utc().map(|d| d.to_unix()).unwrap_or(0);
            shown.set_subtitle(&model::last_backup(status, now, &glib::TimeZone::local()));
            let healthy = status.state == "HEALTHY";
            state.set_icon_name(Some(if healthy {
                "object-select-symbolic"
            } else {
                "dialog-warning-symbolic"
            }));
            state.remove_css_class("success");
            state.remove_css_class("warning");
            state.add_css_class(if healthy { "success" } else { "warning" });
        });
        destination.add(&last);
        page.add(&destination);

        let retention = Self::group("Retention");
        let config = self.config.borrow().clone();
        let automatic = schedule::automatic_row(config.storage.retention.automatic);
        self.switch(Setting::RetentionAutomatic, &automatic, |c| {
            c.storage.retention.automatic
        });
        retention.add(&automatic);
        let keep = schedule::keep_rows(&config.storage.retention);
        let getters: [fn(&Config) -> u32; 4] = [
            |c| c.storage.retention.keep_hourly,
            |c| c.storage.retention.keep_daily,
            |c| c.storage.retention.keep_weekly,
            |c| c.storage.retention.keep_monthly,
        ];
        let settings = [
            Setting::KeepHourly,
            Setting::KeepDaily,
            Setting::KeepWeekly,
            Setting::KeepMonthly,
        ];
        for ((row, setting), get) in keep.iter().zip(settings).zip(getters) {
            self.spin(setting, row, get);
            let shown = row.clone();
            self.show(move |config| shown.set_visible(!config.storage.retention.automatic));
            retention.add(row);
        }
        page.add(&retention);

        let offline = Self::group("When the destination is unreachable");
        let protect = adw::SwitchRow::builder()
            .title("Keep protecting changes on this computer")
            .build();
        protect.add_prefix(&gtk4::Image::from_icon_name("computer-symbolic"));
        self.switch(Setting::OfflineEnabled, &protect, |c| {
            c.storage.offline.enabled
        });
        offline.add(&protect);
        offline.add(&self.space_limit_row());
        let usage = adw::ActionRow::builder().title("").build();
        usage.add_prefix(&gtk4::Image::from_icon_name("dialog-information-symbolic"));
        let shown = usage.clone();
        self.show_status(move |status| shown.set_title(&model::local_usage(status)));
        offline.add(&usage);
        page.add(&offline);
        page
    }

    /// The local snapshot limit: a fixed menu of sizes, with a value set by
    /// hand added to it rather than lost.
    fn space_limit_row(self: &Rc<Self>) -> adw::ComboRow {
        self.bind(Setting::OfflineSpaceLimit);
        let row = adw::ComboRow::builder()
            .title("Space limit for local snapshots")
            .build();
        let offered: Rc<RefCell<Vec<u32>>> = Rc::default();
        let shown = row.clone();
        let list = Rc::clone(&offered);
        self.show(move |config| {
            let current = config.storage.offline.space_limit_gb;
            let limits = model::space_limits(current);
            let labels: Vec<String> = limits
                .iter()
                .map(|gb| model::space_limit_label(*gb))
                .collect();
            let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
            shown.set_model(Some(&gtk4::StringList::new(&labels)));
            shown.set_selected(limits.iter().position(|gb| *gb == current).unwrap_or(0) as u32);
            *list.borrow_mut() = limits;
        });
        let this = Rc::clone(self);
        row.connect_selected_notify(move |row| {
            if this.loading.get() {
                return;
            }
            if let Some(gb) = offered.borrow().get(row.selected() as usize) {
                this.write(Setting::OfflineSpaceLimit, gb);
            }
        });
        row
    }

    fn security(self: &Rc<Self>) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::new();
        let encryption = Self::group("Encryption");

        let status = adw::ActionRow::builder().title("Encrypted").build();
        let shield = gtk4::Image::from_icon_name("security-high-symbolic");
        shield.set_pixel_size(32);
        status.add_prefix(&shield);
        let shown = status.clone();
        self.show_storage(move |info| match info {
            Ok(info) => {
                let (title, subtitle) = model::encryption(&info.encryption);
                shown.set_title(title);
                shown.set_subtitle(subtitle);
                shield.remove_css_class("success");
                shield.remove_css_class("warning");
                shield.add_css_class(if title == "Encrypted" {
                    "success"
                } else {
                    "warning"
                });
            }
            Err(reason) => shown.set_subtitle(reason),
        });
        encryption.add(&status);

        let change = adw::ActionRow::builder()
            .title("Change Passphrase…")
            .activatable(true)
            .build();
        change.add_suffix(&gtk4::Image::from_icon_name("go-next-symbolic"));
        let this = Rc::clone(self);
        change.connect_activated(move |_| this.change_passphrase());
        encryption.add(&change);

        let remember = adw::SwitchRow::builder()
            .title("Remember passphrase")
            .subtitle("Stored in the system keyring so backups run automatically")
            .build();
        self.switch(Setting::RememberPassphrase, &remember, |c| {
            c.security.remember_passphrase
        });
        encryption.add(&remember);

        let buttons = GtkBox::new(Orientation::Horizontal, 12);
        buttons.set_homogeneous(true);
        buttons.set_margin_top(12);
        buttons.set_margin_bottom(12);
        buttons.set_margin_start(12);
        buttons.set_margin_end(12);
        for (icon, label, print) in [
            ("document-save-symbolic", "Save Recovery Key…", false),
            ("printer-symbolic", "Print Recovery Key", true),
        ] {
            let button = Button::builder()
                .child(
                    &adw::ButtonContent::builder()
                        .icon_name(icon)
                        .label(label)
                        .build(),
                )
                .build();
            let this = Rc::clone(self);
            button.connect_clicked(move |_| this.keep_key(print));
            buttons.append(&button);
        }
        let holder = adw::PreferencesRow::builder()
            .activatable(false)
            .child(&buttons)
            .build();
        encryption.add(&holder);
        page.add(&encryption);

        let warning = adw::PreferencesGroup::new();
        warning.add(&crate::ui::recovery::warning_card());
        page.add(&warning);
        page
    }

    fn keep_key(self: &Rc<Self>, print: bool) {
        let this = Rc::clone(self);
        crate::ui::spawn(async move {
            let key = match this.daemon.export_recovery_key().await {
                Ok(key) => key,
                Err(error) => {
                    this.toast(&crate::ui::wizard::explain(&error));
                    return;
                }
            };
            let host = glib::host_name().to_string();
            let repository = this
                .config
                .borrow()
                .storage
                .repository
                .clone()
                .unwrap_or_default();
            let window = this.window.upcast_ref::<gtk4::Window>();
            let kept = if print {
                crate::ui::recovery::print(window, &key, &repository, &host).await
            } else {
                crate::ui::recovery::save(window, &key, &host).await
            };
            match kept {
                Ok(true) => this.toast(if print {
                    "Recovery key sent to the printer"
                } else {
                    "Recovery key saved"
                }),
                Ok(false) => {}
                Err(message) => this.toast(&message),
            }
        });
    }

    fn change_passphrase(self: &Rc<Self>) {
        let fields = adw::PreferencesGroup::new();
        let new = adw::PasswordEntryRow::builder()
            .title("New passphrase")
            .build();
        let confirm = adw::PasswordEntryRow::builder().title("Confirm").build();
        fields.add(&new);
        fields.add(&confirm);
        let strength = Label::new(None);
        strength.set_xalign(0.0);
        strength.set_margin_top(6);
        let body = GtkBox::new(Orientation::Vertical, 6);
        body.append(&fields);
        body.append(&strength);

        let dialog = adw::AlertDialog::builder()
            .heading("Change Passphrase")
            .body(
                "Your backups, including every one already made, will open with the new \
                 passphrase from now on.",
            )
            .extra_child(&body)
            .default_response("change")
            .close_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", "Cancel"), ("change", "Change Passphrase")]);
        dialog.set_response_appearance("change", adw::ResponseAppearance::Suggested);
        dialog.set_response_enabled("change", false);
        crate::ui::prefer_wide_responses(&dialog);

        let host = glib::host_name().to_string();
        for entry in [&new, &confirm] {
            let (dialog, new, confirm, strength, host) = (
                dialog.clone(),
                new.clone(),
                confirm.clone(),
                strength.clone(),
                host.clone(),
            );
            entry.connect_changed(move |_| {
                let text = new.text();
                let rating = crate::model::wizard::strength(&text, &[&host]);
                strength.set_label(rating.label);
                for label in ["weak", "fair", "good", "strong"] {
                    strength.remove_css_class(label);
                }
                if !rating.label.is_empty() {
                    strength.add_css_class(rating.label);
                }
                dialog.set_response_enabled("change", !text.is_empty() && text == confirm.text());
            });
        }

        let this = Rc::clone(self);
        dialog.connect_response(Some("change"), move |_, _| {
            let this = Rc::clone(&this);
            let passphrase = new.text().to_string();
            crate::ui::spawn(async move {
                match this.daemon.change_passphrase(&passphrase).await {
                    Ok(()) => this.toast(
                        "Passphrase changed. Save a new recovery key: the one you saved before \
                         goes with the old passphrase.",
                    ),
                    Err(error) => this.toast(&crate::ui::wizard::explain(&error)),
                }
            });
        });
        dialog.present(Some(&self.window));
    }

    fn advanced(self: &Rc<Self>) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::new();

        let catalogue = Self::group("Catalogue");
        let summary = adw::ActionRow::builder().title("Backup catalogue").build();
        let rebuild = Button::with_label("Rebuild");
        rebuild.set_valign(Align::Center);
        summary.add_suffix(&rebuild);
        let shown = summary.clone();
        self.show_status(move |_| {
            let shown = shown.clone();
            crate::ui::spawn(async move {
                if let Ok(Some(text)) = gio::spawn_blocking(catalogue_line).await {
                    shown.set_subtitle(&text);
                }
            });
        });
        let this = Rc::clone(self);
        rebuild.connect_clicked(move |_| {
            let daemon = this.daemon.clone();
            this.run_job(
                async move { daemon.rebuild_catalogue().await },
                "Rebuilding the catalogue…",
                "The catalogue has been rebuilt",
                "The catalogue could not be rebuilt. The logs say why.",
            );
        });
        catalogue.add(&summary);
        page.add(&catalogue);

        let maintenance = Self::group("Repository maintenance");
        let verify = chevron_row("Verify Repository Health…", None);
        let this = Rc::clone(self);
        verify.connect_activated(move |_| this.verify());
        maintenance.add(&verify);
        let compact = chevron_row(
            "Free Up Space Now…",
            Some("Removes data from deleted backups"),
        );
        let this = Rc::clone(self);
        compact.connect_activated(move |_| this.free_up_space());
        maintenance.add(&compact);
        let compression = adw::ComboRow::builder().title("Compression").build();
        self.combo(
            Setting::Compression,
            &compression,
            &model::COMPRESSION,
            |c| c.advanced.compression,
        );
        maintenance.add(&compression);
        page.add(&maintenance);

        let network = Self::group("Network");
        network.add(&self.upload_limit_row());
        page.add(&network);

        let troubleshooting = Self::group("Troubleshooting");
        let logs = chevron_row("Open Logs", None);
        let this = Rc::clone(self);
        logs.connect_activated(move |_| {
            let folder = gio::File::for_path(backtrack_core::paths::log_dir());
            gtk4::FileLauncher::new(Some(&folder)).launch(
                Some(&this.window),
                gio::Cancellable::NONE,
                |result| {
                    if let Err(error) = result {
                        warn!(%error, "the logs folder could not be opened");
                    }
                },
            );
        });
        troubleshooting.add(&logs);
        let reset = chevron_row(
            "Reset All Settings…",
            Some("Your backups are never deleted"),
        );
        reset.add_css_class("error");
        let this = Rc::clone(self);
        reset.connect_activated(move |_| this.reset());
        troubleshooting.add(&reset);
        page.add(&troubleshooting);
        page
    }

    /// The upload limit: off, or a number of megabytes a second. One control
    /// for one setting, as the mockup draws it: a switch, and the number
    /// beside it that only means something while the switch is on.
    fn upload_limit_row(self: &Rc<Self>) -> adw::ActionRow {
        self.bind(Setting::UploadLimit);
        let row = adw::ActionRow::builder()
            .title("Limit backup upload speed")
            .build();
        let rate = gtk4::SpinButton::with_range(1.0, 1000.0, 1.0);
        rate.set_valign(Align::Center);
        rate.set_tooltip_text(Some("Megabytes a second"));
        let unit = Label::new(Some("MB/s"));
        unit.add_css_class("dim-label");
        let switch = gtk4::Switch::builder().valign(Align::Center).build();
        row.add_suffix(&switch);
        row.add_suffix(&rate);
        row.add_suffix(&unit);

        let (shown_switch, shown_rate) = (switch.clone(), rate.clone());
        self.show(move |config| {
            let limit = config.advanced.upload_limit_mbps.filter(|mb| *mb > 0);
            shown_switch.set_active(limit.is_some());
            shown_rate.set_sensitive(limit.is_some());
            if let Some(mb) = limit {
                shown_rate.set_value(f64::from(mb));
            }
        });
        let this = Rc::clone(self);
        let value = rate.clone();
        switch.connect_active_notify(move |switch| {
            if this.loading.get() {
                return;
            }
            value.set_sensitive(switch.is_active());
            let limit = if switch.is_active() {
                value.value() as u32
            } else {
                0
            };
            this.write(Setting::UploadLimit, &limit);
        });
        let this = Rc::clone(self);
        let on = switch.clone();
        rate.connect_value_changed(move |rate| {
            if !this.loading.get() && on.is_active() {
                this.write(Setting::UploadLimit, &(rate.value() as u32));
            }
        });
        row
    }

    fn verify(self: &Rc<Self>) {
        let this = Rc::clone(self);
        crate::ui::spawn(async move {
            let confirmed = confirm(
                &this.window,
                "Verify Repository Health?",
                "Backtrack reads through all of your backups to check that nothing in them is \
                 damaged. On a large backup this takes hours, and backups wait until it has \
                 finished.",
                "Verify",
                false,
            )
            .await;
            if confirmed {
                let daemon = this.daemon.clone();
                this.run_job(
                    async move { daemon.verify().await },
                    "Checking your backups…",
                    "Your backups are in good health",
                    "The check found a problem with your backups",
                );
            }
        });
    }

    fn free_up_space(self: &Rc<Self>) {
        let this = Rc::clone(self);
        crate::ui::spawn(async move {
            let confirmed = confirm(
                &this.window,
                "Free Up Space Now?",
                "Backtrack removes the data only deleted backups were using. This happens on \
                 its own once a day; doing it now can take a while on a large backup.",
                "Free Up Space",
                false,
            )
            .await;
            if confirmed {
                let daemon = this.daemon.clone();
                this.run_job(
                    async move { daemon.compact().await },
                    "Freeing up space…",
                    "Space freed",
                    "Space could not be freed. The logs say why.",
                );
            }
        });
    }

    /// Reset All Settings: every setting back to its default and the wizard
    /// from the start, with nothing else touched, as the dialog says.
    fn reset(self: &Rc<Self>) {
        let this = Rc::clone(self);
        crate::ui::spawn(async move {
            let confirmed = confirm(
                &this.window,
                "Reset All Settings?",
                "Every setting goes back to how it was before Backtrack was set up, and the \
                 welcome wizard opens so you can set it up again. Your backups are never \
                 deleted: they stay where they are, and the wizard can open them again with \
                 Import.",
                "Reset",
                true,
            )
            .await;
            if !confirmed {
                return;
            }
            if let Err(error) = this.daemon.reset_config().await {
                this.toast(&crate::ui::wizard::explain(&error));
                return;
            }
            info!("settings reset from Preferences");
            this.window.close();
            this.parent.close();
            crate::ui::wizard::present(&this.app, this.daemon.clone(), None, None, None);
        });
    }
}

/// A row that opens something, with the chevron that says so.
fn chevron_row(title: &str, subtitle: Option<&str>) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .activatable(true)
        .build();
    if let Some(subtitle) = subtitle {
        row.set_subtitle(subtitle);
    }
    row.add_suffix(&gtk4::Image::from_icon_name("go-next-symbolic"));
    row
}

/// A row with a title, a line of detail, and a bar under both, which is how
/// mockup 17 shows the space the backups use.
fn bar_row(title: &str, icon: &str) -> (adw::PreferencesRow, Label, gtk4::ProgressBar) {
    let content = GtkBox::new(Orientation::Horizontal, 12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    let image = gtk4::Image::from_icon_name(icon);
    image.set_valign(Align::Center);
    content.append(&image);
    let text = GtkBox::new(Orientation::Vertical, 4);
    text.set_hexpand(true);
    let heading = Label::builder().label(title).xalign(0.0).build();
    let detail = Label::builder().xalign(0.0).wrap(true).build();
    detail.add_css_class("dim-label");
    detail.add_css_class("caption");
    let bar = gtk4::ProgressBar::new();
    bar.set_margin_top(6);
    text.append(&heading);
    text.append(&detail);
    text.append(&bar);
    content.append(&text);
    let row = adw::PreferencesRow::builder()
        .activatable(false)
        .title(title)
        .child(&content)
        .build();
    (row, detail, bar)
}

/// Ask before doing something that takes a while or cannot be taken back.
async fn confirm(
    parent: &adw::Window,
    heading: &str,
    body: &str,
    action: &str,
    destructive: bool,
) -> bool {
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(body)
        .default_response("cancel")
        .close_response("cancel")
        .build();
    dialog.add_responses(&[("cancel", "Cancel"), ("go", action)]);
    dialog.set_response_appearance(
        "go",
        if destructive {
            adw::ResponseAppearance::Destructive
        } else {
            adw::ResponseAppearance::Suggested
        },
    );
    crate::ui::prefer_wide_responses(&dialog);
    dialog.choose_future(Some(parent)).await == "go"
}

/// "47 backups indexed · 132 MB on disk", read from the catalogue itself.
fn catalogue_line() -> Option<String> {
    let path = backtrack_core::paths::index_db();
    let reader = backtrack_core::index::IndexReader::open(&path).ok()?;
    let backups = reader.archives_overview().ok()?.len();
    // The database and the write-ahead log beside it are both the catalogue.
    let bytes: u64 = ["", "-wal"]
        .iter()
        .filter_map(|suffix| {
            std::fs::metadata(format!("{}{suffix}", path.display()))
                .ok()
                .map(|meta| meta.len())
        })
        .sum();
    Some(model::catalogue_summary(backups, bytes))
}
