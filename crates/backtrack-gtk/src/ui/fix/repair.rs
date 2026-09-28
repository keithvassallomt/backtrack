// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! health.md's "The backup needs repair.", as the guided flow it asks for:
//! check, then repair with a plain-language warning, then, if nothing can be
//! saved, a fresh start that keeps the damaged backups rather than deleting
//! them.
//!
//! One dialog that moves through the steps, rather than a dialog per step, so
//! the person can see where they are and the way back out is always Close.
//! Closing does not stop anything: a check or a repair carries on, and the
//! banner says how it went.

use std::cell::Cell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, Label, Orientation, Spinner};
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::info;

use super::{finish, Context};
use crate::model::health::{self, Place};

/// Where the flow is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Intro,
    Checking,
    Healthy,
    Offer,
    Repairing,
    Rechecking,
    Repaired,
    Unrecoverable { on_server: bool },
}

/// What a step says, and the button that moves on from it.
struct Words {
    title: String,
    body: &'static str,
    busy: bool,
    next: Option<(&'static str, bool)>,
}

fn words(step: Step) -> Words {
    let (title, body, busy, next) = match step {
        Step::Intro => (
            backtrack_core::engine::HealthFailure::RepoCorrupt.copy(""),
            "Backtrack can check your backups to find out what is damaged. The check reads \
             all of them and can take a long time, and backups wait until it has finished.",
            false,
            Some(("Check Backups", false)),
        ),
        Step::Checking => (
            "Checking your backups…".to_string(),
            "This can take a long time. You can close this window: the check carries on, \
             and Backtrack says how it went.",
            true,
            None,
        ),
        Step::Healthy => (
            "Your backups are in good health".to_string(),
            "The check found nothing wrong. Backups carry on as before.",
            false,
            Some(("Back Up Now", false)),
        ),
        Step::Offer => (
            "Repair your backups?".to_string(),
            "Part of what your backups hold is damaged. Repairing removes whatever cannot be \
             saved, so that everything else can be used again. Files that were only in the \
             damaged part will be missing from the backups that held them. Nothing on this \
             computer is changed.",
            false,
            Some(("Repair", true)),
        ),
        Step::Repairing => (
            "Repairing your backups…".to_string(),
            "This can take a long time. You can close this window: the repair carries on.",
            true,
            None,
        ),
        Step::Rechecking => (
            "Checking the repair…".to_string(),
            "Backtrack is making sure the repair worked.",
            true,
            None,
        ),
        Step::Repaired => (
            "Your backups are repaired".to_string(),
            "Backups carry on as before. Anything that could not be saved is missing from the \
             backups that held it.",
            false,
            Some(("Back Up Now", false)),
        ),
        Step::Unrecoverable { on_server: false } => (
            "These backups cannot be repaired".to_string(),
            "Backtrack can start a new set of backups in the same place. The damaged ones \
             are not deleted: they are renamed and kept beside the new ones, where whatever \
             is still readable can be recovered by hand.",
            false,
            Some(("Start Fresh…", true)),
        ),
        Step::Unrecoverable { on_server: true } => (
            "These backups cannot be repaired".to_string(),
            "Backups on a server cannot be renamed from here. Choose somewhere new for a \
             fresh start, and the damaged backups are left exactly as they are.",
            false,
            Some(("Choose a New Location…", false)),
        ),
    };
    Words {
        title,
        body,
        busy,
        next,
    }
}

struct Flow {
    context: Context,
    dialog: adw::Dialog,
    title: Label,
    body: Label,
    spinner: Spinner,
    close: Button,
    next: Button,
    step: Cell<Step>,
}

pub fn present(context: &Context) {
    let content = GtkBox::new(Orientation::Vertical, 0);
    let top = GtkBox::new(Orientation::Vertical, 12);
    top.set_margin_top(32);
    top.set_margin_bottom(24);
    top.set_margin_start(32);
    top.set_margin_end(32);
    let spinner = Spinner::builder()
        .width_request(32)
        .height_request(32)
        .build();
    top.append(&spinner);
    let title = Label::builder()
        .wrap(true)
        .justify(gtk4::Justification::Center)
        .build();
    title.add_css_class("title-2");
    top.append(&title);
    let body = Label::builder()
        .wrap(true)
        .justify(gtk4::Justification::Center)
        .build();
    top.append(&body);
    content.append(&top);
    content.append(&gtk4::Separator::new(Orientation::Horizontal));

    let buttons = GtkBox::new(Orientation::Horizontal, 12);
    buttons.set_homogeneous(true);
    buttons.set_margin_top(18);
    buttons.set_margin_bottom(18);
    buttons.set_margin_start(24);
    buttons.set_margin_end(24);
    let close = Button::with_label("Cancel");
    let next = Button::new();
    buttons.append(&close);
    buttons.append(&next);
    content.append(&buttons);

    let dialog = adw::Dialog::builder()
        .content_width(480)
        .child(&content)
        .build();
    let flow = Rc::new(Flow {
        context: context.clone(),
        dialog,
        title,
        body,
        spinner,
        close,
        next,
        step: Cell::new(Step::Intro),
    });
    let closer = flow.dialog.clone();
    flow.close.connect_clicked(move |_| {
        closer.close();
    });
    let mover = Rc::clone(&flow);
    flow.next
        .connect_clicked(move |_| Rc::clone(&mover).advance());
    flow.show(Step::Intro);
    flow.dialog.present(Some(&context.window));
}

impl Flow {
    fn show(&self, step: Step) {
        self.step.set(step);
        let words = words(step);
        self.title.set_label(&words.title);
        self.body.set_label(words.body);
        self.spinner.set_visible(words.busy);
        self.spinner.set_spinning(words.busy);
        self.close.set_label(
            if step == Step::Intro || matches!(step, Step::Offer | Step::Unrecoverable { .. }) {
                "Cancel"
            } else {
                "Close"
            },
        );
        match words.next {
            Some((label, destructive)) => {
                self.next.set_label(label);
                self.next.set_visible(true);
                self.next.set_sensitive(true);
                self.next.remove_css_class("suggested-action");
                self.next.remove_css_class("destructive-action");
                self.next.add_css_class(if destructive {
                    "destructive-action"
                } else {
                    "suggested-action"
                });
            }
            None => self.next.set_visible(false),
        }
    }

