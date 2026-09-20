// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The calendar popover: long jumps, for when you know roughly when.
//!
//! A month grid built from buttons rather than `GtkCalendar`, for one reason
//! that matters: a day with no backup behind it must be insensitive. GtkCalendar
//! will happily let you select the 4th when nothing was backed up on the 4th,
//! and answering that click with "nothing happened" is worse than not offering
//! it. Here the days that can be jumped to are the only ones that respond, and
//! they are shaded so you can see which they are before you reach for them.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Grid, Label, MenuButton, Orientation, Popover, Widget};

use crate::model::calendar::{self, Month};
use crate::state::{AppState, Change};

/// The calendar button and its popover.
pub struct Calendar {
    button: MenuButton,
    popover: Popover,
    heading: Label,
    grid: Grid,
    /// The month on show, which is not necessarily the month being viewed —
    /// the user may have paged away from it.
    showing: RefCell<(i32, i32)>,
    state: Rc<AppState>,
}

/// Build the calendar button for `state`.
pub fn build(state: &Rc<AppState>) -> Rc<Calendar> {
    let heading = Label::new(None);
    heading.add_css_class("heading");
    heading.set_hexpand(true);

    let grid = Grid::builder()
        .row_spacing(2)
        .column_spacing(2)
        .halign(Align::Center)
        .build();
    grid.add_css_class("calendar-grid");

    let popover = Popover::new();
    let button = MenuButton::builder()
        .icon_name("x-office-calendar-symbolic")
        .tooltip_text("Jump to a date")
        .popover(&popover)
        .build();
    button.update_property(&[gtk4::accessible::Property::Label("Jump to a date")]);

    let calendar = Rc::new(Calendar {
        button,
        popover,
        heading,
        grid,
        showing: RefCell::new((1970, 1)),
        state: Rc::clone(state),
    });

    calendar.popover.set_child(Some(&calendar.layout()));

    // Opening it starts on the month being viewed, not on whichever month was
    // last paged to.
    let opening = Rc::clone(&calendar);
    calendar.popover.connect_show(move |_| {
        let view = opening.state.view();
        let tz = glib::TimeZone::local();
        let month = view
            .seq
            .and_then(|seq| calendar::month_of(&view.archives, seq, &tz))
            .unwrap_or_else(|| today(&tz));
        opening.show_month(month);
    });

    // A jump from anywhere else moves the grid under the popover too.
    let watcher = Rc::clone(&calendar);
    state.subscribe(move |_, change| {
        if change == Change::Archives {
            watcher.button.set_sensitive(true);
        }
    });
    calendar.button.set_sensitive(false);

    calendar
}

impl Calendar {
    /// The button to put in the sidebar header.
    pub fn widget(&self) -> Widget {
        self.button.clone().upcast()
    }

    /// Popover contents: month navigation, the grid, and what the shading means.
    fn layout(self: &Rc<Self>) -> GtkBox {
        let content = GtkBox::new(Orientation::Vertical, 8);
        content.set_margin_top(8);
        content.set_margin_bottom(8);
        content.set_margin_start(8);
        content.set_margin_end(8);

        let header = GtkBox::new(Orientation::Horizontal, 4);
        let back = Button::from_icon_name("go-previous-symbolic");
        back.add_css_class("flat");
        back.set_tooltip_text(Some("Previous month"));
        back.update_property(&[gtk4::accessible::Property::Label("Previous month")]);
        let forward = Button::from_icon_name("go-next-symbolic");
        forward.add_css_class("flat");
        forward.set_tooltip_text(Some("Next month"));
        forward.update_property(&[gtk4::accessible::Property::Label("Next month")]);

        self.heading.set_halign(Align::Center);
        header.append(&back);
        header.append(&self.heading);
        header.append(&forward);
        content.append(&header);
        content.append(&self.grid);

        let caption = Label::new(Some("Shaded days have backups"));
        caption.add_css_class("dim-label");
        caption.add_css_class("caption");
        content.append(&caption);

        let earlier = Rc::clone(self);
        back.connect_clicked(move |_| {
            let (year, month) = *earlier.showing.borrow();
            earlier.show_month(calendar::previous(year, month));
        });
        let later = Rc::clone(self);
        forward.connect_clicked(move |_| {
            let (year, month) = *later.showing.borrow();
            later.show_month(calendar::next(year, month));
        });

        content
    }

    /// Draw `(year, month)`.
    fn show_month(self: &Rc<Self>, (year, month): (i32, i32)) {
        let tz = glib::TimeZone::local();
        let now = glib::DateTime::now_utc().map(|d| d.to_unix()).unwrap_or(0);
        let view = self.state.view();
        let grid = calendar::month(&view.archives, year, month, now, &tz);
        *self.showing.borrow_mut() = (year, month);
        self.heading.set_text(&grid.title);

        while let Some(child) = self.grid.first_child() {
            self.grid.remove(&child);
        }
        for (column, initial) in calendar::weekday_initials(&tz).iter().enumerate() {
            let label = Label::new(Some(initial));
            label.add_css_class("calendar-weekday");
            self.grid.attach(&label, column as i32, 0, 1, 1);
        }
        self.attach_days(&grid);
    }

    fn attach_days(self: &Rc<Self>, month: &Month) {
        for (offset, day) in month.days.iter().enumerate() {
            let cell = offset + month.leading_blanks;
            let button = Button::with_label(&day.day_of_month.to_string());
            button.add_css_class("flat");
            if day.has_backups {
                button.add_css_class("has-backups");
            }
            if day.is_today {
                button.add_css_class("today");
            }
            button.set_sensitive(day.has_backups);
            button.update_property(&[gtk4::accessible::Property::Label(&describe(
                month,
                day.day_of_month,
                day.count,
                day.is_today,
            ))]);

            if let Some(seq) = day.seq {
                let this = Rc::clone(self);
                button.connect_clicked(move |_| {
                    this.state.set_seq(seq);
                    this.popover.popdown();
                });
            }
            self.grid
                .attach(&button, (cell % 7) as i32, (cell / 7) as i32 + 1, 1, 1);
        }
    }
}

/// What a screen reader says for a day cell. "12" on its own tells a sighted
/// user everything, because they can see the shading; spoken, it does not.
fn describe(month: &Month, day: i32, count: usize, is_today: bool) -> String {
    let mut spoken = format!("{day} {}", month.title);
    if is_today {
        spoken.push_str(", today");
    }
    match count {
        0 => spoken.push_str(", no backups"),
        1 => spoken.push_str(", 1 backup"),
        many => spoken.push_str(&format!(", {many} backups")),
    }
    spoken
}

/// The current month, for a window with nothing selected yet.
fn today(tz: &glib::TimeZone) -> (i32, i32) {
    glib::DateTime::now(tz)
        .map(|now| (now.year(), now.month()))
        .unwrap_or((1970, 1))
}
