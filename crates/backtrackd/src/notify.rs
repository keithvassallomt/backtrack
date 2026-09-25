// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Desktop notifications, sent by the daemon because the window may be gone.
//!
//! The wizard tells somebody starting their first backup that they can close
//! the window, and a first backup can take hours. Whatever reports its end has
//! to outlive the window, which leaves the daemon.
//!
//! Only the first backup notifies today. Stage 10 brings the rest of health.md's
//! notifications, and the policy behind them, through the same door.

use std::collections::HashMap;

use backtrack_core::config::Notifications;
use tracing::{debug, info};
use zbus::zvariant::Value;

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;
}

/// What to say when the first backup ends, or nothing.
///
/// Sent under "Only when attention is needed" as well as "Every backup",
/// because the wizard promised it: the person was told they could close the
/// window, and the end of the first backup is the thing they closed it
/// waiting for. "Never" means never. A cancelled first backup was somebody's
/// own decision and needs no announcement.
pub fn first_backup(policy: Notifications, outcome: &str) -> Option<(&'static str, &'static str)> {
    if policy == Notifications::None {
        return None;
    }
    match outcome {
        "completed" => Some((
            "Your first backup is complete",
            "Your files are protected. From now on, backups run on their own.",
        )),
        "failed" => Some((
            "Your first backup did not finish",
            "Open Backtrack to see what stopped it.",
        )),
        _ => None,
    }
}

/// How long the notification service gets to answer.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(5);

/// Put a notification on screen. A desktop with no notification service, or
/// one that does not answer, is not an error worth more than a line in the
/// log.
pub async fn send(connection: &zbus::Connection, summary: &str, body: &str) {
    if tokio::time::timeout(PATIENCE, deliver(connection, summary, body))
        .await
        .is_err()
    {
        debug!("the notification service did not answer in time");
    }
}

async fn deliver(connection: &zbus::Connection, summary: &str, body: &str) {
    let proxy = match NotificationsProxy::new(connection).await {
        Ok(proxy) => proxy,
        Err(error) => {
            debug!(%error, "no notification service to tell");
            return;
        }
    };
    // `desktop-entry` is what lets the desktop show the application's own
    // name and icon, and route a click to it.
    let hints = HashMap::from([("desktop-entry", Value::from(backtrack_core::secret::APP_ID))]);
    match proxy
        .notify(
            "Backtrack",
            0,
            backtrack_core::secret::APP_ID,
            summary,
            body,
            &[],
            hints,
            -1,
        )
        .await
    {
        Ok(id) => info!(id, summary, "notification shown"),
        Err(error) => debug!(%error, "the notification could not be shown"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_backup_is_announced_unless_notifications_are_off() {
        assert!(first_backup(Notifications::AttentionOnly, "completed").is_some());
        assert!(first_backup(Notifications::All, "completed").is_some());
        assert_eq!(first_backup(Notifications::None, "completed"), None);
    }

    #[test]
    fn a_failure_is_announced_and_a_cancellation_is_not() {
        let (summary, _) = first_backup(Notifications::AttentionOnly, "failed").unwrap();
        assert!(summary.contains("did not finish"), "{summary}");
        assert_eq!(first_backup(Notifications::All, "cancelled"), None);
    }
}
