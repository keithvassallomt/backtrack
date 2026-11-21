// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Is the backup destination there?
//!
//! This is the question the whole of Stage 5 hangs off: a destination that does
//! not answer is not a failure, it is the moment local protection takes over.
//! So the answer has to be cheap enough to ask often, honest about not knowing,
//! and — above all — **steady**. A status line that alternates between "backed
//! up" and "protecting locally" every few seconds as a flaky wifi link comes and
//! goes is worse than either answer on its own.
//!
//! Three pieces, deliberately separate so each can be tested on its own:
//!
//! - [`DestinationProbe`] — one look at the destination. A trait, because the
//!   real one touches the filesystem and the network, and the interesting
//!   behaviour is what happens across a *sequence* of answers.
//! - [`ReachabilityTracker`] — the smoothing. A pure state machine over
//!   (observation, time), so a flapping link is a list of arguments in a test
//!   rather than a wifi router someone has to unplug.
//! - [`watch`] — the loop that joins them to the daemon, and re-probes when
//!   NetworkManager says something changed.
//!
//! **Uncertainty is never reported as absence.** Every probe can answer
//! [`Reach::Unknown`], and an unknown never moves the state. Saying "your backup
//! drive is gone" because a probe timed out would send the product into local
//! protection over nothing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use backtrack_core::engine::BackupEngine;
use tracing::{debug, info, warn};

/// The longest a single probe may take before the answer is "cannot tell".
///
/// The stage plan's budget. It matters most for a mounted share whose server
/// has gone away: a `stat` on a hung NFS mount can block for minutes, and the
/// daemon must not.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a Borg round-trip to a remote repository may take when confirming a
/// reconnection. Deliberately more generous than [`PROBE_TIMEOUT`]: this runs
/// only on the edge where a TCP connection has just started succeeding again,
/// at most once per reconnection, and spawning Borg over a fresh SSH link is
/// legitimately slower than a socket connect.
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(15);

/// The shortest gap between two reported state changes.
///
/// Hysteresis, not caching. An observation that disagrees with the current
/// state inside this window is held rather than acted on, and the loop looks
/// again once the window passes — so the state always converges on the truth,
/// it just refuses to be dragged back and forth by a link that is flapping.
pub const MIN_TRANSITION_GAP: Duration = Duration::from_secs(60);

/// How often to look when nothing has prompted us to.
const POLL_INTERVAL: Duration = Duration::from_secs(60);

/// How long to let a burst of network-change signals settle before probing.
///
/// A wifi association, a VPN coming up, or a dock being plugged in emits a
/// flurry of `PropertiesChanged` in about a second, and probing on the first of
/// them asks the question before the network can answer it.
const NETWORK_SETTLE: Duration = Duration::from_secs(2);

/// What one look at the destination found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// It answered.
    Yes,
    /// It is definitely not there — an absent path, a refused connection.
    No,
    /// Nothing here could say: a probe that timed out, a destination with
    /// nothing cheap to ask, a repository that is not configured yet.
    Unknown,
}

impl Reach {
    /// The answer as a two-valued one, or `None` when nobody could say.
    pub fn definite(self) -> Option<bool> {
        match self {
            Reach::Yes => Some(true),
            Reach::No => Some(false),
            Reach::Unknown => None,
        }
    }

    fn from(answered: bool) -> Reach {
        if answered {
            Reach::Yes
        } else {
            Reach::No
        }
    }
}

/// One look at the destination.
// See backtrack-core's `BackupEngine`: async-trait's `#[must_use]`, and clippy 1.99.
#[allow(clippy::double_must_use)]
#[async_trait]
pub trait DestinationProbe: Send + Sync {
    /// Whether `repository` answers right now.
    ///
    /// `engine` is offered for destinations that cannot be probed without
    /// speaking Borg; it is `None` before a repository is configured, and a
    /// probe that needs it must answer [`Reach::Unknown`] rather than guess.
    async fn probe(&self, repository: &str, engine: Option<Arc<dyn BackupEngine>>) -> Reach;
}

/// The real probe.
#[derive(Default)]
pub struct RealProbe {
    /// The server behind each repository on an automounted network share, as
    /// last seen while the share was mounted (S15-T10).
    servers: Mutex<HashMap<String, Server>>,
}

