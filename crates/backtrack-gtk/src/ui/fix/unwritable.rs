// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! health.md's "Backtrack can't write to …": the destination is there, and
//! its permissions or a read-only mount refuse the write (S10-T8).
//!
//! What can be done depends on the kind of place. A drive or folder has had
//! its permissions changed or been mounted read-only, and this says so. A
//! network share refuses writes the same way whether its sign-in has lapsed
//! or the account lost its permission, so it is offered a sign-in, which
//! puts right the first. On an SSH server the folder is the server's to fix.

use libadwaita::prelude::*;

use super::{alert, signin, Context};
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
        let heading = backtrack_core::engine::HealthFailure::DestinationUnwritable.copy(&name);
        let place = health::place(&repository);
        let (body, go) = match place {
            Place::Folder => (
                format!(
                    "Backtrack is not allowed to write to {repository}. The drive may have \
                     been mounted read-only, or the folder's permissions may have changed. \
                     Once that is put right, try again."
                ),
                "Try Again",
            ),
            Place::Share => (
                format!(
                    "The network folder on {name} is not letting Backtrack write to it. \
                     Signing in again usually puts that right; if it does not, your account \
                     may no longer be allowed to write to that folder."
                ),
                "Sign In…",
            ),
            Place::Server => (
                format!(
                    "The server {name} is not letting Backtrack write to the folder the \
                     backups are in. Its permissions on the server may have changed, or its \
                     disk may be mounted read-only. Once that is put right, try again."
                ),
                "Try Again",
            ),
        };
        let dialog = alert(&heading, &body, go, true);
        if dialog.choose_future(Some(&context.window)).await != "go" {
            return;
        }
        if place == Place::Share {
            if let Err(message) = signin::sign_in(&context.window, &repository).await {
                context.toast(&message);
                return;
            }
        }
        context.back_up();
    });
}
