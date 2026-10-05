// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Restoring a whole computer, watched: Screen 12, mockup 22.
//!
//! The restore is the daemon's, not this window's. Closing the window leaves
//! it running; opening Backtrack again comes back here, to wherever it has got
//! to. So the window holds nothing of its own beyond what it is showing: it
//! asks the daemon where the restore is, once a second and whenever a job
//! ends, and draws the answer.
//!
//! When it is done, files that clash with this computer's are asked about
//! with the summary every folder restore uses, all of them at once.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use backtrack_core::dbus::RecoveryStatus;
use futures::StreamExt;
use gtk4::prelude::*;
use gtk4::{glib, Align, Box as GtkBox, Button, Image, Label, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::{info, warn};

use crate::daemon::Daemon1Proxy;
use crate::model::everything as copy;

thread_local! {
    /// The one progress window, if it is open.
    static OPEN: RefCell<Option<Rc<Progress>>> = const { RefCell::new(None) };
}

/// Show the progress of restoring this computer, bringing the window forward
/// if it is already open.
pub fn present(app: &adw::Application, daemon: Daemon1Proxy<'static>) {
    if let Some(open) = OPEN.with(|open| open.borrow().clone()) {
        open.window.present();
        open.refresh();
        return;
    }
    let progress = Progress::build(app, daemon);
    OPEN.with(|open| *open.borrow_mut() = Some(Rc::clone(&progress)));
    progress.window.present();
    progress.refresh();
    progress.watch();
}

/// Whether a restore of this computer is under way or waiting on the person,
/// so that opening Backtrack should come here rather than to the timeline.
pub fn wants_attention(status: &RecoveryStatus) -> bool {
    matches!(
        status.state.as_str(),
        "running" | "paused" | "stopped" | "review"
    )
}

struct Progress {
    app: adw::Application,
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    daemon: Daemon1Proxy<'static>,
    heading: Label,
    bar: gtk4::ProgressBar,
    percent: Label,
    line: Label,
    current: Label,
    steps: GtkBox,
    pause: Button,
    pause_content: adw::ButtonContent,
    cancel: Button,
    action: Button,
    /// What was drawn last, so a change of state can be noticed.
    last: RefCell<Option<RecoveryStatus>>,
    /// The steps as drawn, so they are rebuilt only when they change.
    drawn_steps: RefCell<Vec<(String, String)>>,
    /// The restore was cancelled from here, and whether what it had restored
    /// was taken away.
    cancelled: Cell<Option<bool>>,
    /// A button's work is under way; the others wait for it.
    busy: Cell<bool>,
}

impl Progress {
    fn build(app: &adw::Application, daemon: Daemon1Proxy<'static>) -> Rc<Progress> {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Restoring Your Files")
            .default_width(760)
            .default_height(420)
            .width_request(360)
            .height_request(300)
            .build();

        let body = GtkBox::new(Orientation::Vertical, 12);
        body.set_margin_top(24);
        body.set_margin_bottom(24);
        body.set_margin_start(24);
        body.set_margin_end(24);

        let top = GtkBox::new(Orientation::Horizontal, 12);
        let heading = Label::builder()
            .label("Restoring your files…")
            .xalign(0.0)
            .hexpand(true)
            .wrap(true)
            .build();
        heading.add_css_class("title-1");
        top.append(&heading);
        let pause_content = adw::ButtonContent::builder()
            .icon_name("media-playback-pause-symbolic")
            .label("Pause")
            .build();
        let pause = Button::builder()
            .child(&pause_content)
            .valign(Align::Center)
            .build();
        top.append(&pause);
        let cancel = Button::builder()
            .child(
                &adw::ButtonContent::builder()
                    .icon_name("process-stop-symbolic")
                    .label("Cancel")
                    .build(),
            )
            .valign(Align::Center)
            .build();
        top.append(&cancel);
        body.append(&top);

        let bar_row = GtkBox::new(Orientation::Horizontal, 18);
        bar_row.set_margin_top(12);
        let bar = gtk4::ProgressBar::builder()
            .hexpand(true)
            .valign(Align::Center)
            .build();
        bar.add_css_class("recovery-bar");
        bar_row.append(&bar);
        let percent = Label::new(Some("0%"));
        percent.add_css_class("title-3");
        percent.add_css_class("numeric");
        bar_row.append(&percent);
        body.append(&bar_row);

        let line = Label::builder().xalign(0.0).wrap(true).build();
        line.add_css_class("title-4");
        line.add_css_class("numeric");
        body.append(&line);
        let current = Label::builder()
            .xalign(0.0)
            .ellipsize(gtk4::pango::EllipsizeMode::Middle)
            .build();
        current.add_css_class("dim-label");
        body.append(&current);

        let steps = GtkBox::new(Orientation::Horizontal, 0);
        steps.set_halign(Align::Center);
        steps.set_margin_top(18);
        let strip = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Automatic)
            .vscrollbar_policy(gtk4::PolicyType::Never)
            .propagate_natural_height(true)
            .child(&steps)
            .build();
        body.append(&strip);

        let action = Button::builder()
            .halign(Align::Center)
            .margin_top(18)
            .visible(false)
            .build();
        action.add_css_class("pill");
        action.add_css_class("suggested-action");
        body.append(&action);

        let clamp = adw::Clamp::builder().maximum_size(900).child(&body).build();
        let scroller = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&scroller));
        let view = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        // The mockup's header is bare: the heading in the window says it.
        header.set_title_widget(Some(&Label::new(None)));
        view.add_top_bar(&header);
        view.set_content(Some(&toasts));
        window.set_content(Some(&view));

        let progress = Rc::new(Progress {
            app: app.clone(),
            window,
            toasts,
            daemon,
            heading,
            bar,
            percent,
            line,
            current,
            steps,
            pause,
            pause_content,
            cancel,
            action,
            last: RefCell::new(None),
            drawn_steps: RefCell::new(Vec::new()),
            cancelled: Cell::new(None),
            busy: Cell::new(false),
        });

        let this = Rc::clone(&progress);
        progress
            .pause
            .connect_clicked(move |_| this.pause_or_resume());
        let this = Rc::clone(&progress);
        progress
            .cancel
            .connect_clicked(move |_| this.confirm_cancel());
        let this = Rc::clone(&progress);
        progress.action.connect_clicked(move |_| this.act());
        progress.window.connect_close_request(|_| {
            // The restore carries on without the window. Forgetting it here is
            // what lets the next launch build a fresh one.
            OPEN.with(|open| *open.borrow_mut() = None);
            info!("the restore window was closed; the restore carries on");
            glib::Propagation::Proceed
        });
        progress
    }

    /// Ask the daemon again, once a second while the window is open, and
    /// straight away whenever a job ends.
    fn watch(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(1, move || match weak.upgrade() {
            Some(this) if this.window.is_visible() => {
                this.refresh();
                glib::ControlFlow::Continue
            }
            _ => glib::ControlFlow::Break,
        });
        let weak = Rc::downgrade(self);
        let daemon = self.daemon.clone();
        crate::ui::spawn(async move {
            let Ok(mut finished) = daemon.receive_job_finished().await else {
                return;
            };
            while StreamExt::next(&mut finished).await.is_some() {
                match weak.upgrade() {
                    Some(this) => this.refresh(),
                    None => return,
                }
            }
        });
    }

    fn refresh(self: &Rc<Self>) {
        let this = Rc::clone(self);
        crate::ui::spawn(async move {
            match this.daemon.get_recovery().await {
                Ok(status) => this.show(status),
                Err(error) => warn!(%error, "where the restore is could not be read"),
            }
        });
    }

    fn show(self: &Rc<Self>, status: RecoveryStatus) {
        let before = self.last.borrow().as_ref().map(|s| s.state.clone());
        let state = status.state.as_str();

        // Just finished, while somebody was watching: say so here as well as
        // on the desktop.
        if matches!(before.as_deref(), Some("running" | "paused" | "stopped"))
            && matches!(state, "finished" | "review")
        {
            self.toasts
                .add_toast(crate::ui::toast(&copy::finished_line(&status)));
            self.window.present();
        }

        if state == "none" {
            self.show_nothing_left();
            *self.last.borrow_mut() = Some(status);
            return;
        }

        self.heading.set_label(copy::heading(&status));
        let fraction = if status.total == 0 {
            if matches!(state, "finished" | "review") {
                1.0
            } else {
                0.0
            }
        } else {
            status.done as f64 / status.total as f64
        };
        self.bar.set_fraction(fraction.clamp(0.0, 1.0));
        self.percent
            .set_label(&format!("{}%", copy::percent(status.done, status.total)));
        match state {
            "finished" | "review" => self.line.set_label(&copy::finished_line(&status)),
            _ => self.line.set_label(&copy::progress_line(&status)),
        }
        let current = copy::restoring_line(&status);
        self.current.set_visible(!current.is_empty());
        self.current.set_label(&current);
        self.draw_steps(&status);

        let running = state == "running";
        let paused = state == "paused";
        self.pause.set_visible(running || paused);
        self.pause_content
            .set_label(if paused { "Resume" } else { "Pause" });
        self.pause_content.set_icon_name(if paused {
            "media-playback-start-symbolic"
        } else {
            "media-playback-pause-symbolic"
        });
        self.cancel
            .set_visible(matches!(state, "running" | "paused" | "stopped"));
        let action = match state {
            "stopped" => Some("Try Again"),
            "review" => Some("Choose Which to Keep…"),
            "finished" => Some("Open Backtrack"),
            _ => None,
        };
        self.action.set_visible(action.is_some());
        if let Some(label) = action {
            self.action.set_label(label);
        }
        let idle = !self.busy.get();
        for button in [&self.pause, &self.cancel, &self.action] {
            button.set_sensitive(idle);
        }
        *self.last.borrow_mut() = Some(status);
    }

    /// No restore at all: one that was cancelled from here, or one that ended
    /// before this window was opened.
    fn show_nothing_left(&self) {
        let (heading, line) = match self.cancelled.get() {
            Some(discarded) => ("Restoring stopped", copy::cancelled_line(discarded)),
            None => ("Your files are back", "Nothing is being restored."),
        };
        self.heading.set_label(heading);
        self.line.set_label(line);
        self.current.set_visible(false);
        self.pause.set_visible(false);
        self.cancel.set_visible(false);
        self.action.set_label("Open Backtrack");
        self.action.set_visible(true);
        self.action.set_sensitive(true);
    }

    /// The row of folders along the bottom: done, under way, still to come.
    fn draw_steps(&self, status: &RecoveryStatus) {
        let wanted: Vec<(String, String)> = status
            .steps
            .iter()
            .map(|s| (s.key.clone(), s.status.clone()))
            .collect();
        if *self.drawn_steps.borrow() == wanted {
            return;
        }
        while let Some(child) = self.steps.first_child() {
            self.steps.remove(&child);
        }
        for (i, (key, state)) in wanted.iter().enumerate() {
            if i > 0 {
                let joint = gtk4::Separator::new(Orientation::Horizontal);
                joint.set_valign(Align::Start);
                joint.set_margin_top(22);
                joint.set_size_request(48, -1);
                joint.add_css_class("recovery-joint");
                self.steps.append(&joint);
            }
            let step = GtkBox::new(Orientation::Vertical, 8);
            step.set_margin_start(6);
            step.set_margin_end(6);
            let (icon, class) = match state.as_str() {
                "done" => ("object-select-symbolic", "done"),
                "current" => ("media-playback-start-symbolic", "current"),
                _ => ("", "pending"),
            };
            let mark = Image::from_icon_name(icon);
            mark.set_pixel_size(20);
            mark.set_halign(Align::Center);
            mark.add_css_class("recovery-step");
            mark.add_css_class(class);
            step.append(&mark);
            let name = Label::new(Some(&copy::step_name(key)));
            name.add_css_class("recovery-step-name");
            name.add_css_class(class);
            step.append(&name);
            self.steps.append(&step);
        }
        *self.drawn_steps.borrow_mut() = wanted;
    }

    fn pause_or_resume(self: &Rc<Self>) {
        let Some(status) = self.last.borrow().clone() else {
            return;
        };
        let this = Rc::clone(self);
        self.work(async move {
            let outcome = if status.state == "paused" {
                info!("resuming the restore");
                this.daemon.resume_recovery().await.map(|_| ())
            } else {
                info!("pausing the restore");
                this.daemon.pause_job(status.job).await
            };
            if let Err(error) = outcome {
                warn!(%error, "the restore could not be paused or resumed");
                this.toast(&crate::ui::wizard::explain(&error));
            }
        });
    }

    fn confirm_cancel(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::new(Some(copy::CANCEL_TITLE), Some(copy::CANCEL_BODY));
        dialog.add_responses(&[
            ("continue", "Continue Restoring"),
            ("discard", copy::DISCARD),
            ("keep", copy::KEEP),
        ]);
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        dialog.set_response_appearance("keep", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("continue"));
        dialog.set_close_response("continue");
        crate::ui::prefer_wide_responses(&dialog);
        let this = Rc::clone(self);
        dialog.connect_response(None, move |_, response| {
            let discard = match response {
                "keep" => false,
                "discard" => true,
                _ => return,
            };
            this.cancel_it(discard);
        });
        dialog.present(Some(&self.window));
    }

    fn cancel_it(self: &Rc<Self>, discard: bool) {
        let this = Rc::clone(self);
        self.work(async move {
            info!(discard, "cancelling the restore");
            match run_job(&this.daemon, |d| d.cancel_recovery(discard)).await {
                Ok(_) => this.cancelled.set(Some(discard)),
                Err(message) => this.toast(&message),
            }
        });
    }

    /// The button under the steps: whatever the state asks for.
    fn act(self: &Rc<Self>) {
        let state = self
            .last
            .borrow()
            .as_ref()
            .map(|s| s.state.clone())
            .unwrap_or_default();
        match state.as_str() {
            "stopped" => {
                let this = Rc::clone(self);
                self.work(async move {
                    if let Err(error) = this.daemon.resume_recovery().await {
                        this.toast(&crate::ui::wizard::explain(&error));
                    }
                });
            }
            "review" => {
                let this = Rc::clone(self);
                self.work(async move { this.review().await });
            }
            _ => self.open_backtrack(),
        }
    }

    /// Ask about every file that clashed, with the summary any folder restore
    /// shows, and do what was answered.
    async fn review(self: &Rc<Self>) {
        let Some(status) = self.last.borrow().clone() else {
            return;
        };
        let job = match run_job(&self.daemon, |d| d.prepare_recovery_review()).await {
            Ok(job) => job,
            Err(message) => {
                self.toast(&message);
                return;
            }
        };
        let preview = match self.daemon.get_restore_preview(job).await {
            Ok(preview) => preview,
            Err(error) => {
                warn!(%error, "the files waiting could not be read");
                self.toast(&crate::ui::wizard::explain(&error));
                return;
            }
        };
        let home = glib::home_dir();
        let folder = home
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Home".to_string());
        let (blanket, decisions) = match crate::ui::summary::ask(
            &self.window,
            &preview,
            &folder,
            "",
            status.taken,
        )
        .await
        {
            // Nothing is lost by not answering now: the backup's copies
            // wait until somebody does.
            crate::ui::summary::Answer::Cancel => return,
            crate::ui::summary::Answer::KeepBoth => ("keep-both".to_string(), Vec::new()),
            crate::ui::summary::Answer::Replace { decisions } if !decisions.is_empty() => {
                ("skip".to_string(), decisions)
            }
            crate::ui::summary::Answer::Replace { .. } => ("replace".to_string(), Vec::new()),
        };
        match run_job(&self.daemon, |d| {
            d.execute_restore(job, &blanket, &decisions)
        })
        .await
        {
            Ok(_) => self
                .toast("Done. Anything replaced is kept for 30 days in case you change your mind."),
            Err(message) => self.toast(&message),
        }
    }

    fn open_backtrack(self: &Rc<Self>) {
        let app = self.app.clone();
        let window = self.window.clone();
        crate::ui::spawn(async move {
            let target = crate::window::backed_up_target().await;
            if !crate::window::refresh_all(&target) {
                crate::window::Window::build(&app, &target).present();
            }
            window.close();
        });
    }

    /// Run one button's work, keeping the buttons still until it is done.
    fn work(self: &Rc<Self>, task: impl std::future::Future<Output = ()> + 'static) {
        if self.busy.get() {
            return;
        }
        self.busy.set(true);
        for button in [&self.pause, &self.cancel, &self.action] {
            button.set_sensitive(false);
        }
        let this = Rc::clone(self);
        crate::ui::spawn(async move {
            task.await;
            this.busy.set(false);
            this.refresh();
        });
    }

    fn toast(&self, message: &str) {
        self.toasts.add_toast(crate::ui::toast(message));
    }
}

/// Start something that returns a job id and wait for the job to end: its id
/// when it completed, otherwise what to tell the person.
///
/// The signal stream is opened before the call, so a job that ends at once is
/// not missed.
async fn run_job<'a, F, Fut>(daemon: &'a Daemon1Proxy<'static>, start: F) -> Result<u64, String>
where
    F: FnOnce(&'a Daemon1Proxy<'static>) -> Fut,
    Fut: std::future::Future<Output = zbus::Result<u64>> + 'a,
{
    let mut finished = daemon
        .receive_job_finished()
        .await
        .map_err(|e| e.to_string())?;
    let job = start(daemon)
        .await
        .map_err(|e| crate::ui::wizard::explain(&e))?;
    while let Some(signal) = StreamExt::next(&mut finished).await {
        let Ok(args) = signal.args() else { continue };
        if args.job != job {
            continue;
        }
        return match args.outcome {
            "completed" => Ok(job),
            "cancelled" => Err("Cancelled. Nothing was changed.".to_string()),
            _ => Err("That did not finish. The logs say why.".to_string()),
        };
    }
    Err("The Backtrack service stopped answering".to_string())
}