#[async_trait]
impl DestinationProbe for RealProbe {
    async fn probe(&self, repository: &str, engine: Option<Arc<dyn BackupEngine>>) -> Reach {
        if repository.is_empty() {
            return Reach::Unknown;
        }
        match backtrack_core::destination::ssh_endpoint(repository) {
            Some((host, port)) => probe_remote(&host, port, engine).await,
            None => {
                // Reading the mount table touches no mount, so it cannot wake
                // an automount the way looking at the path does.
                let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
                self.probe_path(repository, &mountinfo).await
            }
        }
    }
}

impl RealProbe {
    /// A repository on this computer's filesystem, given its mount table.
    ///
    /// Every look at a path under an automount point counts as using the
    /// share, so a probe once a minute keeps one set to unmount after a few
    /// idle minutes mounted for as long as the daemon runs. For a network
    /// share there, the server is asked instead, as an `ssh://` destination's
    /// is: the share is reachable when its server is.
    ///
    /// The server is read from the share's own mount while it is mounted,
    /// and remembered for when it is not. Until the share has been seen
    /// mounted, and for an automounted disk, the path is looked at as for any
    /// other folder. The backup that follows mounts the share through its
    /// path, as it always has.
    async fn probe_path(&self, repository: &str, mountinfo: &str) -> Reach {
        let server = match mounted(mountinfo, Path::new(repository)) {
            Mounted::Share(server) => {
                let mut servers = self.servers.lock().unwrap();
                servers.insert(repository.to_string(), server.clone());
                Some(server)
            }
            Mounted::Idle => self.servers.lock().unwrap().get(repository).cloned(),
            Mounted::Plain | Mounted::Other => None,
        };
        match server {
            Some(server) => connect(&server.host, server.port).await,
            None => probe_local(repository).await,
        }
    }
}

/// The machine a network share is served from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Server {
    host: String,
    port: u16,
}

/// What holds a repository's path, as far as the probe is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Mounted {
    /// No automount point above it: a folder, a disk, an ordinary mount.
    Plain,
    /// Under an automount point with nothing mounted there now.
    Idle,
    /// Under an automount point, on a network share from this server.
    Share(Server),
    /// Under an automount point, on something other than a network share
    /// the probe can reach: a disk, or a kind of share it does not know.
    Other,
}

/// Where `repository` lies, read from `/proc/self/mountinfo` text.
fn mounted(mountinfo: &str, repository: &Path) -> Mounted {
    let mounts: Vec<Mount> = mountinfo.lines().filter_map(Mount::parse).collect();
    let depth = |m: &Mount| m.point.components().count();
    let Some(automount) = mounts
        .iter()
        .filter(|m| m.fstype == "autofs" && repository.starts_with(&m.point))
        .max_by_key(|m| depth(m))
    else {
        return Mounted::Plain;
    };
    // The filesystem the repository is on: the deepest mount over it at or
    // below the automount point. One mounted over another at the same place
    // comes later in the table, so of two equally deep, the later wins. A
    // map such as autofs's /net mounts each share below its point, not on it.
    let on = mounts
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            m.fstype != "autofs"
                && repository.starts_with(&m.point)
                && m.point.starts_with(&automount.point)
        })
        .max_by_key(|(order, m)| (depth(m), *order));
    match on {
        None => Mounted::Idle,
        Some((_, mount)) => mount.server().map_or(Mounted::Other, Mounted::Share),
    }
}

/// One line of `/proc/self/mountinfo`.
struct Mount {
    point: PathBuf,
    fstype: String,
    source: String,
    options: String,
}

impl Mount {
    /// The fields are: id, parent, device, root, mount point, options,
    /// optional fields, `-`, type, source, superblock options.
    fn parse(line: &str) -> Option<Mount> {
        let (before, after) = line.split_once(" - ")?;
        let point = before.split(' ').nth(4)?;
        let mut after = after.split(' ');
        let fstype = after.next()?;
        let source = after.next()?;
        let options = after.next().unwrap_or("");
        Some(Mount {
            point: PathBuf::from(unescape(point)),
            fstype: fstype.to_string(),
            source: unescape(source),
            options: options.to_string(),
        })
    }

