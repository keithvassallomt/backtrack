// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Rewind's timeline down the right-hand edge (S18-T5), as mockups 25 and 26
//! show it: a tick for the folder as it is now and for each backup listed for
//! it, labelled by day and by month, magnified near the pointer as a dock is,
//! with a marker that follows the stack as it moves; and beside it the two
//! step arrows. Where each thing goes is `model::timeline`'s; this draws it
//! and answers the pointer.
//!
//! The ticks are drawn, not widgets, so a screen reader is given a list of
//! the backups instead: an item at each tick, named as hovering names it, the
//! one in front selected and the list's active item (S18-T8). The list takes
//! the keyboard in its turn, so Tab reaches it, and stepping with it focused
//! reads each backup as the list's item. In high contrast the ticks and
//! labels are drawn solid.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gdk, glib, graphene, gsk, pango};
use libadwaita as adw;

use crate::model::{rewind, timeline};

/// The room the timeline takes on the right of the screen, which the stack
/// keeps clear (`rewind::layout`).
const WIDTH: i32 = 230;

/// From the right edge of the screen to the ticks' right ends.
const EDGE: f32 = 26.0;

/// From the ticks' right ends to the labels' right ends.
const LABEL_SET_BACK: f32 = 50.0;

/// From the ticks' right ends to the marker's point.
const MARKER_SET_BACK: f32 = 44.0;

/// How far left of its right end a tick answers the pointer.
const TICK_REACH: f64 = 64.0;

/// The arrows' distance from the room's left edge.
const ARROWS_INSET: i32 = 6;

/// How quickly the magnification follows the pointer: the time it takes to
/// come most of the way, in seconds. The prototype eases over a tenth.
const EASE: f64 = 0.04;

/// A backup, as its tick shows it.
#[derive(Debug, Clone)]
pub struct Tick {
    /// When it was made; `None` for the folder as it is now.
    pub ts: Option<i64>,
    /// How it is marked, by the sidebar's style class for it.
    pub mark: Option<&'static str>,
    /// What hovering it says.
    pub name: String,
}

