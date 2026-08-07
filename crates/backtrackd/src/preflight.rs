// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The gates a scheduled backup passes before it starts.
//!
//! Three principles shape every rule here:
//!
//! - **A skip is a decision, not a failure.** Declining to back up on battery is
//!   the product doing what it was told. Nothing here raises a health banner,
//!   and nothing here counts as an attempt, so the run happens as soon as the
//!   condition clears rather than an interval later.
//! - **Uncertainty never blocks a backup.** A desktop with no UPower, a machine
//!   with no NetworkManager, a filesystem that will not answer `statvfs` — all
//!   report "cannot tell", and a gate that cannot tell always opens. The
//!   alternative is a machine that quietly stops backing up because a service
//!   the user has never heard of is not installed.
//! - **`BackupNow` passes every gate.** These are conditions on the *schedule*.
//!   Someone pressing the button on battery has already decided.
//!
//! The gates themselves are a pure function of [`Facts`], so each one is tested
//! by constructing the world rather than by arranging for it.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tracing::{debug, info, warn};
use zbus::zvariant::OwnedValue;

/// Below this much free local space, a scheduled backup is skipped. The
/// catalogue and the preview cache both grow during a run, and an index that
/// runs out of disk mid-transaction is a worse outcome than a backup deferred by
/// an hour.
const DISK_FLOOR: u64 = 100 * 1024 * 1024;

/// Below this much, backups still run but the product admits it needs attention.
/// Per health.md's "Local disk full" row: `DEGRADED`, then `BROKEN`.
const DISK_WARN: u64 = 1024 * 1024 * 1024;

/// How long to wait for the system bus to answer before giving up and treating
/// the answer as unknown. A hung UPower must not hold up a backup.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// What preflight decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Run the backup.
    Go,
    /// Do not run, for this reason.
    Skip(Skip),
}

/// Why a scheduled backup did not start. Every variant is a normal state of
/// affairs; none of them is an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    /// The user paused backups.
    Paused,
    /// On battery, and the configuration says not to.
    OnBattery,
    /// The connection is metered, the destination is across it, and the
    /// configuration says not to.
    Metered,
    /// The destination did not answer. Stage 5 turns this into local
    /// protection; today it is recorded and reported.
    Unreachable,
    /// Not enough room on this computer for the catalogue to grow.
    LocalDiskLow { free: u64 },
}

impl Skip {
    /// A stable token for logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            Skip::Paused => "paused",
            Skip::OnBattery => "on-battery",
            Skip::Metered => "metered-connection",
            Skip::Unreachable => "destination-unreachable",
            Skip::LocalDiskLow { .. } => "local-disk-low",
        }
    }

    /// A sentence for the log, in the same plain register as the user-facing
    /// copy — these lines are read by people diagnosing "why didn't it run?".
    pub fn explain(&self) -> String {
        match self {
            Skip::Paused => "backups are paused".into(),
            Skip::OnBattery => "running on battery, and backups on battery are switched off".into(),
            Skip::Metered => {
                "the connection is metered, and backups over metered connections are switched off"
                    .into()
            }
            Skip::Unreachable => "the backup destination is not reachable".into(),
            Skip::LocalDiskLow { free } => {
                format!("only {} MB free on this computer", free / (1024 * 1024))
            }
        }
    }
}

/// Everything the gates are decided from.
///
/// The three-valued fields (`Option<bool>`) are the point: `None` means nothing
/// on this system could answer, which is different from `Some(false)` and must
/// never be mistaken for it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    pub paused: bool,
    pub on_battery: Option<bool>,
    pub allow_on_battery: bool,
    pub metered: Option<bool>,
    pub allow_on_metered: bool,
    /// Whether reaching the destination costs network traffic. A local disk over
    /// a metered phone tether is nobody's business but the disk's.
    pub destination_is_remote: bool,
    /// Whether the destination answered when we looked; `None` when nobody
    /// could say. Supplied by [`crate::reachability`], which owns the question.
    pub destination_reachable: Option<bool>,
    /// Free bytes where the catalogue lives; `None` when the filesystem would
    /// not say.
    pub free_local_bytes: Option<u64>,
}

