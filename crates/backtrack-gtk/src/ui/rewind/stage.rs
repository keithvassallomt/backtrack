// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Rewind's stage: what is behind the stack, and the stack.
//!
//! Each card is laid out once as a widget and its drawing kept (see `card`);
//! the stack is those drawings, each scaled and placed as a perspective would
//! show it (see `model::rewind::scale_at`), so nothing is laid out while the
//! stack moves. The drawings are kept as GTK's render nodes rather than
//! rendered into textures: the first texture a process renders can hold the
//! main loop for seconds on some drivers (three, on an Intel laptop), while
//! nodes are drawn with the window's own frames. Two cards are made at a
//! time, the one in front first, so the view opens at once and the cards
//! behind arrive over the next frames. The motion runs on the frame clock,
//! and when frames run late fewer cards are drawn behind (see
//! `model::rewind::Budget`).

use std::cell::RefCell;
use std::collections::HashMap;

use backtrack_core::index::Entry;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gdk, glib, graphene, gsk};

use tracing::debug;

use super::card::{self, Content};
use crate::model::rewind::{self, Budget, Layout, Motion, Starfield};

/// The corners of a card, as Adwaita rounds a window's.
const RADIUS: f32 = 14.0;
/// The shadow drawn with each card, and the room left for it.
const SHADOW_BLUR: f32 = 40.0;
const SHADOW_DROP: f32 = 16.0;
const MARGIN: f32 = 64.0;
/// Cards made at once.
const MAKING: usize = 2;
/// How long a crossfade takes with animations off, in microseconds.
const CROSSFADE: i64 = 220_000;
/// How long the wallpaper takes to come in once it has been drawn.
const WALLPAPER_IN: i64 = 300_000;
/// Stars behind the stack, where the wallpaper cannot be read.
const STARS: usize = 600;

/// What is behind the stack.
pub enum Background {
    /// Not known yet: the dark the window opens on.
    Waiting,
    /// The desktop's own wallpaper, drawn at the screen's size, darkened by
    /// `dim` (0 to 1).
    Wallpaper { texture: gdk::Texture, dim: f32 },
    /// Backtrack's own, where the wallpaper cannot be read: always night.
    Stars,
}

/// A card's drawing, and the card size it was made at.
struct Made {
    node: gsk::RenderNode,
    size: (i32, i32),
}

/// A card being laid out, to be drawn at the next snapshot.
struct Making {
    slot: usize,
    generation: u64,
    widget: gtk4::Box,
    size: (i32, i32),
    laid_out: bool,
    done: bool,
}