#[derive(Default)]
struct State {
    ticks: Vec<Tick>,
    /// Each tick's group, front first (`timeline::group`).
    groups: Vec<String>,
    /// Where the stack is drawn, in backups, and the backup it is going to.
    position: f64,
    target: usize,
    /// The pointer's height, while it is over the timeline.
    pointer: Option<f64>,
    /// The tick under the pointer.
    hovered: Option<usize>,
    /// Each tick's magnification as drawn, on its way to the pointer's.
    shown: Vec<f64>,
    easing: bool,
    last_frame: Option<i64>,
    chosen: Option<Rc<dyn Fn(usize)>>,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Timeline {
        pub(super) state: RefCell<State>,
        pub(super) arrows: RefCell<Option<gtk4::Box>>,
        /// The backups as a list, for a screen reader: an item at each tick.
        pub(super) list: RefCell<Option<gtk4::Fixed>>,
        pub(super) items: RefCell<Vec<gtk4::Box>>,
        /// The size the items were last placed for.
        pub(super) placed: std::cell::Cell<(i32, i32)>,
        /// Redrawing when high contrast is switched, until this goes.
        pub(super) contrast: RefCell<Option<glib::SignalHandlerId>>,
        pub(super) older: RefCell<Option<gtk4::Button>>,
        pub(super) newer: RefCell<Option<gtk4::Button>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Timeline {
        const NAME: &'static str = "BacktrackRewindTimeline";
        type Type = super::Timeline;
        type ParentType = gtk4::Widget;
    }

    impl ObjectImpl for Timeline {
        fn constructed(&self) {
            self.parent_constructed();
            let older = step_button("pan-up-symbolic", "Older (Up)", "Older");
            let newer = step_button("pan-down-symbolic", "Newer (Down)", "Newer");
            let arrows = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
            arrows.append(&older);
            arrows.append(&newer);
            let list = gtk4::Fixed::builder()
                .accessible_role(gtk4::AccessibleRole::List)
                .can_target(false)
                .focusable(true)
                .build();
            list.update_property(&[gtk4::accessible::Property::Label("Backups")]);
            list.set_parent(&*self.obj());
            *self.list.borrow_mut() = Some(list);
            arrows.set_parent(&*self.obj());
            *self.arrows.borrow_mut() = Some(arrows);
            *self.older.borrow_mut() = Some(older);
            *self.newer.borrow_mut() = Some(newer);
        }

        fn dispose(&self) {
            if let Some(arrows) = self.arrows.borrow_mut().take() {
                arrows.unparent();
            }
            if let Some(list) = self.list.borrow_mut().take() {
                list.unparent();
            }
            if let Some(handler) = self.contrast.borrow_mut().take() {
                adw::StyleManager::default().disconnect(handler);
            }
        }
    }

    impl WidgetImpl for Timeline {
        fn measure(&self, orientation: gtk4::Orientation, _: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk4::Orientation::Horizontal => (WIDTH, WIDTH, -1, -1),
                _ => (0, 0, -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            if let Some(list) = self.list.borrow().clone() {
                let _ = list.measure(gtk4::Orientation::Horizontal, -1);
                list.allocate(width, height, -1, None);
                // The items go where the ticks are, which depends on the
                // size; moved after this allocation, not during it.
                if self.placed.get() != (width, height) {
                    self.placed.set((width, height));
                    let timeline = self.obj().downgrade();
                    glib::idle_add_local_once(move || {
                        if let Some(timeline) = timeline.upgrade() {
                            timeline.place_items();
                        }
                    });
                }
            }
            let Some(arrows) = self.arrows.borrow().clone() else {
                return;
            };
            let (_, w, _, _) = arrows.measure(gtk4::Orientation::Horizontal, -1);
            let (_, h, _, _) = arrows.measure(gtk4::Orientation::Vertical, w);
            // Beside the middle of the window in front, as the prototype
            // puts them; the card's height depends on the screen's alone.
            let cy = rewind::layout(f64::from(width.max(WIDTH)) * 8.0, f64::from(height)).cy;
            let y = (cy as i32 - h / 2).clamp(0, (height - h).max(0));
            arrows.allocate(
                w,
                h,
                -1,
                Some(
                    gsk::Transform::new()
                        .translate(&graphene::Point::new(ARROWS_INSET as f32, y as f32)),
                ),
            );
        }

        fn snapshot(&self, snapshot: &gtk4::Snapshot) {
            self.obj().draw(snapshot);
            if let Some(list) = self.list.borrow().as_ref() {
                self.obj().snapshot_child(list, snapshot);
            }
            if let Some(arrows) = self.arrows.borrow().as_ref() {
                self.obj().snapshot_child(arrows, snapshot);
            }
        }
    }
}

glib::wrapper! {
    pub struct Timeline(ObjectSubclass<imp::Timeline>)
        @extends gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl Default for Timeline {
    fn default() -> Self {
        Self::new()
    }
}

fn step_button(icon: &str, tooltip: &str, name: &str) -> gtk4::Button {
    let button = gtk4::Button::from_icon_name(icon);
    button.add_css_class("rewind-step");
    button.set_tooltip_text(Some(tooltip));
    button.update_property(&[gtk4::accessible::Property::Label(name)]);
    // The keyboard stays with the window in front.
    button.set_focus_on_click(false);
    button
}

fn rgba(red: f32, green: f32, blue: f32, alpha: f32) -> gdk::RGBA {
    gdk::RGBA::new(red, green, blue, alpha)
}

/// A marked tick's colour, the prototype's: Snapper's and local snapshots
/// yellow, the Trash's entries and backups only on this computer red.
fn mark_colour(mark: &str) -> gdk::RGBA {
    match mark {
        "snapper" | "local-snapshot" => rgba(0.965, 0.827, 0.176, 1.0),
        _ => rgba(1.0, 0.482, 0.388, 1.0),
    }
}

impl Timeline {
    pub fn new() -> Timeline {
        let timeline: Timeline = glib::Object::new();
        timeline.set_halign(gtk4::Align::End);
        timeline.set_vexpand(true);
        timeline.set_has_tooltip(true);
        timeline.listen();
        let weak = timeline.downgrade();
        let handler = adw::StyleManager::default().connect_high_contrast_notify(move |_| {
            if let Some(timeline) = weak.upgrade() {
                timeline.queue_draw();
            }
        });
        *timeline.imp().contrast.borrow_mut() = Some(handler);
        timeline
    }

    /// The folder as it is now and its backups, front first, grouped as at
    /// `now`.
    pub fn set_ticks(&self, ticks: Vec<Tick>, now: i64, tz: &glib::TimeZone) {
        let mut state = self.imp().state.borrow_mut();
        state.groups = ticks
            .iter()
            .map(|tick| timeline::group(tick.ts, now, tz))
            .collect();
        state.shown = vec![0.0; ticks.len()];
        state.ticks = ticks;
        state.hovered = None;
        state.target = state.target.min(state.ticks.len().saturating_sub(1));
        drop(state);
        self.list_ticks();
        self.arrows_follow();
        self.ease();
        self.queue_draw();
    }

    /// An item in the list for each tick, named for its backup.
    fn list_ticks(&self) {
        let Some(list) = self.imp().list.borrow().clone() else {
            return;
        };
        for item in self.imp().items.borrow_mut().drain(..) {
            list.remove(&item);
        }
        let state = self.imp().state.borrow();
        let items: Vec<gtk4::Box> = state
            .ticks
            .iter()
            .enumerate()
            .map(|(index, tick)| {
                let item = gtk4::Box::builder()
                    .accessible_role(gtk4::AccessibleRole::ListItem)
                    .can_target(false)
                    .width_request(TICK_REACH as i32)
                    .height_request(8)
                    .build();
                item.update_property(&[gtk4::accessible::Property::Label(&tick.name)]);
                item.update_state(&[gtk4::accessible::State::Selected(Some(
                    index == state.target,
                ))]);
                list.put(&item, 0.0, 0.0);
                item
            })
            .collect();
        drop(state);
        *self.imp().items.borrow_mut() = items;
        self.place_items();
        self.point_at_front();
    }

    /// The list's active item: the backup in front.
    fn point_at_front(&self) {
        let target = self.imp().state.borrow().target;
        if let (Some(list), Some(item)) = (
            self.imp().list.borrow().as_ref(),
            self.imp().items.borrow().get(target),
        ) {
            list.update_relation(&[gtk4::accessible::Relation::ActiveDescendant(
                item.upcast_ref(),
            )]);
        }
    }

    /// Whether the list of backups has the keyboard, where stepping is read
    /// as its items.
    pub fn list_has_focus(&self) -> bool {
        self.imp()
            .list
            .borrow()
            .as_ref()
            .is_some_and(|list| list.has_focus())
    }

    /// Each item where its tick is drawn.
    fn place_items(&self) {
        let Some(list) = self.imp().list.borrow().clone() else {
            return;
        };
        let band = self.band();
        let right = f64::from(self.width()) - f64::from(EDGE);
        let items = self.imp().items.borrow();
        let count = items.len();
        for (index, item) in items.iter().enumerate() {
            let y = timeline::y_of(index, count, band) - 4.0;
            list.move_(item, (right - TICK_REACH).max(0.0), y.max(0.0));
        }
    }

    /// Where the stack is drawn, in backups, and the backup it is going to.
    pub fn set_position(&self, position: f64, target: usize) {
        let mut state = self.imp().state.borrow_mut();
        state.position = position;
        let before = std::mem::replace(&mut state.target, target);
        drop(state);
        if before != target {
            let items = self.imp().items.borrow();
            for (index, item) in [(before, false), (target, true)]
                .into_iter()
                .filter_map(|(index, on)| items.get(index).map(|item| (on, item)))
            {
                item.update_state(&[gtk4::accessible::State::Selected(Some(index))]);
            }
            drop(items);
            self.point_at_front();
        }
        self.arrows_follow();
        self.queue_draw();
    }

    /// Call `f` with the backup whose tick is clicked.
    pub fn connect_chosen(&self, f: impl Fn(usize) + 'static) {
        self.imp().state.borrow_mut().chosen = Some(Rc::new(f));
    }

    /// Call `f` with 1 when the older arrow is clicked, -1 for the newer.
    pub fn connect_step(&self, f: impl Fn(isize) + 'static) {
        let f: Rc<dyn Fn(isize)> = Rc::new(f);
        for (button, steps) in [
            (self.imp().older.borrow().clone(), 1),
            (self.imp().newer.borrow().clone(), -1),
        ] {
            if let Some(button) = button {
                let f = Rc::clone(&f);
                button.connect_clicked(move |_| f(steps));
            }
        }
    }

    /// No older backup at the oldest, nothing newer than now.
    fn arrows_follow(&self) {
        let state = self.imp().state.borrow();
        let (target, count) = (state.target, state.ticks.len());
        drop(state);
        if let Some(older) = self.imp().older.borrow().as_ref() {
            older.set_sensitive(target + 1 < count);
        }
        if let Some(newer) = self.imp().newer.borrow().as_ref() {
            newer.set_sensitive(target > 0);
        }
    }

    fn band(&self) -> (f64, f64) {
        timeline::band(f64::from(self.height()))
    }

    /// The tick the pointer at `x`, `y` is on, if any.
    fn tick_under(&self, x: f64, y: f64) -> Option<usize> {
        let right = f64::from(self.width()) - f64::from(EDGE);
        if x < right - TICK_REACH {
            return None;
        }
        let count = self.imp().state.borrow().ticks.len();
        timeline::tick_at(y, count, self.band())
    }

    /// Hovering magnifies and names; clicking flies the stack.
    fn listen(&self) {
        let motion = gtk4::EventControllerMotion::new();
        motion.connect_motion(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |_, x, y| this.pointer_at(Some((x, y)))
        ));
        motion.connect_leave(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |_| this.pointer_at(None)
        ));
        self.add_controller(motion);

        let click = gtk4::GestureClick::new();
        click.connect_released(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |_, _, x, y| {
                let Some(index) = this.tick_under(x, y) else {
                    return;
                };
                let chosen = this.imp().state.borrow().chosen.clone();
                if let Some(chosen) = chosen {
                    chosen(index);
                }
            }
        ));
        self.add_controller(click);

        self.connect_query_tooltip(|this, x, y, _, tooltip| {
            let Some(index) = this.tick_under(f64::from(x), f64::from(y)) else {
                return false;
            };
            let state = this.imp().state.borrow();
            let Some(tick) = state.ticks.get(index) else {
                return false;
            };
            tooltip.set_text(Some(&tick.name));
            let tick_y = timeline::y_of(index, state.ticks.len(), this.band()) as i32;
            let right = this.width() - EDGE as i32;
            tooltip.set_tip_area(&gdk::Rectangle::new(
                right - TICK_REACH as i32,
                tick_y - 4,
                TICK_REACH as i32,
                8,
            ));
            true
        });
    }