/// Run the gates, in the order the stage plan sets out.
///
/// Order is not arbitrary. The cheapest and most-certain conditions come first,
/// so the log line explaining a skip names the reason a person would name.
pub fn evaluate(facts: &Facts) -> Verdict {
    if facts.paused {
        return Verdict::Skip(Skip::Paused);
    }

    // `== Some(true)` throughout, never `!= Some(false)`: an absent UPower must
    // open the gate, not close it.
    if facts.on_battery == Some(true) && !facts.allow_on_battery {
        return Verdict::Skip(Skip::OnBattery);
    }

    if facts.destination_is_remote && facts.metered == Some(true) && !facts.allow_on_metered {
        return Verdict::Skip(Skip::Metered);
    }

    if facts.destination_reachable == Some(false) {
        return Verdict::Skip(Skip::Unreachable);
    }

    if let Some(free) = facts.free_local_bytes {
        if free < DISK_FLOOR {
            return Verdict::Skip(Skip::LocalDiskLow { free });
        }
    }

    Verdict::Go
}

/// Whether the local disk is low enough to be worth mentioning, without being
/// low enough to stop a backup. Feeds the health model's `needs_attention`.
pub fn local_disk_needs_attention(facts: &Facts) -> bool {
    facts.free_local_bytes.is_some_and(|free| free < DISK_WARN)
}

/// What the machine can be asked about itself.
///
/// A trait because the answers come from two system services that are absent in
/// a CI container and awkward to arrange on a desktop: the tests supply the
/// world instead of arranging it.
#[async_trait]
pub trait SystemProbe: Send + Sync {
    /// Whether the machine is on battery; `None` when nothing can say.
    async fn on_battery(&self) -> Option<bool>;
    /// Whether the active connection is metered; `None` when nothing can say.
    async fn metered(&self) -> Option<bool>;
}

/// The probe used when the system bus is unavailable — in a container, in a
/// minimal session, or on a system without UPower. Everything is unknown, so
/// every gate opens.
pub struct UnknownProbe;

#[async_trait]
impl SystemProbe for UnknownProbe {
    async fn on_battery(&self) -> Option<bool> {
        None
    }
    async fn metered(&self) -> Option<bool> {
        None
    }
}

/// The real probe: UPower and NetworkManager, over the system bus.
///
/// Properties are read with a plain `org.freedesktop.DBus.Properties.Get` rather
/// than through a caching proxy. Preflight runs at most once per backup, so
/// there is nothing to save by caching, and a cache is one more thing that can
/// be stale at exactly the wrong moment — reporting "on mains" to a laptop that
/// was unplugged two minutes ago.
pub struct DbusProbe {
    connection: zbus::Connection,
}

impl DbusProbe {
    /// Connect to the system bus. Returns `None` — with a log line, not an
    /// error — when there is no system bus to connect to, which is the normal
    /// state of affairs inside a build container.
    pub async fn connect() -> Option<DbusProbe> {
        match zbus::Connection::system().await {
            Ok(connection) => Some(DbusProbe { connection }),
            Err(e) => {
                info!("no system bus; battery and metered checks are unavailable: {e}");
                None
            }
        }
    }

    /// The system-bus connection, for the change watchers — one connection is
    /// enough, and opening a second would double the daemon's bus presence for
    /// nothing.
    pub fn connection(&self) -> &zbus::Connection {
        &self.connection
    }

    /// Read one property, or `None` if anything at all goes wrong. Every failure
    /// mode here — service absent, property renamed, bus hung — means the same
    /// thing to a caller: nobody could say.
    async fn property(
        &self,
        service: &str,
        path: &str,
        interface: &str,
        name: &str,
    ) -> Option<OwnedValue> {
        let args = (interface, name);
        let call = self.connection.call_method(
            Some(service),
            path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &args,
        );
        let reply = match tokio::time::timeout(PROBE_TIMEOUT, call).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(e)) => {
                debug!(service, name, "property unavailable: {e}");
                return None;
            }
            Err(_) => {
                warn!(service, name, "the system bus did not answer in time");
                return None;
            }
        };
        reply.body().deserialize::<OwnedValue>().ok()
    }
}

