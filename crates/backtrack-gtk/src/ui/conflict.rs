// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The single-file conflict dialog.
//!
//! Shown only when the file on disk genuinely differs from the backup version.
//! An identical file is skipped in silence — being asked whether to replace a
//! file with itself is the kind of question that teaches people to click
//! through dialogs without reading them.
//!
//! Three things it does that the neighbours do not: it shows both versions'
//! dates and sizes, it says outright which one is newer, and it promises the
//! file being overwritten is kept. Restoring over newer work is the risky
//! direction, and this dialog is the only place that can be said.

use backtrack_core::dbus::RestoreEntry;
use gtk4::prelude::*;
use gtk4::{glib, Align, Box as GtkBox, Image, Label, ListBox, Orientation, SelectionMode};
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::model::restore as copy;

/// How wide the dialog wants to be. Enough for "Modified 20 Sep 2026, 16:18 ·
/// 76 bytes" on one line, and for the three responses side by side.
const CONTENT_WIDTH: i32 = 460;

/// What the user chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Cancel,
    KeepBoth,
    Replace,
}

impl Answer {
    fn from_response(response: &str) -> Answer {
        match response {
            "replace" => Answer::Replace,
            "keep-both" => Answer::KeepBoth,
            _ => Answer::Cancel,
        }
    }
}

/// Ask about one file, and wait for the answer.
pub async fn ask(
    parent: &impl IsA<gtk4::Widget>,
    entry: &RestoreEntry,
    name: &str,
    folder: &str,
) -> Answer {
    let dialog = adw::AlertDialog::new(
        Some(&copy::conflict_title(name)),
        Some(&copy::conflict_body(folder, entry.disk_newer)),
    );

    // Wide enough that the two dates sit on one line each and the three
    // buttons sit in a row. Left to itself the dialog takes its natural width,
    // which for this content is narrow enough to wrap every line and stack the
    // buttons vertically — and a stacked Cancel/Keep Both/Replace loses the
    // left-to-right ordering that puts the safe way out under the hand.
    // It is a natural width, not a fixed one: a narrow window still shrinks it.
    dialog.set_content_width(CONTENT_WIDTH);
    crate::ui::prefer_wide_responses(&dialog);

    let comparison = comparison(entry, name);
    // `content-width` is a natural width, and a dialog whose content asks for
    // less than that keeps its own counsel. The content has to want the room.
    comparison.set_size_request(CONTENT_WIDTH - 40, -1);
    dialog.set_extra_child(Some(&comparison));

    // Cancel first and Replace last, per the GNOME guidelines: the safe way out
    // is where the hand lands, and the destructive one is furthest from it.
    dialog.add_responses(&[
        ("cancel", "Cancel"),
        ("keep-both", "Keep Both"),
        ("replace", "Replace"),
    ]);
    dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
    // Replace is never the default. Escape and Enter both take the safe way
    // out, because a dialog dismissed without being read must not overwrite
    // somebody's work.
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    Answer::from_response(dialog.choose_future(Some(parent)).await.as_str())
}

/// The two-row comparison box: what is on disk, and what is in the backup.
fn comparison(entry: &RestoreEntry, name: &str) -> GtkBox {
    let tz = glib::TimeZone::local();
    let (disk, backup) = copy::version_lines(entry, &tz);

    let rows = ListBox::new();
    rows.set_selection_mode(SelectionMode::None);
    rows.add_css_class("boxed-list");
    rows.append(&version_row(
        icon_for(name),
        "Current file",
        &disk,
        entry.disk_newer,
    ));
    rows.append(&version_row(
        "document-open-recent-symbolic",
        "Backup version",
        &backup,
        !entry.disk_newer,
    ));

    let content = GtkBox::new(Orientation::Vertical, 12);
    content.append(&rows);

    let note = Label::new(Some(copy::SAFETY_NOTE));
    note.add_css_class("dim-label");
    note.add_css_class("caption");
    note.set_wrap(true);
    note.set_halign(Align::Start);
    content.append(&note);
    content
}

/// One side of the comparison, with the `newer` tag on whichever side has it.
fn version_row(icon: &str, title: &str, detail: &str, newer: bool) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .title(title)
        .subtitle(detail)
        .build();
    row.add_prefix(&Image::from_icon_name(icon));
    if newer {
        let tag = Label::new(Some("newer"));
        tag.add_css_class("badge");
        tag.add_css_class("deleted");
        tag.set_valign(Align::Center);
        row.add_suffix(&tag);
    }
    // Spoken as one sentence rather than as three fragments.
    let spoken = if newer {
        format!("{title}, {detail}, newer")
    } else {
        format!("{title}, {detail}")
    };
    row.update_property(&[gtk4::accessible::Property::Label(&spoken)]);
    row
}

/// The themed icon for the file being replaced, guessed from its name the way
/// a file manager does.
fn icon_for(name: &str) -> &'static str {
    // A generic document rather than a per-type icon: the dialog is about two
    // versions of the same file, and giving them different icons would suggest
    // they are different kinds of thing.
    let _ = name;
    "text-x-generic-symbolic"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_response_that_does_nothing_is_what_an_unknown_one_becomes() {
        // A dialog dismissed by the window manager, or closed with Escape,
        // must never be read as consent to overwrite.
        assert_eq!(Answer::from_response("replace"), Answer::Replace);
        assert_eq!(Answer::from_response("keep-both"), Answer::KeepBoth);
        assert_eq!(Answer::from_response("cancel"), Answer::Cancel);
        assert_eq!(Answer::from_response("close"), Answer::Cancel);
        assert_eq!(Answer::from_response(""), Answer::Cancel);
    }
}