    fn pointer_at(&self, at: Option<(f64, f64)>) {
        let hovered = at.and_then(|(x, y)| self.tick_under(x, y));
        let mut state = self.imp().state.borrow_mut();
        state.pointer = at.map(|(_, y)| y);
        let changed = state.hovered != hovered;
        state.hovered = hovered;
        drop(state);
        if changed {
            self.set_cursor_from_name(hovered.map(|_| "pointer"));
        }
        self.ease();
    }

    /// Each tick's magnification for where the pointer is.
    fn wanted(&self) -> Vec<f64> {
        let band = self.band();
        let state = self.imp().state.borrow();
        let count = state.ticks.len();
        (0..count)
            .map(|index| {
                state.pointer.map_or(0.0, |y| {
                    timeline::magnification(timeline::y_of(index, count, band) - y)
                })
            })
            .collect()
    }

    /// Bring the magnification to where the pointer wants it, over a few
    /// frames, or at once with animations off.
    fn ease(&self) {
        let animate = gtk4::Settings::default().is_none_or(|s| s.is_gtk_enable_animations());
        if !animate {
            let wanted = self.wanted();
            self.imp().state.borrow_mut().shown = wanted;
            self.queue_draw();
            return;
        }
        let mut state = self.imp().state.borrow_mut();
        if state.easing {
            return;
        }
        state.easing = true;
        state.last_frame = None;
        drop(state);
        self.add_tick_callback(|this, clock| {
            let now = clock.frame_time();
            let wanted = this.wanted();
            let mut state = this.imp().state.borrow_mut();
            let seconds = state
                .last_frame
                .map_or(1.0 / 60.0, |last| ((now - last) as f64 / 1e6).min(0.1));
            state.last_frame = Some(now);
            let k = 1.0 - (-seconds / EASE).exp();
            let mut settled = state.shown.len() == wanted.len();
            if !settled {
                state.shown = vec![0.0; wanted.len()];
            }
            for (shown, wanted) in state.shown.iter_mut().zip(&wanted) {
                *shown += (wanted - *shown) * k;
                if (*shown - wanted).abs() < 0.002 {
                    *shown = *wanted;
                } else {
                    settled = false;
                }
            }
            state.easing = !settled;
            drop(state);
            this.queue_draw();
            if settled {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    fn draw(&self, snapshot: &gtk4::Snapshot) {
        let state = self.imp().state.borrow();
        let count = state.ticks.len();
        if count == 0 {
            return;
        }
        let band = self.band();
        let spacing = timeline::spacing(count, band);
        let right = self.width() as f32 - EDGE;
        let style = adw::StyleManager::default();
        let accent = style.accent_color_rgba();
        // High contrast: solid ticks and labels, nothing faded.
        let solid = style.is_high_contrast();
        let white = rgba(1.0, 1.0, 1.0, 1.0);
        let shown = |index: usize| state.shown.get(index).copied().unwrap_or(0.0) as f32;
        let y_of = |index: usize| timeline::y_of(index, count, band) as f32;

        // The labels: today, yesterday and the months, light on the
        // wallpaper with a shadow under them, growing near the pointer.
        let font = self.pango_context().font_description().unwrap_or_default();
        for index in timeline::labelled(&state.groups, spacing) {
            let m = shown(index);
            let now = index == 0;
            let layout = self.create_pango_layout(Some(&state.groups[index]));
            let mut font = font.clone();
            font.set_absolute_size(12.0 * f64::from(pango::SCALE));
            font.set_weight(if now {
                pango::Weight::Ultrabold
            } else {
                pango::Weight::Semibold
            });
            layout.set_font_description(Some(&font));
            let (w, h) = layout.pixel_size();
            let scale = 1.0 + 0.3 * m;
            snapshot.save();
            snapshot.translate(&graphene::Point::new(right - LABEL_SET_BACK, y_of(index)));
            snapshot.scale(scale, scale);
            snapshot.push_opacity(if now || solid {
                1.0
            } else {
                f64::from(0.85 + 0.15 * m)
            });
            snapshot.push_shadow(&[gsk::Shadow::new(rgba(0.0, 0.0, 0.0, 0.7), 0.0, 1.0, 3.0)]);
            snapshot.translate(&graphene::Point::new(-(w as f32), -(h as f32) / 2.0));
            snapshot.append_layout(&layout, &white);
            snapshot.pop();
            snapshot.pop();
            snapshot.restore();
        }

        // The ticks, longer and thicker near the pointer; the one in front
        // longest, in the accent colour.
        for (index, tick) in state.ticks.iter().enumerate() {
            let m = shown(index);
            let current = index == state.target;
            let (w, h) = if current {
                (40.0, 3.0 + 1.5 * m)
            } else {
                (20.0 + 22.0 * m, 2.0 + 1.5 * m)
            };
            let colour = if current {
                accent
            } else if let Some(mark) = tick.mark {
                mark_colour(mark)
            } else if state.hovered == Some(index) {
                white
            } else {
                rgba(1.0, 1.0, 1.0, if solid { 1.0 } else { 0.45 })
            };
            let rect = graphene::Rect::new(right - w, y_of(index) - h / 2.0, w, h);
            snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(rect, 1.0));
            snapshot.append_color(&colour, &rect);
            snapshot.pop();
        }

        // The marker, where the stack is drawn, between ticks as it moves.
        let position = state.position.clamp(0.0, (count - 1) as f64);
        let y = if count < 2 {
            band.1
        } else {
            band.0 + ((count - 1) as f64 - position) * spacing
        } as f32;
        let point = right - MARKER_SET_BACK;
        let path = gsk::PathBuilder::new();
        path.move_to(point - 9.0, y - 6.0);
        path.line_to(point, y);
        path.line_to(point - 9.0, y + 6.0);
        path.close();
        snapshot.push_shadow(&[gsk::Shadow::new(rgba(0.0, 0.0, 0.0, 0.4), 0.0, 1.0, 2.0)]);
        snapshot.append_fill(&path.to_path(), gsk::FillRule::Winding, &accent);
        snapshot.pop();
    }
}
