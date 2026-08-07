// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The scheduler: when a backup should run, and the loop that makes it happen.
//!
//! An internal timer rather than a systemd timer, for one reason that decides
//! it: a systemd timer can start a backup, but it cannot *decline* to. Every
//! interesting decision here — is the destination there, is the machine on
//! battery, is the user's pause still in force, is a job already running — needs
//! state only the daemon holds. Splitting the trigger from the state that
//! qualifies it would mean waking a process every hour to discover it should not
//! have been woken.
//!
//! The decision is a pure function, [`decide`]. Everything time-dependent
//! arrives as an argument, so the interesting cases — a laptop that slept
//! through six scheduled runs, a pause that expires mid-wait — are tested by
//! passing times rather than by waiting for them.
//!
//! ## Catch-up after suspend
//!
//! Waking to find a backup overdue is normal on a laptop, and firing the
//! instant the lid opens is the wrong response: the network is still coming up,
//! the disk is still spinning, and the user is trying to do something. A run
//! that is late by more than one whole period is therefore delayed by a jittered
//! interval of up to [`CATCH_UP_WINDOW`] before it is reconsidered. The jitter
//! also stops every Backtrack machine on a network from stampeding a shared NAS
//! at the same second after a power cut.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use backtrack_core::config::Config;
use tokio::sync::Notify;
use tracing::{debug, info, warn};

use crate::service::Shared;

/// The longest the loop sleeps before looking again, even when the next backup
/// is days away. Keeps a suspended-then-resumed machine responsive: the clock
/// jumped, and the only way to notice is to look.
const MAX_SLEEP: Duration = Duration::from_secs(60);

/// The shortest sleep, so a scheduling bug becomes a slow loop rather than a
/// spin that pins a core.
const MIN_SLEEP: Duration = Duration::from_secs(1);

/// How long after a wake-up a missed run may be deferred. Per the stage plan:
/// overdue runs happen "within 2 minutes", jittered.
pub const CATCH_UP_WINDOW: Duration = Duration::from_secs(120);

/// Everything [`decide`] needs. Assembled fresh each tick — the scheduler holds
/// no opinion of its own that could go stale.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScheduleInput {
    /// The gap between scheduled backups; `None` for a manual-only schedule.
    pub interval: Option<Duration>,
    /// When a scheduled backup was last attempted, successful or not.
    ///
    /// Attempts, not successes, are what the cadence counts from. Counting from
    /// successes would turn a destination that has been unplugged for a week
    /// into a backup attempt every tick for a week.
    pub last_attempt: Option<SystemTime>,
    /// When the user's pause lifts, if one is in force.
    pub paused_until: Option<SystemTime>,
    /// Whether a destination has been configured at all.
    pub configured: bool,
    /// Whether a job is already running or queued. A second backup queued behind
    /// the first would run the moment it finished, which is not a schedule.
    pub busy: bool,
}

/// What to do at this moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Nothing to do. Look again after this long — the time until the next
    /// interesting moment, clamped to a sane range.
    Idle(Duration),
    /// Start a scheduled backup now.
    Run,
    /// A run is overdue by more than a whole period, so the machine was almost
    /// certainly asleep. Wait this jittered delay and decide again.
    CatchUp(Duration),
}

