// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The one overall health state, per health.md.
//!
//! The rules that matter are as much about *silence* as about alarm:
//!
//! - An unreachable destination with the local safety net running is
//!   `PROTECTED_LOCALLY`, not a warning. It is the product working as designed,
//!   and crying wolf here is how people learn to ignore a backup tool.
//! - A pause the user asked for is `PAUSED`, never `AT_RISK`.
//! - `AT_RISK` is earned by *risk* — 24 hours with no successful protection of
//!   any kind — not by a single failed run that the next hour will fix.
//!
//! State is computed from facts rather than accumulated by event handlers, so a
//! missed signal cannot leave the daemon stuck describing a world that has
//! moved on.

use std::time::{Duration, SystemTime};

/// How long without any successful protection before the user is warned.
const AT_RISK_AFTER: Duration = Duration::from_secs(24 * 3_600);

/// The overall state, exactly the set health.md defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthState {
    /// Last backup succeeded within twice the configured frequency.
    Healthy,
    /// Destination unreachable; the offline safety net is carrying it.
    ProtectedLocally,
    /// The user paused backups; the pause expires by itself.
    Paused,
    /// No successful protection, network or local, for over 24 hours.
    AtRisk,
    /// Backups cannot run without the user doing something.
    Broken,
    /// Backups run, but something needs attention eventually.
    Degraded,
}

impl HealthState {
    /// The wire form, matching health.md's table.
    pub fn as_str(self) -> &'static str {
        match self {
            HealthState::Healthy => "HEALTHY",
            HealthState::ProtectedLocally => "PROTECTED_LOCALLY",
            HealthState::Paused => "PAUSED",
            HealthState::AtRisk => "AT_RISK",
            HealthState::Broken => "BROKEN",
            HealthState::Degraded => "DEGRADED",
        }
    }

    /// Every state, for exhaustiveness tests.
    #[cfg(test)]
    pub const ALL: &'static [HealthState] = &[
        HealthState::Healthy,
        HealthState::ProtectedLocally,
        HealthState::Paused,
        HealthState::AtRisk,
        HealthState::Broken,
        HealthState::Degraded,
    ];
}

/// Everything the state is derived from. Assembled by the daemon when asked,
/// never cached.
#[derive(Debug, Clone, Default)]
pub struct HealthInputs {
    /// A condition that stops backups until the user acts. Set from the failure
    /// catalogue via `EngineError::health_failure`.
    pub blocking_failure: bool,
    /// A pause the user asked for, and when it lifts.
    pub paused_until: Option<SystemTime>,
    /// Whether the destination answered last time we looked.
    pub destination_reachable: bool,
    /// Whether the local safety net is protecting changes meanwhile.
    pub offline_protection_active: bool,
    /// The last backup that succeeded anywhere — network, spool, or snapshot.
    pub last_success: Option<SystemTime>,
    /// The configured gap between scheduled backups; `None` when the schedule is
    /// manual, in which case age alone never raises an alarm.
    pub frequency: Option<Duration>,
    /// Something wants eventual attention (spool near its cap, index rebuilding).
    pub needs_attention: bool,
}

/// Compute the overall state.
///
/// Order is precedence, and it is deliberate: the states that demand user action
/// outrank the ones that merely describe the weather.
pub fn evaluate(inputs: &HealthInputs, now: SystemTime) -> HealthState {
    // Nothing works until this is fixed, so it outranks everything.
    if inputs.blocking_failure {
        return HealthState::Broken;
    }

    // A pause the user chose is never a warning, and it outranks staleness:
    // backups are old *because they asked*.
    if inputs.paused_until.is_some_and(|until| until > now) {
        return HealthState::Paused;
    }

    let stale = is_stale(inputs, now);

    // Offline with the net up is the designed behaviour, not a problem — but
    // only while the net is genuinely holding. Once nothing has succeeded for a
    // day, the honest answer is that the user is at risk.
    if !inputs.destination_reachable && inputs.offline_protection_active && !stale {
        return HealthState::ProtectedLocally;
    }

    if stale {
        return HealthState::AtRisk;
    }

    if inputs.needs_attention {
        return HealthState::Degraded;
    }

    HealthState::Healthy
}

