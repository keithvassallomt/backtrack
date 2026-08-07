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
//! 3. **Connect the engine and read the catalogue.** A destination that is
//!    merely unreachable does not stop the daemon; reporting that is precisely
//!    what the health model is for, and a client needs a live daemon to hear it
//!    from.
//! 4. **Export the service object and start the signal fan-out.**
//! 5. **Claim the bus name.** Ownership of `org.backtrack.Daemon1` *is* the
//!    single-instance lock — no PID file, nothing stale to clean up after a
//!    crash, and the bus arbitrates the race for us. It is also the readiness
//!    announcement: systemd marks the unit started when the name appears, and an
//!    activating client's queued call arrives immediately afterwards. **Nothing
//!    that shapes an answer may happen after this point**, or the first call
//!    races it.
//! 6. **Start the scheduler and reconcile the catalogue, after the name is
//!    won.** These are the two things that *act on* the repository rather than
//!    merely describe it, and winning the name is what makes this process the
//!    single writer. An instance that is about to lose the race must not have
//!    begun a backup in the meantime.
//!
//! Shutdown is driven by SIGTERM (systemd) or SIGINT (a developer's Ctrl-C).

use std::path::PathBuf;

use backtrack_core::config::Config;
use backtrack_core::index::IndexWriter;
use backtrack_core::{dbus, paths};

use crate::jobs::JobRegistry;
use crate::preflight::{self, DbusProbe};
use crate::reachability;
use crate::schedule::Scheduler;
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

    // The single-writer guarantee: one `IndexWriter` for the process, held for
    // its lifetime. Shared with the service layer because the backup pipeline
    // catalogues through it; nothing else may open the index for writing.
    let index = Arc::new(std::sync::Mutex::new(open_index()?));

    // The one registry every job goes through.
    let jobs = JobRegistry::new();
    let secrets = backtrack_core::secret::default_store().map_err(StartupError::Secrets)?;
    let shared = Shared::new(config, Arc::clone(&jobs), secrets);
    shared.set_index(index);

    // Everything that shapes an answer happens before the name is claimed.
    //
    // A destination that is merely unreachable must not stop the daemon: the
    // health model exists to report exactly that, and a client needs a live
    // daemon to hear it from.
    if let Err(e) = shared.connect_engine().await {
        warn!("backup engine not available yet: {e}");
    }
    // Health must survive a restart: the catalogue remembers when the last
    // backup landed even though this process does not.
    shared.seed_last_backup();
    // As must a pause and the attempt clock, which only this daemon records.
    shared.restore_persisted_state();

    // Export the object, and start turning job updates into signals, before
    // claiming the name — see below for why the order matters.
    let connection = zbus::connection::Builder::session()
        .map_err(StartupError::Bus)?
        .serve_at(dbus::OBJECT_PATH, Daemon1::new(Arc::clone(&shared)))
        .map_err(StartupError::Bus)?
        .build()
        .await
        .map_err(StartupError::Bus)?;

    let emitter = SignalEmitter::new(&connection, dbus::OBJECT_PATH).map_err(StartupError::Bus)?;
    tokio::spawn(service::fan_out_signals(
        Arc::clone(&shared),
        emitter.to_owned(),
    ));

    // Claiming the name is the readiness announcement, so every piece of state
    // that shapes an answer is already in place by here. systemd reports the
    // unit started the moment the name appears, and an activating client's
    // queued call is delivered immediately after — so anything still unfinished
    // at this point is something that call can race. Getting this wrong is not a
    // crash: it is a daemon that answers `GetStatus` with "never backed up" for
    // a machine holding a year of archives, because the catalogue had not been
    // read yet.
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

    // Only now does anything start *touching* the repository. Winning the name
    // is what makes this daemon the single writer, and an instance that is about
    // to discover it lost the race must not have started a backup or begun
    // rewriting the catalogue in the meantime. Borg's own locking would prevent
    // corruption, but the user would see spurious failures from a daemon that
    // was never meant to be running.
    let scheduler = Scheduler::new(Arc::clone(&shared));
    let waker = scheduler.waker();
    shared.set_waker(Arc::clone(&waker));

    // The destination watcher and the system probe share one wake-up path:
    // NetworkManager is what tells us a link came back, and that is the event
    // that both ends local protection and un-defers a metered backup.
    let network_changed = Arc::new(tokio::sync::Notify::new());
    connect_system_probe(&shared, waker, Arc::clone(&network_changed)).await;
    tokio::spawn(scheduler.run());
    tokio::spawn(reachability::watch(Arc::clone(&shared), network_changed));

    // A backup can reach the repository and never be catalogued — the daemon
    // killed mid-ingest, the machine losing power, a listing that broke off. The
    // repository is the authority, so every start asks it what it actually holds
    // and catalogues whatever is missing. It runs as a job because it takes only
    // a shared lock and can take minutes on a large repository: a restore, and
    // the next hourly backup, must not wait for it.
    let outstanding = shared.uncatalogued_count().await;
    if outstanding > 0 {
        warn!(
            count = outstanding,
            "backups exist that are not browsable yet; cataloguing them"
        );
    }
    if let Some(job) = shared.reconcile_catalogue().await {
        info!(job, "reconciling the catalogue against the repository");
    }

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

/// Attach the machine probe used by preflight, and start watching for the power
/// and connection changes that could make a skipped backup runnable.
///
/// Every failure here is survivable and none of them stops the daemon: without
/// UPower the battery gate simply opens, which is the right answer for a desktop
/// that has no battery to be running on.
async fn connect_system_probe(
    shared: &Arc<Shared>,
    waker: Arc<tokio::sync::Notify>,
    network_changed: Arc<tokio::sync::Notify>,
) {
    let Some(probe) = DbusProbe::connect().await else {
        return;
    };
    let connection = probe.connection().clone();
    shared.set_probe(Arc::new(probe));
    preflight::watch_for_changes(connection, waker, network_changed).await;
    info!("battery and metered-connection checks are active");
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
