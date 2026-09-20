// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The timeline density strip.
//!
//! Research said the slider is the wrong primary control for a backup
//! timeline — uneven spacing squashes the hours you care about most, and
//! picking one snapshot out of a cluster is a precision task sliders are bad
//! at. So this is not the primary control. It is the view of the whole history
//! that a list cannot give: where backups are dense, where they are thin, and
//! where there is a week with nothing.
//!
//! As a control it is deliberately coarse, and it never lands between
//! snapshots. It is also fully operable from the keyboard, which is the part of
//! a slider that is usually left out: focus it and the arrow keys step one
//! backup at a time, Home and End go to the ends.
//!
//! Under the pointer it magnifies, dock-fashion, and names the day it is
//! offering. A strip of identical bars gives no reason to believe a click will
//! land anywhere in particular; growing the day under the cursor and saying
//! what it is turns a guess into a choice made before committing to it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{
    gdk, glib, Align, Box as GtkBox, DrawingArea, EventControllerKey, EventControllerMotion, Fixed,
    GestureClick, GestureDrag, Label, Orientation, Overlay, Widget,
};

use libadwaita as adw;

use crate::model::density::{self, Density};
use crate::model::{day_to_date, format};
use crate::state::{AppState, Change};

/// How tall the bars are drawn, in pixels.
const BAR_AREA_HEIGHT: i32 = 52;

/// The tallest a bar is drawn at rest, as a fraction of the height. The
/// headroom is what magnification grows into.
const REST_HEIGHT: f64 = 0.62;

/// How much of the height the bars may occupy at their largest. The rest is
/// left clear for the readout, which sits over them.
const TOP_BAND: f64 = 0.84;

/// Bar widths. A month of backups on a wide window would otherwise be drawn as
/// slabs, and a year of them as hairlines — so a bar takes a share of the space
/// it is given, between these bounds.
///
/// The share matters more than it sounds. A fixed 9px bar in a 96px slot is a
/// tick mark with a lot of nothing either side, and a row of those reads as a
/// ruler rather than as a history.
const MIN_BAR_WIDTH: f64 = 2.0;
const MAX_BAR_WIDTH: f64 = 22.0;
const BAR_SHARE: f64 = 0.6;

/// How many slots either side of the pointer the magnification reaches, and how
/// much it adds at the centre.
const REACH: f64 = 2.6;
const PEAK: f64 = 0.85;

/// The strip, its labels, and what it is currently drawing.
pub struct Strip {
    container: GtkBox,
    area: DrawingArea,
    months: Fixed,
    /// The day named under the pointer, and what holds it over the right bar.
    readout: Label,
    perch: Fixed,
    density: RefCell<Density>,
    /// Which bar the position marker sits on.
    marker: RefCell<Option<usize>>,
    /// Which bar the pointer is over, as a fractional slot position so the
    /// magnification moves smoothly rather than in bar-sized steps.
    hover: Cell<Option<f64>>,
    state: Rc<AppState>,
}

