// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Rewind, the full-screen view (Stage 18): how its stack moves, where each
//! backup in it is drawn, and what is said under it.
//!
//! The stack goes straight back into the distance, as mockups 25 and 26 show.
//! Its measures are those of the S18-T1 prototype Keith settled on
//! (`backtrack_plan/mockups/rewind/rewind.html`, 2026-10-27), given here in
//! card heights rather than the prototype's pixels, so that they hold on any
//! screen.
//!
//! The stack's position counts backups from the front: 0 is the folder as it
//! is now, 1 the newest backup listed for it, and so on back. While it moves
//! the position is fractional, and it always comes to rest on a whole one.

/// How far back the eye is from the front card, in card heights: the
/// prototype's 1300 px of perspective over its 620 px card.
pub const PERSPECTIVE: f64 = 2.1;

/// How many backups are drawn behind the front one when frames keep up.
pub const DEEPEST: usize = 9;

/// How many at the fewest, when they do not.
pub const SHALLOWEST: usize = 3;

/// One step's duration from rest. Keith found the prototype's first motion far
/// too fast, and settled on this.
pub const STEP_SECONDS: f64 = 0.8;

/// Where the front card goes, in logical pixels: its size and its centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub width: f64,
    pub height: f64,
    pub cx: f64,
    pub cy: f64,
}

/// Room kept on the right for the timeline (S18-T5), and below for the lines
/// and the buttons (S18-T6), as the prototype keeps it.
const RIGHT: f64 = 230.0;
const BELOW: f64 = 96.0;

/// Where the front card goes on a stage `width` by `height`: the prototype's
/// 1040 by 620 at most, smaller on a smaller screen, low in the middle of the
/// room left of the timeline so the stack has the height above it to go back
/// into.
pub fn layout(width: f64, height: f64) -> Layout {
    let h = 620f64.min((height - BELOW) * 0.62).max(0.0);
    let w = 1040f64.min((width - RIGHT) * 0.7).min(h * 1.75).max(0.0);
    Layout {
        width: w,
        height: h,
        cx: (width - RIGHT) * 0.5,
        cy: height - BELOW - h / 2.0 - 16.0,
    }
}

/// Where a card is drawn, `d` backups behind the front (negative for one that
/// has come past the front and flies at the viewer as it fades).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Up from where the front card stands, in card heights.
    pub rise: f64,
    /// Away from the viewer, in card heights.
    pub depth: f64,
    /// How much it is darkened, from 0 (not at all) to 1.
    pub dim: f64,
    pub opacity: f64,
}

/// Where the card `d` backups behind the front is drawn, with `deepest` the
/// most drawn behind: the last few fade out rather than stop.
pub fn place(d: f64, deepest: usize) -> Placement {
    let fade_out = ((deepest as f64 - d) / 2.5).clamp(0.0, 1.0);
    if d >= 0.0 {
        Placement {
            rise: d * 0.026,
            depth: d * 0.42,
            dim: (d * 0.09).min(0.7),
            opacity: fade_out,
        }
    } else {
        Placement {
            rise: d * 0.113,
            depth: d * 0.84,
            dim: 0.0,
            opacity: (1.0 + d * 1.7).clamp(0.0, 1.0),
        }
    }
}

/// Whether the card `d` backups behind the front is drawn at all.
pub fn drawn(d: f64, deepest: usize) -> bool {
    d > -1.0 && d < deepest as f64 && place(d, deepest).opacity > 0.001
}

/// The cards to make, most wanted first, with the stack at `position` on its
/// way to `target`, `deepest` drawn behind the front, among `count` cards.
///
/// The cards the stack comes to rest among come first, so that it lands on
/// cards with their contents. While it moves, the cards along the way follow:
/// the next to come into view first, as they are on the screen longest, then
/// those in view, from where they came in.
pub fn to_make(position: f64, target: usize, deepest: usize, count: usize) -> Vec<usize> {
    let mut order: Vec<usize> = std::iter::once(target)
        .chain((1..=deepest).map(|d| target + d))
        .chain(target.checked_sub(1))
        .collect();
    let here = position.floor() as isize;
    let deepest = deepest as isize;
    if target as f64 > position {
        // Going back: cards come into view at the far end of the stack.
        order.extend(
            [here + deepest + 1, here + deepest + 2]
                .into_iter()
                .chain((here..=here + deepest).rev())
                .filter_map(|slot| usize::try_from(slot).ok()),
        );
    } else if (target as f64) < position {
        // Coming forward: they come into view from in front of the eye.
        order.extend(
            [here - 1, here - 2]
                .into_iter()
                .chain(here..=here + deepest)
                .filter_map(|slot| usize::try_from(slot).ok()),
        );
    }
    let mut seen = std::collections::HashSet::new();
    order.retain(|slot| *slot < count && seen.insert(*slot));
    order
}

