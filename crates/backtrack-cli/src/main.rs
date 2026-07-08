// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Backtrack CLI entry point.
//!
//! Stage 0 skeleton: initialise logging, parse arguments, and announce startup.
//! The commands that map the daemon's D-Bus interface (status, doctor, backup,
//! restore) arrive in Stage 3+.

use clap::Parser;
use tracing::info;

/// Thin command-line client for the Backtrack daemon.
#[derive(Parser)]
#[command(name = "backtrack", version, about, long_about = None)]
struct Cli {}

fn main() {
    let _cli = Cli::parse();

    let _log_guard = backtrack_core::logging::init("backtrack");

    info!(version = backtrack_core::VERSION, "backtrack cli starting");
}