/// Build the density strip for `state`.
pub fn build(state: &Rc<AppState>) -> Rc<Strip> {
    let area = DrawingArea::builder()
        .content_height(BAR_AREA_HEIGHT)
        .hexpand(true)
        .focusable(true)
        .accessible_role(gtk4::AccessibleRole::Slider)
        .build();
    // The strip is a control, and a bar chart does not look like one.
    area.set_cursor(gdk::Cursor::from_name("pointer", None).as_ref());

    let readout = Label::builder().visible(false).build();
    readout.add_css_class("density-readout");
    readout.add_css_class("accent");

    // The readout is placed in a `GtkFixed` rather than by its own margin.
    // That is not a stylistic preference: `gtk_widget_measure` includes a
    // widget's margins in what it returns, so a label positioned by
    // `margin-start` measures wider the further right it has been put — and a
    // placement that subtracts half that width drifts further left the further
    // right the pointer goes, which is exactly what it did.
    let perch = Fixed::new();
    perch.set_valign(Align::Start);
    perch.put(&readout, 0.0, 0.0);

    let overlay = Overlay::new();
    overlay.set_child(Some(&area));
    overlay.add_overlay(&perch);

    let months = Fixed::builder().height_request(16).build();

    let container = GtkBox::new(Orientation::Vertical, 0);
    container.add_css_class("density-strip");
    container.append(&overlay);
    container.append(&months);

    let strip = Rc::new(Strip {
        container,
        area,
        months,
        readout,
        perch,
        density: RefCell::new(Density::default()),
        marker: RefCell::new(None),
        hover: Cell::new(None),
        state: Rc::clone(state),
    });

    let painter = Rc::clone(&strip);
    strip
        .area
        .set_draw_func(move |area, context, width, height| {
            painter.draw(area, context, width, height);
        });

    // Click, and drag, both resolve to the nearest real snapshot.
    let clicker = Rc::clone(&strip);
    let click = GestureClick::new();
    click.connect_pressed(move |gesture, _, x, _| {
        if let Some(widget) = gesture.widget() {
            widget.grab_focus();
            clicker.jump_to(x, widget.width());
        }
    });
    strip.area.add_controller(click);

    let dragger = Rc::clone(&strip);
    let drag = GestureDrag::new();
    drag.connect_drag_update(move |gesture, offset_x, _| {
        let Some((start_x, _)) = gesture.start_point() else {
            return;
        };
        if let Some(widget) = gesture.widget() {
            let x = start_x + offset_x;
            dragger.hover_at(x, widget.width());
            dragger.jump_to(x, widget.width());
        }
    });
    strip.area.add_controller(drag);

    let mover = Rc::clone(&strip);
    let motion = EventControllerMotion::new();
    motion.connect_motion(move |controller, x, _| {
        if let Some(widget) = controller.widget() {
            mover.hover_at(x, widget.width());
        }
    });
    let leaver = Rc::clone(&strip);
    motion.connect_leave(move |_| leaver.clear_hover());
    strip.area.add_controller(motion);

    let keys = Rc::clone(&strip);
    let keyboard = EventControllerKey::new();
    keyboard.connect_key_pressed(move |_, key, _, _| keys.on_key(key));
    strip.area.add_controller(keyboard);

    let watcher = Rc::clone(&strip);
    state.subscribe(move |view, change| match change {
        Change::Archives => {
            let tz = glib::TimeZone::local();
            *watcher.density.borrow_mut() = density::density(&view.archives, &tz);
            watcher.relabel();
            watcher.refresh(view);
        }
        Change::Seq => watcher.refresh(view),
        _ => {}
    });

    let relabeller = Rc::clone(&strip);
    strip
        .area
        .connect_resize(move |_, _, _| relabeller.relabel());

    strip
}

impl Strip {
    /// The widget to put in the window.
    pub fn widget(&self) -> Widget {
        self.container.clone().upcast()
    }

    /// Move the marker and tell assistive technology where it now is.
    fn refresh(&self, view: &crate::state::View) {
        let tz = glib::TimeZone::local();
        let marker = view
            .seq
            .and_then(|seq| self.density.borrow().bar_of(&view.archives, seq, &tz));
        *self.marker.borrow_mut() = marker;

        if let Some((ordinal, total)) = view.position() {
            // Counting from the oldest, because that is the direction the strip
            // is drawn in — a screen reader should not be told "1 of 47" for
            // the rightmost end.
            let from_oldest = (total - ordinal) as f64;
            self.area.update_property(&[
                gtk4::accessible::Property::ValueMin(0.0),
                gtk4::accessible::Property::ValueMax((total.saturating_sub(1)) as f64),
                gtk4::accessible::Property::ValueNow(from_oldest),
            ]);
            if let Some(archive) = view.archive() {
                let text = format::position(archive.ts, ordinal, total, &tz);
                self.area
                    .update_property(&[gtk4::accessible::Property::ValueText(&text)]);
            }
        }
        self.area.queue_draw();
    }

    /// Note where the pointer is and name the day it is over.
    fn hover_at(&self, x: f64, width: i32) {
        let bars = self.density.borrow().bars.len();
        if bars == 0 || width <= 0 {
            return;
        }
        let slot = (x / f64::from(width) * bars as f64 - 0.5).clamp(0.0, bars as f64 - 1.0);
        self.hover.set(Some(slot));

        let index = slot.round() as usize;
        if let Some(bar) = self.density.borrow().bars.get(index) {
            self.readout.set_text(&describe(bar));
            self.readout.set_visible(true);
            self.place_readout(index, bars, width);
        }
        // The whole strip, not just the canvas: the readout is a sibling of
        // it, and invalidating only the canvas can leave the label's old
        // position on screen until something else forces a repaint.
        self.container.queue_draw();
    }

