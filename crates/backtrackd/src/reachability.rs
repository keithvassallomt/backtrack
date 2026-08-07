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

use std::sync::Arc;
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
pub struct RealProbe;

#[async_trait]
impl DestinationProbe for RealProbe {
    async fn probe(&self, repository: &str, engine: Option<Arc<dyn BackupEngine>>) -> Reach {
        if repository.is_empty() {
            return Reach::Unknown;
        }
        match ssh_endpoint(repository) {
            Some((host, port)) => probe_remote(&host, port, engine).await,
            None => probe_local(repository).await,
        }
    }
}

/// A local path or a mounted share: it is there and writable, or it is not.
///
/// The write check is [`rustix::fs::access`] rather than actually creating a
/// file. Touching the repository once a minute to prove we can would be a
/// strange thing to do to somebody's backups, and a share that is mounted
/// read-only — the classic "the NAS came back but in a degraded state" — is
/// exactly the case that must not read as reachable, because a backup into it
/// would fail.
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
        use rustix::fs::Access;
        // EXEC_OK on a directory is the permission to traverse into it, which
        // is what Borg needs alongside the ability to write.
        rustix::fs::access(&path, Access::WRITE_OK | Access::EXEC_OK).is_ok()
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
    let connect = tokio::net::TcpStream::connect((host, port));
    match tokio::time::timeout(PROBE_TIMEOUT, connect).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            debug!(host, port, "destination did not accept a connection: {e}");
            return Reach::No;
        }
        Err(_) => {
            debug!(host, port, "destination did not answer in time");
            return Reach::Unknown;
        }
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

/// Host and port for a Borg remote destination, or `None` for a local path.
///
/// Borg accepts `ssh://[user@]host[:port]/path` and the scp-style
/// `[user@]host:path`. The second is the ambiguous one: a plain path containing
/// a colon must not be mistaken for it, so an `@` before the first `/` is
/// required — the same rule preflight uses to decide whether a destination
/// costs network traffic.
pub fn ssh_endpoint(repository: &str) -> Option<(String, u16)> {
    if let Some(rest) = repository.strip_prefix("ssh://") {
        let authority = rest.split('/').next()?;
        let host_port = authority.rsplit('@').next()?;
        return Some(split_host_port(host_port));
    }
    let (prefix, _) = repository.split_once(':')?;
    if !prefix.contains('@') || prefix.contains('/') {
        return None;
    }
    let host = prefix.rsplit('@').next()?;
    (!host.is_empty()).then(|| (host.to_string(), 22))
}

/// Split `host` or `host:port`, defaulting to SSH's port. IPv6 literals arrive
/// bracketed, and the brackets come off — `TcpStream::connect` wants the bare
/// address.
fn split_host_port(text: &str) -> (String, u16) {
    if let Some(rest) = text.strip_prefix('[') {
        if let Some((addr, tail)) = rest.split_once(']') {
            let port = tail
                .strip_prefix(':')
                .and_then(|p| p.parse().ok())
                .unwrap_or(22);
            return (addr.to_string(), port);
        }
    }
    match text.rsplit_once(':') {
        Some((host, port)) => match port.parse() {
            Ok(port) => (host.to_string(), port),
            // Not a port. Borg's scp-style form has already been handled by the
            // caller, so this is a hostname with a colon in it — malformed, but
            // passing it on unchanged gives a better error than inventing one.
            Err(_) => (text.to_string(), 22),
        },
        None => (text.to_string(), 22),
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

    #[test]
    fn borg_remote_destinations_are_recognised_with_their_ports() {
        assert_eq!(
            ssh_endpoint("ssh://nas.local/./backups"),
            Some(("nas.local".into(), 22))
        );
        assert_eq!(
            ssh_endpoint("ssh://keith@nas.local:2222/./backups"),
            Some(("nas.local".into(), 2222))
        );
        assert_eq!(
            ssh_endpoint("keith@nas.local:backups"),
            Some(("nas.local".into(), 22))
        );
        assert_eq!(
            ssh_endpoint("ssh://[2001:db8::1]:2222/./backups"),
            Some(("2001:db8::1".into(), 2222)),
            "an IPv6 literal loses its brackets before it reaches connect()"
        );
    }

    #[test]
    fn a_local_path_is_not_a_remote_destination() {
        assert_eq!(ssh_endpoint("/mnt/usb/backups"), None);
        assert_eq!(
            ssh_endpoint("/mnt/odd:name/backups"),
            None,
            "a colon in a path is still a path"
        );
        assert_eq!(ssh_endpoint(""), None);
    }

    #[tokio::test]
    async fn a_local_destination_is_probed_by_looking_at_it() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        assert_eq!(
            RealProbe.probe(repo.to_str().unwrap(), None).await,
            Reach::No,
            "an unplugged drive presents as an absent path"
        );
        std::fs::create_dir(&repo).unwrap();
        assert_eq!(
            RealProbe.probe(repo.to_str().unwrap(), None).await,
            Reach::Yes
        );
    }

    #[tokio::test]
    async fn a_read_only_destination_does_not_count_as_reachable() {
        // "The NAS came back, but read-only" is a real failure mode, and one a
        // plain existence check reports as fine right up until the backup fails.
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let mut perms = std::fs::metadata(&repo).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o500);
        std::fs::set_permissions(&repo, perms).unwrap();

        let reach = RealProbe.probe(repo.to_str().unwrap(), None).await;

        // Restore before asserting, so a failure does not leave an undeletable
        // temporary directory behind.
        let mut perms = std::fs::metadata(&repo).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o700);
        std::fs::set_permissions(&repo, perms).unwrap();

        assert_eq!(reach, Reach::No);
    }

    #[tokio::test]
    async fn an_unconfigured_destination_is_unknown_rather_than_absent() {
        assert_eq!(RealProbe.probe("", None).await, Reach::Unknown);
    }
}
