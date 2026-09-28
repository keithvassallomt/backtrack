// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! health.md's "Backtrack can't sign in to …": the destination is there and
//! will not let Backtrack write.
//!
//! What can be done about it depends on the kind of place. A network share
//! GIO mounted can be signed in to again, with GNOME's own credentials dialog.
//! An SSH server is signed in to with this computer's key, never a password,
//! so there is nothing to type here; the fix is on the server, and this says
//! so. A drive or folder has no sign-in at all: it went read-only or its
//! permissions changed, and this says that instead.

use gtk4::gio;
use gtk4::prelude::*;
use libadwaita::prelude::*;
use tracing::{info, warn};

use super::{alert, Context};
use crate::model::health::{self, Place};

pub fn present(context: &Context) {
    let context = context.clone();
    crate::ui::spawn(async move {
        let Some(repository) = context
            .config()
            .await
            .and_then(|config| config.storage.repository)
        else {
            return;
        };
        let name = backtrack_core::destination::name(&repository);
        let heading = backtrack_core::engine::HealthFailure::AuthExpired.copy(&name);
        let place = health::place(&repository);
        let (body, go) = match place {
            Place::Share => (
                format!(
                    "The network folder on {name} needs you to sign in again. Backups carry \
                     on once you have."
                ),
                "Sign In…",
            ),
            Place::Server => (
                format!(
                    "The server {name} refused this computer's sign-in. Backtrack signs in \
                     with this computer's SSH key, not a password, so check that the key is \
                     still authorised on the server, then try again."
                ),
                "Try Again",
            ),
            Place::Folder => (
                format!(
                    "Backtrack is not allowed to write to {repository}. The drive may have \
                     been mounted read-only, or the folder's permissions may have changed. \
                     Once that is put right, try again."
                ),
                "Try Again",
            ),
        };
        let dialog = alert(&heading, &body, go, true);
        let answered = dialog.choose_future(Some(&context.window)).await;
        if answered != "go" {
            return;
        }
        if place == Place::Share {
            if let Err(message) = sign_in(&context.window, &repository).await {
                context.toast(&message);
                return;
            }
        }
        context.back_up();
    });
}

/// Sign in to the share again: unmount it if it is mounted, then mount it
/// with GTK's operation, which is what puts GNOME's credentials dialog up.
async fn sign_in(window: &libadwaita::ApplicationWindow, repository: &str) -> Result<(), String> {
    let file = gio::File::for_path(repository);
    let operation = gtk4::MountOperation::new(Some(window));
    // Mounted, GIO knows the share's real address; not mounted, it has to be
    // read from the name of the folder GIO mounted it on last time.
    let address = match file.find_enclosing_mount(gio::Cancellable::NONE) {
        Ok(mount) => {
            let address = mount.root().uri().to_string();
            if let Err(error) = mount
                .unmount_with_operation_future(gio::MountUnmountFlags::NONE, Some(&operation))
                .await
            {
                warn!(%error, "the share could not be unmounted to sign in again");
            }
            Some(address)
        }
        Err(_) => health::share_uri(repository),
    };
    let Some(address) = address else {
        return Err(
            "Backtrack cannot tell which share this is. Open it in Files to sign in, \
                    then try again."
                .to_string(),
        );
    };
    match gio::File::for_uri(&address)
        .mount_enclosing_volume_future(gio::MountMountFlags::NONE, Some(&operation))
        .await
    {
        Ok(()) => {
            info!(address, "signed in to the share again");
            Ok(())
        }
        Err(error) if error.matches(gio::IOErrorEnum::AlreadyMounted) => Ok(()),
        Err(error) if error.matches(gio::IOErrorEnum::FailedHandled) => {
            Err("Signing in was cancelled".to_string())
        }
        Err(error) => {
            warn!(%error, address, "signing in to the share failed");
            Err(format!("Backtrack could not sign in to the share: {error}"))
        }
    }
}
