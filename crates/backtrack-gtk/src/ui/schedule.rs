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

use backtrack_core::config::{Frequency, Retention};
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

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
    pub on_metered: bool,
    pub retention: Retention,
}

/// The rows together, reporting every change as a whole [`Values`].
pub struct Rows {
    frequency: adw::ComboRow,
    battery: adw::SwitchRow,
    metered: adw::SwitchRow,
    automatic: adw::SwitchRow,
    keep: [adw::SpinRow; 4],
    changed: Box<dyn Fn(&Values)>,
}

impl Rows {
    pub fn build(values: &Values, changed: impl Fn(&Values) + 'static) -> Rc<Rows> {
        let rows = Rc::new(Rows {
            frequency: frequency_row(values.frequency),
            battery: battery_row(values.on_battery),
            metered: metered_row(values.on_metered),
            automatic: automatic_row(values.retention.automatic),
            keep: keep_rows(&values.retention),
            changed: Box::new(changed),
        });
        let this = Rc::clone(&rows);
        rows.frequency
            .connect_selected_notify(move |_| this.report());
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
        let count = |row: &adw::SpinRow| row.value().max(0.0) as u32;
        (self.changed)(&Values {
            frequency: frequency_of(&self.frequency),
            on_battery: self.battery.is_active(),
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
