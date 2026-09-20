// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The folder-restore summary, and the checklist behind its Review button.
//!
//! One screen instead of a storm of pop-ups. The counts on it are not an
//! estimate — the whole restore has already been worked out, and nothing has
//! been touched — so Cancel here costs nothing, and the numbers are exactly
//! what will happen.
//!
//! The row that matters most is the last one: files that exist only on disk
//! are kept, and nothing is deleted. It is the first fear people have about
//! restoring a folder over their work, and it is answered in words on the
//! screen where the fear arrives.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;

use backtrack_core::dbus::{RestoreEntry, RestorePreview};
use gtk4::prelude::*;
use gtk4::{
    glib, Align, Box as GtkBox, Button, CheckButton, Image, Label, ListBox, Orientation,
    ScrolledWindow, SelectionMode,
};
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::model::restore as copy;

/// Natural widths for the two screens. Not fixed: a narrow window still
/// shrinks them, which is what `AdwDialog` does with a content width.
const SUMMARY_WIDTH: i32 = 520;
const REVIEW_WIDTH: i32 = 580;

/// What the user chose on the summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Cancel,
    /// Every conflicting file keeps both versions.
    KeepBoth,
    /// Replace, with per-path answers. Empty means "everything the blanket
    /// covers", which is every conflict but no change of type.
    Replace {
        decisions: Vec<(String, String)>,
    },
}

/// Show the summary, and the review list if it is asked for.
pub async fn ask(
    parent: &impl IsA<gtk4::Widget>,
    preview: &RestorePreview,
    folder: &str,
    taken: i64,
) -> Answer {
    loop {
        match summary(parent, preview, folder, taken).await {
            Summary::Cancel => return Answer::Cancel,
            Summary::KeepBoth => return Answer::KeepBoth,
            Summary::Replace => {
                return Answer::Replace {
                    decisions: Vec::new(),
                }
            }
            Summary::Review => {
                if let Some(ticked) = review(parent, preview, folder, taken).await {
                    // Every path is answered explicitly, so a change of type
                    // that was ticked actually happens — the blanket answer
                    // deliberately does not reach those.
                    let decisions = preview
                        .entries
                        .iter()
                        .map(|entry| {
                            let decision = if ticked.contains(&entry.path) {
                                "replace"
                            } else {
                                "skip"
                            };
                            (entry.path.clone(), decision.to_string())
                        })
                        .collect();
                    return Answer::Replace { decisions };
                }
                // "Back" — round again to the summary.
            }
        }
    }
}

/// What the summary screen itself came back with.
enum Summary {
    Cancel,
    KeepBoth,
    Replace,
    Review,
}

async fn summary(
    parent: &impl IsA<gtk4::Widget>,
    preview: &RestorePreview,
    folder: &str,
    taken: i64,
) -> Summary {
    let tz = glib::TimeZone::local();
    let dialog = adw::AlertDialog::new(
        Some(&copy::summary_title(folder, taken, &tz)),
        Some(&copy::summary_subtitle(taken, &tz)),
    );

    // Each row is a whole sentence, and "4 files exist only on your disk —
    // kept. Nothing is deleted." is the one that must not wrap into
    // illegibility: it is the reassurance the screen exists to give.
    dialog.set_content_width(SUMMARY_WIDTH);

    let reviewing = Rc::new(Cell::new(false));
    dialog.set_extra_child(Some(&summary_rows(preview, &dialog, &reviewing)));

    dialog.add_responses(&[
        ("cancel", "Cancel"),
        ("keep-both", "Keep Both Versions"),
        ("replace", "Replace Changed Files"),
    ]);
    dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let response = dialog.choose_future(Some(parent)).await;
    // The Review button lives in a row rather than in the button bar, so it
    // closes the dialog and says why on the way out.
    if reviewing.get() {
        return Summary::Review;
    }
    match response.as_str() {
        "replace" => Summary::Replace,
        "keep-both" => Summary::KeepBoth,
        _ => Summary::Cancel,
    }
}

/// The boxed list of counts, with Review beside the row it belongs to.
fn summary_rows(
    preview: &RestorePreview,
    dialog: &adw::AlertDialog,
    reviewing: &Rc<Cell<bool>>,
) -> GtkBox {
    let rows = ListBox::new();
    rows.set_selection_mode(SelectionMode::None);
    rows.add_css_class("boxed-list");

    let mut review_added = false;
    for summary in copy::summary_rows(preview) {
        let row = adw::ActionRow::builder().title(&summary.text).build();
        row.set_title_lines(2);
        row.add_prefix(&Image::from_icon_name(summary.icon));
        // One Review button, on the first row that offers it: two would be two
        // ways into the same list.
        if summary.reviewable && !review_added {
            review_added = true;
            let button = Button::with_label("Review…");
            button.set_valign(Align::Center);
            let dialog = dialog.clone();
            let reviewing = Rc::clone(reviewing);
            button.connect_clicked(move |_| {
                reviewing.set(true);
                dialog.close();
            });
            row.add_suffix(&button);
        }
        row.update_property(&[gtk4::accessible::Property::Label(&summary.text)]);
        rows.append(&row);
    }

    let content = GtkBox::new(Orientation::Vertical, 12);
    content.append(&rows);
    content.append(&note(copy::SAFETY_NOTE));
    if !preview.refused.is_empty() {
        // Never silent. A file the restore will not touch is something the
        // person asking for it needs to be told.
        content.append(&note(&format!(
            "{} could not be restored safely and will be left out.",
            match preview.refused.len() {
                1 => "1 item".to_string(),
                n => format!("{n} items"),
            }
        )));
    }
    content
}

