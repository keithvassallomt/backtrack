// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The main window: the frame every other pane hangs off.
//!
//! The layout is the one idea the product is built on, made structural. The
//! breadcrumb and the file pane are about *where*; the sidebar, the stepping
//! buttons, the position line and the density strip are all about *when*; and
//! the two never move each other. Walking into a folder does not change the
//! backup you are viewing, and stepping back through time does not change the
//! folder you are looking at.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use backtrack_core::dbus::Reason;

use gtk4::prelude::*;
use gtk4::{gio, glib, Align, Box as GtkBox, Button, Label, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::{info, warn};

use crate::daemon::{Daemon1Proxy, JobFinishedStream};
use crate::index::Index;
use crate::model::format;
use crate::state::{AppState, Change};
use crate::ui;
use crate::Target;

// Windows currently open, so a second launch can re-point one instead of
// opening another. There is no way to attach a Rust value to a `GObject`
// without `unsafe`, and the workspace forbids that.
thread_local! {
    static OPEN: RefCell<Vec<Rc<Window>>> = const { RefCell::new(Vec::new()) };
}

/// One Backtrack window and everything it is wired to.
pub struct Window {
    pub window: adw::ApplicationWindow,
    state: Rc<AppState>,
    index: Index,
    /// `None` until the daemon answers, and if it never does. Browsing does not
    /// need it; previewing and backing up do.
    daemon: Rc<RefCell<Option<Daemon1Proxy<'static>>>>,
    toasts: adw::ToastOverlay,
    position: Label,
    /// The line at the bottom that says how backups are doing.
    status: Label,
    /// The line under the header bar that says what health is about, when it
    /// is about anything, and names the fix. Mockup 23.
    banner: adw::Banner,
    /// What the banner's button does at the moment.
    banner_action: Cell<Option<crate::model::health::Action>>,
    /// The line under that one while a backup runs, saying how far it has
    /// got (S06-T12).
    progress: adw::Banner,
    /// What the person calls the place backups go, for the one banner that
    /// names it. Read when that banner is showing.
    destination: RefCell<String>,
    /// A fix asked for before the daemon had answered: from a notification
    /// that started the window, say.
    pending_fix: RefCell<Option<Reason>>,
    older: adw::SplitButton,
    newer: adw::SplitButton,
    /// The dropdown on each stepping button, rebuilt as the selection changes
    /// so the item can name the file it will jump through.
    older_menu: gio::Menu,
    newer_menu: gio::Menu,
    /// Where each pane goes. They are filled in by their own modules so that
    /// this file stays a description of the layout.
    sidebar_slot: GtkBox,
    files_slot: GtkBox,
    preview_slot: GtkBox,
    strip_slot: GtkBox,
    /// The panes, kept alive and reachable for the actions that drive them.
    sidebar: RefCell<Option<Rc<ui::sidebar::Sidebar>>>,
    files: RefCell<Option<Rc<ui::files::Files>>>,
    preview: RefCell<Option<Rc<ui::preview::Preview>>>,
    restores: RefCell<Option<Rc<ui::restore::Restores>>>,
    /// The one action bar button that does something this stage.
    restore_button: Button,
    restore_to_button: Button,
    compare_button: Button,
    /// The header's title: the breadcrumb, or the search box.
    title_slot: gtk4::Stack,
    /// The window's body: the timeline, or the search results.
    content_slot: gtk4::Stack,
    search: RefCell<Option<Rc<ui::search::Search>>>,
    calendar: RefCell<Option<Rc<ui::calendar::Calendar>>>,
    strip: RefCell<Option<Rc<ui::strip::Strip>>>,
}

impl Window {
    /// Build a window showing `target`, and start loading the index behind it.
    pub fn build(app: &adw::Application, target: &Target) -> adw::ApplicationWindow {
        let state = AppState::new(target.folder.clone());
        let index = Index::spawn(backtrack_core::paths::index_db());

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Backtrack")
            .default_width(1180)
            .default_height(760)
            .width_request(480)
            .height_request(440)
            .build();

        let status = Label::builder()
            .halign(Align::Start)
            .hexpand(true)
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .build();
        status.add_css_class("dim-label");

        let position = Label::new(Some(format::NO_BACKUPS));
        position.add_css_class("position");
        position.set_hexpand(true);
        position.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);

        let older_menu = gio::Menu::new();
        let newer_menu = gio::Menu::new();
        let older = step_button(
            "Older",
            "go-previous-symbolic",
            true,
            "win.older",
            &older_menu,
        );
        let newer = step_button("Newer", "go-next-symbolic", false, "win.newer", &newer_menu);

        let this = Rc::new(Window {
            window: window.clone(),
            state: Rc::clone(&state),
            index,
            daemon: Rc::new(RefCell::new(None)),
            toasts: adw::ToastOverlay::new(),
            position,
            status,
            banner: adw::Banner::new(""),
            banner_action: Cell::new(None),
            progress: adw::Banner::new(""),
            destination: RefCell::new(String::new()),
            pending_fix: RefCell::new(None),
            older,
            newer,
            older_menu,
            newer_menu,
            sidebar_slot: GtkBox::new(Orientation::Vertical, 0),
            files_slot: GtkBox::new(Orientation::Vertical, 0),
            preview_slot: GtkBox::new(Orientation::Vertical, 0),
            strip_slot: GtkBox::new(Orientation::Vertical, 0),
            sidebar: RefCell::new(None),
            files: RefCell::new(None),
            preview: RefCell::new(None),
            restores: RefCell::new(None),
            restore_button: Button::builder().build(),
            restore_to_button: Button::builder().build(),
            compare_button: Button::builder().build(),
            title_slot: gtk4::Stack::new(),
            content_slot: gtk4::Stack::new(),
            search: RefCell::new(None),
            calendar: RefCell::new(None),
            strip: RefCell::new(None),
        });

        this.install_actions();
        this.assemble();
        this.fill_panes();
        this.watch_state();
        this.watch_listing();
        this.load_archives();
        this.connect_daemon();

        let closing = Rc::clone(&this);
        window.connect_close_request(move |_| {
            OPEN.with(|open| open.borrow_mut().retain(|w| w.window != closing.window));
            glib::Propagation::Proceed
        });
        OPEN.with(|open| open.borrow_mut().push(Rc::clone(&this)));

        this.apply_selection(target);
        window
    }

    /// Put the frame together: header bar, sidebar, panes, action bar.
    fn assemble(self: &Rc<Self>) {
        let layout = adw::ToolbarView::new();
        layout.add_top_bar(&self.header());
        // Under the header bar and across the whole window, as mockup 23
        // draws it: it is about the backups, not about any one pane.
        let this = Rc::clone(self);
        self.banner
            .connect_button_clicked(move |_| match this.banner_action.get() {
                Some(crate::model::health::Action::Fix(reason)) => this.open_fix(reason),
                Some(crate::model::health::Action::Resume) => this.resume(),
                None => {}
            });
        layout.add_top_bar(&self.banner);
        layout.add_top_bar(&self.progress);
        layout.set_content(Some(&self.body()));
        layout.add_bottom_bar(&self.actions());

        self.toasts.set_child(Some(&layout));
        self.window.set_content(Some(&self.toasts));
    }

    fn header(self: &Rc<Self>) -> adw::HeaderBar {
        let header = adw::HeaderBar::new();

        let identity = GtkBox::new(Orientation::Horizontal, 8);
        let logo = gtk4::Image::from_icon_name(ui::app_icon_name(&self.window));
        logo.set_pixel_size(24);
        identity.append(&logo);
        let name = Label::new(Some("Backtrack"));
        name.add_css_class("heading");
        identity.append(&name);
        header.pack_start(&identity);
        ui::without_window_icon(&header);

        // The title position holds the breadcrumb while browsing and the search
        // box while searching, because both are the one thing the window is
        // currently about and neither wants to be beside the other.
        self.title_slot
            .add_named(&ui::breadcrumb::build(&self.state), Some("browse"));
        header.set_title_widget(Some(&self.title_slot));

        header.pack_end(&ui::menu::button());

        let search = Button::from_icon_name("system-search-symbolic");
        search.set_tooltip_text(Some("Search every backup"));
        search.update_property(&[gtk4::accessible::Property::Label("Search every backup")]);
        search.set_action_name(Some("win.search"));
        header.pack_end(&search);

        header
    }

    fn body(self: &Rc<Self>) -> adw::OverlaySplitView {
        let sidebar = GtkBox::new(Orientation::Vertical, 0);
        sidebar.add_css_class("sidebar-edge");
        sidebar.append(&self.sidebar_header());
        self.sidebar_slot.set_vexpand(true);
        sidebar.append(&self.sidebar_slot);

        let content = GtkBox::new(Orientation::Vertical, 0);
        content.append(&self.time_bar());

        let panes = GtkBox::new(Orientation::Horizontal, 12);
        panes.set_vexpand(true);
        panes.set_margin_start(12);
        panes.set_margin_end(12);
        // Bordered rather than merely coloured, so the panes stay separate
        // things under a palette that makes their backgrounds the same — see
        // `.pane` in the stylesheet. Clipped, because a rounded corner with a
        // list scrolling under it is only rounded until something reaches it.
        for pane in [&self.files_slot, &self.preview_slot] {
            pane.add_css_class("pane");
            pane.set_overflow(gtk4::Overflow::Hidden);
        }
        self.files_slot.set_hexpand(true);
        panes.append(&self.files_slot);
        // Clamped, not merely given a minimum. The preview's own content
        // decides how wide it would like to be — a line of monospace text, or
        // the "select a file" page — and in a narrow window it was taking that
        // room from the file list, which is the pane the window is for.
        self.preview_slot.set_size_request(240, -1);
        let preview = adw::Clamp::builder()
            .maximum_size(320)
            .tightening_threshold(240)
            .child(&self.preview_slot)
            .build();
        panes.append(&preview);
        content.append(&panes);

        self.strip_slot.set_margin_start(12);
        self.strip_slot.set_margin_end(12);
        self.strip_slot.set_margin_top(12);
        content.append(&self.strip_slot);

        self.content_slot.add_named(&content, Some("browse"));

        adw::OverlaySplitView::builder()
            .sidebar(&sidebar)
            .content(&self.content_slot)
            .min_sidebar_width(200.0)
            .max_sidebar_width(280.0)
            .sidebar_width_fraction(0.22)
            .build()
    }

    fn sidebar_header(self: &Rc<Self>) -> GtkBox {
        let row = GtkBox::new(Orientation::Horizontal, 6);
        row.set_margin_top(12);
        row.set_margin_bottom(6);
        row.set_margin_start(12);
        row.set_margin_end(12);

        let title = Label::new(Some("Snapshots"));
        title.add_css_class("heading");
        title.set_halign(Align::Start);
        title.set_hexpand(true);
        row.append(&title);

        let calendar = ui::calendar::build(&self.state);
        calendar.widget().add_css_class("flat");
        row.append(&calendar.widget());
        *self.calendar.borrow_mut() = Some(calendar);
        row
    }

    /// Older ◀ … position … ▶ Newer.
    fn time_bar(self: &Rc<Self>) -> GtkBox {
        let bar = GtkBox::new(Orientation::Horizontal, 12);
        bar.set_margin_top(12);
        bar.set_margin_bottom(12);
        bar.set_margin_start(12);
        bar.set_margin_end(12);
        bar.append(&self.older);
        bar.append(&self.position);
        bar.append(&self.newer);
        bar
    }

    /// The bottom bar: how backups are doing on the left, what you can do
    /// with the selection on the right.
    fn actions(self: &Rc<Self>) -> GtkBox {
        let bar = GtkBox::new(Orientation::Horizontal, 8);
        bar.add_css_class("toolbar");
        bar.append(&self.status);

        let buttons = GtkBox::new(Orientation::Horizontal, 8);
        buttons.set_halign(Align::End);

        let compare = self.compare_button.clone();
        compare.set_child(Some(&button_content(
            "view-dual-symbolic",
            "Compare with Today",
        )));
        compare.set_tooltip_text(Some("Show this version beside the file on your computer"));
        compare.set_sensitive(false);
        compare.set_action_name(Some("win.compare"));
        buttons.append(&compare);

        // The way out that costs nothing: the files arrive in a folder of
        // their own, so nothing already on the computer is touched and there
        // is nothing to decide.
        let elsewhere = self.restore_to_button.clone();
        elsewhere.set_child(Some(&button_content("folder-symbolic", "Restore To…")));
        elsewhere.set_tooltip_text(Some(
            "Restore into a folder you choose, leaving everything here as it is",
        ));
        elsewhere.set_sensitive(false);
        elsewhere.set_action_name(Some("win.restore-to"));
        buttons.append(&elsewhere);

        let restore = self.restore_button.clone();
        restore.set_child(Some(&button_content("edit-undo-symbolic", "Restore")));
        restore.add_css_class("suggested-action");
        restore.set_tooltip_text(Some("Put this version back where it came from"));
        restore.set_sensitive(false);
        restore.set_action_name(Some("win.restore"));
        buttons.append(&restore);

        bar.append(&buttons);
        bar
    }

    /// The window's actions, and the keys that reach them.
    ///
    /// `Ctrl+←` and `Ctrl+→` rather than the bare arrows: the arrows belong to
    /// whichever list has the focus, and taking them would make the file pane
    /// unusable from the keyboard.
    fn install_actions(self: &Rc<Self>) {
        for (name, accel, direction) in [
            ("older", "<Control>Left", Step::Older),
            ("newer", "<Control>Right", Step::Newer),
        ] {
            let action = gio::SimpleAction::new(name, None);
            let this = Rc::clone(self);
            action.connect_activate(move |_, _| this.step(direction));
            self.window.add_action(&action);
            if let Some(app) = self.window.application() {
                app.set_accels_for_action(&format!("win.{name}"), &[accel]);
            }
        }

        for (name, direction) in [("older-change", Step::Older), ("newer-change", Step::Newer)] {
            let action = gio::SimpleAction::new(name, None);
            let this = Rc::clone(self);
            action.connect_activate(move |_, _| this.step_to_change(direction));
            self.window.add_action(&action);
        }

        // Every backup, or only those in which the folder differs (Stage 15).
        // Stateful and boolean, so the menu draws it as a check item and
        // activating it toggles it. Ctrl+H, as Show Hidden Files in Nautilus.
        let show_all =
            gio::SimpleAction::new_stateful("show-all-backups", None, &false.to_variant());
        let shower = Rc::clone(self);
        show_all.connect_change_state(move |action, value| {
            let Some(on) = value.and_then(|v| v.get::<bool>()) else {
                return;
            };
            action.set_state(&on.to_variant());
            shower.state.set_show_all(on);
            shower.remember_show_all(on);
        });
        self.window.add_action(&show_all);
        if let Some(app) = self.window.application() {
            app.set_accels_for_action("win.show-all-backups", &["<Control>h"]);
        }

        // Up a folder, the way every file manager spells it. The breadcrumb
        // does the same thing with a click; this is the same journey without
        // one.
        let parent = gio::SimpleAction::new("parent", None);
        let climber = Rc::clone(self);
        parent.connect_activate(move |_, _| climber.go_to_parent());
        self.window.add_action(&parent);
        if let Some(app) = self.window.application() {
            app.set_accels_for_action("win.parent", &["<Alt>Up"]);
        }

        // Restoring is an action rather than only a button: it belongs in the
        // shortcuts window, and a button is not reachable by anyone driving
        // the window from the keyboard alone.
        let restore = gio::SimpleAction::new("restore", None);
        restore.set_enabled(false);
        let restorer = Rc::clone(self);
        restore.connect_activate(move |_, _| {
            let driver = restorer.restores.borrow().clone();
            if let Some(driver) = driver {
                driver.start();
            }
        });
        self.window.add_action(&restore);
        if let Some(app) = self.window.application() {
            app.set_accels_for_action("win.restore", &["<Control>r"]);
        }

        let elsewhere = gio::SimpleAction::new("restore-to", None);
        elsewhere.set_enabled(false);
        let chooser = Rc::clone(self);
        elsewhere.connect_activate(move |_, _| {
            let driver = chooser.restores.borrow().clone();
            if let Some(driver) = driver {
                driver.start_elsewhere();
            }
        });
        self.window.add_action(&elsewhere);
        if let Some(app) = self.window.application() {
            app.set_accels_for_action("win.restore-to", &["<Control><Shift>r"]);
        }

        let search = gio::SimpleAction::new("search", None);
        let searcher = Rc::clone(self);
        search.connect_activate(move |_, _| {
            let pane = searcher.search.borrow().clone();
            if let Some(pane) = pane {
                pane.toggle();
            }
        });
        self.window.add_action(&search);
        if let Some(app) = self.window.application() {
            app.set_accels_for_action("win.search", &["<Control>f"]);
        }

        let compare = gio::SimpleAction::new("compare", None);
        compare.set_enabled(false);
        let comparer = Rc::clone(self);
        compare.connect_activate(move |_, _| comparer.open_compare());
        self.window.add_action(&compare);
        if let Some(app) = self.window.application() {
            app.set_accels_for_action("win.compare", &["<Control>d"]);
        }

        self.install_menu_actions();
    }

    /// Move to the folder containing the one being viewed.
    fn go_to_parent(self: &Rc<Self>) {
        let folder = self.state.view().folder;
        match crate::path::parent(&folder) {
            Some(parent) => self.state.set_folder(parent),
            // The top of what was backed up. There is nowhere above it, and
            // saying so is better than doing nothing visible.
            None => self.toast("This is the top of your backup"),
        }
    }

    /// Everything behind the primary menu.
    fn install_menu_actions(self: &Rc<Self>) {
        // The actions that need the daemon start disabled and are enabled when
        // it answers, so the menu never offers something that cannot happen.
        let backup = gio::SimpleAction::new("backup-now", None);
        backup.set_enabled(false);
        let starter = Rc::clone(self);
        backup.connect_activate(move |_, _| starter.backup_now());
        self.window.add_action(&backup);

        let pause = gio::SimpleAction::new("pause", Some(glib::VariantTy::STRING));
        pause.set_enabled(false);
        let pauser = Rc::clone(self);
        pause.connect_activate(move |_, parameter| {
            if let Some(option) = parameter.and_then(|p| p.str()) {
                pauser.pause(option);
            }
        });
        self.window.add_action(&pause);

        let resume = gio::SimpleAction::new("resume", None);
        resume.set_enabled(false);
        let resumer = Rc::clone(self);
        resume.connect_activate(move |_, _| resumer.resume());
        self.window.add_action(&resume);

        // The safety stash, which is what makes Replace a decision rather than
        // a commitment: the toast's Undo covers ten seconds, this covers
        // thirty days.
        let replaced = gio::SimpleAction::new("recently-replaced", None);
        let owner = self.window.clone();
        let service = Rc::clone(&self.daemon);
        replaced.connect_activate(move |_, _| ui::replaced::present(&owner, &service));
        self.window.add_action(&replaced);

        // Enabled once the daemon answers: every setting is read from it and
        // written through it.
        let preferences = gio::SimpleAction::new("preferences", None);
        preferences.set_enabled(false);
        let opener = Rc::clone(self);
        preferences.connect_activate(move |_, _| {
            let (Some(app), Some(proxy)) =
                (opener.window.application(), opener.daemon.borrow().clone())
            else {
                return;
            };
            if let Ok(app) = app.downcast::<adw::Application>() {
                ui::prefs::present(&app, &opener.window, proxy, None);
            }
        });
        self.window.add_action(&preferences);

        let shortcuts = gio::SimpleAction::new("shortcuts", None);
        let owner = self.window.clone();
        shortcuts.connect_activate(move |_, _| {
            ui::menu::shortcuts_dialog().present(Some(&owner));
        });
        self.window.add_action(&shortcuts);

        let help = gio::SimpleAction::new("help", None);
        let helper = self.window.clone();
        help.connect_activate(move |_, _| ui::menu::open_help(&helper));
        self.window.add_action(&help);

        let about = gio::SimpleAction::new("about", None);
        let parent = self.window.clone();
        about.connect_activate(move |_, _| ui::menu::about_dialog().present(Some(&parent)));
        self.window.add_action(&about);

        if let Some(app) = self.window.application() {
            app.set_accels_for_action("win.backup-now", &["<Control>b"]);
            app.set_accels_for_action("win.shortcuts", &["<Control>question"]);
            app.set_accels_for_action("win.preferences", &["<Control>comma"]);
            app.set_accels_for_action("window.close", &["<Control>w"]);
        }
    }

    /// Start a backup, whatever the schedule says, and say when it is done.
    fn backup_now(self: &Rc<Self>) {
        let Some(proxy) = self.daemon.borrow().clone() else {
            self.toast(NO_SERVICE);
            return;
        };
        let this = Rc::clone(self);
        ui::spawn(async move {
            // Subscribed before the backup is asked for, not after: a small
            // backup can finish before a stream opened afterwards would exist,
            // and the completion would be missed entirely.
            let finished = proxy.receive_job_finished().await;

            let job = match proxy.backup_now().await {
                Ok(job) => job,
                Err(error) => {
                    warn!(%error, "the backup could not be started");
                    this.toast(&clean(&error.to_string()));
                    return;
                }
            };
            info!(job, "backup started from the menu");
            this.toast("Backup started");
            this.refresh_status();

            match finished {
                Ok(stream) => this.await_job(stream, job).await,
                Err(error) => warn!(%error, job, "the backup's outcome will not be reported"),
            }
        });
    }

    /// Wait for `job` to end, and say how it went.
    async fn await_job(self: &Rc<Self>, mut finished: JobFinishedStream, job: u64) {
        use futures::StreamExt;
        while let Some(signal) = StreamExt::next(&mut finished).await {
            let Ok(args) = signal.args() else {
                continue;
            };
            if args.job != job {
                continue;
            }
            info!(job, outcome = args.outcome, "the backup ended");
            self.toast(match args.outcome {
                "completed" => "Backup complete",
                "cancelled" => "Backup cancelled",
                _ => "The backup did not finish — see the logs for why",
            });
            self.refresh_status();
            return;
        }
    }

    /// Pause the schedule for one of the menu's self-expiring durations.
    fn pause(self: &Rc<Self>, option: &str) {
        let Some(proxy) = self.daemon.borrow().clone() else {
            self.toast(NO_SERVICE);
            return;
        };
        let now = glib::DateTime::now_utc().map(|d| d.to_unix()).unwrap_or(0);
        let tz = glib::TimeZone::local();
        let Some(seconds) = crate::model::pause::duration(option, now, &tz) else {
            warn!(option, "unknown pause option");
            return;
        };
        let until = now as u64 + seconds;

        let this = Rc::clone(self);
        ui::spawn(async move {
            match proxy.pause(until).await {
                Ok(()) => {
                    info!(until, "backups paused");
                    this.refresh_status();
                }
                Err(error) => {
                    warn!(%error, "backups could not be paused");
                    this.toast(&clean(&error.to_string()));
                }
            }
        });
    }

    /// Lift a pause.
    fn resume(self: &Rc<Self>) {
        let Some(proxy) = self.daemon.borrow().clone() else {
            self.toast(NO_SERVICE);
            return;
        };
        let this = Rc::clone(self);
        ui::spawn(async move {
            match proxy.resume().await {
                Ok(()) => {
                    info!("backups resumed");
                    this.toast("Backups resumed");
                    this.refresh_status();
                }
                Err(error) => {
                    warn!(%error, "backups could not be resumed");
                    this.toast(&clean(&error.to_string()));
                }
            }
        });
    }

    /// Let the Restore action and its button agree about whether there is
    /// anything to restore.
    fn refresh_restore_action(self: &Rc<Self>) {
        let possible = self
            .restores
            .borrow()
            .as_ref()
            .is_some_and(|driver| driver.can_restore());
        ui::menu::set_enabled(&self.window, "restore", possible);
        self.restore_button.set_sensitive(possible);
        ui::menu::set_enabled(&self.window, "restore-to", possible);
        self.restore_to_button.set_sensitive(possible);

        // Comparing needs a today to compare against. A folder has no diff
        // worth showing, and a file that is not on the computer has nothing on
        // the right-hand side — for that one the answer is Restore, and the
        // "not on your disk" badge has already said so.
        let view = self.state.view();
        let comparable = view
            .selected
            .as_ref()
            .is_some_and(|s| !s.is_dir && s.on_disk == ON_DISK_PRESENT)
            && view.seq.is_some();
        ui::menu::set_enabled(&self.window, "compare", comparable);
        self.compare_button.set_sensitive(comparable);
    }

    /// Leave search and show a result where it lived, at the last backup that
    /// had it.
    ///
    /// The last backup rather than the newest: a file deleted three backups
    /// ago is not in the newest one, and landing somebody on a folder that
    /// does not contain the thing they just clicked is the whole failure this
    /// action exists to avoid.
    fn go_to_hit(self: &Rc<Self>, hit: &backtrack_core::dbus::SearchResult) {
        let folder = crate::path::parent(&hit.path).unwrap_or_default();
        info!(
            path = hit.path,
            seq = hit.last_seq,
            "opening a search result"
        );
        // The folder first: a backup asked for is landed against the folder's
        // list (Stage 15), and the last folder's could move it to a backup
        // that no longer holds the file.
        self.state.set_folder(folder);
        self.state.set_seq(hit.last_seq);
        self.state.set_selected(Some(crate::state::Selected {
            path: hit.path.clone(),
            name: hit.name.clone(),
            is_dir: hit.kind == "dir",
            size: hit.size,
            // The catalogue's time for the newest version; the pane replaces
            // this with the entry's own once the folder has loaded.
            mtime: hit.last_ts.saturating_mul(1_000_000),
            on_disk: if hit.gone_from_disk {
                ON_DISK_ABSENT
            } else {
                ON_DISK_PRESENT
            },
        }));
    }

    /// Show the selected file's backed-up version beside the one on disk.
    fn open_compare(self: &Rc<Self>) {
        let view = self.state.view();
        let (Some(selected), Some(archive)) = (view.selected.clone(), view.archive()) else {
            return;
        };
        let restorer = Rc::clone(self);
        ui::compare::present(
            &self.window,
            &self.daemon,
            archive.name.clone(),
            selected,
            move || {
                let driver = restorer.restores.borrow().clone();
                if let Some(driver) = driver {
                    driver.start();
                }
            },
        );
    }

    /// Re-read the daemon's status and redraw the line at the bottom.
    fn refresh_status(self: &Rc<Self>) {
        let Some(proxy) = self.daemon.borrow().clone() else {
            self.status.set_text(NO_SERVICE);
            return;
        };
        let this = Rc::clone(self);
        ui::spawn(async move {
            match proxy.get_status().await {
                Ok(status) => {
                    this.learn_destination(&proxy, &status).await;
                    this.render_status(&status);
                }
                Err(error) => warn!(%error, "the status could not be read"),
            }
        });
    }

    /// Find out what the destination is called, when `status`'s banner names
    /// it. Only some do, so only then is it worth asking for; every path that
    /// draws a status comes through here first, or the banner says "… to .".
    async fn learn_destination(
        &self,
        proxy: &Daemon1Proxy<'static>,
        status: &backtrack_core::dbus::Status,
    ) {
        let names_it = Reason::parse(&status.reason)
            .and_then(Reason::failure)
            .is_some_and(|failure| failure.names_destination());
        if !names_it {
            return;
        }
        if let Ok(text) = proxy.get_config().await {
            if let Ok((config, _)) = backtrack_core::config::Config::parse(&text) {
                *self.destination.borrow_mut() = backtrack_core::destination::name(
                    &config.storage.repository.unwrap_or_default(),
                );
            }
        }
    }

    fn render_status(&self, status: &backtrack_core::dbus::Status) {
        let now = glib::DateTime::now_utc().map(|d| d.to_unix()).unwrap_or(0);
        let tz = glib::TimeZone::local();
        self.status
            .set_text(&crate::model::status::line(status, now, &tz));
        self.render_banner(status, now, &tz);
        // Resuming is only an offer when there is something to resume.
        ui::menu::set_enabled(
            &self.window,
            "resume",
            status.paused_until > now.max(0) as u64,
        );
    }

    /// Show the banner health.md calls for, or none.
    fn render_banner(&self, status: &backtrack_core::dbus::Status, now: i64, tz: &glib::TimeZone) {
        use crate::model::health::{self, Tone};
        let not_browsable = self
            .state
            .view()
            .archives
            .iter()
            .filter(|archive| !archive.catalogued)
            .count();
        let banner = health::banner(status, &self.destination.borrow(), not_browsable, now, tz);
        for tone in ["warning", "error"] {
            self.banner.remove_css_class(tone);
        }
        let Some(banner) = banner else {
            self.banner.set_revealed(false);
            self.banner_action.set(None);
            return;
        };
        match banner.tone {
            Tone::Warning => self.banner.add_css_class("warning"),
            Tone::Error => self.banner.add_css_class("error"),
            Tone::Info => {}
        }
        self.banner.set_title(&banner.title);
        self.banner
            .set_button_label(banner.action.map(|action| action.label()));
        self.banner_action.set(banner.action);
        self.banner.set_revealed(true);
    }

    /// Open the fix for `reason`, once the daemon is there to carry it out.
    fn open_fix(self: &Rc<Self>, reason: Reason) {
        let Some(proxy) = self.daemon.borrow().clone() else {
            *self.pending_fix.borrow_mut() = Some(reason);
            return;
        };
        let Some(app) = self
            .window
            .application()
            .and_then(|app| app.downcast::<adw::Application>().ok())
        else {
            return;
        };
        info!(reason = reason.as_str(), "opening a fix");
        ui::fix::open(
            &ui::fix::Context {
                app,
                window: self.window.clone(),
                daemon: proxy,
                toasts: self.toasts.clone(),
            },
            reason,
        );
    }

    /// A fix asked for from outside the window: a notification clicked, most
    /// likely, which may be older than the problem's solution. Opened only if
    /// it is still the problem.
    fn fix_if_current(self: &Rc<Self>, status: &backtrack_core::dbus::Status, reason: Reason) {
        if status.reason == reason.as_str() {
            self.open_fix(reason);
        } else {
            info!(
                reason = reason.as_str(),
                "a fix was asked for that is no longer needed"
            );
            self.toast("That has been put right already");
        }
    }

    /// Re-read the status on a slow tick.
    ///
    /// Two things go stale on their own, with no announcement to hang a
    /// refresh off: "2 hours ago" becomes "3 hours ago" by the passage of
    /// time, and a pause lifts itself when it reaches its end. Neither is
    /// worth a signal; both are worth not being wrong about.
    async fn keep_status_fresh(self: &Rc<Self>) {
        loop {
            glib::timeout_future_seconds(STATUS_TICK_SECONDS).await;
            if self.window.is_visible() {
                self.refresh_status();
            }
        }
    }

    /// Follow the daemon's health announcements, so the status line does not
    /// go stale while the window sits open.
    async fn follow_status(self: &Rc<Self>, proxy: crate::daemon::Daemon1Proxy<'static>) {
        // Fully qualified: GTK's prelude brings its own `next` into scope for
        // every widget, and the two are indistinguishable at the call site.
        use futures::StreamExt;
        let mut announcements = match proxy.receive_status_changed().await {
            Ok(stream) => stream,
            Err(error) => {
                warn!(%error, "status announcements are not being followed");
                return;
            }
        };
        while StreamExt::next(&mut announcements).await.is_some() {
            self.refresh_status();
        }
    }

    /// Say how far a backup under way has got, in a banner under the header
    /// bar, for as long as it runs (S06-T12): asked once, for a backup
    /// already under way when the window opened, then followed as it goes.
    async fn follow_progress(self: &Rc<Self>, proxy: crate::daemon::Daemon1Proxy<'static>) {
        use futures::StreamExt;
        enum Heard {
            Progress(String),
            Ended,
        }
        // Subscribed before asking, so nothing said in between is missed.
        let (progress, finished) = (
            proxy.receive_backup_progress().await,
            proxy.receive_job_finished().await,
        );
        if let Ok((job, phase, current, total)) = proxy.get_backup_progress().await {
            self.render_progress(
                (job != 0).then(|| backtrack_core::progress::line(&phase, current, total)),
            );
        }
        let (Ok(progress), Ok(finished)) = (progress, finished) else {
            warn!("a backup's progress will not be shown");
            return;
        };
        let progress = progress.filter_map(|signal| async move {
            let args = signal.args().ok()?;
            Some(Heard::Progress(backtrack_core::progress::line(
                args.phase,
                args.current,
                args.total,
            )))
        });
        let finished = finished.filter_map(|signal| async move {
            let args = signal.args().ok()?;
            matches!(args.kind, "backup" | "offline").then_some(Heard::Ended)
        });
        let mut heard = std::pin::pin!(futures::stream::select(progress, finished));
        while let Some(heard) = StreamExt::next(&mut heard).await {
            self.render_progress(match heard {
                Heard::Progress(line) => Some(line),
                Heard::Ended => None,
            });
        }
    }

    /// Show `line` in the progress banner, or, with `None`, put it away.
    fn render_progress(&self, line: Option<String>) {
        match line {
            Some(line) => {
                self.progress.set_title(&line);
                self.progress.set_revealed(true);
            }
            None => self.progress.set_revealed(false),
        }
    }

    /// Re-read the list of backups whenever it may have grown.
    ///
    /// A backup that finishes adds one; a catalogue job makes pending ones
    /// browsable, and after an import it is working through a whole history,
    /// newest first. Without this the timeline would show what was there when
    /// the window opened, with the snapshots imported since badged as still
    /// being catalogued until somebody reopened it.
    async fn follow_catalogue(self: &Rc<Self>, proxy: crate::daemon::Daemon1Proxy<'static>) {
        use futures::StreamExt;
        if let Ok(mut indexing) = proxy.receive_indexing_progress().await {
            let this = Rc::clone(self);
            ui::spawn(async move {
                // One signal per archive as the catalogue job reaches it, so a
                // new name means the one before it is finished.
                let mut current = String::new();
                while let Some(signal) = StreamExt::next(&mut indexing).await {
                    let Ok(args) = signal.args() else { continue };
                    if args.archive != current {
                        current = args.archive.to_string();
                        this.load_archives();
                    }
                }
            });
        }
        let Ok(mut finished) = proxy.receive_job_finished().await else {
            warn!("new backups will not appear until the window is reopened");
            return;
        };
        while let Some(signal) = StreamExt::next(&mut finished).await {
            let Ok(args) = signal.args() else { continue };
            if matches!(args.kind, "backup" | "offline" | "index") {
                self.load_archives();
            }
        }
    }

    /// One backup older or newer.
    fn step(self: &Rc<Self>, direction: Step) {
        let view = self.state.view();
        match direction.of(&view) {
            Some(seq) => self.state.set_seq(seq),
            None => self.toast(direction.at_the_end()),
        }
    }

    /// Jump to where the selected file next changed, skipping the backups in
    /// which it did not — the forty identical hourlies problem.
    fn step_to_change(self: &Rc<Self>, direction: Step) {
        let view = self.state.view();
        let (Some(seq), Some(target), Some(name)) =
            (view.seq, view.change_target(), view.change_target_name())
        else {
            return;
        };

        let this = Rc::clone(self);
        let query_target = target.clone();
        ui::spawn(async move {
            let answer = this
                .index
                .query(move |reader| reader.next_change(&query_target, seq, direction.into()))
                .await;
            match answer {
                Ok(Some(found)) => this.state.set_seq(found),
                Ok(None) => this.toast(&format!("No {} change to “{name}”", direction.adjective())),
                Err(error) => {
                    warn!(%error, path = target, "the next change could not be looked up");
                    this.toast(&error);
                }
            }
        });
    }

    /// Put the panes into their slots. Each one subscribes to the state and
    /// keeps itself current from there, so nothing has to be told twice.
    fn fill_panes(self: &Rc<Self>) {
        let sidebar = ui::sidebar::build(&self.state);
        self.sidebar_slot.append(&sidebar.widget());
        *self.sidebar.borrow_mut() = Some(sidebar);

        let adder = Rc::clone(self);
        let files = ui::files::build(&self.state, &self.index, &self.daemon, move |folder| {
            adder.add_to_backups(folder)
        });
        self.files_slot.append(&files.widget());
        *self.files.borrow_mut() = Some(files);

        let preview = ui::preview::build(&self.state, &self.daemon);
        self.preview_slot.append(&preview.widget());
        *self.preview.borrow_mut() = Some(preview);

        let restores = ui::restore::build(&self.window, &self.toasts, &self.state, &self.daemon);
        *self.restores.borrow_mut() = Some(Rc::clone(&restores));
        let this = Rc::clone(self);
        self.state.subscribe(move |_, change| {
            if matches!(change, Change::Selection | Change::Seq | Change::Archives) {
                this.refresh_restore_action();
            }
        });

        // Search mode. Its two halves live in the two stacks the frame set up,
        // and one callback keeps them swapping together.
        let navigator = Rc::clone(self);
        let restorer = Rc::clone(self);
        let search = ui::search::build(
            &self.state,
            &self.daemon,
            ui::search::Actions {
                view_in_timeline: Rc::new(move |hit| navigator.go_to_hit(hit)),
                restore_latest: Rc::new(move |hit| {
                    restorer.go_to_hit(hit);
                    let driver = restorer.restores.borrow().clone();
                    if let Some(driver) = driver {
                        driver.start();
                    }
                }),
            },
        );
        self.title_slot.add_named(&search.entry(), Some("search"));
        self.content_slot
            .add_named(&search.widget(), Some("search"));
        let swapper = Rc::clone(self);
        search.on_toggle(move |open| {
            let page = if open { "search" } else { "browse" };
            swapper.title_slot.set_visible_child_name(page);
            swapper.content_slot.set_visible_child_name(page);
        });
        *self.search.borrow_mut() = Some(search);

        let strip = ui::strip::build(&self.state);
        self.strip_slot.append(&strip.widget());
        *self.strip.borrow_mut() = Some(strip);
    }

    /// Keep the position line and the stepping buttons in step with the state.
    fn watch_state(self: &Rc<Self>) {
        let this = Rc::clone(self);
        self.state.subscribe(move |view, change| {
            if !matches!(change, Change::Archives | Change::Listed | Change::Seq) {
                return;
            }
            let tz = glib::TimeZone::local();
            match (view.archive(), view.position()) {
                (Some(archive), Some((ordinal, total))) => {
                    this.position.set_text(&format::position(
                        archive.ts,
                        ordinal,
                        total,
                        view.is_filtered(),
                        &tz,
                    ));
                }
                _ => this.position.set_text(format::NO_BACKUPS),
            }
            this.older.set_sensitive(view.older().is_some());
            this.newer.set_sensitive(view.newer().is_some());
        });

        // The dropdown names the thing it will step through, which is the
        // difference between "next change" and "next change to report.odt".
        let menus = Rc::clone(self);
        self.state.subscribe(move |view, change| {
            if !matches!(change, Change::Selection | Change::Folder) {
                return;
            }
            let name = view.change_target_name().unwrap_or_default();
            menus.older_menu.remove_all();
            menus.newer_menu.remove_all();
            if name.is_empty() {
                return;
            }
            menus.older_menu.append(
                Some(&format!("Previous change to “{name}”")),
                Some("win.older-change"),
            );
            menus.newer_menu.append(
                Some(&format!("Next change to “{name}”")),
                Some("win.newer-change"),
            );
        });
    }

    /// Read the list of backups, off the main loop, with the backups to list
    /// for the folder being viewed asked of the same reading, so a prune in
    /// between cannot leave the two naming different backups.
    fn load_archives(self: &Rc<Self>) {
        let this = Rc::clone(self);
        let folder = self.state.view().folder;
        ui::spawn(async move {
            let asked = folder.clone();
            let answer = this
                .index
                .query(move |reader| Ok((reader.archives_overview()?, reader.listed_for(&asked)?)))
                .await;
            match answer {
                Ok((archives, listed)) => {
                    info!(
                        count = archives.len(),
                        listed = listed.len(),
                        "catalogue loaded"
                    );
                    this.state.set_catalogue(archives, &folder, listed);
                }
                Err(error) => {
                    warn!(%error, "the catalogue could not be read");
                    this.toast(&error);
                }
            }
        });
    }

    /// Ask which backups to list for the folder being viewed, whenever the
    /// list on screen is not that folder's: another folder, or a reload of the
    /// catalogue that brought another folder's answer with it. The answer is
    /// dropped by the state if the window has moved on before it arrives.
    fn watch_listing(self: &Rc<Self>) {
        let this = Rc::clone(self);
        self.state.subscribe(move |view, change| {
            if !matches!(change, Change::Folder | Change::Archives) || view.listing_is_current() {
                return;
            }
            let folder = view.folder.clone();
            let generation = view.generation;
            let asker = Rc::clone(&this);
            ui::spawn(async move {
                let asked = folder.clone();
                match asker
                    .index
                    .query(move |reader| reader.listed_for(&asked))
                    .await
                {
                    Ok(listed) => asker.state.set_listing(&folder, generation, listed),
                    Err(error) => {
                        warn!(%error, folder, "the backups to list could not be read");
                    }
                }
            });
        });
    }

    /// Write Show All Backups to the configuration, so the next window opens
    /// the same way. Without the service it holds for this window only.
    fn remember_show_all(self: &Rc<Self>, on: bool) {
        let Some(proxy) = self.daemon.borrow().clone() else {
            return;
        };
        ui::spawn(async move {
            let literal = if on { "true" } else { "false" };
            if let Err(error) = proxy.set_config("general.show_all_backups", literal).await {
                warn!(%error, "Show All Backups could not be remembered");
            }
        });
    }

    /// Take Show All Backups from the configuration, as the window opens.
    /// Sets the action's state directly, which does not write it back.
    async fn adopt_show_all(&self, proxy: &Daemon1Proxy<'static>) {
        let Ok(text) = proxy.get_config().await else {
            return;
        };
        let Ok((config, _)) = backtrack_core::config::Config::parse(&text) else {
            return;
        };
        let on = config.general.show_all_backups;
        if let Some(action) = self
            .window
            .lookup_action("show-all-backups")
            .and_then(|a| a.downcast::<gio::SimpleAction>().ok())
        {
            action.set_state(&on.to_variant());
        }
        self.state.set_show_all(on);
    }

    /// Reach the daemon, which D-Bus starts if it is not already running.
    ///
    /// Building a proxy proves nothing — zbus does not touch the bus until a
    /// call is made — so the connection is settled by asking for the status.
    /// That is both the activation and the first thing the window wants to
    /// know.
    fn connect_daemon(self: &Rc<Self>) {
        let this = Rc::clone(self);
        ui::spawn(async move {
            let proxy = match crate::daemon::connect().await {
                Ok(proxy) => proxy,
                Err(error) => {
                    // Not fatal: the timeline reads the index, not the daemon.
                    warn!(%error, "running without the Backtrack service");
                    return;
                }
            };
            match proxy.get_status().await {
                Ok(status) => {
                    info!(state = status.state, "connected to the Backtrack service");
                    let proxy_for_name = proxy.clone();
                    *this.daemon.borrow_mut() = Some(proxy);
                    // The window is usable before the service answers, so a
                    // file may already be selected with its preview parked on
                    // "needs the service". Now there is one.
                    let preview = this.preview.borrow().clone();
                    if let Some(preview) = preview {
                        preview.refresh_now();
                    }
                    // Likewise the file pane, which asks the daemon what is
                    // still on the disk and, for a folder with nothing to show,
                    // whether it is backed up at all: a window a file manager
                    // opened on a folder outside the backups says so only once
                    // the daemon can say what the backed-up folders are.
                    let files = this.files.borrow().clone();
                    if let Some(files) = files {
                        files.reload();
                    }
                    this.refresh_restore_action();
                    ui::menu::set_enabled(&this.window, "backup-now", true);
                    ui::menu::set_enabled(&this.window, "pause", true);
                    ui::menu::set_enabled(&this.window, "preferences", true);
                    this.adopt_show_all(&proxy_for_name).await;
                    this.learn_destination(&proxy_for_name, &status).await;
                    this.render_status(&status);
                    let pending = this.pending_fix.borrow_mut().take();
                    if let Some(reason) = pending {
                        this.fix_if_current(&status, reason);
                    }
                    let following = Rc::clone(&this);
                    let proxy = following.daemon.borrow().clone();
                    if let Some(proxy) = proxy {
                        let cataloguing = Rc::clone(&following);
                        let catalogue_proxy = proxy.clone();
                        ui::spawn(
                            async move { cataloguing.follow_catalogue(catalogue_proxy).await },
                        );
                        let progressing = Rc::clone(&following);
                        let progress_proxy = proxy.clone();
                        ui::spawn(async move { progressing.follow_progress(progress_proxy).await });
                        ui::spawn(async move { following.follow_status(proxy).await });
                    }
                    let ticking = Rc::clone(&this);
                    ui::spawn(async move { ticking.keep_status_fresh().await });
                }
                Err(error) => {
                    warn!(%error, "the Backtrack service did not answer");
                }
            }
        });
    }

    /// Point this window at a different folder — a second launch from a file
    /// manager, rather than a click inside the window.
    fn retarget(self: &Rc<Self>, target: &Target) {
        self.state.set_folder(target.folder.clone());
        self.apply_selection(target);
    }

    /// Pre-select the file the launcher named.
    ///
    /// The file pane restores the selection by name each time it reloads, so
    /// setting it before the first load is enough: the entry is selected when
    /// the folder arrives, and stays selected as the user steps back through
    /// time for as long as the file existed. `is_dir` is corrected by the pane
    /// once it has the real entry.
    fn apply_selection(self: &Rc<Self>, target: &Target) {
        let Some(select) = &target.select else {
            return;
        };
        if !crate::path::is_within(select, &target.folder) {
            warn!(
                path = select,
                folder = target.folder,
                "the selected file is not in the folder being opened; ignoring it"
            );
            return;
        }
        info!(path = select, "opening with a file selected");
        self.state.set_selected(Some(crate::state::Selected {
            path: select.clone(),
            name: crate::path::name(select).to_string(),
            is_dir: false,
            // Unknown until the folder loads, at which point the pane replaces
            // this with the real entry.
            size: -1,
            mtime: 0,
            on_disk: 0,
        }));
    }

    /// Back up `folder` (an archive path) from now on, and open Preferences on
    /// the list it has joined, where it can be taken out again. The file
    /// pane offers this for a folder outside the backups.
    fn add_to_backups(self: &Rc<Self>, folder: &str) {
        let Some(proxy) = self.daemon.borrow().clone() else {
            self.toast(NO_SERVICE);
            return;
        };
        let path = crate::path::to_filesystem(folder);
        let name = crate::path::name(folder).to_string();
        let this = Rc::clone(self);
        ui::spawn(async move {
            let include = match proxy.get_config().await {
                Ok(text) => match backtrack_core::config::Config::parse(&text) {
                    Ok((config, _)) => {
                        crate::model::prefs::with_folders(&config.backup.include, &[path])
                    }
                    Err(error) => {
                        warn!(error, "the settings could not be read");
                        this.toast(&error);
                        return;
                    }
                },
                Err(error) => {
                    warn!(%error, "the settings could not be read");
                    this.toast(&clean(&error.to_string()));
                    return;
                }
            };
            let literal = match backtrack_core::config::toml_literal(&include) {
                Ok(literal) => literal,
                Err(error) => {
                    warn!(%error, "the folder list could not be written out");
                    return;
                }
            };
            let key = crate::model::prefs::Setting::Include.key();
            if let Err(error) = proxy.set_config(key, &literal).await {
                warn!(%error, folder = name, "the folder was not added to the backups");
                this.toast(&clean(&error.to_string()));
                return;
            }
            info!(folder = name, "folder added to the backups");
            this.toast(&format!("“{name}” added to your backups"));
            let files = this.files.borrow().clone();
            if let Some(files) = files {
                files.reload();
            }
            let app = this
                .window
                .application()
                .and_then(|app| app.downcast::<adw::Application>().ok());
            if let Some(app) = app {
                ui::prefs::present(&app, &this.window, proxy, Some("backup"));
            }
        });
    }

    /// Say something in passing, in the window it concerns.
    pub fn toast(&self, message: &str) {
        self.toasts.add_toast(ui::toast(message));
    }
}

