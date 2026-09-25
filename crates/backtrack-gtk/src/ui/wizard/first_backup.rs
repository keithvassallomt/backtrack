// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! After Start: the first backup, watched for as long as anybody watches.
//!
//! The backup belongs to the daemon, not to this page. Closing the window
//! mid-way stops nothing, which is what the page says; the daemon announces
//! the end with a notification of its own for exactly that reason. The page
//! is only a view of a job that would carry on without it.

use std::rc::Rc;

use futures::StreamExt;
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Label, Orientation, ProgressBar};
use libadwaita as adw;
use tracing::{info, warn};

use super::{explain, Wizard};
use crate::model::wizard as model;

/// The wireframe's promise, made before the backup starts rather than after.
const EXPECTATION: &str = "First backup may take a few hours. After that, hourly backups take \
                           minutes. You can close this window.";

struct Page {
    wizard: Rc<Wizard>,
    page: adw::NavigationPage,
    title: Label,
    body: Label,
    bar: ProgressBar,
    detail: Label,
    browse: Button,
    retry: Button,
}

/// Start the first backup and show it running.
pub async fn start(wizard: &Rc<Wizard>) {
    // Listening begins before the backup is asked for: a small first backup
    // can finish before a subscription made afterwards would exist.
    let finished = wizard.daemon.receive_job_finished().await;
    let progress = wizard.daemon.receive_backup_progress().await;
    let job = match wizard.daemon.backup_now().await {
        Ok(job) => job,
        Err(error) => {
            warn!(%error, "the first backup could not be started");
            wizard.toast(&explain(&error));
            return;
        }
    };
    info!(job, "first backup started");

    let page = build(wizard);
    wizard.nav.push(&page.page);

    if let Ok(mut progress) = progress {
        let watcher = Rc::clone(&page);
        crate::ui::spawn(async move {
            while let Some(signal) = StreamExt::next(&mut progress).await {
                let Ok(args) = signal.args() else { continue };
                if args.job == job {
                    watcher.progress(args.phase, args.current);
                }
            }
        });
    }
    match finished {
        Ok(mut finished) => {
            while let Some(signal) = StreamExt::next(&mut finished).await {
                let Ok(args) = signal.args() else { continue };
                if args.job == job {
                    info!(job, outcome = args.outcome, "first backup ended");
                    page.ended(args.outcome);
                    return;
                }
            }
        }
        Err(error) => warn!(%error, "the first backup's end will not be shown here"),
    }
}

fn build(wizard: &Rc<Wizard>) -> Rc<Page> {
    let body_box = GtkBox::new(Orientation::Vertical, 18);
    body_box.set_valign(Align::Center);
    body_box.set_margin_top(24);
    body_box.set_margin_bottom(24);
    body_box.set_margin_start(24);
    body_box.set_margin_end(24);

    let title = Label::new(Some("Backing up for the first time"));
    title.add_css_class("title-1");
    title.set_wrap(true);
    body_box.append(&title);
    let body = Label::builder()
        .label(EXPECTATION)
        .wrap(true)
        .justify(gtk4::Justification::Center)
        .build();
    body_box.append(&body);

    let bar = ProgressBar::new();
    bar.set_margin_top(12);
    body_box.append(&bar);
    let detail = Label::new(Some("Starting…"));
    detail.add_css_class("dim-label");
    body_box.append(&detail);

    let buttons = GtkBox::new(Orientation::Horizontal, 12);
    buttons.set_halign(Align::Center);
    buttons.set_margin_top(12);
    let retry = Button::with_label("Try Again");
    retry.add_css_class("pill");
    retry.set_visible(false);
    let browse = Button::with_label("Browse Backups");
    browse.add_css_class("pill");
    browse.add_css_class("suggested-action");
    browse.set_visible(false);
    buttons.append(&retry);
    buttons.append(&browse);
    body_box.append(&buttons);

    let clamp = adw::Clamp::builder()
        .maximum_size(520)
        .child(&body_box)
        .build();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&clamp));
    let page = adw::NavigationPage::builder()
        .title("Backtrack")
        .tag("first-backup")
        .child(&view)
        .can_pop(false)
        .build();

    let page = Rc::new(Page {
        wizard: Rc::clone(wizard),
        page,
        title,
        body,
        bar,
        detail,
        browse,
        retry,
    });
    let this = Rc::clone(&page);
    page.browse.connect_clicked(move |_| {
        let wizard = Rc::clone(&this.wizard);
        crate::ui::spawn(async move { wizard.open_timeline().await });
    });
    let this = Rc::clone(&page);
    page.retry.connect_clicked(move |_| {
        let wizard = Rc::clone(&this.wizard);
        wizard.nav.pop();
        crate::ui::spawn(async move { start(&wizard).await });
    });
    page
}

impl Page {
    fn progress(&self, phase: &str, read: u64) {
        match phase {
            backtrack_core::dbus::PHASE_ARCHIVING => {
                let (fraction, text) =
                    model::first_backup_progress(read, self.wizard.estimate.get());
                match fraction {
                    Some(fraction) => self.bar.set_fraction(fraction),
                    None => self.bar.pulse(),
                }
                self.detail.set_label(&text);
            }
            backtrack_core::dbus::PHASE_CATALOGUING => {
                self.bar.set_fraction(0.99);
                self.detail.set_label("Making your backup browsable…");
            }
            backtrack_core::dbus::PHASE_PRUNING => {
                self.bar.set_fraction(0.99);
                self.detail.set_label("Finishing up…");
            }
            _ => {}
        }
    }

    fn ended(&self, outcome: &str) {
        self.bar.set_visible(false);
        match outcome {
            "completed" => {
                self.title.set_label("Your files are backed up");
                self.body.set_label(
                    "From now on Backtrack backs up on its own. Whenever you need something \
                     back, it is in your backups.",
                );
                self.detail.set_visible(false);
                self.browse.set_visible(true);
                self.browse.grab_focus();
            }
            _ => {
                self.title.set_label("The first backup did not finish");
                self.body.set_label(
                    "Nothing is lost: your files are where they were. Try again now, or \
                     Backtrack will try again at the next scheduled backup.",
                );
                self.detail.set_label("The logs say what stopped it.");
                self.retry.set_visible(true);
            }
        }
    }
}
