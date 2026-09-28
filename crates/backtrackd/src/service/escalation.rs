// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! When to tell somebody their backups need them, and when not to.
//!
//! health.md's escalation, exactly:
//!
//! - `AT_RISK` is announced when it begins, again at 72 hours without
//!   protection, then weekly.
//! - `BROKEN` is announced as soon as it is detected, then at most once a day.
//!
//! Everything else is silent. That is most of the point: a backup that fails at
//! 14:00 and succeeds at 15:00 never reaches either state, so nothing here is
//! ever asked about it.
//!
//! The decision is a pure function of the health, the clock, and what has
//! already been said, so the rules are tested by passing times rather than by
//! waiting for them — and what has been said is persisted, so a laptop that
//! restarts does not say it all again.

use backtrack_core::engine::HealthFailure;
use backtrack_core::state::Notified;

use super::health::{Health, HealthState};

const HOUR: u64 = 3_600;
const DAY: u64 = 24 * HOUR;

/// When, measured from the last protection, the second at-risk notice goes.
pub const AT_RISK_SECOND: u64 = 72 * HOUR;

/// How often at-risk notices repeat after the second.
pub const AT_RISK_REPEAT: u64 = 7 * DAY;

/// The least time between two notices about the same broken thing.
pub const BROKEN_REPEAT: u64 = DAY;

/// Something the person should be told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alert {
    /// Nothing has been protected for this many whole days (at least one).
    AtRisk { days: u64 },
    /// Backups are stopped, by this catalogue row.
    Broken(HealthFailure),
}