#[derive(Default)]
struct State {
    slots: Vec<Content>,
    made: HashMap<usize, Made>,
    making: Vec<Making>,
    motion: Option<Motion>,
    budget: Budget,
    background: Option<Background>,
    /// When the background arrived, for the wallpaper to fade in.
    background_since: i64,
    stars: Option<Starfield>,
    /// With animations off: the card fading out, and when it began.
    fade: Option<(usize, i64)>,
    /// Frames drawn since the view opened, for the log.
    frames: u64,
    /// The last frame drawn while the stack moved, for the budget.
    last_moving_frame: Option<i64>,
    last_tick: Option<i64>,
    /// Whether the last frame drew the wallpaper all the way in. A fade that
    /// ends between frames still needs the frame that shows it ended.
    wallpaper_in: std::cell::Cell<bool>,
    card: (i32, i32),
    /// Hidden files are shown on the cards (S18-T11).
    show_hidden: bool,
    /// Counts changes to how cards are drawn, so that a card laid out before
    /// one is not kept after it.
    generation: u64,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Stage {
        pub(super) state: RefCell<State>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Stage {
        const NAME: &'static str = "BacktrackRewindStage";
        type Type = super::Stage;
        type ParentType = gtk4::Widget;
    }

    impl ObjectImpl for Stage {
        fn dispose(&self) {
            let making: Vec<gtk4::Box> = self
                .state
                .borrow_mut()
                .making
                .drain(..)
                .map(|m| m.widget)
                .collect();
            for widget in making {
                widget.unparent();
            }
        }
    }

    impl WidgetImpl for Stage {
        fn measure(&self, _: gtk4::Orientation, _: i32) -> (i32, i32, i32, i32) {
            (0, 0, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let layout = rewind::layout(f64::from(width), f64::from(height));
            let mut state = self.state.borrow_mut();
            state.card = (layout.width.round() as i32, layout.height.round() as i32);
            let to_lay_out: Vec<(gtk4::Box, (i32, i32))> = state
                .making
                .iter_mut()
                .filter(|m| !m.done)
                .map(|m| {
                    m.laid_out = true;
                    (m.widget.clone(), m.size)
                })
                .collect();
            drop(state);
            for (widget, (w, h)) in to_lay_out {
                widget.allocate(w, h, -1, None);
            }
        }

        fn snapshot(&self, snapshot: &gtk4::Snapshot) {
            self.obj().capture();
            self.obj().draw(snapshot);
        }
    }
}

glib::wrapper! {
    pub struct Stage(ObjectSubclass<imp::Stage>)
        @extends gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl Default for Stage {
    fn default() -> Self {
        Self::new()
    }
}

impl Stage {
    pub fn new() -> Stage {
        let stage: Stage = glib::Object::new();
        stage.set_hexpand(true);
        stage.set_vexpand(true);
        stage.add_tick_callback(|stage, clock| {
            stage.tick(clock);
            glib::ControlFlow::Continue
        });
        stage
    }

    /// The folder at each backup, front first, with the stack at `at`.
    pub fn set_slots(&self, slots: Vec<Content>, at: usize) {
        let mut state = self.imp().state.borrow_mut();
        let oldest = slots.len().saturating_sub(1);
        state.slots = slots;
        state.made.clear();
        state.motion = Some(Motion::new(at, oldest));
    }

    /// Show hidden files on the cards, or stop: every card is made again.
    pub fn set_show_hidden(&self, on: bool) {
        let mut state = self.imp().state.borrow_mut();
        if state.show_hidden == on {
            return;
        }
        state.show_hidden = on;
        state.generation += 1;
        state.made.clear();
        drop(state);
        self.queue_draw();
    }

    pub fn show_hidden(&self) -> bool {
        self.imp().state.borrow().show_hidden
    }

    /// What the folder held at one of the backups, once it has been read.
    pub fn set_entries(&self, slot: usize, entries: Vec<Entry>) {
        let mut state = self.imp().state.borrow_mut();
        if let Some(content) = state.slots.get_mut(slot) {
            content.entries = Some(entries);
            state.made.remove(&slot);
        }
    }

    pub fn set_background(&self, background: Background) {
        let mut state = self.imp().state.borrow_mut();
        if matches!(background, Background::Stars) && state.stars.is_none() {
            state.stars = Some(Starfield::new(STARS, glib::random_double));
        }
        state.background = Some(background);
        state.wallpaper_in.set(false);
        state.background_since = self.now();
        drop(state);
        self.queue_draw();
    }

    /// The backup in front, or on its way there.
    pub fn target(&self) -> usize {
        self.imp()
            .state
            .borrow()
            .motion
            .as_ref()
            .map_or(0, Motion::target)
    }

    /// `steps` backups older (positive) or newer; `repeat` for a key the
    /// keyboard repeats.
    pub fn step(&self, steps: isize, repeat: bool) -> usize {
        self.moved(|motion, now| motion.step(steps, repeat, now))
    }

    pub fn go_to(&self, index: usize) -> usize {
        self.moved(|motion, now| motion.go_to(index, now))
    }

    pub fn oldest(&self) -> usize {
        self.imp()
            .state
            .borrow()
            .motion
            .as_ref()
            .map_or(0, Motion::oldest)
    }

    /// Move, with the motion, or with animations off by a crossfade.
    fn moved(&self, how: impl FnOnce(&mut Motion, f64)) -> usize {
        let now = self.now();
        let animate = gtk4::Settings::default().is_none_or(|s| s.is_gtk_enable_animations());
        let mut state = self.imp().state.borrow_mut();
        let Some(motion) = state.motion.as_mut() else {
            return 0;
        };
        let before = motion.target();
        how(motion, now as f64 / 1e6);
        let after = motion.target();
        if !animate && after != before {
            motion.jump(after);
            state.fade = Some((before, now));
        }
        drop(state);
        self.queue_draw();
        after
    }

    /// The frame clock's time, or the monotonic clock's before there is one.
    fn now(&self) -> i64 {
        self.frame_clock()
            .map_or_else(glib::monotonic_time, |clock| clock.frame_time())
    }

    fn tick(&self, clock: &gdk::FrameClock) {
        let now = clock.frame_time();
        let period = self
            .native()
            .and_then(|n| n.surface())
            .and_then(|s| self.display().monitor_at_surface(&s))
            .map(|m| m.refresh_rate())
            .filter(|rate| *rate > 0)
            .map_or(1.0 / 60.0, |rate| 1000.0 / f64::from(rate));
        let layout = rewind::layout(f64::from(self.width()), f64::from(self.height()));
        let (width, height) = (f64::from(self.width()), f64::from(self.height()));

        let mut guard = self.imp().state.borrow_mut();
        let state = &mut *guard;
        let was_moving = state.motion.as_ref().is_some_and(Motion::is_moving);
        if let Some(motion) = state.motion.as_mut() {
            motion.advance(now as f64 / 1e6);
        }
        if was_moving {
            if let Some(last) = state.last_moving_frame {
                state.budget.frame((now - last) as f64 / 1e6, period);
            }
            state.last_moving_frame = Some(now);
        } else {
            state.last_moving_frame = None;
            state.budget.at_rest(now as f64 / 1e6);
        }
        let since_last = state
            .last_tick
            .map_or(0.0, |last| ((now - last) as f64 / 1e6).min(0.1));
        state.last_tick = Some(now);
        if let (Some(stars), Some(motion)) = (state.stars.as_mut(), state.motion.as_ref()) {
            stars.advance(
                since_last,
                Starfield::speed(motion.velocity()),
                glib::random_double,
            );
            // A star that has gone off the screen comes back somewhere in it.
            let size = width.max(height);
            let (vx, vy) = vanishing(&layout);
            for star in &mut stars.stars {
                let (x, y, _, _) = Starfield::project(star, size);
                if !(-50.0..width + 50.0).contains(&(vx + x))
                    || !(-50.0..height + 50.0).contains(&(vy + y))
                {
                    star.x = glib::random_double() * 2.0 - 1.0;
                    star.y = glib::random_double() * 2.0 - 1.0;
                    star.z = 0.4 + glib::random_double() * 0.6;
                }
            }
        }
        // The frame after a crossfade's end draws it ended: without it, the
        // last frame drawn, partly faded, would stay on the screen.
        let fade_ended = state.fade.is_some_and(|(_, since)| now - since > CROSSFADE);
        if fade_ended {
            state.fade = None;
        }

        // Cards laid out and drawn are let go; the cards the stack needs next
        // are made, those it comes to rest among first (see
        // `rewind::to_make`).
        let done: Vec<gtk4::Box> = state
            .making
            .iter()
            .filter(|m| m.done)
            .map(|m| m.widget.clone())
            .collect();
        state.making.retain(|m| !m.done);
        let target = state.motion.as_ref().map_or(0, Motion::target);
        let position = state.motion.as_ref().map_or(0.0, Motion::position);
        let deepest = state.budget.deepest();
        let fading = state.fade.map(|(slot, _)| slot);
        state
            .made
            .retain(|slot, _| rewind::kept(*slot, position, target) || Some(*slot) == fading);
        let card = state.card;
        let mut wanted = Vec::new();
        if card.0 > 0 && card.1 > 0 {
            for slot in rewind::to_make(position, target, deepest, state.slots.len()) {
                if wanted.len() + state.making.len() >= MAKING {
                    break;
                }
                let current = state.made.get(&slot).is_some_and(|m| m.size == card);
                let busy = state.making.iter().any(|m| m.slot == slot);
                if let Some(content) = state.slots.get(slot) {
                    if !current && !busy {
                        wanted.push((slot, content.clone()));
                    }
                }
            }
        }
        let wallpaper_coming = matches!(state.background, Some(Background::Wallpaper { .. }))
            && !state.wallpaper_in.get();
        // Cards coming back at rest are drawn as they come.
        let returning = !was_moving && state.budget.deepest() < rewind::DEEPEST;
        let busy = was_moving
            || returning
            || state.fade.is_some()
            || fade_ended
            || state.stars.is_some()
            || !wanted.is_empty()
            || wallpaper_coming;
        let (show_hidden, generation) = (state.show_hidden, state.generation);
        drop(guard);
        for widget in done {
            widget.unparent();
        }
        if !wanted.is_empty() {
            let mut making = Vec::new();
            debug!(slots = ?wanted.iter().map(|(slot, _)| slot).collect::<Vec<_>>(), "Rewind is making cards");
            for (slot, content) in wanted {
                let widget = card::build(&content, card.0, card.1, show_hidden);
                widget.set_parent(self);
                making.push(Making {
                    slot,
                    generation,
                    widget,
                    size: card,
                    laid_out: false,
                    done: false,
                });
            }
            self.imp().state.borrow_mut().making.extend(making);
            self.queue_allocate();
        }
        if busy {
            self.queue_draw();
        }
    }

    /// Keep the drawing of every card that has been laid out.
    fn capture(&self) {
        let ready: Vec<(usize, gtk4::Box, (i32, i32), u64)> = self
            .imp()
            .state
            .borrow()
            .making
            .iter()
            .filter(|m| m.laid_out && !m.done)
            .map(|m| (m.slot, m.widget.clone(), m.size, m.generation))
            .collect();
        for (slot, widget, size, generation) in ready {
            let (w, h) = (size.0 as f32, size.1 as f32);
            let snapshot = gtk4::Snapshot::new();
            snapshot.translate(&graphene::Point::new(MARGIN, MARGIN));
            let outline = gsk::RoundedRect::from_rect(graphene::Rect::new(0.0, 0.0, w, h), RADIUS);
            snapshot.append_outset_shadow(
                &outline,
                &gdk::RGBA::new(0.0, 0.0, 0.0, 0.45),
                0.0,
                SHADOW_DROP,
                0.0,
                SHADOW_BLUR,
            );
            self.snapshot_child(&widget, &snapshot);
            let node = snapshot.to_node();
            debug!(slot, made = node.is_some(), "Rewind drew a card");
            let mut state = self.imp().state.borrow_mut();
            // Laid out before hidden files were shown or hidden: made again.
            if let Some(node) = node.filter(|_| generation == state.generation) {
                state.made.insert(slot, Made { node, size });
            }
            if let Some(m) = state.making.iter_mut().find(|m| m.slot == slot) {
                m.done = true;
            }
        }
    }

    fn draw(&self, snapshot: &gtk4::Snapshot) {
        let (width, height) = (self.width() as f32, self.height() as f32);
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let layout = rewind::layout(f64::from(width), f64::from(height));
        let frames = {
            let mut state = self.imp().state.borrow_mut();
            state.frames += 1;
            state.frames
        };
        if frames == 1 {
            debug!(width, height, "Rewind drew its first frame");
        }
        let state = self.imp().state.borrow();
        let now = self.now();
        let full = graphene::Rect::new(0.0, 0.0, width, height);
        snapshot.append_color(&rgba(0x04, 0x05, 0x0a, 1.0), &full);
        match &state.background {
            Some(Background::Wallpaper { texture, dim }) => {
                let shown =
                    ((now - state.background_since) as f64 / WALLPAPER_IN as f64).clamp(0.0, 1.0);
                state.wallpaper_in.set(shown >= 1.0);
                snapshot.push_opacity(shown);
                snapshot.append_texture(texture, &full);
                snapshot.append_color(&rgba(0, 0, 0, *dim), &full);
                snapshot.pop();
            }
            Some(Background::Stars) => {
                let velocity = state.motion.as_ref().map_or(0.0, Motion::velocity);
                if let Some(stars) = &state.stars {
                    draw_stars(snapshot, width, height, &layout, stars, velocity);
                }
            }
            Some(Background::Waiting) | None => {}
        }

        let Some(motion) = &state.motion else {
            return;
        };
        let position = motion.position();
        let deepest = state.budget.deepest();
        let mut order: Vec<(usize, f64)> = (0..state.slots.len())
            .map(|slot| (slot, slot as f64 - position))
            .filter(|(_, d)| rewind::drawn(*d, deepest))
            .collect();
        // The furthest first, so each nearer card is drawn over it.
        order.sort_by(|a, b| b.1.total_cmp(&a.1));
        let fade = state.fade.map(|(slot, since)| {
            (
                slot,
                ((now - since) as f64 / CROSSFADE as f64).clamp(0.0, 1.0),
            )
        });
        for (slot, d) in order {
            let opacity = match fade {
                Some((_, k)) if slot == motion.target() => k,
                _ => 1.0,
            };
            draw_card(
                snapshot,
                width,
                &layout,
                d,
                deepest,
                opacity,
                state.made.get(&slot),
            );
        }
        if let Some((slot, k)) = fade {
            draw_card(
                snapshot,
                width,
                &layout,
                0.0,
                deepest,
                1.0 - k,
                state.made.get(&slot),
            );
        }
    }
}

/// Where the stars stream from: just above the stack's far end.
fn vanishing(layout: &Layout) -> (f64, f64) {
    (layout.cx, layout.cy - layout.height * 0.9)
}

fn rgba(r: u8, g: u8, b: u8, a: f32) -> gdk::RGBA {
    gdk::RGBA::new(
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        a,
    )
}

/// One card, `d` backups behind the front.
fn draw_card(
    snapshot: &gtk4::Snapshot,
    width: f32,
    layout: &Layout,
    d: f64,
    deepest: usize,
    opacity: f64,
    made: Option<&Made>,
) {
    let place = rewind::place(d, deepest);
    let (w, h) = (layout.width as f32, layout.height as f32);
    snapshot.save();
    // The eye looks at the top of the screen's middle: the stack goes back
    // towards it, each card scaled towards that point as a perspective would
    // (see `rewind::scale_at`).
    let scale = rewind::scale_at(place.depth) as f32;
    snapshot.translate(&graphene::Point::new(width / 2.0, 0.0));
    snapshot.scale(scale, scale);
    snapshot.translate(&graphene::Point::new(
        layout.cx as f32 - width / 2.0,
        (layout.cy - place.rise * layout.height) as f32,
    ));
    snapshot.push_opacity(place.opacity * opacity);
    let card = graphene::Rect::new(-w / 2.0, -h / 2.0, w, h);
    let rounded = gsk::RoundedRect::from_rect(card, RADIUS);
    match made {
        Some(made) => {
            // Made at a card size a rounding away from this one, at most.
            let (mw, mh) = (made.size.0 as f32, made.size.1 as f32);
            snapshot.save();
            snapshot.translate(&graphene::Point::new(-w / 2.0, -h / 2.0));
            snapshot.scale(w / mw, h / mh);
            snapshot.translate(&graphene::Point::new(-MARGIN, -MARGIN));
            snapshot.append_node(&made.node);
            snapshot.restore();
        }
        // Not made yet: its outline, so the stack is there from the first
        // frame.
        None => {
            snapshot.push_rounded_clip(&rounded);
            snapshot.append_color(&rgba(255, 255, 255, 0.08), &card);
            snapshot.pop();
        }
    }
    if place.dim > 0.0 {
        snapshot.push_rounded_clip(&rounded);
        snapshot.append_color(&rgba(6, 6, 10, place.dim as f32), &card);
        snapshot.pop();
    }
    snapshot.pop();
    snapshot.restore();
}

/// The night sky, with its stars streaming as the stack moves.
fn draw_stars(
    snapshot: &gtk4::Snapshot,
    width: f32,
    height: f32,
    layout: &Layout,
    stars: &Starfield,
    velocity: f64,
) {
    let radius = width.max(height) * 0.8;
    snapshot.append_radial_gradient(
        &graphene::Rect::new(0.0, 0.0, width, height),
        &graphene::Point::new(width / 2.0, height * 0.3),
        radius,
        radius,
        0.0,
        1.0,
        &[
            gsk::ColorStop::new(0.0, rgba(0x18, 0x20, 0x3a, 1.0)),
            gsk::ColorStop::new(1.0, rgba(0x04, 0x05, 0x0a, 1.0)),
        ],
    );
    let size = f64::from(width.max(height));
    let (vx, vy) = vanishing(layout);
    let speed = Starfield::speed(velocity);
    for star in &stars.stars {
        let (x, y, r, alpha) = Starfield::project(star, size);
        let (sx, sy) = ((vx + x) as f32, (vy + y) as f32);
        if sx < -10.0 || sy < -10.0 || sx > width + 10.0 || sy > height + 10.0 {
            continue;
        }
        let colour = rgba(255, 255, 255, alpha as f32);
        let r = r as f32;
        if speed.abs() > 0.25 {
            // Fast: a streak back along its path.
            let tail = rewind::Star {
                z: star.z + speed * 0.035,
                ..*star
            };
            let (tx, ty, _, _) = Starfield::project(&tail, size);
            let (dx, dy) = ((vx + tx) as f32 - sx, (vy + ty) as f32 - sy);
            let length = dx.hypot(dy).max(r * 2.0);
            snapshot.save();
            snapshot.translate(&graphene::Point::new(sx, sy));
            snapshot.rotate(dy.atan2(dx).to_degrees());
            snapshot.append_color(&colour, &graphene::Rect::new(0.0, -r / 2.0, length, r));
            snapshot.restore();
        } else {
            snapshot.append_color(
                &colour,
                &graphene::Rect::new(sx - r / 2.0, sy - r / 2.0, r, r),
            );
        }
    }
}