/// Where a window opens when it is not told: over the top of the newest
/// backup. For backups of the usual folders that is the home folder; for one
/// folder chosen deep in the tree it is that folder's parent; for somebody's
/// imported backups it may be anywhere at all.
///
/// With nothing catalogued yet, as when the wizard has just started the first
/// backup, it is over the folders the settings back up, which is where that
/// backup will be once it is catalogued; and with no daemon to ask, the home
/// folder.
pub async fn backed_up_target(daemon: Option<&Daemon1Proxy<'static>>) -> Target {
    let catalogued = gio::spawn_blocking(|| {
        let reader =
            backtrack_core::index::IndexReader::open(&backtrack_core::paths::index_db()).ok()?;
        let seq = reader.latest_catalogued_seq().ok()??;
        let roots = reader.backed_up_roots(seq).ok()?;
        crate::path::common_parent(&roots)
    })
    .await
    .ok()
    .flatten();
    let found = match (catalogued, daemon) {
        (Some(folder), _) => Some(folder),
        (None, Some(daemon)) => chosen_parent(daemon).await,
        (None, None) => None,
    };
    Target {
        folder: found.unwrap_or_else(|| crate::path::to_archive(&glib::home_dir())),
        select: None,
    }
}

/// The folder holding every folder the settings back up, or `None` if the
/// settings cannot be read.
async fn chosen_parent(daemon: &Daemon1Proxy<'static>) -> Option<String> {
    let text = daemon.get_config().await.ok()?;
    let (config, _) = backtrack_core::config::Config::parse(&text).ok()?;
    let roots: Vec<String> = backtrack_core::roots::Roots::of(&config)
        .roots
        .iter()
        .map(|root| crate::path::to_archive(root))
        .collect();
    crate::path::common_parent(&roots)
}

