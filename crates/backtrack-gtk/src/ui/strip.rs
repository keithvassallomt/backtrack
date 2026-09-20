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

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{
    gdk, glib, Align, Box as GtkBox, DrawingArea, EventControllerKey, Fixed, GestureClick,
    GestureDrag, Label, Orientation, Widget,
};

use crate::model::density::{self, Density};
use crate::state::{AppState, Change};

/// How tall the bars are drawn, in pixels.
const BAR_AREA_HEIGHT: i32 = 44;

/// The strip, its month labels, and what it is currently drawing.
pub struct Strip {
    container: GtkBox,
    area: DrawingArea,
    months: Fixed,
    density: RefCell<Density>,
    /// Which bar the position marker sits on.
    marker: RefCell<Option<usize>>,
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

    let months = Fixed::builder().height_request(18).build();

    let container = GtkBox::new(Orientation::Vertical, 2);
    container.add_css_class("density-strip");
    container.append(&area);
    container.append(&months);

    let strip = Rc::new(Strip {
        container,
        area,
        months,
        density: RefCell::new(Density::default()),
        marker: RefCell::new(None),
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
            dragger.jump_to(start_x + offset_x, widget.width());
        }
    });
    strip.area.add_controller(drag);

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
                let text = crate::model::format::position(archive.ts, ordinal, total, &tz);
                self.area
                    .update_property(&[gtk4::accessible::Property::ValueText(&text)]);
            }
        }
        self.area.queue_draw();
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

    /// One bar per day, and a line where you are.
    fn draw(&self, area: &DrawingArea, context: &gtk4::cairo::Context, width: i32, height: i32) {
        let density = self.density.borrow();
        if density.is_empty() || width <= 0 {
            return;
        }
        let colour = area.color();
        let (width, height) = (f64::from(width), f64::from(height));
        let per_bar = width / density.bars.len() as f64;
        // Hairlines at the far end of a long history: never thinner than a
        // pixel, or a sparse month disappears entirely.
        let bar_width = (per_bar * 0.7).max(1.0);

        for (index, bar) in density.bars.iter().enumerate() {
            if bar.count == 0 {
                continue;
            }
            let scale = f64::from(bar.count) / f64::from(density.max);
            // A floor, so one backup in a day full of nothing is still visible
            // beside a day that holds twenty.
            let bar_height = (height * (0.25 + 0.75 * scale)).min(height);
            context.set_source_rgba(
                colour.red().into(),
                colour.green().into(),
                colour.blue().into(),
                0.35,
            );
            context.rectangle(
                index as f64 * per_bar,
                height - bar_height,
                bar_width,
                bar_height,
            );
            let _ = context.fill();
        }

        if let Some(marker) = *self.marker.borrow() {
            let x = marker as f64 * per_bar + bar_width / 2.0;
            context.set_source_rgba(
                colour.red().into(),
                colour.green().into(),
                colour.blue().into(),
                1.0,
            );
            context.rectangle(x - 1.0, 0.0, 2.0, height);
            let _ = context.fill();
        }
    }
}