#[async_trait]
impl SystemProbe for DbusProbe {
    async fn on_battery(&self) -> Option<bool> {
        let value = self
            .property(
                "org.freedesktop.UPower",
                "/org/freedesktop/UPower",
                "org.freedesktop.UPower",
                "OnBattery",
            )
            .await?;
        bool::try_from(value).ok()
    }

    async fn metered(&self) -> Option<bool> {
        let value = self
            .property(
                "org.freedesktop.NetworkManager",
                "/org/freedesktop/NetworkManager",
                "org.freedesktop.NetworkManager",
                "Metered",
            )
            .await?;
        u32::try_from(value).ok().and_then(metered_from_nm)
    }
}

/// NetworkManager's `NMMetered` enum, which is four-valued for a good reason:
/// it distinguishes what the connection *says* it is from what NetworkManager
/// guesses. A guess is good enough to act on — the whole point of the setting is
/// to protect a data allowance, and being wrong costs one deferred backup.
///
/// `0` is UNKNOWN and stays unknown; treating it as "not metered" would let a
/// misconfigured connection quietly burn somebody's allowance.
fn metered_from_nm(value: u32) -> Option<bool> {
    match value {
        1 | 3 => Some(true),  // YES, GUESS_YES
        2 | 4 => Some(false), // NO, GUESS_NO
        _ => None,            // UNKNOWN, or something newer than this build
    }
}

/// Whether reaching this destination crosses the network.
///
/// Borg's remote syntax (`ssh://host/path`, `user@host:path`) is unambiguous. A
/// *mounted* share is not: `/mnt/nas/backups` looks exactly like a local
/// directory, so the mounted filesystem's type is what settles it.
pub fn destination_is_remote(repository: &str) -> bool {
    if repository.starts_with("ssh://") {
        return true;
    }
    // `user@host:path` — but not a Windows-style or relative path with a colon,
    // hence the requirement for an `@` before the first `/`.
    if let Some((prefix, _)) = repository.split_once(':') {
        if prefix.contains('@') && !prefix.contains('/') {
            return true;
        }
    }
    path_is_network_mounted(Path::new(repository))
}

/// Filesystem magic numbers for the network filesystems a destination is
/// plausibly mounted from. From `linux/magic.h`; rustix does not export them.
const NETWORK_FS_MAGIC: &[rustix::fs::FsWord] = &[
    0x6969,      // NFS_SUPER_MAGIC
    0xff53_4d42, // CIFS/SMB
    0xfe53_4d42, // SMB2
    0x5346_5342, // AFS
    0x7461_636f, // OCFS2
    0x0187,      // AUTOFS (a share not yet mounted)
];

/// Whether `path` — or, if it does not exist yet, its nearest existing
/// ancestor — sits on a network filesystem.
fn path_is_network_mounted(path: &Path) -> bool {
    let mut candidate = path;
    loop {
        match rustix::fs::statfs(candidate) {
            Ok(stat) => return NETWORK_FS_MAGIC.contains(&stat.f_type),
            Err(_) => match candidate.parent() {
                Some(parent) => candidate = parent,
                None => return false,
            },
        }
    }
}

/// Free bytes available to this user on the filesystem holding `path`.
///
/// `f_bavail`, not `f_bfree`: the difference is the root reserve, and counting
/// space the daemon cannot actually use would make the check a lie.
pub fn free_bytes(path: &Path) -> Option<u64> {
    let mut candidate = path;
    loop {
        match rustix::fs::statvfs(candidate) {
            Ok(stat) => return Some(stat.f_bavail.saturating_mul(stat.f_frsize)),
            Err(_) => candidate = candidate.parent()?,
        }
    }
}