/// Point every open window at `target` and re-read what it shows, as after
/// the wizard has moved the backups somewhere else. Returns whether there
/// was a window to do it to.
pub fn refresh_all(target: &Target) -> bool {
    let open: Vec<Rc<Window>> = OPEN.with(|open| open.borrow().clone());
    for window in &open {
        window.retarget(target);
        window.load_archives();
        window.refresh_status();
    }
    !open.is_empty()
}

/// Open the fix for `reason` in `window`, if it is still needed.
pub fn fix(window: &gtk4::Window, reason: Reason) {
    let found = OPEN.with(|open| {
        open.borrow()
            .iter()
            .find(|w| w.window.upcast_ref::<gtk4::Window>() == window)
            .cloned()
    });
    let Some(found) = found else { return };
    let Some(proxy) = found.daemon.borrow().clone() else {
        *found.pending_fix.borrow_mut() = Some(reason);
        return;
    };
    ui::spawn(async move {
        match proxy.get_status().await {
            Ok(status) => found.fix_if_current(&status, reason),
            Err(error) => warn!(%error, "the status could not be read"),
        }
    });
}

/// Point an already-open window at `target`.
pub fn retarget(window: &gtk4::Window, target: &Target) {
    OPEN.with(|open| {
        let found = open
            .borrow()
            .iter()
            .find(|w| w.window.upcast_ref::<gtk4::Window>() == window)
            .cloned();
        if let Some(found) = found {
            found.retarget(target);
        }
    });
}