    /// Sit the readout above the bar it names, kept inside the widget.
    ///
    /// The bar's centre is computed exactly as `draw` computes it, so the two
    /// cannot drift apart; the measurement is of a label carrying no margins,
    /// so it is the width of the words and nothing else.
    fn place_readout(&self, index: usize, bars: usize, width: i32) {
        let per_bar = f64::from(width) / bars as f64;
        let centre = (index as f64 + 0.5) * per_bar;
        let label_width = f64::from(self.readout.measure(Orientation::Horizontal, -1).1);
        let left =
            (centre - label_width / 2.0).clamp(0.0, (f64::from(width) - label_width).max(0.0));
        self.perch.move_(&self.readout, left, 0.0);
    }

    fn clear_hover(&self) {
        self.hover.set(None);
        self.readout.set_visible(false);
        self.container.queue_draw();
    }

    /// Resolve a horizontal position to a snapshot and go there.
    fn jump_to(&self, x: f64, width: i32) {
        if width <= 0 {
            return;
        }
        let view = self.state.view();
        let tz = glib::TimeZone::local();
        let fraction = x / f64::from(width);
        if let Some(seq) = density::seq_at(&self.density.borrow(), &view.archives, fraction, &tz) {
            self.state.set_seq(seq);
        }
    }

    /// Arrow keys step one backup; Home and End go to the ends of the history.
    fn on_key(&self, key: gdk::Key) -> glib::Propagation {
        let view = self.state.view();
        let target = match key {
            gdk::Key::Left | gdk::Key::Down => view.older(),
            gdk::Key::Right | gdk::Key::Up => view.newer(),
            gdk::Key::Home => view.archives.last().map(|a| a.seq),
            gdk::Key::End => view.archives.first().map(|a| a.seq),
            _ => return glib::Propagation::Proceed,
        };
        if let Some(seq) = target {
            self.state.set_seq(seq);
        }
        glib::Propagation::Stop
    }

    /// Place the month labels under the bars they begin at.
    ///
    /// Real labels in a `GtkFixed` rather than text drawn into the canvas: the
    /// month names come from the locale and may be in any script, and cairo's
    /// own text API is not equipped for that.
    fn relabel(&self) {
        while let Some(child) = self.months.first_child() {
            self.months.remove(&child);
        }
        let density = self.density.borrow();
        let width = f64::from(self.area.width());
        if density.is_empty() || width <= 0.0 {
            return;
        }
        let per_bar = width / density.bars.len() as f64;
        for mark in &density.months {
            let label = Label::new(Some(&mark.text));
            label.add_css_class("caption");
            label.add_css_class("dim-label");
            label.set_halign(Align::Start);
            self.months.put(&label, mark.bar as f64 * per_bar, 0.0);
        }
    }

    /// One bar per day, a line where you are, and a bulge where the pointer is.
    fn draw(&self, area: &DrawingArea, context: &gtk4::cairo::Context, width: i32, height: i32) {
        let density = self.density.borrow();
        if density.is_empty() || width <= 0 {
            return;
        }
        let colour = area.color();
        let paint = |alpha: f64| {
            context.set_source_rgba(
                colour.red().into(),
                colour.green().into(),
                colour.blue().into(),
                alpha,
            );
        };

        let (width, height) = (f64::from(width), f64::from(height));
        let slots = density.bars.len() as f64;
        let per_bar = width / slots;
        let rest_width = (per_bar * BAR_SHARE).clamp(MIN_BAR_WIDTH, MAX_BAR_WIDTH);
        let hover = self.hover.get();
        let marker = *self.marker.borrow();

        // A baseline, so the bars read as standing on a timeline rather than
        // floating in a box. Worth seeing: it is the only thing tying them
        // together when every day holds the same number of backups.
        paint(0.22);
        context.rectangle(0.0, height - 1.5, width, 1.5);
        let _ = context.fill();

        for (index, bar) in density.bars.iter().enumerate() {
            let grow = hover.map_or(1.0, |at| magnify((index as f64 - at).abs()));
            let centre = (index as f64 + 0.5) * per_bar;
            let is_here = marker == Some(index);

            if bar.count == 0 {
                // A day with nothing is drawn as a stub rather than skipped:
                // the gap is the information, and an empty space could just as
                // easily be the end of the history.
                paint(0.10 * grow);
                rounded_bar(context, centre, rest_width * 0.6, 2.0, height);
                let _ = context.fill();
                continue;
            }

            let share = f64::from(bar.count) / f64::from(density.max);
            // A floor, so one backup in a quiet day is still visible next to a
            // day that holds twenty.
            // Weighted towards "this day has backups" over "how many": the
            // count is worth showing, but a day that was protected should not
            // look like a day that nearly wasn't.
            let rest = height * REST_HEIGHT * (0.55 + 0.45 * share);
            // Capped short of the top, which belongs to the readout.
            let bar_height = (rest * grow).min(height * TOP_BAND);
            let bar_width = rest_width * (1.0 + (grow - 1.0) * 0.5);

            let alpha = if is_here {
                0.95
            } else {
                0.26 + 0.34 * (grow - 1.0) / PEAK
            };
            paint(alpha);
            rounded_bar(context, centre, bar_width, bar_height, height);
            let _ = context.fill();
        }

        // Where you are: a full-height line with a cap, in the desktop's own
        // accent colour. The bars stay monochrome on purpose — they have to
        // survive any palette a user has set — but the one mark that answers
        // "where am I?" earns a colour of its own, and taking it from the
        // system means it is a colour the user already chose.
        if let Some(index) = marker {
            let accent = adw::StyleManager::default().accent_color_rgba();
            let centre = (index as f64 + 0.5) * per_bar;
            context.set_source_rgba(
                accent.red().into(),
                accent.green().into(),
                accent.blue().into(),
                1.0,
            );
            context.rectangle(centre - 1.5, 0.0, 3.0, height);
            let _ = context.fill();
            rounded_bar(context, centre, 10.0, 5.0, 5.0);
            let _ = context.fill();
        }
    }
}

