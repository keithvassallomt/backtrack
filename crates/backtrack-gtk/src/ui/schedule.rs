// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The schedule and retention controls, built once for two places.
//!
//! Preferences puts them on the Backup and Storage pages and writes each
//! change to the daemon as it happens. The wizard puts them all under one
//! expander on its last page and keeps them with its other choices until the
//! end. Same rows, same words, so a setting chosen in one is recognisable in
//! the other.

use std::rc::Rc;

use backtrack_core::config::{Backup, Frequency, Retention};
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::model::prefs as model;

/// The frequencies the menu offers, in order, with what each one says.
pub const FREQUENCIES: [(Frequency, &str); 4] = [
    (Frequency::Hourly, "Every hour"),
    (Frequency::Daily, "Every day"),
    (Frequency::Weekly, "Every week"),
    (Frequency::Manual, "Only when I ask"),
];

pub fn frequency_row(current: Frequency) -> adw::ComboRow {
    let labels: Vec<&str> = FREQUENCIES.iter().map(|(_, label)| *label).collect();
    let row = adw::ComboRow::builder()
        .title("Frequency")
        .model(&gtk4::StringList::new(&labels))
        .build();
    let index = FREQUENCIES
        .iter()
        .position(|(frequency, _)| *frequency == current)
        .unwrap_or(0);
    row.set_selected(index as u32);
    row
}

/// The frequency a row is showing.
pub fn frequency_of(row: &adw::ComboRow) -> Frequency {
    FREQUENCIES
        .get(row.selected() as usize)
        .map(|(frequency, _)| *frequency)
        .unwrap_or_default()
}

pub fn battery_row(on: bool) -> adw::SwitchRow {
    adw::SwitchRow::builder()
        .title("Back up on battery power")
        .active(on)
        .build()
}

/// How long a backup waits for mains power with backups on battery off
/// (S18-T14). Shown only then: with them on, nothing waits.
pub fn battery_wait_row(on_battery: bool) -> adw::ComboRow {
    adw::ComboRow::builder()
        .title("Wait for mains power")
        .subtitle("Counted from the last backup")
        .visible(!on_battery)
        .build()
}

/// Show `hours` in a [`battery_wait_row`], offering what
/// [`model::battery_waits`] offers; returns what each choice stands for.
pub fn show_battery_wait(row: &adw::ComboRow, hours: u32) -> Vec<u32> {
    let waits = model::battery_waits(hours);
    let labels: Vec<String> = waits
        .iter()
        .map(|h| model::battery_wait_label(*h))
        .collect();
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    row.set_model(Some(&gtk4::StringList::new(&labels)));
    row.set_selected(waits.iter().position(|h| *h == hours).unwrap_or(0) as u32);
    waits
}

pub fn metered_row(on: bool) -> adw::SwitchRow {
    adw::SwitchRow::builder()
        .title("Back up on metered connections")
        .active(on)
        .build()
}

pub fn automatic_row(on: bool) -> adw::SwitchRow {
    adw::SwitchRow::builder()
        .title("Automatic (recommended)")
        .subtitle(
            "Keeps hourly for 24 hours, daily for a week, weekly for a month, \
             monthly for 6 months",
        )
        .active(on)
        .build()
}

/// The four counts, for when retention is not automatic. Shown only then:
/// with automatic on they are not in force, and editing numbers that do
/// nothing would be a trap.
pub fn keep_rows(retention: &Retention) -> [adw::SpinRow; 4] {
    let row = |title: &str, value: u32| {
        let row = adw::SpinRow::with_range(0.0, 999.0, 1.0);
        row.set_title(title);
        row.set_value(f64::from(value));
        row.set_visible(!retention.automatic);
        row
    };
    [
        row("Hourly backups kept", retention.keep_hourly),
        row("Daily backups kept", retention.keep_daily),
        row("Weekly backups kept", retention.keep_weekly),
        row("Monthly backups kept", retention.keep_monthly),
    ]
}

/// Everything the wizard's expander edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Values {
    pub frequency: Frequency,
    pub on_battery: bool,
    pub on_battery_after: u32,
    pub on_metered: bool,
    pub retention: Retention,
}

/// The rows together, reporting every change as a whole [`Values`].
pub struct Rows {
    frequency: adw::ComboRow,
    battery: adw::SwitchRow,
    battery_wait: adw::ComboRow,
    /// What each of `battery_wait`'s choices stands for, in hours.
    waits: Vec<u32>,
    metered: adw::SwitchRow,
    automatic: adw::SwitchRow,
    keep: [adw::SpinRow; 4],
    changed: Box<dyn Fn(&Values)>,
}

impl Rows {
    pub fn build(values: &Values, changed: impl Fn(&Values) + 'static) -> Rc<Rows> {
        let battery_wait = battery_wait_row(values.on_battery);
        let waits = show_battery_wait(&battery_wait, values.on_battery_after);
        let rows = Rc::new(Rows {
            frequency: frequency_row(values.frequency),
            battery: battery_row(values.on_battery),
            battery_wait,
            waits,
            metered: metered_row(values.on_metered),
            automatic: automatic_row(values.retention.automatic),
            keep: keep_rows(&values.retention),
            changed: Box::new(changed),
        });
        for combo in [&rows.frequency, &rows.battery_wait] {
            let this = Rc::clone(&rows);
            combo.connect_selected_notify(move |_| this.report());
        }
        for switch in [&rows.battery, &rows.metered, &rows.automatic] {
            let this = Rc::clone(&rows);
            switch.connect_active_notify(move |_| this.report());
        }
        for spin in &rows.keep {
            let this = Rc::clone(&rows);
            spin.connect_value_notify(move |_| this.report());
        }
        rows
    }

    /// Every row, in the order they are shown.
    pub fn all(&self) -> Vec<gtk4::Widget> {
        let mut rows: Vec<gtk4::Widget> = vec![
            self.frequency.clone().upcast(),
            self.battery.clone().upcast(),
            self.battery_wait.clone().upcast(),
            self.metered.clone().upcast(),
            self.automatic.clone().upcast(),
        ];
        rows.extend(self.keep.iter().map(|row| row.clone().upcast()));
        rows
    }

    fn report(&self) {
        let automatic = self.automatic.is_active();
        for row in &self.keep {
            row.set_visible(!automatic);
        }
        self.battery_wait.set_visible(!self.battery.is_active());
        let count = |row: &adw::SpinRow| row.value().max(0.0) as u32;
        (self.changed)(&Values {
            frequency: frequency_of(&self.frequency),
            on_battery: self.battery.is_active(),
            on_battery_after: self
                .waits
                .get(self.battery_wait.selected() as usize)
                .copied()
                .unwrap_or_else(|| Backup::default().on_battery_after),
            on_metered: self.metered.is_active(),
            retention: Retention {
                automatic,
                keep_hourly: count(&self.keep[0]),
                keep_daily: count(&self.keep[1]),
                keep_weekly: count(&self.keep[2]),
                keep_monthly: count(&self.keep[3]),
            },
        });
    }
}