    /// The server, for the network shares the probe knows how to reach.
    fn server(&self) -> Option<Server> {
        let (host, default_port) = match self.fstype.as_str() {
            // //server/share
            "cifs" | "smb3" => (self.source.strip_prefix("//")?.split('/').next()?, 445),
            // server:/export, or [address]:/export
            "nfs" | "nfs4" => match self.source.strip_prefix('[') {
                Some(bracketed) => (bracketed.split_once(']')?.0, 2049),
                None => (self.source.split_once(':')?.0, 2049),
            },
            _ => return None,
        };
        if host.is_empty() {
            return None;
        }
        // A share mounted from another port says so among its options.
        let port = self
            .options
            .split(',')
            .find_map(|option| option.strip_prefix("port=")?.parse().ok())
            .filter(|&port| port != 0)
            .unwrap_or(default_port);
        Some(Server {
            host: host.to_string(),
            port,
        })
    }
}

/// Undo `mountinfo`'s octal escapes: `\040` for a space, and the like.
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'\\')
            .then(|| field.get(i + 1..i + 4))
            .flatten()
            .and_then(|digits| u8::from_str_radix(digits, 8).ok());
        match escaped {
            Some(byte) => {
                out.push(byte);
                i += 4;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A local path or a mounted share: it is there, or it is not.
///
/// Only *there* is asked, not whether it can be written to. A share that came
/// back read-only, or a folder whose permissions changed, is not an absent
/// destination for the local safety net to cover quietly: it is health.md's
/// "can't sign in" failure, which stops backups until somebody acts, and the
/// person has to hear about it. Reporting it as reachable lets the backup run
/// and fail with exactly that. A folder that cannot even be looked into is the
/// same case, and is there too.
///
/// Nothing is written to find out. Touching the repository once a minute to
/// prove we can would be a strange thing to do to somebody's backups.
///
/// Run on a blocking thread under a timeout: a `stat` on a mount whose server
/// has vanished does not return promptly, and on some it does not return at
/// all. The timeout abandons the *wait*, not the thread — a hung syscall keeps
/// its thread until the kernel gives up on the mount. That is a bounded leak on
/// a pool built for exactly this, and much the better trade against a daemon
/// that stops answering.
async fn probe_local(repository: &str) -> Reach {
    let path = std::path::PathBuf::from(repository);
    let look = tokio::task::spawn_blocking(move || {
        match rustix::fs::access(&path, rustix::fs::Access::EXISTS) {
            Ok(()) => true,
            Err(e) => e == rustix::io::Errno::ACCESS || e == rustix::io::Errno::PERM,
        }
    });
    match tokio::time::timeout(PROBE_TIMEOUT, look).await {
        Ok(Ok(answered)) => Reach::from(answered),
        Ok(Err(e)) => {
            warn!("could not look at the destination: {e}");
            Reach::Unknown
        }
        Err(_) => {
            // Almost always a hung mount. "Cannot tell" is the honest answer:
            // the share may well still be there, just not answering.
            debug!(
                seconds = PROBE_TIMEOUT.as_secs(),
                "the destination did not answer in time"
            );
            Reach::Unknown
        }
    }
}

/// An `ssh://` destination: a TCP connection first, then Borg.
///
/// The connection is the cheap half and carries the common answer — away from
/// the network, the connect fails in milliseconds and no subprocess is spawned.
/// Only once it succeeds is Borg asked, which is what distinguishes "the host is
/// up" from "the repository is actually usable": a laptop on a café network can
/// reach *a* host on that port without it being the NAS at home.
///
/// A Borg call that fails is reported as unreachable only when Borg itself says
/// so. Anything else — a wrong passphrase, a corrupt repository — means the
/// destination answered, and those are health failures with their own banners,
/// not reasons to start protecting locally.
async fn probe_remote(host: &str, port: u16, engine: Option<Arc<dyn BackupEngine>>) -> Reach {
    let connected = connect(host, port).await;
    if connected != Reach::Yes {
        return connected;
    }

    let Some(engine) = engine else {
        // The host is up and there is nothing else we can cheaply ask.
        return Reach::Yes;
    };
    match tokio::time::timeout(CONFIRM_TIMEOUT, engine.repo_info()).await {
        Ok(Ok(_)) => Reach::Yes,
        Ok(Err(backtrack_core::engine::EngineError::RepoUnreachable)) => Reach::No,
        Ok(Err(e)) => {
            // It answered, and had something else to complain about. That is a
            // health failure, not an offline destination.
            debug!("the destination answered but the repository is not usable: {e}");
            Reach::Yes
        }
        Err(_) => Reach::Unknown,
    }
}

/// Whether `host` accepts a TCP connection on `port`.
async fn connect(host: &str, port: u16) -> Reach {
    let connect = tokio::net::TcpStream::connect((host, port));
    match tokio::time::timeout(PROBE_TIMEOUT, connect).await {
        Ok(Ok(_)) => Reach::Yes,
        Ok(Err(e)) => {
            debug!(host, port, "destination did not accept a connection: {e}");
            Reach::No
        }
        Err(_) => {
            debug!(host, port, "destination did not answer in time");
            Reach::Unknown
        }
    }
}

/// What observing a probe result did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observed {
    /// Nothing to report: the observation agrees with the state, or nobody
    /// could say.
    Unchanged,
    /// The observation disagrees, but the last change was too recent to act on
    /// it. Look again after `until`.
    Held { until: SystemTime },
    /// The state changed.
    Changed { reachable: bool },
}

/// Smooths a sequence of probe results into a state that does not thrash.
#[derive(Debug)]
pub struct ReachabilityTracker {
    reachable: bool,
    last_transition: Option<SystemTime>,
}

impl ReachabilityTracker {
    /// Start from what the daemon currently believes.
    ///
    /// No `last_transition`, so the first disagreement is acted on at once:
    /// a machine that starts up away from its backup drive should say so
    /// immediately, not a minute later.
    pub fn new(reachable: bool) -> ReachabilityTracker {
        ReachabilityTracker {
            reachable,
            last_transition: None,
        }
    }

    /// What the daemon currently believes.
    pub fn reachable(&self) -> bool {
        self.reachable
    }

    /// Fold one probe result in.
    pub fn observe(&mut self, reach: Reach, now: SystemTime) -> Observed {
        // Uncertainty never moves the state. A timed-out probe is not evidence
        // that a drive was unplugged.
        let Some(observed) = reach.definite() else {
            return Observed::Unchanged;
        };
        if observed == self.reachable {
            return Observed::Unchanged;
        }
        if let Some(last) = self.last_transition {
            let ready = last + MIN_TRANSITION_GAP;
            if now < ready {
                return Observed::Held { until: ready };
            }
        }
        self.reachable = observed;
        self.last_transition = Some(now);
        Observed::Changed {
            reachable: observed,
        }
    }
}

/// Watch the destination for the daemon's lifetime.
///
/// Wakes on three things: its own poll interval, a NetworkManager change (after
/// a short settling delay), and the expiry of a held observation. Every path
/// ends in the same place — one probe, one [`ReachabilityTracker::observe`], and
/// a state change reported at most as often as the hysteresis allows.
pub async fn watch(shared: Arc<crate::service::Shared>, network_changed: Arc<tokio::sync::Notify>) {
    let mut tracker = ReachabilityTracker::new(shared.destination_reachable());
    info!("watching the backup destination");
    loop {
        let mut next = POLL_INTERVAL;
        let reach = shared.probe_destination().await;
        match tracker.observe(reach, SystemTime::now()) {
            Observed::Changed { reachable } => shared.destination_changed(reachable).await,
            Observed::Held { until } => {
                // Look again the moment the window closes, so a link that
                // settled the other way is picked up promptly rather than
                // waiting out a whole poll interval.
                next = until
                    .duration_since(SystemTime::now())
                    .unwrap_or(Duration::from_secs(1))
                    .max(Duration::from_secs(1));
                debug!(
                    seconds = next.as_secs(),
                    believed = tracker.reachable(),
                    "the destination disagrees with the current state; holding off"
                );
            }
            Observed::Unchanged => {}
        }

        tokio::select! {
            _ = tokio::time::sleep(next) => {}
            _ = network_changed.notified() => {
                // Let the burst finish before asking. Further signals arriving
                // during the wait are coalesced by the next iteration rather
                // than each costing a probe.
                tokio::time::sleep(NETWORK_SETTLE).await;
                debug!("the network changed; looking at the destination again");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_000_000)
    }

    #[test]
    fn an_agreeing_observation_changes_nothing() {
        let mut t = ReachabilityTracker::new(true);
        assert_eq!(t.observe(Reach::Yes, now()), Observed::Unchanged);
        assert!(t.reachable());
    }

    #[test]
    fn the_first_disagreement_is_acted_on_immediately() {
        // A laptop that boots away from its backup drive must say so at once,
        // not after a minute of claiming to be protected.
        let mut t = ReachabilityTracker::new(true);
        assert_eq!(
            t.observe(Reach::No, now()),
            Observed::Changed { reachable: false }
        );
        assert!(!t.reachable());
    }

    #[test]
    fn uncertainty_never_reports_the_destination_as_gone() {
        // The failure this prevents: a `stat` that timed out on a busy NAS
        // sending the product into local protection and the status line into
        // "your backup drive isn't reachable".
        let mut t = ReachabilityTracker::new(true);
        for _ in 0..10 {
            assert_eq!(t.observe(Reach::Unknown, now()), Observed::Unchanged);
        }
        assert!(t.reachable(), "an unknown is not a no");
    }

    #[test]
    fn a_flapping_link_does_not_thrash_the_state() {
        // The acceptance criterion. Wifi dropping and returning every few
        // seconds must not produce a state change every few seconds.
        let mut t = ReachabilityTracker::new(true);
        let mut clock = now();

        // The first drop is real and is reported.
        assert_eq!(
            t.observe(Reach::No, clock),
            Observed::Changed { reachable: false }
        );

        // Then it flaps for most of a minute. Every one of these disagrees with
        // the state at the time, and none of them may be acted on.
        let mut changes = 0;
        for step in 1..12 {
            clock = now() + Duration::from_secs(step * 5);
            let reach = if step % 2 == 0 { Reach::Yes } else { Reach::No };
            match t.observe(reach, clock) {
                Observed::Changed { .. } => changes += 1,
                Observed::Held { until } => {
                    assert_eq!(until, now() + MIN_TRANSITION_GAP);
                }
                Observed::Unchanged => {}
            }
        }
        assert_eq!(
            changes, 0,
            "no transition may be reported inside the hysteresis window"
        );
    }

    #[test]
    fn the_state_still_converges_on_the_truth_after_the_window() {
        // Hysteresis must delay a transition, never swallow it — otherwise a
        // link that dropped and stayed down would be reported as fine forever.
        let mut t = ReachabilityTracker::new(true);
        let mut clock = now();
        t.observe(Reach::No, clock);

        clock += Duration::from_secs(10);
        assert!(matches!(
            t.observe(Reach::Yes, clock),
            Observed::Held { .. }
        ));

        clock = now() + MIN_TRANSITION_GAP;
        assert_eq!(
            t.observe(Reach::Yes, clock),
            Observed::Changed { reachable: true },
            "once the window passes, the current truth is adopted"
        );
        assert!(t.reachable());
    }

    #[test]
    fn a_held_observation_that_went_away_is_not_applied_later() {
        // The other half of converging: if the link recovered and then dropped
        // again before the window closed, the state is already correct and
        // nothing is reported.
        let mut t = ReachabilityTracker::new(true);
        let mut clock = now();
        t.observe(Reach::No, clock);

        clock += Duration::from_secs(10);
        assert!(matches!(
            t.observe(Reach::Yes, clock),
            Observed::Held { .. }
        ));

        clock = now() + MIN_TRANSITION_GAP + Duration::from_secs(1);
        assert_eq!(
            t.observe(Reach::No, clock),
            Observed::Unchanged,
            "the state already says unreachable; there is nothing to change"
        );
        assert!(!t.reachable());
    }

    #[test]
    fn transitions_are_at_least_a_minute_apart() {
        // Stated as the property rather than as a scenario: whatever sequence
        // of observations arrives, two reported changes are never closer than
        // the hysteresis window.
        let mut t = ReachabilityTracker::new(true);
        let mut last_change: Option<SystemTime> = None;
        for step in 0..600u64 {
            let clock = now() + Duration::from_secs(step);
            // A pathological source: alternating every single second.
            let reach = if step % 2 == 0 { Reach::No } else { Reach::Yes };
            if let Observed::Changed { .. } = t.observe(reach, clock) {
                if let Some(previous) = last_change {
                    let gap = clock.duration_since(previous).unwrap();
                    assert!(
                        gap >= MIN_TRANSITION_GAP,
                        "two changes {gap:?} apart, which is inside the window"
                    );
                }
                last_change = Some(clock);
            }
        }
    }

    #[tokio::test]
    async fn a_local_destination_is_probed_by_looking_at_it() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        assert_eq!(
            RealProbe::default()
                .probe(repo.to_str().unwrap(), None)
                .await,
            Reach::No,
            "an unplugged drive presents as an absent path"
        );
        std::fs::create_dir(&repo).unwrap();
        assert_eq!(
            RealProbe::default()
                .probe(repo.to_str().unwrap(), None)
                .await,
            Reach::Yes
        );
    }

    #[tokio::test]
    async fn a_read_only_destination_is_there_and_left_to_fail_loudly() {
        // "The NAS came back, but read-only" is a real failure mode. It used to
        // read as unreachable, which handed it to the local safety net and
        // said nothing; it is health.md's "can't sign in" row, and the backup
        // that runs because the destination is there is what reports it.
        //
        // Root is exempt from permission checks, so on a machine running as
        // root — which is how CI's container runs — a read-only directory is
        // still writable and there is nothing here to observe.
        if rustix::process::geteuid().is_root() {
            eprintln!("skipping: running as root, which bypasses permission checks");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let mut perms = std::fs::metadata(&repo).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o500);
        std::fs::set_permissions(&repo, perms).unwrap();

        let reach = RealProbe::default()
            .probe(repo.to_str().unwrap(), None)
            .await;

        // Restore before asserting, so a failure does not leave an undeletable
        // temporary directory behind.
        let mut perms = std::fs::metadata(&repo).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o700);
        std::fs::set_permissions(&repo, perms).unwrap();

        assert_eq!(reach, Reach::Yes);
    }

    #[tokio::test]
    async fn a_destination_that_cannot_even_be_looked_into_is_there() {
        if rustix::process::geteuid().is_root() {
            eprintln!("skipping: running as root, which bypasses permission checks");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let share = dir.path().join("share");
        let repo = share.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let mut perms = std::fs::metadata(&share).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o000);
        std::fs::set_permissions(&share, perms).unwrap();

        let reach = RealProbe::default()
            .probe(repo.to_str().unwrap(), None)
            .await;

        let mut perms = std::fs::metadata(&share).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o700);
        std::fs::set_permissions(&share, perms).unwrap();

        assert_eq!(reach, Reach::Yes, "refused is not the same as absent");
    }

    #[tokio::test]
    async fn an_unconfigured_destination_is_unknown_rather_than_absent() {
        assert_eq!(RealProbe::default().probe("", None).await, Reach::Unknown);
    }

    // Taken from a laptop with a systemd automount for an SMB share and one
    // for an encrypted disk, with the user and address replaced.
    const ROOT: &str =
        "26 1 0:25 /@ / rw,noatime shared:1 - btrfs /dev/mapper/root rw,compress=zstd:3";
    const SHARE_AUTOMOUNT: &str = "141 44 0:48 / /mnt/truenas-backups rw,relatime shared:54 - autofs systemd-1 rw,fd=81,pgrp=1,timeout=900,minproto=5,maxproto=5,direct,pipe_ino=12616";
    const SHARE: &str = "733 141 0:130 / /mnt/truenas-backups rw,nosuid,nodev,noexec,relatime shared:699 - cifs //192.0.2.10/Backup rw,vers=3.1.1,cache=strict,username=someone,uid=1000,forceuid,gid=1000,forcegid,addr=192.0.2.10,file_mode=0600,dir_mode=0700,soft,nounix,serverino,mapposix,rsize=4194304,wsize=4194304,bsize=1048576,echo_interval=60,actimeo=1,closetimeo=1";
    const DISK_AUTOMOUNT: &str = "56 44 0:43 / /mnt/tank rw,relatime shared:24 - autofs systemd-1 rw,fd=76,pgrp=1,timeout=0,minproto=5,maxproto=5,direct,pipe_ino=12611";
    const DISK: &str = "159 56 253:1 / /mnt/tank rw,noatime shared:722 - ext4 /dev/mapper/tank rw";

    fn table(lines: &[&str]) -> String {
        lines.join("\n")
    }

    fn server(host: &str, port: u16) -> Mounted {
        Mounted::Share(Server {
            host: host.into(),
            port,
        })
    }

    #[test]
    fn an_automounted_smb_share_is_known_by_its_server() {
        let mounts = table(&[ROOT, SHARE_AUTOMOUNT, SHARE]);
        assert_eq!(
            mounted(&mounts, Path::new("/mnt/truenas-backups/borg")),
            server("192.0.2.10", 445)
        );
    }

    #[test]
    fn an_automount_with_nothing_mounted_is_idle() {
        let mounts = table(&[ROOT, SHARE_AUTOMOUNT]);
        assert_eq!(
            mounted(&mounts, Path::new("/mnt/truenas-backups/borg")),
            Mounted::Idle
        );
    }

    #[test]
    fn an_automounted_disk_and_an_ordinary_folder_are_looked_at_by_path() {
        let mounts = table(&[ROOT, DISK_AUTOMOUNT, DISK, SHARE_AUTOMOUNT, SHARE]);
        assert_eq!(
            mounted(&mounts, Path::new("/mnt/tank/borg")),
            Mounted::Other
        );
        assert_eq!(
            mounted(&mounts, Path::new("/home/someone/borg")),
            Mounted::Plain
        );
        // A neighbour whose name only begins the same is not under it.
        assert_eq!(
            mounted(&mounts, Path::new("/mnt/truenas-backups-old/borg")),
            Mounted::Plain
        );
    }

    #[test]
    fn nfs_shares_and_autofs_maps_are_known_by_their_servers_too() {
        // autofs's /net map mounts each export below its own point.
        let mounts = table(&[
            ROOT,
            "90 26 0:60 / /net rw,relatime shared:80 - autofs /etc/auto.net rw,fd=7,pgrp=900,timeout=300,minproto=5,maxproto=5,indirect",
            "95 90 0:61 / /net/nas/backups rw,relatime shared:81 - nfs4 nas:/backups rw,vers=4.2,addr=192.0.2.20",
            "96 90 0:62 / /net/old/backups rw,relatime shared:82 - nfs [2001:db8::5]:/backups rw,vers=3,port=2050",
        ]);
        assert_eq!(
            mounted(&mounts, Path::new("/net/nas/backups/borg")),
            server("nas", 2049)
        );
        assert_eq!(
            mounted(&mounts, Path::new("/net/old/backups/borg")),
            server("2001:db8::5", 2050),
            "an address in brackets, and a port of its own"
        );
        assert_eq!(
            mounted(&mounts, Path::new("/net/elsewhere/borg")),
            Mounted::Idle
        );
    }

    #[test]
    fn escaped_spaces_in_a_mount_point_are_read_as_spaces() {
        let mounts = table(&[
            ROOT,
            r"141 44 0:48 / /mnt/my\040backups rw,relatime shared:54 - autofs systemd-1 rw,direct",
            r"733 141 0:130 / /mnt/my\040backups rw,relatime shared:699 - smb3 //nas/My\040Share rw",
        ]);
        assert_eq!(
            mounted(&mounts, Path::new("/mnt/my backups/borg")),
            server("nas", 445)
        );
    }

    /// A share on a server that listens on this machine, at `port`.
    fn share_at(port: u16) -> String {
        table(&[
            ROOT,
            SHARE_AUTOMOUNT,
            &format!("733 141 0:130 / /mnt/truenas-backups rw,relatime shared:699 - cifs //127.0.0.1/Backup rw,port={port}"),
        ])
    }

    #[tokio::test]
    async fn an_automounted_share_is_checked_at_its_server_not_its_path() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let probe = RealProbe::default();
        // Nothing is at this path, so a look at it would answer No.
        let repository = "/mnt/truenas-backups/borg";

        assert_eq!(
            probe.probe_path(repository, &share_at(port)).await,
            Reach::Yes,
            "the server answers"
        );
        assert_eq!(
            probe
                .probe_path(repository, &table(&[ROOT, SHARE_AUTOMOUNT]))
                .await,
            Reach::Yes,
            "the share has gone idle, and its server is still asked"
        );

        drop(listener);
        assert_eq!(
            probe
                .probe_path(repository, &table(&[ROOT, SHARE_AUTOMOUNT]))
                .await,
            Reach::No,
            "a server that refuses the connection is not there"
        );
    }

    #[tokio::test]
    async fn an_idle_share_never_seen_mounted_is_looked_at_by_path() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let point = dir.path().display().to_string();
        let mounts = table(&[
            ROOT,
            &format!("141 44 0:48 / {point} rw,relatime shared:54 - autofs systemd-1 rw,direct"),
        ]);
        let probe = RealProbe::default();
        let repository = repo.to_str().unwrap();

        assert_eq!(probe.probe_path(repository, &mounts).await, Reach::No);
        std::fs::create_dir(&repo).unwrap();
        assert_eq!(probe.probe_path(repository, &mounts).await, Reach::Yes);
    }
}
