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

use std::cell::RefCell;
use std::rc::Rc;

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
            calendar: RefCell::new(None),
            strip: RefCell::new(None),
        });

        this.install_actions();
        this.assemble();
        this.fill_panes();
        this.watch_state();
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
        layout.set_content(Some(&self.body()));
        layout.add_bottom_bar(&self.actions());

        self.toasts.set_child(Some(&layout));
        self.window.set_content(Some(&self.toasts));
    }

    fn header(self: &Rc<Self>) -> adw::HeaderBar {
        let header = adw::HeaderBar::new();

        let identity = GtkBox::new(Orientation::Horizontal, 8);
        identity.append(&gtk4::Image::from_icon_name(ui::app_icon_name(
            &self.window,
        )));
        let name = Label::new(Some("Backtrack"));
        name.add_css_class("heading");
        identity.append(&name);
        header.pack_start(&identity);

        header.set_title_widget(Some(&ui::breadcrumb::build(&self.state)));

        header.pack_end(&ui::menu::button());

        // Stage 8 fills this in; it is here now because its absence would
        // change the shape of the header bar when it arrives.
        let search = Button::from_icon_name("system-search-symbolic");
        search.set_tooltip_text(Some("Search every backup (coming soon)"));
        search.update_property(&[gtk4::accessible::Property::Label("Search every backup")]);
        search.set_sensitive(false);
        header.pack_end(&search);

        header
    }

    fn body(self: &Rc<Self>) -> adw::OverlaySplitView {
        let sidebar = GtkBox::new(Orientation::Vertical, 0);
        sidebar.append(&self.sidebar_header());
        self.sidebar_slot.set_vexpand(true);
        sidebar.append(&self.sidebar_slot);

        let content = GtkBox::new(Orientation::Vertical, 0);
        content.append(&self.time_bar());

        let panes = GtkBox::new(Orientation::Horizontal, 12);
        panes.set_vexpand(true);
        panes.set_margin_start(12);
        panes.set_margin_end(12);
        self.files_slot.set_hexpand(true);
        panes.append(&self.files_slot);
        self.preview_slot.set_size_request(280, -1);
        panes.append(&self.preview_slot);
        content.append(&panes);

        self.strip_slot.set_margin_start(12);
        self.strip_slot.set_margin_end(12);
        self.strip_slot.set_margin_top(12);
        content.append(&self.strip_slot);

        adw::OverlaySplitView::builder()
            .sidebar(&sidebar)
            .content(&content)
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

        // All three land in Stages 7 and 8. They are built now, insensitive,
        // because the action bar is part of the frame this stage delivers.
        for (label, icon, tooltip) in [
            (
                "Compare with Today",
                "view-dual-symbolic",
                "Compare this version with the file on your computer (coming soon)",
            ),
            (
                "Restore To…",
                "folder-symbolic",
                "Restore to a folder you choose (coming soon)",
            ),
        ] {
            let button = Button::builder().build();
            button.set_child(Some(&button_content(icon, label)));
            button.set_tooltip_text(Some(tooltip));
            button.set_sensitive(false);
            buttons.append(&button);
        }

        let restore = Button::builder().build();
        restore.set_child(Some(&button_content("edit-undo-symbolic", "Restore")));
        restore.add_css_class("suggested-action");
        restore.set_tooltip_text(Some("Restore the selected item (coming soon)"));
        restore.set_sensitive(false);
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

        self.install_menu_actions();
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

        // Stage 7 and Stage 9. Present so the menu is the shape it will keep,
        // and disabled so it cannot promise anything it will not do.
        for name in ["recently-replaced", "preferences"] {
            let action = gio::SimpleAction::new(name, None);
            action.set_enabled(false);
            self.window.add_action(&action);
        }

        let shortcuts = gio::SimpleAction::new("shortcuts", None);
        let owner = self.window.clone();
        shortcuts.connect_activate(move |_, _| ui::menu::shortcuts_window(&owner).present());
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
        let Some(seconds) = ui::menu::pause_duration(option, now, &tz) else {
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

    /// Re-read the daemon's status and redraw the line at the bottom.
    fn refresh_status(self: &Rc<Self>) {
        let Some(proxy) = self.daemon.borrow().clone() else {
            self.status.set_text(NO_SERVICE);
            return;
        };
        let this = Rc::clone(self);
        ui::spawn(async move {
            match proxy.get_status().await {
                Ok(status) => this.render_status(&status),
                Err(error) => warn!(%error, "the status could not be read"),
            }
        });
    }

    fn render_status(&self, status: &backtrack_core::dbus::Status) {
        let now = glib::DateTime::now_utc().map(|d| d.to_unix()).unwrap_or(0);
        let tz = glib::TimeZone::local();
        self.status
            .set_text(&crate::model::status::line(status, now, &tz));
        // Resuming is only an offer when there is something to resume.
        ui::menu::set_enabled(
            &self.window,
            "resume",
            status.paused_until > now.max(0) as u64,
        );
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

        let files = ui::files::build(&self.state, &self.index);
        self.files_slot.append(&files.widget());
        *self.files.borrow_mut() = Some(files);

        let preview = ui::preview::build(&self.state, &self.daemon);
        self.preview_slot.append(&preview.widget());
        *self.preview.borrow_mut() = Some(preview);

        let strip = ui::strip::build(&self.state);
        self.strip_slot.append(&strip.widget());
        *self.strip.borrow_mut() = Some(strip);
    }

    /// Keep the position line and the stepping buttons in step with the state.
    fn watch_state(self: &Rc<Self>) {
        let this = Rc::clone(self);
        self.state.subscribe(move |view, change| {
            if !matches!(change, Change::Archives | Change::Seq) {
                return;
            }
            let tz = glib::TimeZone::local();
            match (view.archive(), view.position()) {
                (Some(archive), Some((ordinal, total))) => {
                    this.position
                        .set_text(&format::position(archive.ts, ordinal, total, &tz));
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

    /// Read the list of backups, off the main loop.
    fn load_archives(self: &Rc<Self>) {
        let this = Rc::clone(self);
        ui::spawn(async move {
            match this.index.query(|reader| reader.archives_overview()).await {
                Ok(archives) => {
                    info!(count = archives.len(), "catalogue loaded");
                    this.state.set_archives(archives);
                }
                Err(error) => {
                    warn!(%error, "the catalogue could not be read");
                    this.toast(&error);
                }
            }
        });
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
                    *this.daemon.borrow_mut() = Some(proxy);
                    // The window is usable before the service answers, so a
                    // file may already be selected with its preview parked on
                    // "needs the service". Now there is one.
                    let preview = this.preview.borrow().clone();
                    if let Some(preview) = preview {
                        preview.refresh_now();
                    }
                    ui::menu::set_enabled(&this.window, "backup-now", true);
                    ui::menu::set_enabled(&this.window, "pause", true);
                    this.render_status(&status);
                    let following = Rc::clone(&this);
                    let proxy = following.daemon.borrow().clone();
                    if let Some(proxy) = proxy {
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
        }));
    }

    /// Say something in passing, in the window it concerns.
    pub fn toast(&self, message: &str) {
        self.toasts.add_toast(adw::Toast::new(message));
    }
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
