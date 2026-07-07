//! Backtrack daemon entry point.
//!
//! Stage 0 skeleton: initialise logging, announce startup, and shut down
//! cleanly on SIGTERM/SIGINT. The scheduler, D-Bus service, and job queue
//! arrive in Stage 3+.

use tokio::signal::unix::{signal, SignalKind};
use tracing::info;

#[tokio::main]
async fn main() {
    // Minimal structured logging for now; replaced by backtrack_core::logging
    // (JSONL rotation, panic hook) in S00-T3.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!(version = backtrack_core::VERSION, "backtrackd starting");

    let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut sigint = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    tokio::select! {
        _ = sigterm.recv() => info!("received SIGTERM, shutting down"),
        _ = sigint.recv() => info!("received SIGINT, shutting down"),
    }

    info!("backtrackd stopped");
}