/// Decide whether `health` is worth telling the person about now.
///
/// `protected_until` is when protection was last in place, in epoch seconds;
/// it names the stretch an at-risk notice is about. `now` is epoch seconds too.
/// Returns the alert and the record to keep, or `None` to stay quiet.
pub fn decide(
    health: Health,
    protected_until: Option<u64>,
    told: &Notified,
    now: u64,
) -> Option<(Alert, Notified)> {
    match health.state {
        HealthState::AtRisk => {
            let since = protected_until?;
            let age = now.saturating_sub(since);
            let same_stretch = told.at_risk_since == Some(since);
            let due = match (same_stretch, told.at_risk_at) {
                (true, Some(last)) => {
                    // The second notice goes at the 72-hour mark, unless the
                    // first went out after it — a machine that was off for
                    // four days — in which case it counts as the second and
                    // the weekly cadence starts from it.
                    let next = if told.at_risk_count == 1 && last < since + AT_RISK_SECOND {
                        since + AT_RISK_SECOND
                    } else {
                        last + AT_RISK_REPEAT
                    };
                    now >= next
                }
                // A new stretch, or one nothing has been said about yet.
                _ => true,
            };
            if !due {
                return None;
            }
            let count = if same_stretch {
                told.at_risk_count + 1
            } else {
                1
            };
            Some((
                Alert::AtRisk {
                    days: (age / DAY).max(1),
                },
                Notified {
                    at_risk_since: Some(since),
                    at_risk_count: count,
                    at_risk_at: Some(now),
                    ..told.clone()
                },
            ))
        }
        HealthState::Broken => {
            let failure = health.reason.and_then(|reason| reason.failure())?;
            let token = health.reason_str();
            // Kept across the problem clearing and coming back, deliberately:
            // something that fails, recovers and fails again inside the hour is
            // one problem, and announcing it each time is crying wolf.
            let repeat = told.broken_reason.as_deref() == Some(token)
                && told
                    .broken_at
                    .is_some_and(|at| now.saturating_sub(at) < BROKEN_REPEAT);
            if repeat {
                return None;
            }
            Some((
                Alert::Broken(failure),
                Notified {
                    broken_reason: Some(token.to_string()),
                    broken_at: Some(now),
                    ..told.clone()
                },
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use backtrack_core::dbus::Reason;

    use super::super::health::{evaluate, protected_until, HealthInputs};
    use super::*;

    /// Monday 2026-06-01 00:00 UTC, so "14:00" below reads as a time of day.
    const MONDAY: u64 = 1_780_272_000;

    fn at(day: u64, hour: u64) -> u64 {
        MONDAY + day * DAY + hour * HOUR
    }

    fn time(epoch: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(epoch)
    }

    /// A machine on the hourly default, as health would be computed for it.
    fn inputs(last_success: u64, blocking: Option<HealthFailure>) -> HealthInputs {
        HealthInputs {
            blocking,
            paused_until: None,
            destination_reachable: true,
            offline_protection_active: false,
            last_success: Some(time(last_success)),
            protected_since: Some(time(MONDAY - 30 * DAY)),
            frequency: Some(Duration::from_secs(HOUR)),
            attention: None,
        }
    }

    /// What happened to the machine: at this time, the last success was this,
    /// and this failure (if any) was holding backups up.
    type Event = (u64, u64, Option<HealthFailure>);

    /// Run a timeline through the real evaluator and the real escalation, with
    /// a clock that ticks every `step` seconds between events, and return every
    /// notice that would have gone out.
    fn replay(events: &[Event], until: u64, step: u64) -> Vec<(u64, Alert)> {
        let mut told = Notified::default();
        let mut sent = Vec::new();
        let mut now = events.first().map(|e| e.0).unwrap_or(0);
        while now <= until {
            let current = events
                .iter()
                .rev()
                .find(|(when, ..)| *when <= now)
                .expect("the timeline starts with an event");
            let inputs = inputs(current.1, current.2);
            let health = evaluate(&inputs, time(now));
            let protected_until = backtrack_core::state::to_epoch(protected_until(&inputs));
            if let Some((alert, record)) = decide(health, protected_until, &told, now) {
                sent.push((now, alert));
                told = record;
            }
            now += step;
        }
        sent
    }

    #[test]
    fn failed_at_two_succeeded_at_three_and_nothing_was_ever_said() {
        // health.md, principle 2, as a timeline: hourly backups for two days,
        // except that the one at 14:00 on the first day fails with something
        // that is not one of the catalogue's blocking rows, so its last
        // success stays at 13:00 until 15:00 succeeds. Watched minute by
        // minute.
        let events: Vec<Event> = (0..48)
            .map(|hour| {
                let when = at(0, 9 + hour);
                let last = if when == at(0, 14) { at(0, 13) } else { when };
                (when, last, None)
            })
            .collect();
        let sent = replay(&events, at(2, 9), 60);
        assert!(sent.is_empty(), "nothing deserved a notice: {sent:?}");
    }

    #[test]
    fn at_risk_is_announced_at_a_day_again_at_three_then_weekly() {
        // The last backup was Monday 09:00 and nothing works after it.
        let events = [(at(0, 9), at(0, 9), None)];
        let sent = replay(&events, at(20, 0), 60);
        let times: Vec<u64> = sent.iter().map(|(when, _)| *when).collect();
        // "Over 24 hours" is the first minute past the day.
        let first = at(1, 9) + 60;
        assert_eq!(
            times,
            vec![first, at(3, 9), at(3, 9) + 7 * DAY, at(3, 9) + 14 * DAY],
            "at 24 hours, at 72 hours, then each week"
        );
        assert_eq!(sent[0].1, Alert::AtRisk { days: 1 });
        assert_eq!(sent[1].1, Alert::AtRisk { days: 3 });
        assert_eq!(sent[2].1, Alert::AtRisk { days: 10 });
    }

    #[test]
    fn a_machine_off_through_the_72_hour_mark_does_not_say_it_twice_on_waking() {
        let since = at(0, 9);
        let health = Health {
            state: HealthState::AtRisk,
            reason: Some(Reason::NoRecentBackup),
        };
        // The first look is four days in.
        let woke = at(4, 9);
        let (alert, told) = decide(health, Some(since), &Notified::default(), woke).unwrap();
        assert_eq!(alert, Alert::AtRisk { days: 4 });
        assert_eq!(
            decide(health, Some(since), &told, woke + HOUR),
            None,
            "the 72-hour notice is already overdue, but one just went"
        );
        assert!(decide(health, Some(since), &told, woke + 7 * DAY).is_some());
    }

    #[test]
    fn a_new_success_ends_the_stretch_and_the_next_one_starts_its_own_count() {
        let events = [
            (at(0, 9), at(0, 9), None),
            // Rescued on day 2, then broken again for good.
            (at(2, 12), at(2, 12), None),
        ];
        let sent = replay(&events, at(6, 0), 60);
        let times: Vec<u64> = sent.iter().map(|(when, _)| *when).collect();
        assert_eq!(times, vec![at(1, 9) + 60, at(3, 12) + 60, at(5, 12)]);
    }

    #[test]
    fn broken_is_announced_at_once_and_then_at_most_daily() {
        let events = [
            (at(0, 9), at(0, 9), None),
            (at(0, 14), at(0, 9), Some(HealthFailure::DestinationFull)),
        ];
        let sent = replay(&events, at(2, 15), 60);
        let broken: Vec<u64> = sent
            .iter()
            .filter(|(_, alert)| matches!(alert, Alert::Broken(_)))
            .map(|(when, _)| *when)
            .collect();
        assert_eq!(broken, vec![at(0, 14), at(1, 14), at(2, 14)]);
        assert!(sent
            .iter()
            .all(|(_, alert)| *alert == Alert::Broken(HealthFailure::DestinationFull)));
    }

    #[test]
    fn a_failure_that_comes_and_goes_within_the_day_is_announced_once() {
        let events = [
            (at(0, 9), at(0, 9), None),
            (at(0, 10), at(0, 9), Some(HealthFailure::PassphraseMissing)),
            (at(0, 11), at(0, 11), None),
            (at(0, 12), at(0, 11), Some(HealthFailure::PassphraseMissing)),
        ];
        let sent = replay(&events, at(0, 23), 60);
        assert_eq!(
            sent,
            vec![(at(0, 10), Alert::Broken(HealthFailure::PassphraseMissing))]
        );
    }

    #[test]
    fn a_different_failure_is_news_and_is_announced_at_once() {
        let events = [
            (at(0, 9), at(0, 9), None),
            (at(0, 10), at(0, 9), Some(HealthFailure::DestinationFull)),
            (at(0, 11), at(0, 9), Some(HealthFailure::RepoCorrupt)),
        ];
        let sent = replay(&events, at(0, 12), 60);
        assert_eq!(
            sent,
            vec![
                (at(0, 10), Alert::Broken(HealthFailure::DestinationFull)),
                (at(0, 11), Alert::Broken(HealthFailure::RepoCorrupt)),
            ]
        );
    }

    #[test]
    fn the_quiet_states_never_speak() {
        for state in [
            HealthState::Healthy,
            HealthState::ProtectedLocally,
            HealthState::Paused,
            HealthState::Degraded,
        ] {
            let health = Health {
                state,
                reason: None,
            };
            assert_eq!(
                decide(health, Some(0), &Notified::default(), 10 * DAY),
                None,
                "{state:?}"
            );
        }
    }
}
