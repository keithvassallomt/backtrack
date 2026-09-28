// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! `org.backtrack.Daemon1.Dev`: controls for development, served only under
//! `BACKTRACK_DEV`.
//!
//! An interface of its own rather than methods on the published one, so that
//! an installed daemon does not have them at all and the published interface's
//! snapshot is the same in both modes.

use std::sync::Arc;

use backtrack_core::dbus::Reason;
use tracing::info;

use super::health::{Health, HealthState};
use super::{DaemonError, Result, Shared};

/// The development object.
pub struct Dev {
    shared: Arc<Shared>,
}

impl Dev {
    pub fn new(shared: Arc<Shared>) -> Dev {
        Dev { shared }
    }
}

#[zbus::interface(name = "org.backtrack.Daemon1.Dev")]
impl Dev {
    /// Show `state` for `reason` in place of the real health, with its banner
    /// and its notification, until called again with an empty state.
    ///
    /// How each banner is seen without breaking something to get it.
    pub(crate) async fn force_health(&self, state: &str, reason: &str) -> Result<()> {
        if state.is_empty() {
            info!("health is the real state again");
            self.shared.force(None);
            return Ok(());
        }
        let state = HealthState::parse(state)
            .ok_or_else(|| DaemonError::InvalidArgument(format!("no health state {state:?}")))?;
        let reason = match reason {
            "" => None,
            token => Some(
                Reason::parse(token)
                    .ok_or_else(|| DaemonError::InvalidArgument(format!("no reason {token:?}")))?,
            ),
        };
        info!(
            state = state.as_str(),
            reason = reason.map(Reason::as_str).unwrap_or(""),
            "health forced for development"
        );
        self.shared.force(Some(Health { state, reason }));
        Ok(())
    }
}
