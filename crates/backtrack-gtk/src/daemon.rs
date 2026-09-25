// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The window's side of `org.backtrack.Daemon1`.
//!
//! Browsing does not go through here: the timeline reads the index directly,
//! which is what makes it instant and what makes it work with the daemon
//! stopped. The daemon is needed for the things only it can do — extracting a
//! file for the preview, starting a backup, pausing the schedule — and for
//! telling the window when the catalogue has grown.
//!
//! The connection is made through D-Bus activation, so there is no "is the
//! daemon running?" check anywhere in the app. Asking for it starts it.

use backtrack_core::dbus::{ReplacedFile, RestorePreview, SearchResult, Status};
use tracing::warn;

/// The daemon, as the window calls it.
#[zbus::proxy(
    interface = "org.backtrack.Daemon1",
    default_path = "/org/backtrack/Daemon1"
)]
pub trait Daemon1 {
    fn backup_now(&self) -> zbus::Result<u64>;
    fn pause(&self, until: u64) -> zbus::Result<()>;
    fn resume(&self) -> zbus::Result<()>;
    fn get_status(&self) -> zbus::Result<Status>;
    /// A readable descriptor onto one file's contents as of `archive` (the
    /// archive's name, as the index records it).
    fn preview_file(&self, archive: &str, path: &str) -> zbus::Result<zbus::zvariant::OwnedFd>;
    fn search_files(&self, query: &str) -> zbus::Result<Vec<SearchResult>>;

    /// A readable descriptor onto the *live* copy of a catalogued path — the
    /// other half of `preview_file`, for the side of a comparison that says
    /// "today". Size and modification time come from the descriptor, so the
    /// header and the content can never describe different files.
    fn live_file(&self, path: &str) -> zbus::Result<zbus::zvariant::OwnedFd>;

    /// Which of these catalogued paths are still on this computer: one byte
    /// each, in the order asked — 0 unknown, 1 absent, 2 present.
    ///
    /// Asked of the daemon rather than answered here, because a sandboxed
    /// build of this application has no path to the user's files at all. An
    /// unknown is a real answer and means nothing may be said.
    fn paths_on_disk(&self, paths: &[String]) -> zbus::Result<Vec<u8>>;

    /// Work out what restoring `paths` into `dest` would do, without doing any
    /// of it. Returns the job doing the working out; everything after this is
    /// addressed with the same id.
    fn prepare_restore(&self, archive: &str, paths: &[String], dest: &str) -> zbus::Result<u64>;
    /// What the restore prepared under `job` would do.
    fn get_restore_preview(&self, job: u64) -> zbus::Result<RestorePreview>;
    /// Carry it out: `blanket` answers every conflict, `decisions` overrides
    /// individual paths.
    fn execute_restore(
        &self,
        job: u64,
        blanket: &str,
        decisions: &[(String, String)],
    ) -> zbus::Result<u64>;
    /// Restore one path into `dest`, a directory made for it. No conflicts can
    /// arise, so there is nothing to ask and no preview to read: the job id
    /// this returns is the whole of it.
    fn restore_into(&self, archive: &str, paths: &[String], dest: &str) -> zbus::Result<u64>;
    /// Put back everything that restore moved.
    fn undo_restore(&self, job: u64) -> zbus::Result<u64>;
    /// Throw away a prepared restore and the copy it extracted.
    fn discard_restore(&self, job: u64) -> zbus::Result<()>;

    /// The files the safety stash is keeping, newest restore first.
    fn list_replaced(&self, limit: u32) -> zbus::Result<Vec<ReplacedFile>>;
    /// Put one of them back, naming it by where the stash is keeping it.
    /// Answers with where whatever was in its place went, or "" if nothing was.
    fn put_back_replaced(&self, stashed: &str) -> zbus::Result<String>;
    fn cancel_job(&self, id: u64) -> zbus::Result<()>;

    /// The whole configuration, as the TOML `config.toml` holds.
    fn get_config(&self) -> zbus::Result<String>;
    /// Change one setting: a dotted key and a TOML literal.
    fn set_config(&self, key: &str, value: &str) -> zbus::Result<()>;
    /// What is at a destination before anything is created there: `empty`,
    /// `existing`, `occupied` or `unwritable`.
    fn inspect_destination(&self, repository: &str) -> zbus::Result<String>;
    /// Create a repository and make it the destination.
    fn setup_repo(&self, path: &str, passphrase: &str) -> zbus::Result<()>;
    /// Adopt an existing repository. Returns once its newest backup can be
    /// browsed; the rest are catalogued behind.
    fn import_repo(&self, path: &str, passphrase: &str) -> zbus::Result<()>;
    /// The configured repository's recovery key, as `borg key export` writes
    /// it.
    fn export_recovery_key(&self) -> zbus::Result<String>;

    /// Progress of a running backup.
    #[zbus(signal)]
    fn backup_progress(&self, job: u64, phase: &str, current: u64, total: u64) -> zbus::Result<()>;

    /// Progress of catalogue ingest for one archive, as a percentage.
    #[zbus(signal)]
    fn indexing_progress(&self, archive: &str, pct: u32) -> zbus::Result<()>;

    /// The overall health state changed.
    #[zbus(signal)]
    fn status_changed(&self, state: &str) -> zbus::Result<()>;

    /// A job ended. `outcome` is `completed`, `cancelled` or `failed`.
    #[zbus(signal)]
    fn job_finished(&self, job: u64, kind: &str, outcome: &str) -> zbus::Result<()>;
}

/// Connect to the daemon, honoring `BACKTRACK_DEV` for the bus name.
///
/// Failing here is not fatal and must not be treated as such. Everything the
/// timeline shows comes from the index, so a window with no daemon behind it is
/// still a working browser — it simply cannot preview, back up or pause, and it
/// says so where those controls are.
pub async fn connect() -> Result<Daemon1Proxy<'static>, String> {
    let connection = zbus::Connection::session()
        .await
        .map_err(|e| format!("Backtrack could not reach the session bus: {e}"))?;
    let name = backtrack_core::dbus::bus_name();
    Daemon1Proxy::builder(&connection)
        .destination(name)
        .map_err(|e| e.to_string())?
        .build()
        .await
        .map_err(|e| explain(&e, name))
}

/// Turn a connection failure into something a person can act on.
fn explain(error: &zbus::Error, name: &str) -> String {
    match error {
        zbus::Error::MethodError(err_name, _, _)
            if err_name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" =>
        {
            warn!(bus_name = name, "no daemon, and the bus cannot start one");
            "The Backtrack service is not installed, so backups cannot be started from \
             here. Browsing and restoring still work."
                .to_string()
        }
        other => {
            warn!(bus_name = name, error = %other, "could not reach the daemon");
            format!("Backtrack could not reach its background service: {other}")
        }
    }
}
