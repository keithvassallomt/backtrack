// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The one overall health state, per health.md, and the reason for it.
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

use backtrack_core::dbus::Reason;
use backtrack_core::engine::HealthFailure;

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

    /// Read the wire form back.
    pub fn parse(name: &str) -> Option<HealthState> {
        HealthState::ALL
            .iter()
            .copied()
            .find(|state| state.as_str() == name)
    }

    /// Every state.
    pub const ALL: &'static [HealthState] = &[
        HealthState::Healthy,
        HealthState::ProtectedLocally,
        HealthState::Paused,
        HealthState::AtRisk,
        HealthState::Broken,
        HealthState::Degraded,
    ];
}

/// The state, and what it is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Health {
    pub state: HealthState,
    /// `None` only when healthy: every other state is about something.
    pub reason: Option<Reason>,
}

impl Health {
    pub const HEALTHY: Health = Health {
        state: HealthState::Healthy,
        reason: None,
    };

    fn because(state: HealthState, reason: Reason) -> Health {
        Health {
            state,
            reason: Some(reason),
        }
    }

    /// The reason's wire token, or empty when healthy.
    pub fn reason_str(&self) -> &'static str {
        self.reason.map(Reason::as_str).unwrap_or("")
    }
}

/// Everything the state is derived from. Assembled by the daemon when asked,
/// never cached.
#[derive(Debug, Clone, Default)]
pub struct HealthInputs {
    /// The catalogue row stopping backups until the user acts, if one is.
    pub blocking: Option<HealthFailure>,
    /// A pause the user asked for, and when it lifts.
    pub paused_until: Option<SystemTime>,
    /// Whether the destination answered last time we looked.
    pub destination_reachable: bool,
    /// Whether the local safety net is protecting changes meanwhile.
    pub offline_protection_active: bool,
    /// The last backup that succeeded anywhere — network, spool, or snapshot.
    pub last_success: Option<SystemTime>,
    /// When this computer started backing up to its destination, if known.
    /// Protection is never counted as missing from before then.
    pub protected_since: Option<SystemTime>,
    /// The gap between scheduled backups; `None` when nothing is scheduled —
    /// a manual schedule, or a destination with no folders chosen yet — in
    /// which case age alone never raises an alarm.
    pub frequency: Option<Duration>,
    /// Something wants eventual attention: the most pressing thing, if any.
    pub attention: Option<Reason>,
}

/// Compute the overall state.
///
/// Order is precedence, and it is deliberate: the states that demand user action
/// outrank the ones that merely describe the weather.
pub fn evaluate(inputs: &HealthInputs, now: SystemTime) -> Health {
    // Nothing works until this is fixed, so it outranks everything.
    if let Some(failure) = inputs.blocking {
        return Health::because(HealthState::Broken, failure.into());
    }

    // A pause the user chose is never a warning, and it outranks staleness:
    // backups are old *because they asked*.
    if inputs.paused_until.is_some_and(|until| until > now) {
        return Health::because(HealthState::Paused, Reason::Paused);
    }

    let stale = is_stale(inputs, now);

    // Offline with the net up is the designed behaviour, not a problem — but
    // only while the net is genuinely holding. Once nothing has succeeded for a
    // day, the honest answer is that the user is at risk.
    if !inputs.destination_reachable && inputs.offline_protection_active && !stale {
        return Health::because(HealthState::ProtectedLocally, Reason::DestinationAway);
    }

    if stale {
        return Health::because(HealthState::AtRisk, Reason::NoRecentBackup);
    }

    if let Some(reason) = inputs.attention {
        return Health::because(HealthState::Degraded, reason);
    }

    Health::HEALTHY
}

/// When protection was last known to be in place: the later of the last
/// success and the start of protection. `None` when neither is known, and
/// nothing can then be said to be late.
pub fn protected_until(inputs: &HealthInputs) -> Option<SystemTime> {
    match (inputs.last_success, inputs.protected_since) {
        (Some(last), Some(since)) => Some(last.max(since)),
        (last, since) => last.or(since),
    }
}

/// How long without protection counts as risk for a schedule this frequent.
///
/// Twenty-four hours, as health.md's escalation says, but never less than two
/// scheduled periods, which is its definition of healthy. The two only differ
/// for schedules longer than twelve hours, and there they must: a daily backup
/// is 24 hours old every day just before it runs, and a warning that fires
/// daily on a working machine is a warning people learn to dismiss.
pub fn at_risk_after(frequency: Duration) -> Duration {
    AT_RISK_AFTER.max(frequency.saturating_mul(2))
}

