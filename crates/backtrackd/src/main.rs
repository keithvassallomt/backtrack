// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Backtrack daemon entry point.
//!
//! Everything of substance lives in [`daemon`]; `main` only installs logging,
//! runs the daemon, and maps its outcome to an exit code. Keeping the two apart
//! means the startup path can be reasoned about without a process around it.

mod daemon;
mod jobs;
mod schedule;
mod service;
// Exists only to assert that the packaged units and the code agree.
#[cfg(test)]
mod units;

use std::process::ExitCode;

use tracing::info;

fn main() -> ExitCode {
    let _log_guard = backtrack_core::logging::init("backtrackd");
    info!(version = backtrack_core::VERSION, "backtrackd starting");

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            tracing::error!("cannot start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(daemon::run()) {
        Ok(outcome) => {
            info!(?outcome, "backtrackd stopped");
            ExitCode::from(outcome.exit_code())
        }
        Err(e) => {
            daemon::report(&e);
            ExitCode::FAILURE
        }
    }
}