/// Decide what the scheduler should do at `now`.
///
/// `jitter` is the delay to use if this turns out to be a catch-up; it is passed
/// in rather than generated here so the function stays pure and the tests stay
/// deterministic.
pub fn decide(input: &ScheduleInput, now: SystemTime, jitter: Duration) -> Decision {
    // No destination, no schedule: the wizard has not run. Nothing to do, and
    // nothing to warn about — an unconfigured machine is not a broken one.
    let Some(interval) = input.interval.filter(|_| input.configured) else {
        return Decision::Idle(MAX_SLEEP);
    };

    // A pause the user asked for. Sleep until it lifts (or until the next look,
    // whichever is sooner) so a four-hour pause does not cost 240 wake-ups.
    if let Some(until) = input.paused_until.filter(|until| *until > now) {
        return Decision::Idle(clamp(remaining(until, now)));
    }

    // Something is already using the repository. Come back shortly rather than
    // queueing a backup that would start the instant the current job ends.
    if input.busy {
        return Decision::Idle(MAX_SLEEP);
    }

    // Never attempted: due immediately. A machine that has just been set up
    // should protect itself now, not in an hour.
    let Some(last) = input.last_attempt else {
        return Decision::Run;
    };

    let due = last + interval;
    if now < due {
        return Decision::Idle(clamp(remaining(due, now)));
    }

    // Late by more than a whole period means time passed without us running —
    // suspend, hibernation, or a daemon that was not there. Defer briefly.
    if now >= due + interval {
        return Decision::CatchUp(jitter.min(CATCH_UP_WINDOW));
    }

    Decision::Run
}

/// How long from `now` until `then`, or nothing if it has already passed.
fn remaining(then: SystemTime, now: SystemTime) -> Duration {
    then.duration_since(now).unwrap_or(Duration::ZERO)
}

fn clamp(d: Duration) -> Duration {
    d.clamp(MIN_SLEEP, MAX_SLEEP)
}

/// The configured interval, honouring the development override.
///
/// `BACKTRACK_DEV_INTERVAL_SECS` shortens the cadence so a day of scheduling
/// behaviour can be watched over a coffee break. It is read only under
/// `BACKTRACK_DEV`, so it cannot shorten anybody's real backup schedule by
/// leaking into an installed daemon's environment.
pub fn interval_for(config: &Config) -> Option<Duration> {
    let configured = config.backup.frequency.interval()?;
    if std::env::var_os("BACKTRACK_DEV").is_none() {
        return Some(configured);
    }
    match std::env::var("BACKTRACK_DEV_INTERVAL_SECS").ok()?.parse() {
        Ok(secs) if secs > 0 => {
            let dev = Duration::from_secs(secs);
            debug!(seconds = secs, "development backup interval in force");
            Some(dev)
        }
        _ => Some(configured),
    }
}

/// A jitter of up to [`CATCH_UP_WINDOW`], derived from the clock.
///
/// Not cryptographic and not trying to be: the only requirement is that two
/// machines waking together pick different delays, and sub-second clock noise
/// does that without adding a dependency.
fn jitter() -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    Duration::from_millis(u64::from(nanos) % (CATCH_UP_WINDOW.as_millis() as u64))
}

/// Runs the schedule for the daemon's lifetime.
///
/// Held by the daemon so the loop stops when the daemon does; woken early
/// through [`Scheduler::wake`] whenever something happens that could change the
/// answer (the configuration changed, a pause was lifted, a job finished).
pub struct Scheduler {
    shared: Arc<Shared>,
    wake: Arc<Notify>,
}

impl Scheduler {
    pub fn new(shared: Arc<Shared>) -> Scheduler {
        Scheduler {
            shared,
            wake: Arc::new(Notify::new()),
        }
    }

    /// A handle that makes the loop reconsider immediately.
    pub fn waker(&self) -> Arc<Notify> {
        Arc::clone(&self.wake)
    }

    /// The scheduling loop. Never returns.
    pub async fn run(self) {
        info!("scheduler started");
        loop {
            let sleep_for = self.tick().await;
            tokio::select! {
                _ = tokio::time::sleep(sleep_for) => {}
                _ = self.wake.notified() => debug!("scheduler woken early"),
            }
        }
    }

