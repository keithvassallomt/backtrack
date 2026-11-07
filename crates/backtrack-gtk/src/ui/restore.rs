// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Restoring, from the window's side.
//!
//! The shape of it comes straight from the two-call interface: work out what
//! would happen, show it, and only then do it. Which dialog appears is decided
//! by what was found rather than by what was clicked — one file with a clash
//! gets the conflict dialog, a folder gets the summary, and a restore with
//! nothing to ask about gets neither and simply happens.
//!
//! Cancelling at any point before the last step costs nothing, because nothing
//! has been touched. Cancelling after it costs nothing either: Undo, in the
//! banner under the window's header bar, puts everything back from the stash.
//! The banner stays until it is used or another restore takes its place; a
//! restore from Rewind opened from outside the window offers the same in a
//! notification, and clicking that opens the window with the banner (S18-T7).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use backtrack_core::dbus::RestorePreview;
use futures::StreamExt;
use gtk4::prelude::*;
use gtk4::{gio, glib, Align, Box as GtkBox, Label, Orientation, Spinner};
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::{info, warn};

use crate::daemon::Daemon1Proxy;
use crate::model::restore as copy;
use crate::path;
use crate::state::AppState;
use crate::ui::{conflict, summary};

/// Restoring in place means writing back where the files came from, and an
/// archive stores its members with the leading separator removed — so the
/// destination that makes those line up is the root.
const IN_PLACE: &str = "/";

/// Where a restore asks its questions and says what happened: Backtrack's
/// window, or Rewind over it (S18-T6).
pub struct Asking {
    pub parent: gtk4::Widget,
    pub toasts: adw::ToastOverlay,
}

/// How a restore offers Undo: in the window, or, for a restore from Rewind
/// opened from outside the window, in a notification (S18-T7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UndoBy {
    /// The banner under the window's header bar.
    Window,
    /// A notification, whose Undo button undoes it and which, clicked, opens
    /// the window with the banner.
    Notification,
}

/// The notification a restore from outside the window offers Undo in, and
/// then says it was put back in.
const UNDO_NOTIFICATION: &str = "restore-undo";

/// What the notification's Undo button passes to `app.undo-restore`: the
/// restore's job and what it restored.
pub const UNDO_TARGET: &str = "(ts)";

/// What clicking the notification passes to `app.show-restore`: the same,
/// what the notification says, and the path restored.
pub const SHOW_TARGET: &str = "(tsss)";

fn undo_target(job: u64, name: &str) -> glib::Variant {
    (job, name.to_string()).to_variant()
}

fn show_target(job: u64, name: &str, said: &str, path: &str) -> glib::Variant {
    (job, name.to_string(), said.to_string(), path.to_string()).to_variant()
}

/// How long Backtrack keeps running for that notification's Undo, after
/// which the notification is withdrawn. Not every desktop can start
/// Backtrack again to answer a notification's button.
const UNDO_MINUTES: u32 = 10;

/// A restore carried out, which Undo can put back.
struct Done {
    job: u64,
    preview: RestorePreview,
    name: String,
    /// What was restored, archive-relative: where the notification opens the
    /// window.
    target: String,
}

impl Done {
    /// What the restore did, in a few words.
    fn said(&self) -> String {
        let p = &self.preview;
        copy::restored_toast(
            (p.conflicts + p.only_in_backup + p.type_changed) as usize,
            &self.name,
        )
    }
}

/// Drives a restore from the window.
pub struct Restores {
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    /// Under the header bar: what the last restore did, and Undo.
    banner: adw::Banner,
    state: Rc<AppState>,
    daemon: Rc<RefCell<Option<Daemon1Proxy<'static>>>>,
    /// The prepared restore that was last carried out, and what it was called,
    /// which is what the banner's Undo reaches for.
    undoable: RefCell<Option<(u64, String)>>,
    /// One at a time. Two restores of the same folder at once is not a
    /// situation worth having opinions about.
    busy: Cell<bool>,
}

