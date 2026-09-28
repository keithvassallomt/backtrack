// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The two rows about room: the destination is full, or this computer is.

use backtrack_core::dbus::LocalStorage;
use backtrack_core::engine::HealthFailure;
use gtk4::prelude::*;
use gtk4::{glib, Align, Button};
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::warn;

use super::{finish, Context};
use crate::model::prefs as settings;

/// "The backup drive is full.": the three ways out health.md lists.
///
/// Free Up Space applies the retention policy and then reclaims what that and
/// any deleted backups were holding. Pruning comes first on purpose: it
/// normally runs after each backup, and a backup that cannot start because
/// the drive is full never gets that far.
pub fn destination(context: &Context) {
    let dialog = adw::AlertDialog::builder()
        .heading(HealthFailure::DestinationFull.copy(""))
        .body(
            "There is no room left there for new backups. Backtrack can free the space old \
             backups no longer need, you can keep fewer old backups, or backups can go \
             somewhere with more room.",
        )
        .default_response("free")
        .close_response("close")
        .build();
    dialog.add_responses(&[
        ("close", "Close"),
        ("keep", "Keep Fewer Backups…"),
        ("move", "Change Destination…"),
        ("free", "Free Up Space"),
    ]);
    dialog.set_response_appearance("free", adw::ResponseAppearance::Suggested);

    let responder = context.clone();
    dialog.connect_response(None, move |_, response| {
        let context = responder.clone();
        match response {
            "free" => crate::ui::spawn(free_up_space(context)),
            "keep" => crate::ui::prefs::present(
                &context.app,
                &context.window,
                context.daemon.clone(),
                Some("storage"),
            ),
            "move" => crate::ui::spawn(async move { context.wizard("where").await }),
            _ => {}
        }
    });
    dialog.present(Some(&context.window));
}

async fn free_up_space(context: Context) {
    let daemon = context.daemon.clone();
    context.toast("Freeing up space…");
    let freed = match finish(&daemon, daemon.prune()).await {
        Ok(true) => finish(&daemon, daemon.compact()).await,
        other => other,
    };
    match freed {
        Ok(true) => {
            context.toast("Space freed. Backing up now…");
            context.back_up();
        }
        Ok(false) => context.toast("Space could not be freed. The logs say why."),
        Err(error) => context.toast(&crate::ui::wizard::explain(&error)),
    }
}

/// "Not enough space on this computer to keep protecting changes.": where the
/// room Backtrack uses on this computer has gone, and what can be given up.
pub fn local(context: &Context) {
    let context = context.clone();
    crate::ui::spawn(async move {
        let limit = context
            .config()
            .await
            .map(|config| config.storage.offline.space_limit_gb)
            .unwrap_or(10);
        let overview = Overview::build(context.clone(), limit);
        overview.refresh();
        overview.dialog.present(Some(&context.window));
    });
}

struct Overview {
    context: Context,
    dialog: adw::AlertDialog,
    free: adw::ActionRow,
    snapshots: adw::ComboRow,
    stash: adw::ActionRow,
    cache: adw::ActionRow,
}

impl Overview {
    fn build(context: Context, limit: u32) -> Overview {
        let group = adw::PreferencesGroup::new();

        let free = adw::ActionRow::builder()
            .title("Free on this computer")
            .build();
        group.add(&free);

        let snapshots = adw::ComboRow::builder()
            .title("Space for local snapshots")
            .build();
        let limits = settings::space_limits(limit);
        let labels: Vec<String> = limits
            .iter()
            .map(|gb| settings::space_limit_label(*gb))
            .collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        snapshots.set_model(Some(&gtk4::StringList::new(&labels)));
        snapshots.set_selected(limits.iter().position(|gb| *gb == limit).unwrap_or(0) as u32);
        group.add(&snapshots);

        let stash = adw::ActionRow::builder()
            .title("Files restores replaced")
            .build();
        let empty = Button::with_label("Empty…");
        empty.set_valign(Align::Center);
        stash.add_suffix(&empty);
        group.add(&stash);

        let cache = adw::ActionRow::builder()
            .title("Previews and comparisons")
            .build();
        let clear = Button::with_label("Clear");
        clear.set_valign(Align::Center);
        cache.add_suffix(&clear);
        group.add(&cache);

        let dialog = adw::AlertDialog::builder()
            .heading(HealthFailure::LocalDiskFull.copy(""))
            .body(
                "Backtrack keeps a few things of its own on this computer. This is what they \
                 use, and what can be given up to make room. Backups try again by themselves.",
            )
            .extra_child(&group)
            .default_response("done")
            .close_response("done")
            .build();
        dialog.add_response("done", "Done");

        let overview = Overview {
            context,
            dialog,
            free,
            snapshots,
            stash,
            cache,
        };
        overview.wire(limits, empty, clear);
        overview
    }

