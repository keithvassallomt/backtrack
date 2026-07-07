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

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!(version = backtrack_core::VERSION, "backtrack cli starting");
}
