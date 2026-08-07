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
    /// Whether the loop has already served a [`Decision::CatchUp`] delay and is
    /// coming back to act on it.
    ///
    /// Without this the catch-up never happens. A run that is overdue by more
    /// than a period is deferred; the loop sleeps the jitter and asks again; and
    /// because nothing has advanced the attempt clock in the meantime it is
    /// *still* overdue by more than a period, so it is deferred again, forever.
    /// A laptop that was closed for two hours would never back up again.
    pub catch_up_deferred: bool,
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

/// How often `borg compact` runs. Compaction rewrites the repository to reclaim
/// the space pruned archives were holding; it is worth doing regularly and not
/// worth doing often.
pub const COMPACT_EVERY: Duration = Duration::from_secs(24 * 3_600);

/// Whether to compact now.
///
/// The stage plan asks for "daily, 03:00-ish, off-peak, skipped if a job is
/// active". Two of those three are here exactly. The wall-clock hour is not,
/// deliberately: knowing when 03:00 is *locally* costs a timezone database, and
/// the property behind the requirement — never compact while the user's backup
/// is competing for the same repository — is the one that is implemented, by
/// requiring an idle daemon. Worth revisiting at Stage 6, which needs local
/// dates for the timeline sidebar and will bring the means with it.
///
/// A daemon that has never compacted is not due immediately: the first start
/// records the clock and the first compaction happens a day later. Compacting a
/// repository that has just been adopted, possibly over a slow link, before a
/// single backup has been taken, would be work with nothing to reclaim.
pub fn compact_due(last_compact: Option<SystemTime>, now: SystemTime, busy: bool) -> bool {
    if busy {
        return false;
    }
    last_compact.is_some_and(|last| {
        now.duration_since(last)
            .is_ok_and(|age| age >= COMPACT_EVERY)
    })
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
    // suspend, hibernation, or a daemon that was not there. Defer briefly, but
    // only once: the deferral is there to let the network and the disk come
    // back, not to postpone the backup indefinitely.
    if now >= due + interval && !input.catch_up_deferred {
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
    Some(apply_dev_override(
        configured,
        std::env::var_os("BACKTRACK_DEV").is_some(),
        std::env::var("BACKTRACK_DEV_INTERVAL_SECS").ok().as_deref(),
    ))
}

/// The override rule, separated from the environment so every branch is
/// reachable from a test. Anything unusable — absent, unparseable, zero — leaves
/// the configured interval alone. A development hook that could silently switch
/// scheduled backups off would be worse than no hook at all.
fn apply_dev_override(configured: Duration, dev: bool, raw: Option<&str>) -> Duration {
    if !dev {
        return configured;
    }
    match raw.map(str::parse::<u64>) {
        Some(Ok(secs)) if secs > 0 => {
            debug!(seconds = secs, "development backup interval in force");
            Duration::from_secs(secs)
        }
        _ => configured,
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
        // Carried across ticks: a deferred catch-up has to be *acted on* next
        // time round, not deferred again. See `ScheduleInput::catch_up_deferred`.
        let mut deferred = false;
        loop {
            let (sleep_for, defer_now) = self.tick(deferred).await;
            deferred = defer_now;
            tokio::select! {
                _ = tokio::time::sleep(sleep_for) => {}
                _ = self.wake.notified() => debug!("scheduler woken early"),
            }
        }
    }

    /// One pass: decide, act, and report how long to wait next and whether a
    /// catch-up is now owed.
    async fn tick(&self, deferred: bool) -> (Duration, bool) {
        let mut input = self.shared.schedule_input();
        input.catch_up_deferred = deferred;

        // Maintenance first, and only when nothing else wants the repository.
        // If it starts, the backup decision below sees a busy daemon and waits
        // — which is the intended precedence: a compaction that keeps being
        // deferred by the hourly backup would never run at all.
        if input.configured {
            self.shared.maybe_compact().await;
        }

        match decide(&input, SystemTime::now(), jitter()) {
            Decision::Idle(d) => (d, false),
            Decision::CatchUp(d) => {
                info!(
                    delay_secs = d.as_secs(),
                    "a scheduled backup was missed; catching up shortly"
                );
                (d, true)
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
                // A full sleep either way. A started job wakes the loop when it
                // finishes, and a declined one is woken by the power and
                // connection watchers — polling faster would only mean a
                // laptop left on battery reading two D-Bus properties every few
                // seconds all afternoon.
                (MAX_SLEEP, false)
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
            catch_up_deferred: false,
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
    fn a_deferred_catch_up_actually_runs() {
        // The bug this pins was found by running the daemon, not by a test: a
        // run overdue by more than a period was deferred, and on the next tick
        // it was *still* overdue by more than a period, so it was deferred
        // again — with a fresh jitter each time, forever. A laptop closed for
        // two hours would never have backed up again, which is precisely the
        // case the catch-up exists for.
        let input = ScheduleInput {
            last_attempt: ago(8 * 3_600),
            ..hourly()
        };
        assert!(matches!(decide_now(&input), Decision::CatchUp(_)));

        let after_waiting = ScheduleInput {
            catch_up_deferred: true,
            ..input
        };
        assert_eq!(
            decide_now(&after_waiting),
            Decision::Run,
            "once the delay has been served, the backup has to happen"
        );
    }

    #[test]
    fn a_served_catch_up_still_respects_the_other_gates() {
        // "Act on it now" must not mean "ignore everything else": a pause the
        // user set still holds, and a busy repository still waits.
        let base = ScheduleInput {
            last_attempt: ago(8 * 3_600),
            catch_up_deferred: true,
            ..hourly()
        };
        let paused = ScheduleInput {
            paused_until: Some(now() + Duration::from_secs(30)),
            ..base.clone()
        };
        assert_eq!(decide_now(&paused), Decision::Idle(Duration::from_secs(30)));

        let busy = ScheduleInput { busy: true, ..base };
        assert_eq!(decide_now(&busy), Decision::Idle(MAX_SLEEP));
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
    fn compaction_waits_a_day_between_runs() {
        let yesterday = now() - Duration::from_secs(25 * 3_600);
        let recent = now() - Duration::from_secs(3_600);
        assert!(compact_due(Some(yesterday), now(), false));
        assert!(!compact_due(Some(recent), now(), false));
    }

    #[test]
    fn compaction_never_competes_with_a_running_job() {
        // It takes an exclusive repository lock; starting one behind a backup
        // would simply queue a repository rewrite in front of the next backup.
        let yesterday = now() - Duration::from_secs(25 * 3_600);
        assert!(!compact_due(Some(yesterday), now(), true));
    }

    #[test]
    fn a_daemon_that_has_never_compacted_does_not_start_by_compacting() {
        // A freshly-adopted repository, possibly across a slow link, with
        // nothing yet to reclaim.
        assert!(!compact_due(None, now(), false));
    }

    #[test]
    fn jitter_stays_inside_the_window() {
        for _ in 0..100 {
            assert!(jitter() < CATCH_UP_WINDOW);
        }
    }

    #[test]
    fn the_development_override_is_inert_outside_development() {
        // An environment variable that shortens backup intervals must not do
        // anything on an installed daemon that happens to inherit it.
        assert_eq!(apply_dev_override(HOUR, false, Some("20")), HOUR);
    }

    #[test]
    fn the_development_override_shortens_the_interval() {
        assert_eq!(
            apply_dev_override(HOUR, true, Some("20")),
            Duration::from_secs(20)
        );
    }

    #[test]
    fn development_mode_without_an_override_keeps_the_configured_interval() {
        // The bug this pins: an early version propagated the absent variable out
        // of the whole function as "no interval", which is the encoding for
        // manual-only. Every development daemon silently stopped backing up on
        // schedule, and did so without a single line in the log.
        assert_eq!(apply_dev_override(HOUR, true, None), HOUR);
    }

    #[test]
    fn an_unusable_override_is_ignored_rather_than_obeyed() {
        // Zero would mean "back up continuously"; the rest cannot mean anything.
        assert_eq!(apply_dev_override(HOUR, true, Some("0")), HOUR);
        assert_eq!(apply_dev_override(HOUR, true, Some("soon")), HOUR);
        assert_eq!(apply_dev_override(HOUR, true, Some("")), HOUR);
        assert_eq!(apply_dev_override(HOUR, true, Some("-5")), HOUR);
    }

    #[test]
    fn a_manual_frequency_has_no_interval_even_in_development() {
        let mut config = Config::default();
        config.backup.frequency = backtrack_core::config::Frequency::Manual;
        assert_eq!(interval_for(&config), None);
    }
}