/// Wake the scheduler when the machine's power or connection changes.
///
/// This is what turns "skipped: on battery" into a backup that runs the moment
/// the charger goes in, rather than at the top of the next hour. Failure to
/// subscribe is logged and otherwise ignored: without the watcher the schedule
/// still ticks, so the cost is latency, not correctness.
pub async fn watch_for_changes(
    connection: zbus::Connection,
    wake: Arc<tokio::sync::Notify>,
    network_changed: Arc<tokio::sync::Notify>,
) {
    // NetworkManager's changes are the ones that can make an unreachable
    // destination reachable, so they wake the reachability watcher as well as
    // the scheduler. A battery event cannot bring a NAS back, so UPower's do
    // not — probing the network every time a charger is plugged in would be
    // work for nothing.
    let watchers = [
        ("org.freedesktop.UPower", "/org/freedesktop/UPower", false),
        (
            "org.freedesktop.NetworkManager",
            "/org/freedesktop/NetworkManager",
            true,
        ),
    ];
    for (service, path, is_network) in watchers {
        let connection = connection.clone();
        let wake = Arc::clone(&wake);
        let network_changed = Arc::clone(&network_changed);
        tokio::spawn(async move {
            let proxy = match zbus::fdo::PropertiesProxy::builder(&connection)
                .destination(service)
                .and_then(|b| b.path(path))
                .map(|b| b.build())
            {
                Ok(builder) => match builder.await {
                    Ok(proxy) => proxy,
                    Err(e) => return debug!(service, "not watching properties: {e}"),
                },
                Err(e) => return debug!(service, "not watching properties: {e}"),
            };
            let mut changes = match proxy.receive_properties_changed().await {
                Ok(stream) => stream,
                Err(e) => return debug!(service, "not watching properties: {e}"),
            };
            debug!(service, "watching for power and connection changes");
            use futures::StreamExt;
            while changes.next().await.is_some() {
                // Which property changed does not matter: the scheduler
                // re-reads everything it needs anyway, and a spurious wake-up
                // costs one cheap re-evaluation.
                wake.notify_one();
                if is_network {
                    network_changed.notify_one();
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A machine that should back up: on mains, unmetered, disk reachable and
    /// roomy.
    fn ready() -> Facts {
        Facts {
            paused: false,
            on_battery: Some(false),
            allow_on_battery: false,
            metered: Some(false),
            allow_on_metered: false,
            destination_is_remote: false,
            destination_reachable: Some(true),
            free_local_bytes: Some(50 * 1024 * 1024 * 1024),
        }
    }

    #[test]
    fn a_ready_machine_backs_up() {
        assert_eq!(evaluate(&ready()), Verdict::Go);
    }

    #[test]
    fn a_pause_stops_a_scheduled_run() {
        let facts = Facts {
            paused: true,
            ..ready()
        };
        assert_eq!(evaluate(&facts), Verdict::Skip(Skip::Paused));
    }

    #[test]
    fn battery_stops_a_run_unless_the_user_allowed_it() {
        let on_battery = Facts {
            on_battery: Some(true),
            ..ready()
        };
        assert_eq!(evaluate(&on_battery), Verdict::Skip(Skip::OnBattery));

        let allowed = Facts {
            allow_on_battery: true,
            ..on_battery
        };
        assert_eq!(evaluate(&allowed), Verdict::Go);
    }

    #[test]
    fn an_absent_upower_does_not_stop_backups() {
        // The failure mode this guards against: a desktop with no UPower
        // quietly never backing up because a gate could not prove it was on
        // mains.
        let facts = Facts {
            on_battery: None,
            allow_on_battery: false,
            ..ready()
        };
        assert_eq!(evaluate(&facts), Verdict::Go);
    }

    #[test]
    fn metering_only_matters_for_a_destination_across_the_network() {
        // A USB disk does not care what the wifi costs.
        let local = Facts {
            metered: Some(true),
            destination_is_remote: false,
            ..ready()
        };
        assert_eq!(evaluate(&local), Verdict::Go);

        let remote = Facts {
            destination_is_remote: true,
            ..local
        };
        assert_eq!(evaluate(&remote), Verdict::Skip(Skip::Metered));

        let allowed = Facts {
            allow_on_metered: true,
            ..remote
        };
        assert_eq!(evaluate(&allowed), Verdict::Go);
    }

    #[test]
    fn an_absent_network_manager_does_not_stop_backups() {
        let facts = Facts {
            metered: None,
            destination_is_remote: true,
            ..ready()
        };
        assert_eq!(evaluate(&facts), Verdict::Go);
    }

    #[test]
    fn an_unreachable_destination_is_skipped_not_failed() {
        let facts = Facts {
            destination_reachable: Some(false),
            ..ready()
        };
        assert_eq!(evaluate(&facts), Verdict::Skip(Skip::Unreachable));
    }

    #[test]
    fn a_destination_that_cannot_be_probed_is_left_to_the_run_itself() {
        let facts = Facts {
            destination_reachable: None,
            ..ready()
        };
        assert_eq!(evaluate(&facts), Verdict::Go);
    }

    #[test]
    fn a_nearly_full_local_disk_defers_the_backup() {
        let facts = Facts {
            free_local_bytes: Some(10 * 1024 * 1024),
            ..ready()
        };
        assert_eq!(
            evaluate(&facts),
            Verdict::Skip(Skip::LocalDiskLow {
                free: 10 * 1024 * 1024
            })
        );
    }

    #[test]
    fn a_merely_low_disk_warns_without_stopping_anything() {
        // "Soft check": the catalogue growing is a reason to mention the disk,
        // not a reason to stop protecting the user's files.
        let facts = Facts {
            free_local_bytes: Some(512 * 1024 * 1024),
            ..ready()
        };
        assert_eq!(evaluate(&facts), Verdict::Go);
        assert!(local_disk_needs_attention(&facts));
        assert!(!local_disk_needs_attention(&ready()));
    }

    #[test]
    fn a_filesystem_that_will_not_answer_does_not_stop_backups() {
        let facts = Facts {
            free_local_bytes: None,
            ..ready()
        };
        assert_eq!(evaluate(&facts), Verdict::Go);
        assert!(!local_disk_needs_attention(&facts));
    }

    #[test]
    fn the_pause_gate_outranks_the_others() {
        // The log line should say "paused", because that is the answer a person
        // asking "why didn't it run?" is looking for.
        let facts = Facts {
            paused: true,
            on_battery: Some(true),
            destination_reachable: Some(false),
            ..ready()
        };
        assert_eq!(evaluate(&facts), Verdict::Skip(Skip::Paused));
    }

    #[test]
    fn network_manager_guesses_are_acted_on_but_unknown_is_not() {
        assert_eq!(metered_from_nm(1), Some(true), "YES");
        assert_eq!(metered_from_nm(2), Some(false), "NO");
        assert_eq!(metered_from_nm(3), Some(true), "GUESS_YES");
        assert_eq!(metered_from_nm(4), Some(false), "GUESS_NO");
        assert_eq!(metered_from_nm(0), None, "UNKNOWN stays unknown");
        assert_eq!(metered_from_nm(99), None, "a newer value is not guessed at");
    }

    #[test]
    fn borg_remote_syntax_is_recognised() {
        assert!(destination_is_remote("ssh://nas.local/./backups"));
        assert!(destination_is_remote("keith@nas.local:backups"));
    }

    #[test]
    fn a_local_path_is_not_remote() {
        assert!(!destination_is_remote("/mnt/usb/backups"));
        // A path containing a colon is still a path.
        assert!(!destination_is_remote("/mnt/odd:name/backups"));
    }

    #[test]
    fn free_space_is_reported_for_a_real_directory() {
        let dir = tempfile::tempdir().unwrap();
        let free = free_bytes(dir.path()).expect("a temporary directory has a filesystem");
        assert!(free > 0, "a writable temporary directory has some room");
    }

    #[test]
    fn free_space_falls_back_to_the_nearest_existing_ancestor() {
        // The data directory may not exist yet on a first run; the answer that
        // matters is the filesystem it will be created on.
        let dir = tempfile::tempdir().unwrap();
        let unborn = dir.path().join("not").join("created").join("yet");
        assert!(free_bytes(&unborn).is_some());
    }

    #[tokio::test]
    async fn the_unknown_probe_answers_nothing() {
        let probe = UnknownProbe;
        assert_eq!(probe.on_battery().await, None);
        assert_eq!(probe.metered().await, None);
    }
}