/// Whether a card already made is kept, with the stack at `position` on its
/// way to `target`: those near either are, so that the cards on the screen as
/// a flight sets off keep their contents.
pub fn kept(slot: usize, position: f64, target: usize) -> bool {
    let near = |at: usize| (at.saturating_sub(2)..=at + DEEPEST + 2).contains(&slot);
    near(target) || near(position.max(0.0).floor() as usize)
}

/// The stack's motion.
///
/// From rest a move eases in and out (cubic, as `ADW_EASE_IN_OUT_CUBIC`):
/// slow, fast, then slow, over a step's duration for one backup and somewhat
/// longer for a longer flight. A move begun while the stack is still moving
/// keeps the speed it had and eases out, so that a held key glides instead of
/// stopping at each backup. Times are in seconds on any steady clock.
#[derive(Debug, Clone)]
pub struct Motion {
    position: f64,
    velocity: f64,
    target: usize,
    oldest: usize,
    tween: Option<Tween>,
}

#[derive(Debug, Clone, Copy)]
struct Tween {
    from: f64,
    to: f64,
    /// The speed it started at, times its duration, which is what the curve
    /// that keeps it uses.
    momentum: f64,
    start: f64,
    seconds: f64,
    from_rest: bool,
}

impl Motion {
    /// At rest at `at`, of a stack whose oldest backup is at `oldest`.
    pub fn new(at: usize, oldest: usize) -> Motion {
        let at = at.min(oldest);
        Motion {
            position: at as f64,
            velocity: 0.0,
            target: at,
            oldest,
            tween: None,
        }
    }

    pub fn position(&self) -> f64 {
        self.position
    }

    /// Backups a second, positive going back in time.
    pub fn velocity(&self) -> f64 {
        self.velocity
    }

    /// The backup in front, or on its way there.
    pub fn target(&self) -> usize {
        self.target
    }

    pub fn oldest(&self) -> usize {
        self.oldest
    }

    pub fn is_moving(&self) -> bool {
        self.tween.is_some()
    }

    /// Set off for `index`.
    pub fn go_to(&mut self, index: usize, now: f64) {
        let index = index.min(self.oldest);
        if index == self.target {
            return;
        }
        self.target = index;
        let span = index as f64 - self.position;
        let seconds = STEP_SECONDS * (1.0 + 0.35 * span.abs().max(1.0).ln());
        let from_rest = self.velocity.abs() < 0.05;
        // Never so fast at the start that it would sail past the backup.
        let limit = 2.5 * span.abs();
        self.tween = Some(Tween {
            from: self.position,
            to: index as f64,
            momentum: (self.velocity * seconds).clamp(-limit, limit),
            start: now,
            seconds,
            from_rest,
        });
    }

    /// `steps` backups older (positive) or newer. A step the keyboard repeats
    /// while a key is held goes at most two backups ahead of the stack, so the
    /// stack glides at its own pace instead of racing off.
    pub fn step(&mut self, steps: isize, repeat: bool, now: f64) {
        let wanted = (self.target as isize + steps).clamp(0, self.oldest as isize) as usize;
        if repeat && (wanted as f64 - self.position).abs() > 2.0 {
            return;
        }
        self.go_to(wanted, now);
    }

    /// Straight to `index`, with no motion: animations are off.
    pub fn jump(&mut self, index: usize) {
        let index = index.min(self.oldest);
        self.target = index;
        self.position = index as f64;
        self.velocity = 0.0;
        self.tween = None;
    }