    /// One pass: decide, act, and report how long to wait next.
    async fn tick(&self) -> Duration {
        let input = self.shared.schedule_input();
        match decide(&input, SystemTime::now(), jitter()) {
            Decision::Idle(d) => d,
            Decision::CatchUp(d) => {
                info!(
                    delay_secs = d.as_secs(),
                    "a scheduled backup was missed; catching up shortly"
                );
                d
            }
            Decision::Run => {
                match self.shared.start_scheduled_backup().await {
                    Ok(Some(job)) => info!(job, "scheduled backup started"),
                    // Preflight declined. The reason is logged there; the
                    // attempt clock is deliberately not advanced, so the run
                    // happens as soon as the condition clears rather than an
                    // hour later.
                    Ok(None) => {}
                    Err(e) => warn!("scheduled backup could not start: {e}"),
                }
                MIN_SLEEP.max(Duration::from_secs(5))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: Duration = Duration::from_secs(3_600);

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_000_000)
    }

    fn ago(seconds: u64) -> Option<SystemTime> {
        Some(now() - Duration::from_secs(seconds))
    }

    /// A configured machine on the hourly default, backed up ten minutes ago.
    fn hourly() -> ScheduleInput {
        ScheduleInput {
            interval: Some(HOUR),
            last_attempt: ago(600),
            paused_until: None,
            configured: true,
            busy: false,
        }
    }

    fn decide_now(input: &ScheduleInput) -> Decision {
        decide(input, now(), Duration::from_secs(7))
    }

    #[test]
    fn nothing_is_due_before_the_interval_has_passed() {
        // Ten minutes into an hourly cadence: sleep, and not past the due time.
        let Decision::Idle(d) = decide_now(&hourly()) else {
            panic!("a backup ten minutes old is not due");
        };
        assert!(d <= MAX_SLEEP, "slept {d:?}, longer than the maximum");
    }

    #[test]
    fn a_backup_is_due_once_the_interval_elapses() {
        let input = ScheduleInput {
            last_attempt: ago(3_601),
            ..hourly()
        };
        assert_eq!(decide_now(&input), Decision::Run);
    }

    #[test]
    fn the_normal_cadence_does_not_take_the_catch_up_path() {
        // One second late is the timer working, not a machine waking up. Adding
        // up to two minutes of jitter to every hourly run would be absurd.
        let input = ScheduleInput {
            last_attempt: ago(3_601),
            ..hourly()
        };
        assert_eq!(decide_now(&input), Decision::Run);

        // Even most of a period late still counts as the ordinary cadence.
        let input = ScheduleInput {
            last_attempt: ago(7_000),
            ..hourly()
        };
        assert_eq!(decide_now(&input), Decision::Run);
    }

    #[test]
    fn waking_after_a_long_sleep_catches_up_within_the_window() {
        // The laptop-lid case: eight hours of missed runs.
        let input = ScheduleInput {
            last_attempt: ago(8 * 3_600),
            ..hourly()
        };
        let Decision::CatchUp(delay) = decide_now(&input) else {
            panic!("a run missed by hours must be a catch-up, not an instant fire");
        };
        assert!(
            delay <= CATCH_UP_WINDOW,
            "catch-up must happen inside the two-minute window, got {delay:?}"
        );
    }

    #[test]
    fn catch_up_never_exceeds_the_window_however_large_the_jitter() {
        let input = ScheduleInput {
            last_attempt: ago(8 * 3_600),
            ..hourly()
        };
        let decision = decide(&input, now(), Duration::from_secs(9_999));
        assert_eq!(decision, Decision::CatchUp(CATCH_UP_WINDOW));
    }

    #[test]
    fn a_pause_is_honoured_and_the_loop_sleeps_until_it_lifts() {
        let input = ScheduleInput {
            last_attempt: ago(8 * 3_600),
            paused_until: Some(now() + Duration::from_secs(30)),
            ..hourly()
        };
        assert_eq!(
            decide_now(&input),
            Decision::Idle(Duration::from_secs(30)),
            "an overdue backup must still respect a pause"
        );
    }

    #[test]
    fn a_pause_expires_by_itself() {
        let input = ScheduleInput {
            last_attempt: ago(8 * 3_600),
            paused_until: Some(now() - Duration::from_secs(1)),
            ..hourly()
        };
        assert!(
            matches!(decide_now(&input), Decision::CatchUp(_)),
            "an expired pause must stop applying without anyone lifting it"
        );
    }

    #[test]
    fn a_long_pause_does_not_cost_a_wake_up_per_minute() {
        let input = ScheduleInput {
            paused_until: Some(now() + Duration::from_secs(4 * 3_600)),
            ..hourly()
        };
        assert_eq!(decide_now(&input), Decision::Idle(MAX_SLEEP));
    }

    #[test]
    fn a_manual_schedule_never_fires() {
        let input = ScheduleInput {
            interval: None,
            last_attempt: ago(90 * 24 * 3_600),
            ..hourly()
        };
        assert_eq!(decide_now(&input), Decision::Idle(MAX_SLEEP));
    }

    #[test]
    fn an_unconfigured_machine_never_fires() {
        // Nothing to back up to. Firing would produce an error an hour, forever.
        let input = ScheduleInput {
            configured: false,
            last_attempt: None,
            ..hourly()
        };
        assert_eq!(decide_now(&input), Decision::Idle(MAX_SLEEP));
    }

    #[test]
    fn a_configured_machine_that_has_never_run_backs_up_immediately() {
        let input = ScheduleInput {
            last_attempt: None,
            ..hourly()
        };
        assert_eq!(decide_now(&input), Decision::Run);
    }

    #[test]
    fn a_busy_daemon_does_not_queue_a_second_backup() {
        let input = ScheduleInput {
            last_attempt: ago(8 * 3_600),
            busy: true,
            ..hourly()
        };
        assert_eq!(decide_now(&input), Decision::Idle(MAX_SLEEP));
    }

    #[test]
    fn the_loop_never_sleeps_for_zero() {
        // A due time a millisecond away must not become a spin.
        let input = ScheduleInput {
            last_attempt: Some(now() - HOUR + Duration::from_millis(1)),
            ..hourly()
        };
        assert_eq!(decide_now(&input), Decision::Idle(MIN_SLEEP));
    }

    #[test]
    fn a_full_cadence_advances_one_interval_at_a_time() {
        // Walk a day of hourly backups, feeding each run's time back in as the
        // next attempt. The point is that Run happens once per interval — no
        // double-fires, no skipped hours.
        let mut clock = now();
        let mut input = ScheduleInput {
            last_attempt: Some(clock),
            ..hourly()
        };
        let mut runs = 0;
        for _ in 0..(24 * 60) {
            clock += Duration::from_secs(60);
            match decide(&input, clock, Duration::ZERO) {
                Decision::Run => {
                    runs += 1;
                    input.last_attempt = Some(clock);
                }
                Decision::Idle(_) => {}
                Decision::CatchUp(_) => panic!("an uninterrupted cadence never catches up"),
            }
        }
        assert_eq!(runs, 24, "one backup per hour over a day");
    }

    #[test]
    fn jitter_stays_inside_the_window() {
        for _ in 0..100 {
            assert!(jitter() < CATCH_UP_WINDOW);
        }
    }

    #[test]
    fn the_development_override_is_ignored_outside_development() {
        // Belt and braces around an environment variable that shortens backup
        // intervals: it must be inert unless BACKTRACK_DEV is set. The test
        // reads whatever the environment already has rather than mutating it,
        // since setting environment variables races other tests in-process.
        let config = Config::default();
        if std::env::var_os("BACKTRACK_DEV").is_none() {
            assert_eq!(interval_for(&config), Some(HOUR));
        }
    }

    #[test]
    fn a_manual_frequency_has_no_interval_even_in_development() {
        let mut config = Config::default();
        config.backup.frequency = backtrack_core::config::Frequency::Manual;
        assert_eq!(interval_for(&config), None);
    }
}
