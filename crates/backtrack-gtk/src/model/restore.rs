// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! What the restore dialogs say.
//!
//! The words are the feature here. A conflict dialog that does not say which
//! version is newer is the single most complained-about thing about Time
//! Machine's, and a folder-restore summary that does not say outright that
//! nothing gets deleted leaves people to assume the worst — which, for a
//! restore, is the assumption that stops them using it.
//!
//! So the copy lives here, as functions over the numbers, and is tested
//! against the mockups rather than typed into a widget where nothing can check
//! it.

use backtrack_core::dbus::{RestoreEntry, RestorePreview};
use gtk4::glib;

use super::format;

/// The line every dialog ends on, and the reason Replace is not frightening.
pub const SAFETY_NOTE: &str = "Replaced files are kept as safety copies for 30 days.";

/// The single-file dialog's title: `Replace "report.odt"?`
pub fn conflict_title(name: &str) -> String {
    format!("Replace “{name}”?")
}

/// The single-file dialog's body.
///
/// Two sentences: where the clash is, and which side is newer. The second is
/// the one that matters — restoring over newer work is the risky direction,
/// and a dialog that leaves the user to work that out from two timestamps has
/// wasted the only chance it had to say so.
pub fn conflict_body(folder: &str, disk_newer: bool) -> String {
    let where_ = match folder {
        "" => "A file with this name already exists here.".to_string(),
        folder => format!("A file with this name already exists in {folder}."),
    };
    let which = if disk_newer {
        "The file on your disk is newer than the backup version."
    } else {
        "The backup version is newer than the file on your disk."
    };
    format!("{where_}\n{which}")
}

/// The two rows of the comparison box: what each version is.
pub fn version_lines(entry: &RestoreEntry, tz: &glib::TimeZone) -> (String, String) {
    let disk = format!(
        "Modified {} · {}",
        format::modified(entry.disk_mtime, tz),
        bytes(entry.disk_size)
    );
    let backup = format!(
        "From backup of {} · {}",
        format::modified(entry.backup_mtime, tz),
        bytes(entry.backup_size)
    );
    (disk, backup)
}

/// The folder summary's title: `Restore "Projects" from 28 June?`
pub fn summary_title(folder: &str, taken: i64, tz: &glib::TimeZone) -> String {
    format!(
        "Restore “{folder}” from {}?",
        format::at(taken, tz, "%e %B")
    )
}

/// The folder summary's subtitle, which dates the backup in full.
pub fn summary_subtitle(taken: i64, tz: &glib::TimeZone) -> String {
    format!(
        "Restoring from the backup of {}",
        format::at(taken, tz, "%e %b %Y, %H:%M")
    )
}

/// One row of the summary, with the icon that goes beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryRow {
    pub icon: &'static str,
    pub text: String,
    /// Whether this row offers the per-file review.
    pub reviewable: bool,
}

/// The summary, as rows.
///
/// Rows for counts of zero are left out: "0 files exist only on your disk" is
/// noise, and the screen is meant to be read at a glance rather than parsed.
/// The one that is never left out when it applies is the last — that nothing
/// is deleted is the thing people most need to be told.
pub fn summary_rows(preview: &RestorePreview) -> Vec<SummaryRow> {
    let mut rows = Vec::new();
    if preview.identical > 0 {
        rows.push(SummaryRow {
            icon: "object-select-symbolic",
            text: format!(
                "{} {} identical — left alone",
                count_files(preview.identical),
                verb(preview.identical, "is", "are")
            ),
            reviewable: false,
        });
    }
    if preview.conflicts > 0 {
        let newer = match preview.disk_newer {
            0 => String::new(),
            n => format!(" ({n} newer on your disk)"),
        };
        rows.push(SummaryRow {
            icon: "view-refresh-symbolic",
            text: format!("{} will be replaced{newer}", count_files(preview.conflicts)),
            reviewable: true,
        });
    }
    if preview.only_in_backup > 0 {
        rows.push(SummaryRow {
            icon: "list-add-symbolic",
            text: format!(
                "{} {} only in the backup — will be added",
                count_files(preview.only_in_backup),
                verb(preview.only_in_backup, "exists", "exist")
            ),
            reviewable: false,
        });
    }
    if preview.only_on_disk > 0 {
        rows.push(SummaryRow {
            icon: "security-high-symbolic",
            text: format!(
                "{} {} only on your disk — kept. Nothing is deleted.",
                count_files(preview.only_on_disk),
                verb(preview.only_on_disk, "exists", "exist")
            ),
            reviewable: false,
        });
    }
    if preview.type_changed > 0 {
        // Not in the mockup, and it has to be here: a file where the backup has
        // a folder is not something "Replace Changed Files" should quietly do,
        // so it is called out and left for the review list.
        rows.push(SummaryRow {
            icon: "dialog-warning-symbolic",
            text: format!(
                "{} {} changed type — review before replacing",
                count_files(preview.type_changed),
                verb(preview.type_changed, "has", "have")
            ),
            reviewable: true,
        });
    }
    rows
}

/// The review list's button, which counts what is ticked: `Replace 5 Files`.
pub fn replace_button_label(selected: usize) -> String {
    match selected {
        0 => "Replace Nothing".to_string(),
        1 => "Replace 1 File".to_string(),
        n => format!("Replace {n} Files"),
    }
}