    /// Where the stack is at `now`.
    pub fn advance(&mut self, now: f64) {
        let Some(t) = self.tween else {
            return;
        };
        let s = ((now - t.start) / t.seconds).clamp(0.0, 1.0);
        let span = t.to - t.from;
        if t.from_rest {
            let (eased, slope) = if s < 0.5 {
                (4.0 * s * s * s, 12.0 * s * s)
            } else {
                let r = 1.0 - s;
                (1.0 - 4.0 * r * r * r, 12.0 * r * r)
            };
            self.position = t.from + span * eased;
            self.velocity = span * slope / t.seconds;
        } else {
            // A cubic from where it was, at the speed it had, to the backup,
            // at rest.
            let (s2, s3) = (s * s, s * s * s);
            self.position = (2.0 * s3 - 3.0 * s2 + 1.0) * t.from
                + (s3 - 2.0 * s2 + s) * t.momentum
                + (-2.0 * s3 + 3.0 * s2) * t.to;
            self.velocity = ((6.0 * s2 - 6.0 * s) * t.from
                + (3.0 * s2 - 4.0 * s + 1.0) * t.momentum
                + (-6.0 * s2 + 6.0 * s) * t.to)
                / t.seconds;
        }
        if s >= 1.0 {
            self.position = t.to;
            self.velocity = 0.0;
            self.tween = None;
        }
    }
}

/// How many backups are drawn behind the front, from how frames are keeping
/// up: when they run late, fewer, rather than the motion stuttering; when they
/// keep up again for a while, one more at a time.
#[derive(Debug, Clone)]
pub struct Budget {
    deepest: usize,
    late: Vec<bool>,
    on_time: u32,
}

/// Frames looked back over, and how many of them late are too many.
const WINDOW: usize = 12;
const TOO_MANY_LATE: usize = 3;
/// On-time frames in a row before one more is drawn again.
const BEFORE_DEEPER: u32 = 120;

impl Default for Budget {
    fn default() -> Budget {
        Budget {
            deepest: DEEPEST,
            late: Vec::with_capacity(WINDOW),
            on_time: 0,
        }
    }
}

impl Budget {
    pub fn deepest(&self) -> usize {
        self.deepest
    }

    /// A frame drawn while the stack moved, `interval` seconds after the one
    /// before, on a display that refreshes every `period` seconds. A frame
    /// half as late again as it should be is late.
    pub fn frame(&mut self, interval: f64, period: f64) {
        let late = interval > period * 1.5;
        if self.late.len() == WINDOW {
            self.late.remove(0);
        }
        self.late.push(late);
        if late {
            self.on_time = 0;
        } else {
            self.on_time += 1;
        }
        if self.late.iter().filter(|l| **l).count() >= TOO_MANY_LATE && self.deepest > SHALLOWEST {
            self.deepest -= 1;
            self.late.clear();
        } else if self.on_time >= BEFORE_DEEPER && self.deepest < DEEPEST {
            self.deepest += 1;
            self.on_time = 0;
        }
    }
}

/// The two lines under the stack: which backup is in front, and where it is
/// among them.
pub fn lines(
    index: usize,
    ts: Option<i64>,
    backups: usize,
    filtered: bool,
    in_trash: bool,
    tz: &glib::TimeZone,
) -> (String, String) {
    let Some(ts) = ts.filter(|_| index > 0) else {
        return (
            "Today (Now)".to_owned(),
            "The folder as it is now".to_owned(),
        );
    };
    let when = super::format::at(ts, tz, "%a %e %b %Y, %H:%M")
        .trim_start()
        .to_owned();
    let where_ = if in_trash {
        "In the Trash".to_owned()
    } else if filtered {
        format!("Backup {index} of {backups} with changes to this folder")
    } else {
        format!("Backup {index} of {backups}")
    };
    (when, where_)
}

/// The stars behind the stack, where the desktop's wallpaper cannot be shown:
/// points in a box in front of the eye, which come at it as the stack goes
/// back in time and recede as it comes forward.
#[derive(Debug, Clone)]
pub struct Starfield {
    pub stars: Vec<Star>,
}

/// A star: across and up from the centre (each from -1 to 1), and how far
/// away (0 at the eye, 1 at the back).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Star {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// The nearest a star comes before it is sent to the back again.
const NEAREST: f64 = 0.04;

impl Starfield {
    /// `count` stars, placed by `random`, which gives numbers from 0 to 1.
    pub fn new(count: usize, mut random: impl FnMut() -> f64) -> Starfield {
        let stars = (0..count)
            .map(|_| Star {
                x: random() * 2.0 - 1.0,
                y: random() * 2.0 - 1.0,
                z: NEAREST + random(),
            })
            .collect();
        Starfield { stars }
    }