    /// The primary button: whatever the step says comes next.
    fn advance(self: Rc<Self>) {
        self.next.set_sensitive(false);
        match self.step.get() {
            Step::Intro => crate::ui::spawn(self.check(Step::Checking)),
            Step::Offer => crate::ui::spawn(self.repair()),
            Step::Healthy | Step::Repaired => {
                self.dialog.close();
                self.context.back_up();
            }
            Step::Unrecoverable { on_server } => crate::ui::spawn(self.start_fresh(on_server)),
            Step::Checking | Step::Repairing | Step::Rechecking => {}
        }
    }

    /// Run a check, showing `step` while it does.
    async fn check(self: Rc<Self>, step: Step) {
        self.show(step);
        let daemon = self.context.daemon.clone();
        match finish(&daemon, daemon.verify()).await {
            Ok(true) => self.show(if step == Step::Checking {
                Step::Healthy
            } else {
                Step::Repaired
            }),
            Ok(false) if step == Step::Checking => self.show(Step::Offer),
            Ok(false) => self.unrecoverable().await,
            Err(error) => self.failed_to_start(&error),
        }
    }

    async fn repair(self: Rc<Self>) {
        self.show(Step::Repairing);
        let daemon = self.context.daemon.clone();
        info!("repairing the backups");
        match finish(&daemon, daemon.repair()).await {
            Ok(true) => Rc::clone(&self).check(Step::Rechecking).await,
            Ok(false) => self.unrecoverable().await,
            Err(error) => self.failed_to_start(&error),
        }
    }

    async fn unrecoverable(&self) {
        let on_server = self
            .context
            .config()
            .await
            .and_then(|config| config.storage.repository)
            .is_some_and(|repository| health::place(&repository) == Place::Server);
        self.show(Step::Unrecoverable { on_server });
    }

    /// Put the damaged backups aside, and set up new ones where they were;
    /// or, on a server, somewhere new.
    async fn start_fresh(self: Rc<Self>, on_server: bool) {
        if !on_server {
            match self.context.daemon.start_fresh().await {
                Ok(aside) => {
                    info!(aside, "the damaged backups were put aside");
                    self.context
                        .toast(&format!("The damaged backups are kept in {aside}"));
                }
                Err(error) => {
                    self.failed_to_start(&error);
                    return;
                }
            }
        }
        self.dialog.close();
        self.context.wizard("where").await;
    }

    /// A step could not begin: say why and let it be tried again.
    fn failed_to_start(&self, error: &zbus::Error) {
        self.context.toast(&crate::ui::wizard::explain(error));
        let back = match self.step.get() {
            Step::Checking => Step::Intro,
            Step::Repairing => Step::Offer,
            other => other,
        };
        self.show(back);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flow_opens_in_the_catalogue_s_words() {
        assert_eq!(words(Step::Intro).title, "The backup needs repair.");
    }

    #[test]
    fn repairing_is_the_one_step_marked_as_dangerous_before_the_fresh_start() {
        assert_eq!(words(Step::Offer).next, Some(("Repair", true)));
        assert_eq!(words(Step::Intro).next, Some(("Check Backups", false)));
        assert_eq!(
            words(Step::Unrecoverable { on_server: false }).next,
            Some(("Start Fresh…", true))
        );
    }

    #[test]
    fn the_warning_says_what_repair_can_lose_and_what_it_cannot_touch() {
        let offer = words(Step::Offer).body;
        assert!(
            offer.contains("will be missing from the backups"),
            "{offer}"
        );
        assert!(
            offer.contains("Nothing on this computer is changed"),
            "{offer}"
        );
    }

    #[test]
    fn a_fresh_start_says_the_old_backups_are_kept() {
        assert!(words(Step::Unrecoverable { on_server: false })
            .body
            .contains("are not deleted"));
        assert!(words(Step::Unrecoverable { on_server: true })
            .body
            .contains("left exactly as they are"));
    }

    #[test]
    fn a_step_that_is_working_has_nothing_to_press_but_close() {
        for step in [Step::Checking, Step::Repairing, Step::Rechecking] {
            let words = words(step);
            assert!(words.busy);
            assert_eq!(words.next, None);
        }
    }
}