/// How often the status line re-reads what it is saying.
/// The daemon's "this path is on the computer" answer, as `PathsOnDisk`
/// encodes it. Only a known presence gives the compare view a right-hand side.
const ON_DISK_PRESENT: u8 = 2;
const ON_DISK_ABSENT: u8 = 1;

const STATUS_TICK_SECONDS: u32 = 30;

/// What the window says when something needs the daemon and there isn't one.
const NO_SERVICE: &str = "The Backtrack service is not running, so this cannot be done from here";

/// Strip the D-Bus error-name prefix, which is addressed to programs.
fn clean(message: &str) -> String {
    message
        .rsplit_once(": ")
        .map_or(message, |(_, tail)| tail)
        .to_string()
}

/// An icon-and-label button, the way the action bar draws them.
fn button_content(icon: &str, label: &str) -> GtkBox {
    let content = GtkBox::new(Orientation::Horizontal, 8);
    content.append(&gtk4::Image::from_icon_name(icon));
    content.append(&Label::new(Some(label)));
    content
}

/// Older / Newer: one step on the button, and the per-file jump on its
/// dropdown. A split button rather than a long-press, because a long-press is
/// undiscoverable and unreachable from the keyboard.
fn step_button(
    label: &str,
    icon: &str,
    icon_first: bool,
    action: &str,
    menu: &gio::Menu,
) -> adw::SplitButton {
    let content = GtkBox::new(Orientation::Horizontal, 8);
    let image = gtk4::Image::from_icon_name(icon);
    let text = Label::new(Some(label));
    if icon_first {
        content.append(&image);
        content.append(&text);
    } else {
        content.append(&text);
        content.append(&image);
    }
    let button = adw::SplitButton::builder()
        .child(&content)
        .action_name(action)
        .menu_model(menu)
        .build();
    button.set_sensitive(false);
    button
}

/// Which way through time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Older,
    Newer,
}

impl Step {
    fn of(self, view: &crate::state::View) -> Option<i64> {
        match self {
            Step::Older => view.older(),
            Step::Newer => view.newer(),
        }
    }

    fn at_the_end(self) -> &'static str {
        match self {
            Step::Older => "This is the oldest backup",
            Step::Newer => "This is the most recent backup",
        }
    }

    fn adjective(self) -> &'static str {
        match self {
            Step::Older => "earlier",
            Step::Newer => "later",
        }
    }
}

impl From<Step> for backtrack_core::index::Direction {
    fn from(step: Step) -> Self {
        match step {
            Step::Older => backtrack_core::index::Direction::Older,
            Step::Newer => backtrack_core::index::Direction::Newer,
        }
    }
}