/// Whether protection has gone stale.
fn is_stale(inputs: &HealthInputs, now: SystemTime) -> bool {
    // Nothing scheduled cannot be late for anything.
    let Some(frequency) = inputs.frequency else {
        return false;
    };
    // Neither a backup nor a known start: a machine from before the clock
    // was recorded, which has never managed a backup. There is nothing to
    // measure from, and inventing a start would light a banner at random.
    let Some(since) = protected_until(inputs) else {
        return false;
    };
    now.duration_since(since)
        .is_ok_and(|age| age > at_risk_after(frequency))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: u64 = 3_600;

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000)
    }

    fn ago(seconds: u64) -> Option<SystemTime> {
        Some(now() - Duration::from_secs(seconds))
    }

    /// A machine that is working normally.
    fn healthy() -> HealthInputs {
        HealthInputs {
            blocking: None,
            paused_until: None,
            destination_reachable: true,
            offline_protection_active: false,
            last_success: ago(600),
            protected_since: ago(90 * 24 * HOUR),
            frequency: Some(Duration::from_secs(HOUR)),
            attention: None,
        }
    }

    fn state(state: HealthState, reason: Reason) -> Health {
        Health {
            state,
            reason: Some(reason),
        }
    }

    /// Every row of health.md's state table, and every catalogue row that
    /// lands in one, as the inputs that produce it and what they must produce.
    #[test]
    fn every_row_of_the_state_table() {
        let rows: Vec<(&str, HealthInputs, Health)> = vec![
            (
                "HEALTHY: last backup succeeded within twice the frequency",
                HealthInputs {
                    last_success: ago(2 * HOUR - 60),
                    ..healthy()
                },
                Health::HEALTHY,
            ),
            (
                "PROTECTED_LOCALLY: destination unreachable, safety net active",
                HealthInputs {
                    destination_reachable: false,
                    offline_protection_active: true,
                    last_success: ago(3 * HOUR),
                    ..healthy()
                },
                state(HealthState::ProtectedLocally, Reason::DestinationAway),
            ),
            (
                "PAUSED: the user paused, and it has not expired",
                HealthInputs {
                    paused_until: Some(now() + Duration::from_secs(HOUR)),
                    ..healthy()
                },
                state(HealthState::Paused, Reason::Paused),
            ),
            (
                "AT_RISK: no successful backup, network or local, for over 24h",
                HealthInputs {
                    last_success: ago(25 * HOUR),
                    ..healthy()
                },
                state(HealthState::AtRisk, Reason::NoRecentBackup),
            ),
            (
                "AT_RISK: offline, and the safety net has not held for a day",
                HealthInputs {
                    destination_reachable: false,
                    offline_protection_active: true,
                    last_success: ago(72 * HOUR),
                    ..healthy()
                },
                state(HealthState::AtRisk, Reason::NoRecentBackup),
            ),
            (
                "DEGRADED: spool near its cap, or local disk low",
                HealthInputs {
                    attention: Some(Reason::LocalDiskFull),
                    ..healthy()
                },
                state(HealthState::Degraded, Reason::LocalDiskFull),
            ),
            (
                "DEGRADED: catalogue rebuilding",
                HealthInputs {
                    attention: Some(Reason::CatalogueRebuilding),
                    ..healthy()
                },
                state(HealthState::Degraded, Reason::CatalogueRebuilding),
            ),
            (
                "DEGRADED: a backup not yet browsable",
                HealthInputs {
                    attention: Some(Reason::NotYetBrowsable),
                    ..healthy()
                },
                state(HealthState::Degraded, Reason::NotYetBrowsable),
            ),
        ];
        let broken = HealthFailure::ALL.iter().map(|failure| {
            (
                "BROKEN: a catalogue row that stops backups",
                HealthInputs {
                    blocking: Some(*failure),
                    ..healthy()
                },
                state(HealthState::Broken, Reason::from(*failure)),
            )
        });

        for (row, inputs, expected) in rows.into_iter().chain(broken) {
            assert_eq!(evaluate(&inputs, now()), expected, "{row}");
        }
    }

    #[test]
    fn every_state_is_reachable_from_the_table() {
        // The table above is only complete if it produces every state.
        let reached = [
            evaluate(&healthy(), now()).state,
            evaluate(
                &HealthInputs {
                    destination_reachable: false,
                    offline_protection_active: true,
                    ..healthy()
                },
                now(),
            )
            .state,
            evaluate(
                &HealthInputs {
                    paused_until: Some(now() + Duration::from_secs(1)),
                    ..healthy()
                },
                now(),
            )
            .state,
            evaluate(
                &HealthInputs {
                    last_success: ago(48 * HOUR),
                    ..healthy()
                },
                now(),
            )
            .state,
            evaluate(
                &HealthInputs {
                    blocking: Some(HealthFailure::RepoCorrupt),
                    ..healthy()
                },
                now(),
            )
            .state,
            evaluate(
                &HealthInputs {
                    attention: Some(Reason::NotYetBrowsable),
                    ..healthy()
                },
                now(),
            )
            .state,
        ];
        for state in HealthState::ALL {
            assert!(reached.contains(state), "{state:?} is never produced");
        }
    }

    #[test]
    fn a_working_machine_is_healthy_and_says_nothing() {
        let health = evaluate(&healthy(), now());
        assert_eq!(health, Health::HEALTHY);
        assert_eq!(health.reason_str(), "");
    }

    #[test]
    fn offline_without_a_safety_net_goes_stale_normally() {
        let inputs = HealthInputs {
            destination_reachable: false,
            offline_protection_active: false,
            last_success: ago(48 * HOUR),
            ..healthy()
        };
        assert_eq!(evaluate(&inputs, now()).state, HealthState::AtRisk);
    }

    #[test]
    fn an_expired_pause_stops_applying() {
        let inputs = HealthInputs {
            paused_until: Some(now() - Duration::from_secs(1)),
            ..healthy()
        };
        assert_eq!(
            evaluate(&inputs, now()),
            Health::HEALTHY,
            "a self-expiring pause must expire by itself"
        );
    }

    #[test]
    fn a_blocking_failure_outranks_everything() {
        let inputs = HealthInputs {
            blocking: Some(HealthFailure::PassphraseMissing),
            paused_until: Some(now() + Duration::from_secs(HOUR)),
            destination_reachable: false,
            offline_protection_active: true,
            last_success: ago(72 * HOUR),
            attention: Some(Reason::LocalDiskFull),
            ..healthy()
        };
        assert_eq!(
            evaluate(&inputs, now()),
            state(HealthState::Broken, Reason::PassphraseMissing)
        );
    }

    #[test]
    fn at_risk_is_earned_by_a_day_not_by_one_missed_run() {
        // health.md: "A failed backup at 14:00 that succeeds at 15:00 never
        // deserved a notification."
        let recent = HealthInputs {
            last_success: ago(23 * HOUR),
            ..healthy()
        };
        assert_eq!(evaluate(&recent, now()), Health::HEALTHY);

        let old = HealthInputs {
            last_success: ago(25 * HOUR),
            ..healthy()
        };
        assert_eq!(evaluate(&old, now()).state, HealthState::AtRisk);
    }

    #[test]
    fn a_daily_schedule_is_not_at_risk_in_the_hour_before_it_runs() {
        // A daily backup is 24 hours old every day just before it runs, and
        // late by an hour whenever the machine was asleep at the time.
        let daily = HealthInputs {
            frequency: Some(Duration::from_secs(24 * HOUR)),
            last_success: ago(25 * HOUR),
            ..healthy()
        };
        assert_eq!(evaluate(&daily, now()), Health::HEALTHY);

        let missed_two = HealthInputs {
            last_success: ago(49 * HOUR),
            ..daily
        };
        assert_eq!(evaluate(&missed_two, now()).state, HealthState::AtRisk);
    }

    #[test]
    fn degraded_is_reported_only_when_nothing_worse_applies() {
        let worse = HealthInputs {
            attention: Some(Reason::LocalDiskFull),
            last_success: ago(48 * HOUR),
            ..healthy()
        };
        assert_eq!(evaluate(&worse, now()).state, HealthState::AtRisk);
    }

    #[test]
    fn nothing_scheduled_is_never_late() {
        let inputs = HealthInputs {
            frequency: None,
            last_success: ago(90 * 24 * HOUR),
            ..healthy()
        };
        assert_eq!(
            evaluate(&inputs, now()),
            Health::HEALTHY,
            "manual-only backups, or none chosen yet, cannot be overdue"
        );
    }

    #[test]
    fn a_first_backup_is_not_at_risk_while_it_runs() {
        let inputs = HealthInputs {
            last_success: None,
            protected_since: ago(2 * HOUR),
            ..healthy()
        };
        assert_eq!(
            evaluate(&inputs, now()),
            Health::HEALTHY,
            "a banner during the very first backup would be a lie"
        );
    }

    #[test]
    fn a_machine_that_has_never_managed_a_backup_is_at_risk_after_a_day() {
        let inputs = HealthInputs {
            last_success: None,
            protected_since: ago(26 * HOUR),
            ..healthy()
        };
        assert_eq!(evaluate(&inputs, now()).state, HealthState::AtRisk);
    }

    #[test]
    fn imported_history_does_not_make_a_new_computer_late() {
        // The newest backup in somebody's imported repository may be weeks
        // old. This computer started protecting itself an hour ago.
        let inputs = HealthInputs {
            last_success: ago(21 * 24 * HOUR),
            protected_since: ago(HOUR),
            ..healthy()
        };
        assert_eq!(evaluate(&inputs, now()), Health::HEALTHY);
    }

    #[test]
    fn a_machine_with_no_clock_at_all_is_not_invented_one() {
        let inputs = HealthInputs {
            last_success: None,
            protected_since: None,
            ..healthy()
        };
        assert_eq!(evaluate(&inputs, now()), Health::HEALTHY);
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