    fn wire(&self, limits: Vec<u32>, empty: Button, clear: Button) {
        let context = self.context.clone();
        let dialog = self.dialog.clone();
        let rows = self.rows();
        self.snapshots.connect_selected_notify(move |row| {
            let Some(gb) = limits.get(row.selected() as usize).copied() else {
                return;
            };
            let context = context.clone();
            let rows = rows.clone();
            crate::ui::spawn(async move {
                if let Err(error) = context
                    .daemon
                    .set_config("storage.offline.space_limit_gb", &gb.to_string())
                    .await
                {
                    context.toast(&crate::ui::wizard::explain(&error));
                }
                rows.refresh(&context).await;
            });
        });

        let context = self.context.clone();
        let rows = self.rows();
        empty.connect_clicked(move |_| {
            let context = context.clone();
            let dialog = dialog.clone();
            let rows = rows.clone();
            crate::ui::spawn(async move {
                let confirm = adw::AlertDialog::builder()
                    .heading("Empty the Safety Stash?")
                    .body(
                        "The files that restores replaced are deleted, apart from the last \
                         hour's. Those restores can no longer be undone.",
                    )
                    .default_response("cancel")
                    .close_response("cancel")
                    .build();
                confirm.add_responses(&[("cancel", "Cancel"), ("empty", "Empty")]);
                confirm.set_response_appearance("empty", adw::ResponseAppearance::Destructive);
                crate::ui::prefer_wide_responses(&confirm);
                if confirm.choose_future(Some(&dialog)).await != "empty" {
                    return;
                }
                match context.daemon.empty_stash().await {
                    Ok(bytes) => context.toast(&freed(bytes)),
                    Err(error) => context.toast(&crate::ui::wizard::explain(&error)),
                }
                rows.refresh(&context).await;
            });
        });

        let context = self.context.clone();
        let rows = self.rows();
        clear.connect_clicked(move |_| {
            let context = context.clone();
            let rows = rows.clone();
            crate::ui::spawn(async move {
                match context.daemon.clear_preview_cache().await {
                    Ok(bytes) => context.toast(&freed(bytes)),
                    Err(error) => context.toast(&crate::ui::wizard::explain(&error)),
                }
                rows.refresh(&context).await;
            });
        });
    }

    fn rows(&self) -> Rows {
        Rows {
            free: self.free.clone(),
            snapshots: self.snapshots.clone(),
            stash: self.stash.clone(),
            cache: self.cache.clone(),
        }
    }

    fn refresh(&self) {
        let context = self.context.clone();
        let rows = self.rows();
        crate::ui::spawn(async move { rows.refresh(&context).await });
    }
}

/// The rows that show figures, apart from the dialog they are in.
#[derive(Clone)]
struct Rows {
    free: adw::ActionRow,
    snapshots: adw::ComboRow,
    stash: adw::ActionRow,
    cache: adw::ActionRow,
}

impl Rows {
    async fn refresh(&self, context: &Context) {
        match context.daemon.get_local_storage().await {
            Ok(storage) => self.show(&storage),
            Err(error) => warn!(%error, "the local storage figures could not be read"),
        }
    }

    fn show(&self, storage: &LocalStorage) {
        self.free.set_subtitle(&glib::format_size(storage.free));
        self.snapshots.set_subtitle(&format!(
            "Using {} of {}",
            glib::format_size(storage.snapshots),
            glib::format_size(storage.snapshot_limit)
        ));
        self.stash.set_subtitle(&format!(
            "{}, kept for thirty days so a restore can be undone",
            glib::format_size(storage.stash)
        ));
        self.cache.set_subtitle(&format!(
            "{}, extracted again when they are next looked at",
            glib::format_size(storage.cache)
        ));
    }
}

fn freed(bytes: u64) -> String {
    format!("{} freed", glib::format_size(bytes))
}