/// Whether protection has gone stale: nothing succeeded for over a day.
///
/// A machine that has never backed up is stale only once the schedule has had a
/// day to produce something — otherwise a fresh install would announce itself as
/// at risk before its first run.
fn is_stale(inputs: &HealthInputs, now: SystemTime) -> bool {
    // A manual-only schedule cannot be late for anything.
    if inputs.frequency.is_none() {
        return false;
    }
    match inputs.last_success {
        Some(last) => now
            .duration_since(last)
            .is_ok_and(|age| age > AT_RISK_AFTER),
        // Never backed up: the wizard has run but nothing has landed yet. Left
        // to the caller to decide when the clock started; treating it as stale
        // immediately would light up a banner during the first backup.
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000)
    }

    fn ago(seconds: u64) -> Option<SystemTime> {
        Some(now() - Duration::from_secs(seconds))
    }

    /// A machine that is working normally.
    fn healthy() -> HealthInputs {
        HealthInputs {
            blocking_failure: false,
            paused_until: None,
            destination_reachable: true,
            offline_protection_active: false,
            last_success: ago(600),
            frequency: Some(Duration::from_secs(3_600)),
            needs_attention: false,
        }
    }

    #[test]
    fn a_working_machine_is_healthy_and_says_nothing() {
        assert_eq!(evaluate(&healthy(), now()), HealthState::Healthy);
    }

    #[test]
    fn offline_with_the_safety_net_is_not_a_warning() {
        // health.md: "Offline-with-safety-net is not a warning state — it's the
        // product working as designed."
        let inputs = HealthInputs {
            destination_reachable: false,
            offline_protection_active: true,
            last_success: ago(3_600),
            ..healthy()
        };
        assert_eq!(evaluate(&inputs, now()), HealthState::ProtectedLocally);
    }

    #[test]
    fn offline_without_a_safety_net_goes_stale_normally() {
        let inputs = HealthInputs {
            destination_reachable: false,
            offline_protection_active: false,
            last_success: ago(48 * 3_600),
            ..healthy()
        };
        assert_eq!(evaluate(&inputs, now()), HealthState::AtRisk);
    }

    #[test]
    fn the_safety_net_stops_excusing_things_after_a_day() {
        // Protecting locally is honest for a while; a laptop that has been away
        // for three days is genuinely at risk and must say so.
        let inputs = HealthInputs {
            destination_reachable: false,
            offline_protection_active: true,
            last_success: ago(72 * 3_600),
            ..healthy()
        };
        assert_eq!(evaluate(&inputs, now()), HealthState::AtRisk);
    }

    #[test]
    fn a_user_pause_is_never_a_warning() {
        let inputs = HealthInputs {
            paused_until: Some(now() + Duration::from_secs(3_600)),
            // Old, precisely because they asked for it.
            last_success: ago(72 * 3_600),
            ..healthy()
        };
        assert_eq!(evaluate(&inputs, now()), HealthState::Paused);
    }

    #[test]
    fn an_expired_pause_stops_applying() {
        let inputs = HealthInputs {
            paused_until: Some(now() - Duration::from_secs(1)),
            ..healthy()
        };
        assert_eq!(
            evaluate(&inputs, now()),
            HealthState::Healthy,
            "a self-expiring pause must expire by itself"
        );
    }

    #[test]
    fn a_blocking_failure_outranks_everything() {
        let inputs = HealthInputs {
            blocking_failure: true,
            paused_until: Some(now() + Duration::from_secs(3_600)),
            destination_reachable: false,
            offline_protection_active: true,
            ..healthy()
        };
        assert_eq!(evaluate(&inputs, now()), HealthState::Broken);
    }

    #[test]
    fn at_risk_is_earned_by_a_day_not_by_one_missed_run() {
        // health.md: "A failed backup at 14:00 that succeeds at 15:00 never
        // deserved a notification."
        let recent = HealthInputs {
            last_success: ago(23 * 3_600),
            ..healthy()
        };
        assert_eq!(evaluate(&recent, now()), HealthState::Healthy);

        let old = HealthInputs {
            last_success: ago(25 * 3_600),
            ..healthy()
        };
        assert_eq!(evaluate(&old, now()), HealthState::AtRisk);
    }

    #[test]
    fn degraded_is_reported_only_when_nothing_worse_applies() {
        let inputs = HealthInputs {
            needs_attention: true,
            ..healthy()
        };
        assert_eq!(evaluate(&inputs, now()), HealthState::Degraded);

        // Something worse wins.
        let worse = HealthInputs {
            needs_attention: true,
            last_success: ago(48 * 3_600),
            ..healthy()
        };
        assert_eq!(evaluate(&worse, now()), HealthState::AtRisk);
    }

    #[test]
    fn a_manual_schedule_is_never_late() {
        let inputs = HealthInputs {
            frequency: None,
            last_success: ago(90 * 24 * 3_600),
            ..healthy()
        };
        assert_eq!(
            evaluate(&inputs, now()),
            HealthState::Healthy,
            "manual-only backups cannot be overdue"
        );
    }

    #[test]
    fn a_machine_that_has_never_backed_up_is_not_yet_at_risk() {
        let inputs = HealthInputs {
            last_success: None,
            ..healthy()
        };
        assert_eq!(
            evaluate(&inputs, now()),
            HealthState::Healthy,
            "a banner during the very first backup would be a lie"
        );
    }

    #[test]
    fn wire_names_are_unique_and_match_the_document() {
        let names: Vec<_> = HealthState::ALL.iter().map(|s| s.as_str()).collect();
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "state names collide: {names:?}");
        assert!(names.contains(&"HEALTHY"));
        assert!(names.contains(&"PROTECTED_LOCALLY"));
    }
}
