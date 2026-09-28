// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Mockup 24: the passphrase, again.
//!
//! For the catalogue's two passphrase rows: the keyring no longer has it, or
//! what it has no longer opens the backups. The words are the mockup's where
//! the mockup has words. The wrong-passphrase case opens differently, because
//! the mockup's opening says the keyring has lost the passphrase, and in that
//! case it has not.
//!
//! A dialog of its own rather than an alert, because a passphrase that does
//! not open the backups has to be answered here, with the dialog still open,
//! not by closing it and raising the same banner again.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, CheckButton, Label, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::{info, warn};

use super::{error_name, Context};

/// Which of the two rows the dialog is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    Missing,
    Wrong,
}

/// The mockup's heading and opening, word for word.
const HEADING: &str = "Enter your backup passphrase";
const MISSING: &str = "Backtrack needs your backup passphrase again — the system keyring no \
                       longer has it. Backups are paused until it is entered.";
const REMEMBER: &str = "Remember it so backups run automatically";
const LOST: &str = "Lost the passphrase? Use your recovery key…";

/// For a saved passphrase that no longer opens the backups.
const WRONG: &str = "The passphrase saved on this computer no longer opens your backups. \
                     Backups are paused until the current one is entered.";

/// Once a recovery key has been chosen: what it does, and what it does not.
const WITH_KEY: &str = "Enter the passphrase that was in use when this recovery key was \
                        saved. The key goes back into your backups, and from then on that \
                        passphrase opens them. A recovery key cannot bring back a passphrase \
                        that has been forgotten.";

struct Dialog {
    context: Context,
    dialog: adw::Dialog,
    opening: Label,
    entry: gtk4::PasswordEntry,
    problem: Label,
    remember: CheckButton,
    lost: Button,
    lost_label: Label,
    unlock: Button,
    /// The recovery key chosen, if one has been: the file's name, and the key.
    key: RefCell<Option<(String, String)>>,
}

pub fn present(context: &Context, why: Why) {
    let context = context.clone();
    crate::ui::spawn(async move {
        // What the setting says now, so the box starts as the person left it.
        let remember = context
            .config()
            .await
            .map(|config| config.security.remember_passphrase)
            .unwrap_or(true);
        let dialog = build(context.clone(), why, remember);
        dialog.dialog.present(Some(&context.window));
        dialog.entry.grab_focus();
    });
}

fn build(context: Context, why: Why, remember: bool) -> Rc<Dialog> {
    let content = GtkBox::new(Orientation::Vertical, 0);
    let body = GtkBox::new(Orientation::Vertical, 12);
    body.set_margin_top(32);
    body.set_margin_bottom(24);
    body.set_margin_start(32);
    body.set_margin_end(32);

    let heading = Label::builder()
        .label(HEADING)
        .wrap(true)
        .justify(gtk4::Justification::Center)
        .build();
    heading.add_css_class("title-2");
    body.append(&heading);

    let opening = Label::builder()
        .label(match why {
            Why::Missing => MISSING,
            Why::Wrong => WRONG,
        })
        .wrap(true)
        .justify(gtk4::Justification::Center)
        .build();
    body.append(&opening);

    let entry = gtk4::PasswordEntry::builder()
        .show_peek_icon(true)
        .activates_default(true)
        .margin_top(12)
        .build();
    entry.update_property(&[gtk4::accessible::Property::Label("Backup passphrase")]);
    body.append(&entry);

    let problem = Label::builder()
        .wrap(true)
        .xalign(0.0)
        .visible(false)
        .build();
    problem.add_css_class("error");
    body.append(&problem);

    let remember_box = CheckButton::builder()
        .label(REMEMBER)
        .active(remember)
        .build();
    body.append(&remember_box);

    let lost_label = Label::builder().label(LOST).wrap(true).xalign(0.0).build();
    let lost_content = GtkBox::new(Orientation::Horizontal, 8);
    lost_content.append(&gtk4::Image::from_icon_name("dialog-password-symbolic"));
    lost_content.append(&lost_label);
    let lost = Button::builder()
        .child(&lost_content)
        .halign(Align::Start)
        .build();
    lost.add_css_class("flat");
    lost.add_css_class("accent");
    body.append(&lost);
    content.append(&body);

    content.append(&gtk4::Separator::new(Orientation::Horizontal));
    let buttons = GtkBox::new(Orientation::Horizontal, 12);
    buttons.set_homogeneous(true);
    buttons.set_margin_top(18);
    buttons.set_margin_bottom(18);
    buttons.set_margin_start(24);
    buttons.set_margin_end(24);
    let cancel = Button::with_label("Cancel");
    let unlock = Button::with_label("Unlock Backups");
    unlock.add_css_class("suggested-action");
    unlock.set_sensitive(false);
    buttons.append(&cancel);
    buttons.append(&unlock);
    content.append(&buttons);

    let dialog = adw::Dialog::builder()
        .content_width(480)
        .child(&content)
        .default_widget(&unlock)
        .build();

    let this = Rc::new(Dialog {
        context,
        dialog,
        opening,
        entry,
        problem,
        remember: remember_box,
        lost,
        lost_label,
        unlock,
        key: RefCell::new(None),
    });

    let shown = Rc::clone(&this);
    this.entry.connect_changed(move |entry| {
        shown.unlock.set_sensitive(!entry.text().is_empty());
    });
    let closer = this.dialog.clone();
    cancel.connect_clicked(move |_| {
        closer.close();
    });
    let unlocker = Rc::clone(&this);
    this.unlock
        .connect_clicked(move |_| Rc::clone(&unlocker).unlock());
    let chooser = Rc::clone(&this);
    this.lost
        .connect_clicked(move |_| Rc::clone(&chooser).choose_key());
    this
}