pub fn build(
    window: &adw::ApplicationWindow,
    toasts: &adw::ToastOverlay,
    banner: &adw::Banner,
    state: &Rc<AppState>,
    daemon: &Rc<RefCell<Option<Daemon1Proxy<'static>>>>,
) -> Rc<Restores> {
    let restores = Rc::new(Restores {
        window: window.clone(),
        toasts: toasts.clone(),
        banner: banner.clone(),
        state: Rc::clone(state),
        daemon: Rc::clone(daemon),
        undoable: RefCell::new(None),
        busy: Cell::new(false),
    });
    banner.set_button_label(Some("Undo"));
    let weak = Rc::downgrade(&restores);
    banner.connect_button_clicked(move |_| {
        if let Some(restores) = weak.upgrade() {
            restores.undo();
        }
    });
    restores
}

impl Restores {
    /// Restore whatever is selected, back where it came from.
    pub fn start(self: &Rc<Self>) {
        if self.busy.get() {
            return;
        }
        let this = Rc::clone(self);
        crate::ui::spawn(async move {
            this.busy.set(true);
            let asking = Asking {
                parent: this.window.clone().upcast(),
                toasts: this.toasts.clone(),
            };
            let state = Rc::clone(&this.state);
            if let Some(done) = this.run(&state, &asking).await {
                this.carried_out(done, UndoBy::Window);
            }
            this.busy.set(false);
        });
    }

    /// Restore what `state` (Rewind's) has selected, or its folder, from the
    /// backup it is at, asking over `asking`. `finished` is told whether it
    /// was carried out, before Undo is offered as `undo` says.
    pub fn start_from(
        self: &Rc<Self>,
        state: Rc<AppState>,
        asking: Asking,
        undo: UndoBy,
        finished: impl FnOnce(bool) + 'static,
    ) {
        if self.busy.get() {
            finished(false);
            return;
        }
        let this = Rc::clone(self);
        crate::ui::spawn(async move {
            this.busy.set(true);
            match this.run(&state, &asking).await {
                Some(done) => {
                    finished(true);
                    this.carried_out(done, undo);
                }
                None => finished(false),
            }
            this.busy.set(false);
        });
    }

    /// Offer to put it back, and show the folder as it is now.
    fn carried_out(self: &Rc<Self>, done: Done, undo: UndoBy) {
        match undo {
            UndoBy::Window => self.offer(done.job, &done.name, &done.said()),
            UndoBy::Notification => {
                *self.undoable.borrow_mut() = Some((done.job, done.name.clone()));
                self.notify_undo(&done);
            }
        }
        // The pane is showing a folder whose contents just changed.
        self.state.disk_changed();
    }

    /// Restore what is selected into a folder the user picks.
    ///
    /// The other way out of a restore, and the one that costs nothing: the
    /// files arrive in a directory made for them, so nothing already on the
    /// machine is touched and there is nothing to decide. No conflict dialog,
    /// no summary, no Undo needed. What comes from the Trash is moved out of
    /// it, as the file manager moves what is dragged out of the Trash.
    pub fn start_elsewhere(self: &Rc<Self>) {
        if self.busy.get() {
            return;
        }
        let this = Rc::clone(self);
        crate::ui::spawn(async move {
            this.busy.set(true);
            this.run_elsewhere().await;
            this.busy.set(false);
        });
    }

    async fn run_elsewhere(self: &Rc<Self>) {
        let view = self.state.view();
        let (Some(proxy), Some(taken), Some(restoring), Some(target)) = (
            self.daemon.borrow().clone(),
            view.archive().map(|a| a.ts),
            view.restoring(),
            view.change_target(),
        ) else {
            self.toast("Backtrack cannot restore without its background service");
            return;
        };

        // The portal's own picker under Flatpak, and the toolkit's outside it.
        // Either way the app never sees a path it was not handed.
        let dialog = gtk4::FileDialog::builder()
            .title("Restore into which folder?")
            .accept_label("Restore Here")
            .modal(true)
            .build();
        let Ok(chosen) = dialog.select_folder_future(Some(&self.window)).await else {
            info!("restoring elsewhere was cancelled at the folder picker");
            return;
        };
        let Some(chosen) = chosen.path() else {
            warn!("the chosen folder has no path this side of the portal");
            self.toast("That folder cannot be written to directly");
            return;
        };

        let name = path::name(&target).to_string();
        let folder = copy::restored_folder_name(&name, taken, &glib::TimeZone::local());
        let dest = chosen.join(&folder).to_string_lossy().to_string();
        info!(
            target,
            archive = restoring.archive,
            from_trash = restoring.trashed.len(),
            dest,
            "restoring into a folder of its own"
        );

        let waiting = preparing(self.window.upcast_ref(), &name);
        let done = run_job(&proxy, |p| {
            p.restore_into_with_trash(
                &restoring.archive,
                &restoring.trashed,
                std::slice::from_ref(&target),
                &dest,
            )
        })
        .await;
        waiting.close();

        match done {
            Ok(_) => self.toast(&copy::restored_into_toast(&folder)),
            Err(error) => {
                warn!(%error, "the restore into a chosen folder did not finish");
                self.toast(&error);
            }
        }
    }

