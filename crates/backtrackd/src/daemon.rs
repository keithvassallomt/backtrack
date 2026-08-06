// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Daemon startup, single-instance enforcement, and shutdown.
//!
//! Startup order is deliberate:
//!
//! 1. **Load the configuration.** Failures here are fatal: see
//!    [`backtrack_core::config`] for why a bad file is worth refusing to start
//!    over, while a merely *unknown* key is not.
//! 2. **Open the index for writing.** The daemon owns the one and only
//!    [`IndexWriter`]; every other process reads through a read-only connection.
//! 3. **Export the service object**, then **claim the bus name**. That order
//!    matters: a client that activated us fires its method call the moment the
//!    name appears, so owning the name on a connection that serves nothing is a
//!    race we would lose. Ownership of `org.backtrack.Daemon1` *is* the
//!    single-instance lock — no PID file, nothing stale to clean up after a
//!    crash, and the bus arbitrates the race for us.
//! 4. **Connect the engine.** A destination that is merely unreachable does not
//!    stop the daemon; reporting that is precisely what the health model is for,
//!    and a client needs a live daemon to hear it from.
//!
//! Shutdown is driven by SIGTERM (systemd) or SIGINT (a developer's Ctrl-C).

use std::path::PathBuf;

use backtrack_core::config::Config;
use backtrack_core::index::IndexWriter;
use backtrack_core::{dbus, paths};

use crate::jobs::JobRegistry;
use crate::service::{self, Daemon1, Shared};
use std::sync::Arc;
use tokio::signal::unix::{signal, SignalKind};
use tracing::{error, info, warn};
use zbus::fdo::RequestNameFlags;
use zbus::fdo::RequestNameReply;
use zbus::object_server::SignalEmitter;

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

    #[error("cannot reach the system keyring: {0}")]
    Secrets(#[source] backtrack_core::engine::EngineError),

    #[error("cannot install the {signal} handler: {source}")]
    Signals {
        signal: &'static str,
        #[source]
        source: std::io::Error,
    },
}

/// Start the daemon and run until a shutdown signal arrives.
pub async fn run() -> Result<Outcome, StartupError> {
    let config = Config::load()?;
    info!(
        configured = config.is_configured(),
        frequency = ?config.backup.frequency,
        "configuration loaded"
    );

    // Held for the daemon's lifetime: this binding is the single-writer
    // guarantee. Nothing else may open the index for writing while we live.
    let _index = open_index()?;

    // The one registry every job goes through.
    let jobs = JobRegistry::new();
    let secrets = backtrack_core::secret::default_store().map_err(StartupError::Secrets)?;
    let shared = Shared::new(config, Arc::clone(&jobs), secrets);

    // Export the object *before* claiming the name. A client that activated us
    // sends its method call the instant the name appears, so a name owned by a
    // connection with nothing on it is a race we would lose.
    let connection = zbus::connection::Builder::session()
        .map_err(StartupError::Bus)?
        .serve_at(dbus::OBJECT_PATH, Daemon1::new(Arc::clone(&shared)))
        .map_err(StartupError::Bus)?
        .build()
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

    // A destination that is merely unreachable must not stop the daemon: the
    // health model exists to report exactly that, and a GUI needs a live daemon
    // to hear it from.
    if let Err(e) = shared.connect_engine().await {
        warn!("backup engine not available yet: {e}");
    }
    // Health must survive a restart: the catalogue remembers when the last
    // backup landed even though this process does not.
    shared.seed_last_backup();

    // Turn job updates into signals for as long as we run.
    let emitter = SignalEmitter::new(&connection, dbus::OBJECT_PATH).map_err(StartupError::Bus)?;
    tokio::spawn(service::fan_out_signals(
        Arc::clone(&shared),
        emitter.to_owned(),
    ));

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