impl Dialog {
    /// Hand the passphrase to the daemon, which tries it before keeping it.
    fn unlock(self: Rc<Self>) {
        let passphrase = self.entry.text().to_string();
        let key = self
            .key
            .borrow()
            .as_ref()
            .map(|(_, key)| key.clone())
            .unwrap_or_default();
        self.unlock.set_sensitive(false);
        self.problem.set_visible(false);
        crate::ui::spawn(async move {
            let answer = self
                .context
                .daemon
                .unlock_backups(&passphrase, self.remember.is_active(), &key)
                .await;
            match answer {
                Ok(job) => {
                    info!(job, "backups unlocked");
                    self.dialog.close();
                    self.context.toast(if job > 0 {
                        "Backups unlocked. Backing up now…"
                    } else {
                        "Backups unlocked"
                    });
                }
                Err(error) => {
                    warn!(%error, "the passphrase was not accepted");
                    self.problem.set_label(&problem(&error));
                    self.problem.set_visible(true);
                    self.entry.set_text("");
                    self.entry.grab_focus();
                }
            }
        });
    }

    /// Pick the saved recovery key, and say what it is for.
    fn choose_key(self: Rc<Self>) {
        crate::ui::spawn(async move {
            let chooser = gtk4::FileDialog::builder()
                .title("Choose Your Recovery Key")
                .accept_label("Use This Key")
                .modal(true)
                .build();
            let Ok(file) = chooser.open_future(Some(&self.context.window)).await else {
                return;
            };
            let loaded = file.load_contents_future().await;
            let text = match loaded {
                Ok((bytes, _)) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(error) => {
                    warn!(%error, "the recovery key could not be read");
                    self.problem
                        .set_label(&format!("That file could not be read: {error}"));
                    self.problem.set_visible(true);
                    return;
                }
            };
            let name = file
                .basename()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| file.uri().to_string());
            info!(file = name, "a recovery key was chosen");
            self.lost_label
                .set_label(&format!("Using the recovery key in {name}"));
            self.opening.set_label(WITH_KEY);
            self.problem.set_visible(false);
            *self.key.borrow_mut() = Some((name, text));
            self.entry.grab_focus();
        });
    }
}

/// What to say when the daemon did not take it, in the dialog's own words.
fn problem(error: &zbus::Error) -> String {
    refusal(error_name(error))
        .map(str::to_string)
        .unwrap_or_else(|| crate::ui::wizard::explain(error))
}

/// The refusals this dialog answers itself, by the daemon's name for them.
fn refusal(name: &str) -> Option<&'static str> {
    Some(match name {
        "PassphraseWrong" => "That passphrase does not open your backups.",
        "KeyForAnotherRepository" => "That recovery key belongs to a different backup.",
        "NotARecoveryKey" => "That file is not a Backtrack recovery key.",
        "RepoUnreachable" => {
            "Backtrack could not reach your backups to check the passphrase. Try again once \
             they are reachable."
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_refusal_is_answered_in_words_the_person_can_act_on() {
        assert_eq!(
            refusal("PassphraseWrong"),
            Some("That passphrase does not open your backups.")
        );
        assert_eq!(
            refusal("KeyForAnotherRepository"),
            Some("That recovery key belongs to a different backup.")
        );
        assert_eq!(
            refusal("NotARecoveryKey"),
            Some("That file is not a Backtrack recovery key.")
        );
        assert!(refusal("RepoUnreachable").is_some());
        assert_eq!(
            refusal("BorgFailed"),
            None,
            "left to the general explanation"
        );
    }

    #[test]
    fn the_mockup_s_words_are_kept() {
        assert_eq!(HEADING, "Enter your backup passphrase");
        assert_eq!(REMEMBER, "Remember it so backups run automatically");
        assert_eq!(LOST, "Lost the passphrase? Use your recovery key…");
        assert!(MISSING.starts_with("Backtrack needs your backup passphrase again"));
        assert!(
            !WRONG.contains("keyring no longer has it"),
            "untrue for this case"
        );
    }
}