    /// Whether there is anything to restore right now.
    pub fn can_restore(&self) -> bool {
        self.state.view().restoring().is_some() && self.daemon.borrow().is_some()
    }

    async fn run(self: &Rc<Self>, state: &AppState, asking: &Asking) -> Option<Done> {
        let view = state.view();
        let (Some(proxy), Some(taken), Some(restoring), Some(target)) = (
            self.daemon.borrow().clone(),
            view.archive().map(|a| a.ts),
            view.restoring(),
            view.change_target(),
        ) else {
            warn!(
                service = self.daemon.borrow().is_some(),
                backup = view.seq.is_some(),
                target = view.change_target().is_some(),
                "a restore was asked for with something missing"
            );
            say(
                asking,
                "Backtrack cannot restore without its background service",
            );
            return None;
        };
        let name = path::name(&target).to_string();
        info!(
            target,
            archive = restoring.archive,
            from_trash = restoring.trashed.len(),
            "restoring"
        );
        // A restore of the selected file is about that file; with nothing
        // selected it is about the folder being viewed.
        let single_file = view.selected.as_ref().is_some_and(|s| !s.is_dir);

        // ── Work it out ──
        let waiting = preparing(&asking.parent, &name);
        let prepared = run_job(&proxy, |p| {
            p.prepare_restore_with_trash(
                &restoring.archive,
                &restoring.trashed,
                std::slice::from_ref(&target),
                IN_PLACE,
            )
        })
        .await;
        waiting.close();

        let job = match prepared {
            Ok(job) => job,
            Err(error) => {
                warn!(%error, target, "the restore could not be worked out");
                say(asking, &error);
                return None;
            }
        };

        let preview = match proxy.get_restore_preview(job).await {
            Ok(preview) => preview,
            Err(error) => {
                warn!(%error, job, "the prepared restore could not be read");
                say(asking, &clean(&error.to_string()));
                let _ = proxy.discard_restore(job).await;
                return None;
            }
        };

        // ── Ask, if there is anything to ask ──
        let Some((blanket, decisions)) = decide(
            asking,
            &preview,
            &name,
            &target,
            taken,
            &restoring.origin,
            single_file,
        )
        .await
        else {
            info!(job, "restore cancelled before anything was touched");
            let _ = proxy.discard_restore(job).await;
            return None;
        };

        // ── Carry it out ──
        let done = run_job(&proxy, |p| p.execute_restore(job, &blanket, &decisions)).await;
        match done {
            Ok(_) => Some(Done {
                job,
                preview,
                name,
                target,
            }),
            Err(error) => {
                warn!(%error, job, "the restore did not finish");
                say(asking, &error);
                None
            }
        }
    }

    /// The offer that makes Replace safe to click: restore `job`, which
    /// restored `name`, in the banner, saying `said`, until it is used or
    /// another restore takes its place.
    pub fn offer(&self, job: u64, name: &str, said: &str) {
        info!(job, name, "offering to undo a restore");
        *self.undoable.borrow_mut() = Some((job, name.to_string()));
        self.banner.set_title(said);
        self.banner.set_revealed(true);
    }

    /// Restore `job` has been put back from its notification: no longer
    /// offered here.
    pub fn forget(&self, job: u64) {
        let ours = self
            .undoable
            .borrow()
            .as_ref()
            .is_some_and(|(undoable, _)| *undoable == job);
        if ours {
            *self.undoable.borrow_mut() = None;
            self.banner.set_revealed(false);
        }
        self.state.disk_changed();
    }