/// What the toast says after a restore.
pub fn restored_toast(count: usize, name: &str) -> String {
    match count {
        0 => "Nothing needed restoring".to_string(),
        1 => format!("Restored {name}"),
        n => format!("Restored {n} files"),
    }
}

/// `"1 file"` / `"214 files"`.
fn count_files(count: u32) -> String {
    if count == 1 {
        "1 file".to_string()
    } else {
        format!("{count} files")
    }
}

/// The verb that agrees with a count.
fn verb(count: u32, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 {
        one
    } else {
        many
    }
}

/// A size, or an em dash where there is nothing to size.
fn bytes(size: u64) -> String {
    if size == 0 {
        "—".to_string()
    } else {
        glib::format_size(size).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc() -> glib::TimeZone {
        glib::TimeZone::utc()
    }

    fn preview() -> RestorePreview {
        RestorePreview {
            archive: "snapshot-28".to_string(),
            dest: "/home/keith/Documents".to_string(),
            identical: 214,
            conflicts: 6,
            disk_newer: 3,
            only_in_backup: 2,
            only_on_disk: 4,
            type_changed: 0,
            entries: Vec::new(),
            refused: Vec::new(),
        }
    }

    #[test]
    fn the_conflict_dialog_always_says_which_side_is_newer() {
        // The single most complained-about omission in Time Machine's.
        let newer_on_disk = conflict_body("Documents", true);
        assert!(newer_on_disk.contains("A file with this name already exists in Documents."));
        assert!(newer_on_disk.contains("The file on your disk is newer"));

        let newer_in_backup = conflict_body("Documents", false);
        assert!(newer_in_backup.contains("The backup version is newer"));
        assert_ne!(newer_on_disk, newer_in_backup);
    }

    #[test]
    fn a_restore_into_the_top_of_the_tree_does_not_name_an_empty_folder() {
        assert!(conflict_body("", true).starts_with("A file with this name already exists here."));
    }

    #[test]
    fn the_conflict_title_names_the_file() {
        assert_eq!(conflict_title("report.odt"), "Replace “report.odt”?");
    }

    #[test]
    fn the_summary_reads_like_the_mockup() {
        let rows = summary_rows(&preview());
        let text: Vec<&str> = rows.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(
            text,
            [
                "214 files are identical — left alone",
                "6 files will be replaced (3 newer on your disk)",
                "2 files exist only in the backup — will be added",
                "4 files exist only on your disk — kept. Nothing is deleted.",
            ]
        );
        assert!(
            rows[1].reviewable,
            "the replacements are the reviewable row"
        );
        assert!(rows.iter().filter(|r| r.reviewable).count() == 1);
    }

    #[test]
    fn one_of_something_is_not_described_as_one_files() {
        let mut one = preview();
        one.identical = 1;
        one.conflicts = 1;
        one.disk_newer = 0;
        one.only_in_backup = 1;
        one.only_on_disk = 1;
        let text: Vec<String> = summary_rows(&one).iter().map(|r| r.text.clone()).collect();
        assert_eq!(
            text,
            [
                "1 file is identical — left alone",
                "1 file will be replaced",
                "1 file exists only in the backup — will be added",
                "1 file exists only on your disk — kept. Nothing is deleted.",
            ]
        );
    }

    #[test]
    fn a_count_of_zero_is_left_out_rather_than_written_as_zero() {
        let mut nothing = preview();
        nothing.only_on_disk = 0;
        nothing.only_in_backup = 0;
        let rows = summary_rows(&nothing);
        assert_eq!(rows.len(), 2);
        assert!(!rows.iter().any(|r| r.text.starts_with('0')));
    }

    #[test]
    fn a_change_of_type_is_called_out_even_though_the_mockup_has_no_row_for_it() {
        let mut typed = preview();
        typed.type_changed = 1;
        let rows = summary_rows(&typed);
        let last = rows.last().unwrap();
        assert_eq!(
            last.text,
            "1 file has changed type — review before replacing"
        );
        assert!(last.reviewable);
    }

    #[test]
    fn the_replacements_row_only_mentions_newer_copies_when_there_are_some() {
        let mut none_newer = preview();
        none_newer.disk_newer = 0;
        let rows = summary_rows(&none_newer);
        assert_eq!(rows[1].text, "6 files will be replaced");
    }

    #[test]
    fn the_review_button_counts_what_is_ticked() {
        assert_eq!(replace_button_label(0), "Replace Nothing");
        assert_eq!(replace_button_label(1), "Replace 1 File");
        assert_eq!(replace_button_label(5), "Replace 5 Files");
    }

    #[test]
    fn the_toast_names_one_file_and_counts_several() {
        assert_eq!(restored_toast(1, "report.odt"), "Restored report.odt");
        assert_eq!(restored_toast(6, "report.odt"), "Restored 6 files");
        assert_eq!(restored_toast(0, "report.odt"), "Nothing needed restoring");
    }

    #[test]
    fn the_summary_dates_the_backup_twice_over() {
        // 2026-06-28 09:00:00 UTC.
        let taken = 1_782_637_200;
        assert_eq!(
            summary_title("Projects", taken, &utc()),
            "Restore “Projects” from 28 June?"
        );
        assert_eq!(
            summary_subtitle(taken, &utc()),
            "Restoring from the backup of 28 Jun 2026, 09:00"
        );
    }
}