/// The per-file checklist. `None` means Back.
async fn review(
    parent: &impl IsA<gtk4::Widget>,
    preview: &RestorePreview,
    folder: &str,
    taken: i64,
) -> Option<BTreeSet<String>> {
    let tz = glib::TimeZone::local();
    let dialog = adw::AlertDialog::new(
        Some("Review files to be replaced"),
        Some(&format!(
            "Restoring “{folder}” from {}",
            crate::model::format::at(taken, &tz, "%e %b %Y, %H:%M")
        )),
    );

    // Wider still: every row carries two dated versions of the same file.
    dialog.set_content_width(REVIEW_WIDTH);

    // A change of type starts unticked: replacing a folder with a file is not
    // something to do by accepting a default.
    let ticked: Rc<RefCell<BTreeSet<String>>> = Rc::new(RefCell::new(
        preview
            .entries
            .iter()
            .filter(|e| e.class != "type-changed")
            .map(|e| e.path.clone())
            .collect(),
    ));

    let rows = ListBox::new();
    rows.set_selection_mode(SelectionMode::None);
    rows.add_css_class("boxed-list");
    let checks: Rc<RefCell<Vec<CheckButton>>> = Rc::new(RefCell::new(Vec::new()));

    for entry in &preview.entries {
        let (check, row) = review_row(entry, &tz);
        let starts_ticked = ticked.borrow().contains(&entry.path);
        let path = entry.path.clone();
        let watched = Rc::clone(&ticked);
        let dialog_for_label = dialog.clone();
        check.connect_toggled(move |check| {
            if check.is_active() {
                watched.borrow_mut().insert(path.clone());
            } else {
                watched.borrow_mut().remove(&path);
            }
            let count = watched.borrow().len();
            dialog_for_label.set_response_label("replace", &copy::replace_button_label(count));
        });
        check.set_active(starts_ticked);
        checks.borrow_mut().push(check);
        rows.append(&row);
    }

    let scroller = ScrolledWindow::builder()
        .child(&rows)
        .min_content_height(240)
        .max_content_height(420)
        .propagate_natural_height(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .build();

    let content = GtkBox::new(Orientation::Vertical, 12);
    content.append(&select_buttons(&checks));
    content.append(&scroller);
    content.append(&note("Unticked files are left exactly as they are."));
    content.append(&note(copy::SAFETY_NOTE));
    dialog.set_extra_child(Some(&content));

    dialog.add_responses(&[("back", "Back"), ("replace", "Replace")]);
    dialog.set_response_label(
        "replace",
        &copy::replace_button_label(ticked.borrow().len()),
    );
    dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("back"));
    dialog.set_close_response("back");

    match dialog.choose_future(Some(parent)).await.as_str() {
        "replace" => Some(ticked.borrow().clone()),
        _ => None,
    }
}

/// One row of the checklist: both versions, and a tag if the disk copy is newer.
fn review_row(entry: &RestoreEntry, tz: &glib::TimeZone) -> (CheckButton, adw::ActionRow) {
    let (disk, backup) = copy::version_lines(entry, tz);
    let name = entry
        .path
        .rsplit_once('/')
        .map_or(entry.path.as_str(), |(_, n)| n);

    let row = adw::ActionRow::builder()
        .title(name)
        .subtitle(format!("On disk: {disk}\nBackup: {backup}"))
        .build();
    row.set_subtitle_lines(2);

    let check = CheckButton::new();
    check.set_valign(Align::Center);
    row.add_prefix(&check);
    row.set_activatable_widget(Some(&check));

    if entry.class == "type-changed" {
        row.add_suffix(&tag("changed type"));
    } else if entry.disk_newer {
        row.add_suffix(&tag("newer on disk"));
    }

    let spoken = match (entry.class.as_str(), entry.disk_newer) {
        ("type-changed", _) => format!("{name}, changed type. {disk}. {backup}"),
        (_, true) => format!("{name}, newer on disk. {disk}. {backup}"),
        _ => format!("{name}. {disk}. {backup}"),
    };
    check.update_property(&[gtk4::accessible::Property::Label(&spoken)]);
    (check, row)
}

/// Select All / Select None, for a list long enough that ticking by hand is a
/// chore.
fn select_buttons(checks: &Rc<RefCell<Vec<CheckButton>>>) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 6);
    row.set_halign(Align::Start);
    for (label, wanted) in [("Select All", true), ("Select None", false)] {
        let button = Button::with_label(label);
        button.add_css_class("flat");
        let checks = Rc::clone(checks);
        button.connect_clicked(move |_| {
            for check in checks.borrow().iter() {
                check.set_active(wanted);
            }
        });
        row.append(&button);
    }
    row
}

fn tag(text: &str) -> Label {
    let tag = Label::new(Some(text));
    tag.add_css_class("badge");
    tag.add_css_class("deleted");
    tag.set_valign(Align::Center);
    tag
}

fn note(text: &str) -> Label {
    let note = Label::new(Some(text));
    note.add_css_class("dim-label");
    note.add_css_class("caption");
    note.set_wrap(true);
    note.set_halign(Align::Start);
    note
}
