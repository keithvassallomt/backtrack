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
//! has been touched. Cancelling after it costs nothing either: the toast's
//! Undo puts everything back from the stash.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use backtrack_core::dbus::RestorePreview;
use futures::StreamExt;
use gtk4::prelude::*;
use gtk4::{glib, Align, Box as GtkBox, Label, Orientation, Spinner};
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

/// Drives a restore from the window.
pub struct Restores {
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    state: Rc<AppState>,
    daemon: Rc<RefCell<Option<Daemon1Proxy<'static>>>>,
    /// The prepared restore that was last carried out, which is what the
    /// toast's Undo reaches for. Kept until the next one, so the offer
    /// outlives the toast that made it.
    undoable: Cell<Option<u64>>,
    /// One at a time. Two restores of the same folder at once is not a
    /// situation worth having opinions about.
    busy: Cell<bool>,
}

pub fn build(
    window: &adw::ApplicationWindow,
    toasts: &adw::ToastOverlay,
    state: &Rc<AppState>,
    daemon: &Rc<RefCell<Option<Daemon1Proxy<'static>>>>,
) -> Rc<Restores> {
    Rc::new(Restores {
        window: window.clone(),
        toasts: toasts.clone(),
        state: Rc::clone(state),
        daemon: Rc::clone(daemon),
        undoable: Cell::new(None),
        busy: Cell::new(false),
    })
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
            this.run().await;
            this.busy.set(false);
        });
    }

    /// Restore what is selected into a folder the user picks.
    ///
    /// The other way out of a restore, and the one that costs nothing: the
    /// files arrive in a directory made for them, so nothing already on the
    /// machine is touched and there is nothing to decide. No conflict dialog,
    /// no summary, no Undo needed.
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
        let (Some(proxy), Some((archive, taken)), Some(target)) = (
            self.daemon.borrow().clone(),
            view.archive().map(|a| (a.name.clone(), a.ts)),
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
        info!(target, archive, dest, "restoring into a folder of its own");

        let waiting = self.preparing(&name);
        let done = run_job(&proxy, |p| {
            p.restore_into(&archive, std::slice::from_ref(&target), &dest)
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
        let view = self.state.view();
        view.seq.is_some() && view.change_target().is_some() && self.daemon.borrow().is_some()
    }

    async fn run(self: &Rc<Self>) {
        let view = self.state.view();
        let (Some(proxy), Some(archive), Some(target)) = (
            self.daemon.borrow().clone(),
            view.archive().map(|a| (a.name.clone(), a.ts)),
            view.change_target(),
        ) else {
            warn!(
                service = self.daemon.borrow().is_some(),
                backup = view.seq.is_some(),
                target = view.change_target().is_some(),
                "a restore was asked for with something missing"
            );
            self.toast("Backtrack cannot restore without its background service");
            return;
        };
        let (archive, taken) = archive;
        let name = path::name(&target).to_string();
        info!(target, archive, "restoring");
        // A restore of the selected file is about that file; with nothing
        // selected it is about the folder being viewed.
        let single_file = view.selected.as_ref().is_some_and(|s| !s.is_dir);

        // ── Work it out ──
        let waiting = self.preparing(&name);
        let prepared = run_job(&proxy, |p| {
            p.prepare_restore(&archive, std::slice::from_ref(&target), IN_PLACE)
        })
        .await;
        waiting.close();

        let job = match prepared {
            Ok(job) => job,
            Err(error) => {
                warn!(%error, target, "the restore could not be worked out");
                self.toast(&error);
                return;
            }
        };

        let preview = match proxy.get_restore_preview(job).await {
            Ok(preview) => preview,
            Err(error) => {
                warn!(%error, job, "the prepared restore could not be read");
                self.toast(&clean(&error.to_string()));
                let _ = proxy.discard_restore(job).await;
                return;
            }
        };

        // ── Ask, if there is anything to ask ──
        let Some((blanket, decisions)) = self
            .decide(&preview, &name, &target, taken, single_file)
            .await
        else {
            info!(job, "restore cancelled before anything was touched");
            let _ = proxy.discard_restore(job).await;
            return;
        };

        // ── Carry it out ──
        let done = run_job(&proxy, |p| p.execute_restore(job, &blanket, &decisions)).await;
        match done {
            Ok(_) => {
                self.undoable.set(Some(job));
                self.offer_undo(&preview, &name);
                // The pane is showing a folder whose contents just changed.
                self.state.set_folder(self.state.view().folder.clone());
            }
            Err(error) => {
                warn!(%error, job, "the restore did not finish");
                self.toast(&error);
            }
        }
    }

    /// Which dialog to put up, and what it answered.
    ///
    /// `None` means the user backed out. An empty decision list means "use the
    /// blanket answer", which is every conflict but never a change of type.
    async fn decide(
        self: &Rc<Self>,
        preview: &RestorePreview,
        name: &str,
        target: &str,
        taken: i64,
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
            self.toast(&copy::not_in_this_backup(name));
            return None;
        }

        // Nothing to ask: no clashes, so the restore is uncontroversial and
        // asking would be noise.
        if preview.conflicts == 0 && preview.type_changed == 0 {
            if preview.only_in_backup == 0 {
                self.toast("Everything is already up to date");
                return None;
            }
            return Some(("replace".to_string(), Vec::new()));
        }

        if single_file && preview.conflicts == 1 && preview.type_changed == 0 {
            let entry = preview.entries.first()?;
            let folder = path::parent(target)
                .map(|p| path::name(&p).to_string())
                .unwrap_or_default();
            return match conflict::ask(&self.window, entry, name, &folder).await {
                conflict::Answer::Cancel => None,
                conflict::Answer::KeepBoth => Some(("keep-both".to_string(), Vec::new())),
                conflict::Answer::Replace => Some(("replace".to_string(), Vec::new())),
            };
        }

        match summary::ask(&self.window, preview, name, target, taken).await {
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

    /// The toast, with the offer that makes Replace safe to click.
    fn offer_undo(self: &Rc<Self>, preview: &RestorePreview, name: &str) {
        let moved = (preview.conflicts + preview.only_in_backup + preview.type_changed) as usize;
        let toast = adw::Toast::new(&copy::restored_toast(moved, name));
        toast.set_button_label(Some("Undo"));
        // Long enough to read and reach; the stash keeps the files far longer,
        // so a missed toast is an inconvenience rather than a loss.
        toast.set_timeout(10);
        let this = Rc::clone(self);
        toast.connect_button_clicked(move |_| this.undo());
        self.toasts.add_toast(toast);
    }

    /// Put back the restore that was just carried out.
    fn undo(self: &Rc<Self>) {
        let (Some(job), Some(proxy)) = (self.undoable.get(), self.daemon.borrow().clone()) else {
            return;
        };
        let this = Rc::clone(self);
        crate::ui::spawn(async move {
            match run_job(&proxy, |p| p.undo_restore(job)).await {
                Ok(_) => {
                    info!(job, "restore undone");
                    this.undoable.set(None);
                    this.toast("Put back");
                    this.state.set_folder(this.state.view().folder.clone());
                }
                Err(error) => {
                    warn!(%error, job, "the restore could not be undone");
                    this.toast(&error);
                }
            }
        });
    }

    /// A dialog while the copy is fetched, which for a folder is not instant.
    fn preparing(&self, name: &str) -> adw::AlertDialog {
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
        dialog.present(Some(&self.window));
        dialog
    }

    fn toast(&self, message: &str) {
        self.toasts.add_toast(adw::Toast::new(message));
    }
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
