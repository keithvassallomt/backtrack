// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Backtrack GTK application entry point.
//!
//! Stage 0 skeleton: initialise logging and announce startup. The application
//! shell, timeline browser, wizard, and preferences arrive in Stage 6+.

use tracing::info;

/// Freedesktop application ID (see stack.md / brief.md).
const APP_ID: &str = "io.github.keithvassallomt.Backtrack";

fn main() {
    let _log_guard = backtrack_core::logging::init("backtrack-gtk");

    info!(
        version = backtrack_core::VERSION,
        app_id = APP_ID,
        "backtrack-gtk starting"
    );
}