    /// The same offer in a notification, for a restore from Rewind opened
    /// from outside the window, which has gone back there (S18-T7).
    fn notify_undo(&self, done: &Done) {
        let Some(app) = self.window.application() else {
            return;
        };
        let said = done.said();
        let notification = gio::Notification::new(&said);
        notification.add_button_with_target_value(
            "Undo",
            &format!("app.{}", backtrack_core::dbus::APP_ACTION_UNDO_RESTORE),
            Some(&undo_target(done.job, &done.name)),
        );
        // Clicked, rather than its button: the window, at what was restored,
        // with the same offer in its banner. The click dismisses the
        // notification, Undo and all, so the offer must not be only here.
        notification.set_default_action_and_target_value(
            &format!("app.{}", backtrack_core::dbus::APP_ACTION_SHOW_RESTORE),
            Some(&show_target(done.job, &done.name, &said, &done.target)),
        );
        app.send_notification(Some(UNDO_NOTIFICATION), &notification);
        let hold = app.hold();
        glib::timeout_add_seconds_local_once(UNDO_MINUTES * 60, move || {
            app.withdraw_notification(UNDO_NOTIFICATION);
            drop(hold);
        });
    }

    /// Put back the restore that was just carried out.
    fn undo(self: &Rc<Self>) {
        let undoable = self.undoable.borrow().clone();
        let (Some((job, name)), Some(proxy)) = (undoable, self.daemon.borrow().clone()) else {
            return;
        };
        let this = Rc::clone(self);
        crate::ui::spawn(async move {
            match run_job(&proxy, |p| p.undo_restore(job)).await {
                Ok(_) => {
                    info!(job, "restore undone");
                    *this.undoable.borrow_mut() = None;
                    this.banner.set_revealed(false);
                    // Its notification's Undo would now undo nothing.
                    if let Some(app) = this.window.application() {
                        app.withdraw_notification(UNDO_NOTIFICATION);
                    }
                    this.toast(&copy::put_back_toast(&name));
                    this.state.disk_changed();
                }
                Err(error) => {
                    warn!(%error, job, "the restore could not be undone");
                    this.toast(&error);
                }
            }
        });
    }

    fn toast(&self, message: &str) {
        self.toasts.add_toast(crate::ui::toast(message));
    }
}

/// Which dialog to put up, and what it answered.
///
/// `None` means the user backed out. An empty decision list means "use the
/// blanket answer", which is every conflict but never a change of type.
async fn decide(
    asking: &Asking,
    preview: &RestorePreview,
    name: &str,
    target: &str,
    taken: i64,
    origin: &copy::Origin,
    single_file: bool,
) -> Option<(String, Vec<(String, String)>)> {
    // What was asked for is not in this backup. Said plainly and first,
    // because the alternative reading of an empty plan — "everything is
    // already up to date" — is the opposite of the truth, and is a backup
    // tool reassuring somebody about a file it could not find.
    if !preview.missing.is_empty() {
        warn!(
            missing = preview.missing.len(),
            archive = preview.archive,
            "the backup does not contain what was asked for"
        );
        say(asking, &copy::not_in_this_backup(name));
        return None;
    }

    // Nothing to ask: no clashes, so the restore is uncontroversial and
    // asking would be noise.
    if preview.conflicts == 0 && preview.type_changed == 0 {
        if preview.only_in_backup == 0 {
            say(asking, "Everything is already up to date");
            return None;
        }
        return Some(("replace".to_string(), Vec::new()));
    }

    if single_file && preview.conflicts == 1 && preview.type_changed == 0 {
        let entry = preview.entries.first()?;
        let folder = path::parent(target)
            .map(|p| path::name(&p).to_string())
            .unwrap_or_default();
        return match conflict::ask(&asking.parent, entry, name, &folder, origin).await {
            conflict::Answer::Cancel => None,
            conflict::Answer::KeepBoth => Some(("keep-both".to_string(), Vec::new())),
            conflict::Answer::Replace => Some(("replace".to_string(), Vec::new())),
        };
    }

    match summary::ask(&asking.parent, preview, name, target, taken, origin).await {
        summary::Answer::Cancel => None,
        summary::Answer::KeepBoth => Some(("keep-both".to_string(), Vec::new())),
        // A review list answers every path by name, so the blanket has
        // nothing left to cover.
        summary::Answer::Replace { decisions } if !decisions.is_empty() => {
            Some(("skip".to_string(), decisions))
        }
        summary::Answer::Replace { .. } => Some(("replace".to_string(), Vec::new())),
    }
}

