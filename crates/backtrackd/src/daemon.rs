// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Daemon startup, single-instance enforcement, and shutdown.
//!
//! Startup order is deliberate:
//!
//! 1. **Claim the bus name.** Ownership of `org.backtrack.Daemon1` *is* the
//!    single-instance lock — no PID file, no stale lock to clean up after a
//!    crash, and the bus arbitrates races for us. Claiming first also means a
//!    losing instance exits before it has opened anything.
//! 2. **Load the configuration.** Failures here are fatal: see
//!    [`backtrack_core::config`] for why a bad file is worth refusing to start
//!    over, while a merely *unknown* key is not.
//! 3. **Open the index for writing.** The daemon owns the one and only
//!    [`IndexWriter`]; every other process reads through a read-only connection.
//!
//! Shutdown is the reverse and is driven by SIGTERM (systemd) or SIGINT (a
//! developer's Ctrl-C).

use std::path::PathBuf;

use backtrack_core::config::Config;
use backtrack_core::index::IndexWriter;
use backtrack_core::{dbus, paths};
use tokio::signal::unix::{signal, SignalKind};
use tracing::{error, info};
use zbus::fdo::RequestNameFlags;
use zbus::fdo::RequestNameReply;

/// Why the daemon stopped. Both variants are a successful exit: losing the
/// single-instance race is a normal outcome, not a failure, which is what makes
/// D-Bus activation safe to trigger from several clients at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Ran and shut down cleanly on a signal.
    ShutDown,
    /// Another instance already owned the bus name.
    AlreadyRunning,
}

impl Outcome {
    /// The process exit code for this outcome. Both are zero — see [`Outcome`].
    pub fn exit_code(self) -> u8 {
        match self {
            Outcome::ShutDown | Outcome::AlreadyRunning => 0,
        }
    }
}

/// Anything that stops the daemon coming up.
#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error("cannot connect to the session bus: {0}")]
    Bus(#[source] zbus::Error),

    #[error("cannot claim the bus name {name}: {source}")]
    NameRequest {
        name: String,
        #[source]
        source: zbus::Error,
    },

    #[error("cannot create the data directory {path}: {source}")]
    DataDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Config(#[from] backtrack_core::config::ConfigError),

    #[error("cannot open the index at {path}: {source}")]
    Index {
        path: PathBuf,
        #[source]
        source: backtrack_core::index::IndexError,
    },

    #[error("cannot install the {signal} handler: {source}")]
    Signals {
        signal: &'static str,
        #[source]
        source: std::io::Error,
    },
}

/// Start the daemon and run until a shutdown signal arrives.
pub async fn run() -> Result<Outcome, StartupError> {
    let connection = zbus::Connection::session()
        .await
        .map_err(StartupError::Bus)?;

    let name = dbus::bus_name();
    if !claim_name(&connection, name).await? {
        // Not an error: this is how a second `systemctl --user start`, or a
        // D-Bus activation racing a manual launch, is supposed to end.
        info!(
            name,
            "another backtrackd already owns the bus name; exiting"
        );
        return Ok(Outcome::AlreadyRunning);
    }
    info!(name, "claimed the bus name");

    let config = Config::load()?;
    info!(
        configured = config.is_configured(),
        frequency = ?config.backup.frequency,
        "configuration loaded"
    );

    // Held for the daemon's lifetime: this binding is the single-writer
    // guarantee. S03-T2 hands it to the job registry; until then nothing writes
    // through it, but nothing else may open the index for writing either.
    let _index = open_index()?;

    // The service interface arrives in S03-T3. Owning the name without serving
    // it is deliberate for now — it makes the single-instance behaviour
    // testable before there is anything to call.

    let reason = wait_for_shutdown().await?;
    info!(reason, "shutting down");
    Ok(Outcome::ShutDown)
}

/// Request `name` with `DoNotQueue`, returning whether we became its owner.
///
/// `DoNotQueue` is what makes this a lock rather than a waiting room: without
/// it a second instance would sit queued behind the first and silently take
/// over the name when the first exits, leaving two live daemons that both
/// believe they are authoritative.
async fn claim_name(connection: &zbus::Connection, name: &str) -> Result<bool, StartupError> {
    let reply = connection
        .request_name_with_flags(name, RequestNameFlags::DoNotQueue.into())
        .await;
    match reply {
        Ok(RequestNameReply::PrimaryOwner) => Ok(true),
        // We already hold it (belt and braces — we only ask once).
        Ok(RequestNameReply::AlreadyOwner) => Ok(true),
        Ok(RequestNameReply::Exists) | Ok(RequestNameReply::InQueue) => Ok(false),
        // zbus reports a `DoNotQueue` refusal as a typed error rather than a
        // reply on some paths; both mean the same thing.
        Err(zbus::Error::NameTaken) => Ok(false),
        Err(source) => Err(StartupError::NameRequest {
            name: name.to_string(),
            source,
        }),
    }
}

/// Create the data directory if needed and open the index for writing.
fn open_index() -> Result<IndexWriter, StartupError> {
    let dir = paths::data_dir();
    std::fs::create_dir_all(&dir).map_err(|source| StartupError::DataDir {
        path: dir.clone(),
        source,
    })?;

    let path = paths::index_db();
    let writer = IndexWriter::open(&path).map_err(|source| StartupError::Index {
        path: path.clone(),
        source,
    })?;
    info!(path = %path.display(), "index opened for writing");
    Ok(writer)
}

/// Block until SIGTERM or SIGINT, returning which arrived.
///
/// Once the job model lands (S03-T2) this is where a running job is given its
/// chance to checkpoint; while the daemon is idle it returns immediately, which
/// is what keeps the stop well inside systemd's patience.
async fn wait_for_shutdown() -> Result<&'static str, StartupError> {
    let mut sigterm = signal(SignalKind::terminate()).map_err(|source| StartupError::Signals {
        signal: "SIGTERM",
        source,
    })?;
    let mut sigint = signal(SignalKind::interrupt()).map_err(|source| StartupError::Signals {
        signal: "SIGINT",
        source,
    })?;
    tokio::select! {
        _ = sigterm.recv() => Ok("SIGTERM"),
        _ = sigint.recv() => Ok("SIGINT"),
    }
}

/// Log a startup failure with the whole error chain, so the JSONL log records
/// the underlying cause and not just the top-level summary.
pub fn report(error: &StartupError) {
    let mut chain = Vec::new();
    let mut source = std::error::Error::source(error);
    while let Some(e) = source {
        chain.push(e.to_string());
        source = e.source();
    }
    error!(
        cause = %chain.join(": "),
        "backtrackd failed to start: {error}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn losing_the_name_race_is_a_successful_outcome() {
        // Encoded as a test because the exit code depends on it: systemd and
        // D-Bus activation both treat a non-zero exit as a failed unit.
        assert_eq!(Outcome::AlreadyRunning.exit_code(), 0);
        assert_eq!(Outcome::ShutDown.exit_code(), 0);
    }

    #[test]
    fn startup_errors_render_with_their_cause() {
        let err = StartupError::Index {
            path: PathBuf::from("/x/index.db"),
            source: backtrack_core::index::IndexError::Corrupt("bad header".into()),
        };
        let text = err.to_string();
        assert!(text.contains("/x/index.db"), "names the path: {text}");
        assert!(
            std::error::Error::source(&err).is_some(),
            "keeps the underlying cause for the log chain"
        );
    }
}