    /// How fast the stars move, in depths a second, for a stack moving at
    /// `velocity` backups a second: a slow drift at rest.
    pub fn speed(velocity: f64) -> f64 {
        0.012 + velocity * 0.2
    }

    /// Move every star on by `seconds` at `speed`. One that comes past the
    /// eye goes to the back, and one that goes past the back comes to the
    /// front, each somewhere new.
    pub fn advance(&mut self, seconds: f64, speed: f64, mut random: impl FnMut() -> f64) {
        for star in &mut self.stars {
            star.z -= speed * seconds;
            if star.z <= NEAREST || star.z > 1.0 + NEAREST {
                star.x = random() * 2.0 - 1.0;
                star.y = random() * 2.0 - 1.0;
                star.z = if star.z <= NEAREST {
                    1.0 + NEAREST - 0.001
                } else {
                    NEAREST + 0.001
                };
            }
        }
    }

    /// Where a star falls on a screen `size` across (its larger side), from
    /// the point it all streams from, and how big and bright it is.
    pub fn project(star: &Star, size: f64) -> (f64, f64, f64, f64) {
        let scale = size * 0.5;
        let x = star.x / star.z * scale;
        let y = star.y / star.z * scale;
        let near = (1.0 + NEAREST - star.z).max(0.0);
        let radius = near * 1.8 + 0.25;
        let alpha = (near * 1.3).clamp(0.0, 1.0);
        (x, y, radius, alpha)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(motion: &mut Motion, from: f64, to: f64) -> Vec<(f64, f64)> {
        let mut seen = Vec::new();
        let mut t = from;
        while t <= to + 1e-9 {
            motion.advance(t);
            seen.push((t, motion.position()));
            t += 1.0 / 60.0;
        }
        seen
    }

    #[test]
    fn a_step_from_rest_eases_in_and_out_and_lands_on_the_backup() {
        let mut motion = Motion::new(0, 40);
        motion.go_to(1, 10.0);
        motion.advance(10.0 + STEP_SECONDS * 0.2);
        let early = motion.position();
        motion.advance(10.0 + STEP_SECONDS * 0.5);
        let middle = motion.position();
        motion.advance(10.0 + STEP_SECONDS * 0.8);
        let late = motion.position();
        assert!(early < 0.05, "slow to start: {early}");
        assert!(
            (middle - 0.5).abs() < 1e-9,
            "half way at half time: {middle}"
        );
        assert!(late > 0.95, "slow to finish: {late}");
        motion.advance(10.0 + STEP_SECONDS);
        assert_eq!(motion.position(), 1.0);
        assert!(!motion.is_moving());
        assert_eq!(motion.velocity(), 0.0);
    }

    #[test]
    fn a_longer_flight_takes_somewhat_longer() {
        let mut motion = Motion::new(0, 40);
        motion.go_to(40, 0.0);
        motion.advance(STEP_SECONDS * 2.0);
        assert!(
            motion.is_moving(),
            "forty backups take more than two steps' time"
        );
        motion.advance(STEP_SECONDS * (1.0 + 0.35 * 40f64.ln()));
        assert_eq!(motion.position(), 40.0);
    }

    #[test]
    fn a_held_key_glides_without_stopping_and_never_runs_far_ahead() {
        let mut motion = Motion::new(0, 40);
        motion.step(1, false, 0.0);
        let mut t = 0.0;
        let mut last = 0.0;
        let mut slowest = f64::MAX;
        // A keyboard's repeat: after half a second, thirty a second, for three.
        while t < 3.5 {
            if t >= 0.5 {
                motion.step(1, true, t);
            }
            motion.advance(t);
            assert!(motion.position() >= last - 1e-9, "never backs up");
            assert!(motion.target() as f64 - motion.position() <= 2.0 + 1e-9);
            if t > 1.0 {
                slowest = slowest.min(motion.velocity());
            }
            last = motion.position();
            t += 1.0 / 30.0;
        }
        assert!(slowest > 0.5, "never comes to a stop on the way: {slowest}");
        assert!(last > 5.0, "gets somewhere: {last}");
        // Let go: it settles on a whole backup.
        run(&mut motion, t, t + 3.0);
        assert_eq!(motion.position().fract(), 0.0);
        assert!(!motion.is_moving());
    }

    #[test]
    fn turning_back_mid_flight_is_smooth_and_ends_where_asked() {
        let mut motion = Motion::new(5, 40);
        motion.go_to(6, 0.0);
        motion.advance(0.4);
        let before = motion.velocity();
        motion.go_to(4, 0.4);
        motion.advance(0.4 + 1e-6);
        assert!((motion.velocity() - before).abs() < 0.05, "no jolt");
        run(&mut motion, 0.4, 3.0);
        assert_eq!(motion.position(), 4.0);
    }

    #[test]
    fn the_ends_hold_and_a_jump_goes_straight_there() {
        let mut motion = Motion::new(0, 3);
        motion.step(-1, false, 0.0);
        assert!(!motion.is_moving(), "nothing newer than now");
        motion.go_to(9, 0.0);
        assert_eq!(motion.target(), 3, "nothing older than the oldest");
        motion.jump(2);
        assert_eq!((motion.position(), motion.target()), (2.0, 2));
        assert!(!motion.is_moving());
    }

    #[test]
    fn the_front_card_keeps_its_size_and_leaves_room_for_the_timeline() {
        let big = layout(2560.0, 1440.0);
        assert_eq!((big.width, big.height), (1040.0, 620.0));
        assert_eq!(big.cx, (2560.0 - RIGHT) / 2.0);
        assert!(
            big.cy + big.height / 2.0 < 1440.0 - BELOW,
            "clear of the lines below"
        );
        assert!(
            big.cy - big.height / 2.0 > 1440.0 * 0.3,
            "room above to go back into"
        );
        let small = layout(1280.0, 720.0);
        assert!(small.height < 620.0 && small.width <= small.height * 1.75);
        assert!(small.cx + small.width / 2.0 < 1280.0 - RIGHT + 1.0);
    }

    #[test]
    fn cards_behind_go_further_back_higher_and_darker() {
        let front = place(0.0, DEEPEST);
        assert_eq!(
            (front.rise, front.depth, front.dim, front.opacity),
            (0.0, 0.0, 0.0, 1.0)
        );
        let one = place(1.0, DEEPEST);
        let five = place(5.0, DEEPEST);
        assert!(five.depth > one.depth && five.rise > one.rise && five.dim > one.dim);
        assert!(place(20.0, DEEPEST).dim <= 0.7, "darkened, never black");
        // The last ones fade out rather than stop.
        assert!(place(8.0, DEEPEST).opacity < 1.0);
        assert!(!drawn(9.0, DEEPEST));
        assert!(!drawn(5.0, 4), "fewer drawn when frames run late");
        // One come past the front flies at the viewer and is gone by -0.6.
        let past = place(-0.3, DEEPEST);
        assert!(past.depth < 0.0 && past.opacity < 1.0);
        assert!(!drawn(-0.6, DEEPEST));
    }

    #[test]
    fn the_cards_landed_on_are_made_first_then_those_along_the_way() {
        // At rest: the front card, those behind it, and the one in front.
        let mut at_rest: Vec<usize> = (4..=13).collect();
        at_rest.push(3);
        assert_eq!(to_make(4.0, 4, DEEPEST, 50), at_rest);
        assert_eq!(
            to_make(0.0, 0, DEEPEST, 3),
            vec![0, 1, 2],
            "only cards there are"
        );

        // Going back, on the way from 0 to 30: the cards at 30 first, then the
        // next two to come into view at the far end, then those in view, from
        // the back.
        let going_back = to_make(5.3, 30, DEEPEST, 50);
        let mut expected: Vec<usize> = (30..=39).collect();
        expected.extend([29, 15, 16, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5]);
        assert_eq!(going_back, expected);

        // Coming forward, on the way from 30 to 2: the next two to come into
        // view in front, then those in view, from the front.
        let coming_forward = to_make(20.4, 2, DEEPEST, 50);
        let mut expected: Vec<usize> = (2..=11).collect();
        expected.push(1);
        expected.extend([19, 18]);
        expected.extend(20..=29);
        assert_eq!(coming_forward, expected);

        // Near the end, nothing past it, and nothing twice.
        let near_the_end = to_make(45.5, 49, DEEPEST, 50);
        assert!(near_the_end.iter().all(|slot| *slot < 50));
        let mut unique = near_the_end.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), near_the_end.len());
    }

    #[test]
    fn cards_near_the_stack_or_where_it_is_going_are_kept() {
        // Setting off from 5 for 30: the cards on the screen stay made.
        assert!((3..=16).all(|slot| kept(slot, 5.3, 30)));
        assert!((28..=41).all(|slot| kept(slot, 5.3, 30)));
        assert!(!kept(22, 5.3, 30), "neither here nor there");
        assert!(!kept(1, 5.3, 30));
        assert!(kept(0, 0.0, 0));
    }

    #[test]
    fn late_frames_draw_fewer_and_a_good_run_draws_more_again() {
        let period = 1.0 / 60.0;
        let mut budget = Budget::default();
        for _ in 0..30 {
            budget.frame(period, period);
        }
        assert_eq!(budget.deepest(), DEEPEST, "on time");
        for _ in 0..3 {
            budget.frame(period * 2.0, period);
        }
        assert_eq!(budget.deepest(), DEEPEST - 1);
        for _ in 0..60 {
            budget.frame(period * 3.0, period);
        }
        assert_eq!(budget.deepest(), SHALLOWEST, "never fewer than three");
        for _ in 0..BEFORE_DEEPER {
            budget.frame(period, period);
        }
        assert_eq!(budget.deepest(), SHALLOWEST + 1);
        // One slightly slow frame is not late.
        let mut steady = Budget::default();
        for _ in 0..30 {
            steady.frame(period * 1.3, period);
        }
        assert_eq!(steady.deepest(), DEEPEST);
    }

    #[test]
    fn the_lines_say_which_backup_and_where_among_them() {
        let utc = glib::TimeZone::utc();
        // 2026-10-21 18:00 UTC, a Wednesday.
        let ts = 1_792_605_600;
        assert_eq!(
            lines(0, Some(ts), 41, false, false, &utc),
            ("Today (Now)".into(), "The folder as it is now".into())
        );
        assert_eq!(
            lines(12, Some(ts), 41, false, false, &utc),
            ("Wed 21 Oct 2026, 18:00".into(), "Backup 12 of 41".into())
        );
        assert_eq!(
            lines(3, Some(ts), 7, true, false, &utc).1,
            "Backup 3 of 7 with changes to this folder"
        );
        assert_eq!(lines(3, Some(ts), 7, false, true, &utc).1, "In the Trash");
    }

    #[test]
    fn stars_come_at_the_eye_going_back_and_recede_going_forward() {
        let mut next = 0.37;
        let mut random = move || {
            next = (next * 997.0 + 0.131) % 1.0;
            next
        };
        let mut field = Starfield::new(200, &mut random);
        // How many stars came nearer in a step, and how many went further: a
        // star that passes the eye goes to the back, so the few that do go
        // further whichever way the stack moves.
        let mut moved = |field: &mut Starfield, velocity: f64| {
            let before: Vec<f64> = field.stars.iter().map(|s| s.z).collect();
            field.advance(0.1, Starfield::speed(velocity), &mut random);
            let nearer = field
                .stars
                .iter()
                .zip(&before)
                .filter(|(s, z)| s.z < **z)
                .count();
            (nearer, before.len() - nearer)
        };
        let (nearer, further) = moved(&mut field, 3.0);
        assert!(
            nearer > further * 5,
            "going back: {nearer} nearer, {further} further"
        );
        let (nearer, further) = moved(&mut field, -6.0);
        assert!(
            further > nearer * 5,
            "going forward: {nearer} nearer, {further} further"
        );
        assert!(field
            .stars
            .iter()
            .all(|s| s.z > NEAREST && s.z <= 1.0 + NEAREST));
        let near = Starfield::project(
            &Star {
                x: 0.5,
                y: 0.0,
                z: 0.1,
            },
            1000.0,
        );
        let far = Starfield::project(
            &Star {
                x: 0.5,
                y: 0.0,
                z: 0.9,
            },
            1000.0,
        );
        assert!(near.0 > far.0, "nearer is further out from the centre");
        assert!(near.2 > far.2 && near.3 > far.3, "bigger and brighter");
    }
}
