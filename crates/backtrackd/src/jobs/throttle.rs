// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Rate limiting for progress signals.
//!
//! Borg emits progress far faster than anything needs to see it — a backup of
//! many small files can produce thousands of events a second. Forwarding each
//! one as a D-Bus signal would spend more time waking the GUI than backing up,
//! so progress is capped at 4 Hz, which is already past the point where a
//! progress bar looks smooth.
//!
//! Two rules keep the cap from losing information that matters:
//!
//! - **The last event always goes out.** Dropping the final tick is how a
//!   progress bar ends up frozen at 97% next to a finished job.
//! - **Phase changes always go out.** "Archiving" → "Checking" is a change of
//!   what the user is being told, not a finer-grained number, and it happens
//!   once per phase rather than thousands of times.
//!
//! The clock is a parameter rather than read inside, so the tests assert the
//! exact cadence without sleeping through it.

use std::time::{Duration, Instant};

/// The cap: at most one progress signal every 250 ms.
pub const MIN_INTERVAL: Duration = Duration::from_millis(250);

/// Decides which progress events reach the bus.
#[derive(Debug, Clone)]
pub struct ProgressThrottle {
    min_interval: Duration,
    last_emit: Option<Instant>,
    last_phase: Option<String>,
}

impl Default for ProgressThrottle {
    fn default() -> ProgressThrottle {
        ProgressThrottle::new(MIN_INTERVAL)
    }
}

impl ProgressThrottle {
    /// A throttle with an explicit interval. Production uses
    /// [`ProgressThrottle::default`]; tests vary it.
    pub fn new(min_interval: Duration) -> ProgressThrottle {
        ProgressThrottle {
            min_interval,
            last_emit: None,
            last_phase: None,
        }
    }

    /// Whether a progress event in `phase` observed at `now` should be emitted.
    ///
    /// Call once per event and act on the answer: the throttle records the
    /// emission, so asking twice about the same event lets the second one
    /// through on a later tick that never happened.
    pub fn should_emit(&mut self, now: Instant, phase: &str) -> bool {
        let phase_changed = self.last_phase.as_deref() != Some(phase);
        let due = match self.last_emit {
            None => true,
            Some(last) => now.duration_since(last) >= self.min_interval,
        };
        if phase_changed || due {
            self.last_emit = Some(now);
            self.last_phase = Some(phase.to_string());
            true
        } else {
            false
        }
    }

    /// Force the next [`ProgressThrottle::should_emit`] through, whatever the
    /// clock says. Used for a job's final progress event so the bar lands on
    /// its true finishing value.
    pub fn release(&mut self) {
        self.last_emit = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_event_always_goes_out() {
        let mut t = ProgressThrottle::default();
        assert!(t.should_emit(Instant::now(), "archiving"));
    }

    #[test]
    fn events_inside_the_window_are_dropped() {
        let mut t = ProgressThrottle::default();
        let start = Instant::now();
        assert!(t.should_emit(start, "archiving"));
        assert!(!t.should_emit(start + Duration::from_millis(1), "archiving"));
        assert!(!t.should_emit(start + Duration::from_millis(249), "archiving"));
    }

    #[test]
    fn the_cap_is_four_per_second() {
        let mut t = ProgressThrottle::default();
        let start = Instant::now();
        // One event per millisecond for a second — a slow imitation of what
        // Borg does on a tree of small files.
        let emitted = (0..1000)
            .filter(|ms| t.should_emit(start + Duration::from_millis(*ms), "archiving"))
            .count();
        assert_eq!(emitted, 4, "expected 4 Hz, got {emitted} in one second");
    }

    #[test]
    fn a_phase_change_is_never_withheld() {
        let mut t = ProgressThrottle::default();
        let start = Instant::now();
        assert!(t.should_emit(start, "archiving"));
        // Well inside the window, but the user-visible phase changed.
        assert!(t.should_emit(start + Duration::from_millis(5), "checking"));
        // And the new phase is then throttled like any other.
        assert!(!t.should_emit(start + Duration::from_millis(6), "checking"));
    }

    #[test]
    fn release_lets_the_final_event_through() {
        let mut t = ProgressThrottle::default();
        let start = Instant::now();
        assert!(t.should_emit(start, "archiving"));
        assert!(!t.should_emit(start + Duration::from_millis(10), "archiving"));
        t.release();
        assert!(
            t.should_emit(start + Duration::from_millis(11), "archiving"),
            "the last tick must reach the UI or the bar freezes short of the end"
        );
    }

    #[test]
    fn the_window_is_measured_from_the_last_emission_not_the_last_event() {
        let mut t = ProgressThrottle::default();
        let start = Instant::now();
        assert!(t.should_emit(start, "archiving"));
        // A flood of dropped events must not push the next emission out.
        for ms in 1..250 {
            t.should_emit(start + Duration::from_millis(ms), "archiving");
        }
        assert!(t.should_emit(start + Duration::from_millis(250), "archiving"));
    }
}