/// How much a bar `distance` slots from the pointer grows.
///
/// A Gaussian falloff rather than a step, so the bulge slides along the strip
/// instead of snapping from one bar to the next, and its neighbours move with
/// it. The reach is deliberately a couple of bars: magnifying half the strip
/// would move the very bar the pointer is aiming at.
fn magnify(distance: f64) -> f64 {
    if distance > REACH * 2.5 {
        return 1.0;
    }
    1.0 + PEAK * (-(distance / REACH).powi(2)).exp()
}

/// A bar of `width`, `height` pixels tall, centred on `centre` and standing on
/// `baseline`, with its top corners rounded.
fn rounded_bar(
    context: &gtk4::cairo::Context,
    centre: f64,
    width: f64,
    height: f64,
    baseline: f64,
) {
    let radius = (width / 2.0).min(height / 2.0).min(4.0);
    let (left, right) = (centre - width / 2.0, centre + width / 2.0);
    let top = baseline - height;
    context.new_sub_path();
    context.arc(left + radius, top + radius, radius, PI, 1.5 * PI);
    context.arc(right - radius, top + radius, radius, 1.5 * PI, 2.0 * PI);
    context.line_to(right, baseline);
    context.line_to(left, baseline);
    context.close_path();
}

const PI: f64 = std::f64::consts::PI;

/// What the readout says for a day.
fn describe(bar: &density::Bar) -> String {
    let (year, month, day) = day_to_date(bar.day);
    let tz = glib::TimeZone::local();
    let date = glib::DateTime::new(&tz, year, month, day, 12, 0, 0.0)
        .ok()
        .map(|dt| format::weekday_and_day(dt.to_unix(), &tz))
        .unwrap_or_default();
    match bar.count {
        0 => format!("{date} · no backups"),
        1 => format!("{date} · 1 backup"),
        many => format!("{date} · {many} backups"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_under_the_pointer_grows_the_most() {
        let here = magnify(0.0);
        assert!(here > 1.0);
        assert!(here > magnify(1.0));
        assert!(magnify(1.0) > magnify(2.0));
    }

    #[test]
    fn magnification_runs_out_rather_than_lifting_the_whole_strip() {
        // A bulge that reached everywhere would move the bar being aimed at.
        assert_eq!(magnify(20.0), 1.0);
        assert!(magnify(REACH * 2.0) < 1.02);
    }

    #[test]
    fn the_readout_names_the_day_and_says_how_many() {
        // 2026-06-09 was a Tuesday; the day number is days since the epoch.
        let day = crate::model::date_to_day(2026, 6, 9);
        assert_eq!(
            describe(&density::Bar { day, count: 1 }),
            "Tue 9 Jun · 1 backup"
        );
        assert_eq!(
            describe(&density::Bar { day, count: 4 }),
            "Tue 9 Jun · 4 backups"
        );
        assert_eq!(
            describe(&density::Bar { day, count: 0 }),
            "Tue 9 Jun · no backups"
        );
    }
}
