// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>

//! Backtrack daemon entry point.
//!
//! Stage 0 skeleton: initialise logging, announce startup, and shut down
//! cleanly on SIGTERM/SIGINT. The scheduler, D-Bus service, and job queue
//! arrive in Stage 3+.

use tokio::signal::unix::{signal, SignalKind};
use tracing::info;

#[tokio::main]
async fn main() {
    let _log_guard = backtrack_core::logging::init("backtrackd");

    info!(version = backtrack_core::VERSION, "backtrackd starting");

    let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut sigint = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    tokio::select! {
        _ = sigterm.recv() => info!("received SIGTERM, shutting down"),
        _ = sigint.recv() => info!("received SIGINT, shutting down"),
    }

    info!("backtrackd stopped");
}