/// A dialog while the copy is fetched, which for a folder is not instant.
fn preparing(parent: &gtk4::Widget, name: &str) -> adw::AlertDialog {
    let dialog = adw::AlertDialog::new(
        Some(&format!("Working out how to restore “{name}”")),
        Some("Backtrack is fetching this version and comparing it with what is on your computer. Nothing has been changed yet."),
    );
    let spinner = Spinner::builder().spinning(true).build();
    spinner.set_size_request(32, 32);
    let content = GtkBox::new(Orientation::Vertical, 12);
    content.set_halign(Align::Center);
    content.append(&spinner);
    content.append(&Label::new(None));
    dialog.set_extra_child(Some(&content));
    dialog.present(Some(parent));
    dialog
}

/// Put back restore `job`, which restored `name`, from its notification's
/// Undo (S18-T7), and say how it went in the same notification. No window
/// is needed: Rewind's may have closed, with the window made for it.
pub fn undo_from_notification(app: &adw::Application, job: u64, name: String) {
    let hold = app.hold();
    let app = app.clone();
    crate::ui::spawn(async move {
        let done = match crate::daemon::connect().await {
            Ok(proxy) => run_job(&proxy, |p| p.undo_restore(job)).await,
            Err(error) => Err(error),
        };
        let said = match done {
            Ok(_) => {
                info!(job, "restore undone from its notification");
                crate::window::undone(job);
                copy::put_back_toast(&name)
            }
            Err(error) => {
                warn!(%error, job, "the restore could not be undone");
                error
            }
        };
        app.send_notification(Some(UNDO_NOTIFICATION), &gio::Notification::new(&said));
        drop(hold);
    });
}

/// Say `message` where the restore is asking.
fn say(asking: &Asking, message: &str) {
    asking.toasts.add_toast(crate::ui::toast(message));
}

/// Start something that returns a job id, and wait for that job to end.
///
/// The stream is opened before the call, not after: a small restore can finish
/// before a stream opened afterwards would exist, and the completion would be
/// missed by exactly the caller that cared about it.
async fn run_job<'a, F, Fut>(proxy: &'a Daemon1Proxy<'static>, start: F) -> Result<u64, String>
where
    F: FnOnce(&'a Daemon1Proxy<'static>) -> Fut,
    Fut: std::future::Future<Output = zbus::Result<u64>> + 'a,
{
    let mut finished = proxy
        .receive_job_finished()
        .await
        .map_err(|e| e.to_string())?;
    let job = start(proxy).await.map_err(|e| clean(&e.to_string()))?;

    while let Some(signal) = StreamExt::next(&mut finished).await {
        let Ok(args) = signal.args() else { continue };
        if args.job != job {
            continue;
        }
        return match args.outcome {
            "completed" => Ok(job),
            "cancelled" => Err("Cancelled — nothing was changed".to_string()),
            _ => Err(
                "The restore did not finish. Nothing was left half-done; see the logs for why."
                    .to_string(),
            ),
        };
    }
    Err("The Backtrack service stopped answering".to_string())
}

/// Strip the D-Bus error-name prefix, which is addressed to programs.
fn clean(message: &str) -> String {
    message
        .rsplit_once(": ")
        .map_or(message, |(_, tail)| tail)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notification_passes_what_its_actions_take() {
        // A target of another type than the action's is dropped without a
        // word, and the button or the click does nothing.
        assert_eq!(undo_target(7, "a.txt").type_().as_str(), UNDO_TARGET);
        assert_eq!(
            show_target(7, "a.txt", "Restored a.txt", "home/k/a.txt")
                .type_()
                .as_str(),
            SHOW_TARGET
        );
        let shown = show_target(7, "a.txt", "Restored a.txt", "home/k/a.txt");
        assert_eq!(
            shown.get::<(u64, String, String, String)>(),
            Some((
                7,
                "a.txt".to_string(),
                "Restored a.txt".to_string(),
                "home/k/a.txt".to_string()
            ))
        );
    }
}
