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

/// Shown in the review list when something has changed type.
///
/// Those rows start unticked because turning a folder into a file is not a
/// thing to do by accepting a default — and Select All would undo that in one
/// click without saying so, which is the same failure wearing a button.
pub const TYPE_CHANGE_NOTE: &str = "Select All leaves changes of type alone. Tick those yourself.";

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
///
/// Symmetrical, and deliberately so. The backup line used to read "From backup
/// of 22 Jul 2026" — which dated the *backup* by the file's modification time,
/// on a screen whose own subtitle said the backup was from 20 September. One
/// dialog, two dates for the same backup, and the person left to work out
/// which of them was lying.
///
/// The value was never the problem: a file's modification time is exactly what
/// [`RestoreEntry::disk_newer`] is computed from, so showing the archive's
/// timestamp instead would put "newer on disk" above two dates that say the
/// opposite. Only the words needed fixing. Dating the backup is the job of the
/// title, which has the archive to do it with.
pub fn version_lines(entry: &RestoreEntry, tz: &glib::TimeZone) -> (String, String) {
    (
        side(&entry.disk_kind, entry.disk_mtime, entry.disk_size, tz),
        side(
            &entry.backup_kind,
            entry.backup_mtime,
            entry.backup_size,
            tz,
        ),
    )
}

/// One side of the comparison.
///
/// A folder says it is a folder and carries no size. That is the whole content
/// of a change of type — "this is a file in the backup and a folder on your
/// disk" — and a row that shows a size of `—` and leaves the rest to be
/// inferred has given away the useful half.
fn side(kind: &str, mtime: i64, size: u64, tz: &glib::TimeZone) -> String {
    let when = format::modified(mtime, tz);
    match kind {
        "dir" => format!("Folder, modified {when}"),
        "symlink" => format!("Link, modified {when}"),
        _ => format!("Modified {when} · {}", bytes(size)),
    }
}

/// How a review row names a file: relative to the folder being restored.
///
/// The list is ordered by path, so basenames alone make it look shuffled —
/// README, redirects, contact, 2026-06-pricing — and two files of the same name
/// in different subfolders become two identical rows with different answers.
pub fn review_name(path: &str, target: &str) -> String {
    match path.strip_prefix(target).and_then(|r| r.strip_prefix('/')) {
        Some(rest) if !rest.is_empty() => rest.to_string(),
        // Restoring a single file: the target *is* the path, and naming it
        // relative to itself would leave the row blank.
        _ => path
            .rsplit_once('/')
            .map_or(path, |(_, name)| name)
            .to_string(),
    }
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
                count_items(preview.identical),
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
                count_items(preview.only_in_backup),
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
                count_items(preview.only_on_disk),
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
                count_items(preview.type_changed),
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

/// What the toast says when the chosen backup does not hold what was asked
/// for — because it was taken before the file existed, or after it was
/// deleted, or because it simply covers something else.
pub fn not_in_this_backup(name: &str) -> String {
    format!("“{name}” isn't in this backup — try a more recent one")
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

/// `"1 item"` / `"214 items"`.
///
/// Most of the summary counts plan entries, and a folder that exists on both
/// sides is an entry — nothing to restore, but counted. "19 files are
/// identical" over a folder holding eleven was a number that could be checked
/// and found wrong, which is the worst kind to put on a screen whose whole
/// claim is that its numbers are the restore itself, counted.
///
/// Only the replacements stay "files", because they only ever are: a directory
/// on both sides is identical, and a directory against a file is a change of
/// type.
fn count_items(count: u32) -> String {
    if count == 1 {
        "1 item".to_string()
    } else {
        format!("{count} items")
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

/// The folder a "Restore To…" makes for itself: `Restored website 20 Sep 2026`.
///
/// Made rather than chosen, and that is the feature. A directory that did not
/// exist a moment ago cannot contain anything that clashes with what is coming
/// out of the backup, so this path has no conflict dialog, no summary and no
/// decisions on it — and nothing already on the machine is touched. It is the
/// answer to "let me look at the old one first", which restoring in place
/// cannot be.
///
/// Dated by the backup rather than by today, because which version you have is
/// the thing you will want to know in a week, and two of these side by side
/// are told apart by nothing else.
pub fn restored_folder_name(name: &str, taken: i64, tz: &glib::TimeZone) -> String {
    let name = name.trim_matches('/');
    let stem = if name.is_empty() { "backup" } else { name };
    format!("Restored {stem} {}", format::at(taken, tz, "%e %b %Y")).replace('/', "-")
}

/// The toast after a restore that went somewhere else.
pub fn restored_into_toast(folder: &str) -> String {
    format!("Restored into “{folder}”")
}

/// The "Recently Replaced Files" window's heading for one restore.
///
/// Every file a single restore displaced shares a time, which is what groups
/// them: somebody looking for a file they lost is looking for the moment they
/// lost it, not for the file's name — they usually remember the restore.
pub fn stash_group_title(replaced_at: i64, tz: &glib::TimeZone) -> String {
    // To the second, because two restores a few seconds apart are two restores
    // and a listing that titles them identically has stopped grouping and
    // started confusing.
    format!(
        "Replaced {}",
        format::at(replaced_at, tz, "%e %b %Y, %H:%M:%S")
    )
}

/// One row: where the file lived, how big it was, and when it was last edited.
///
/// The folder rather than the full path. A stash listing is read down the
/// left-hand edge, and a column of identical path prefixes hides the one part
/// of each line that differs.
pub fn stash_row_subtitle(
    original: &str,
    size: u64,
    mtime: i64,
    home: &str,
    tz: &glib::TimeZone,
) -> String {
    let folder =
        original.rsplit_once('/').map_or(
            "/",
            |(parent, _)| if parent.is_empty() { "/" } else { parent },
        );
    format!(
        "{} · {} · modified {}",
        short_folder(folder, home),
        bytes(size),
        format::at(mtime, tz, "%e %b %Y, %H:%M")
    )
}

/// How many components of a folder a row keeps when it elides the middle.
const PATH_COMPONENTS: usize = 3;

/// A folder as a person reads it: their home as `~`, and a long middle elided
/// rather than wrapped.
///
/// Every row in the stash shares most of its path with every other, so the part
/// that differs — the end — is the part that has to survive. Written out in
/// full it wraps to two lines of identical prefix and buries it.
pub fn short_folder(folder: &str, home: &str) -> String {
    let shortened = match folder.strip_prefix(home) {
        Some("") => "~".to_string(),
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => folder.to_string(),
    };

    let parts: Vec<&str> = shortened.split('/').collect();
    if parts.len() <= PATH_COMPONENTS + 1 {
        return shortened;
    }
    // The head says which tree this is, the tail says which folder. What lies
    // between is the part every row has in common.
    let head = parts.first().copied().unwrap_or_default();
    let tail = parts[parts.len() - PATH_COMPONENTS..].join("/");
    format!("{head}/…/{tail}")
}

/// What the window says when the stash is empty.
///
/// Not an error and not an apology: an empty stash means nothing has been
/// overwritten, which is the normal state of affairs.
pub const NOTHING_REPLACED: &str = "Nothing has been replaced";
pub const NOTHING_REPLACED_BODY: &str =
    "When a restore replaces a file, the version it replaced is kept here for 30 days.";

/// The toast after putting a file back.
pub fn put_back_toast(name: &str) -> String {
    format!("Put “{name}” back")
}

/// What the window says under its title, which is the promise it exists to keep.
pub const STASH_NOTE: &str = "Files replaced by a restore are kept for 30 days, then given up. \
     Putting one back keeps whatever is in its place, in here.";

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
            missing: Vec::new(),
        }
    }

    /// One conflicting file, dated the way the demo fixture dates them.
    fn entry() -> RestoreEntry {
        RestoreEntry {
            path: "home/keith/Projects/website/content/home.md".to_string(),
            class: "conflict".to_string(),
            disk_newer: true,
            backup_size: 228,
            // 2026-07-22 15:57 UTC and 2026-09-19 15:57 UTC, in microseconds.
            backup_mtime: 1_784_735_865_000_000,
            disk_size: 318,
            disk_mtime: 1_789_833_465_000_000,
            backup_kind: "file".to_string(),
            disk_kind: "file".to_string(),
        }
    }

    #[test]
    fn a_stash_row_shows_the_end_of_the_path_rather_than_the_start() {
        // Every row shares the prefix; the part that tells them apart is the
        // end, and it is the part that must not be the part that gets cut.
        let home = "/home/keith";
        assert_eq!(
            short_folder(
                "/home/keith/.local/share/backtrack-dev/demo-src/home/Projects/website/content/blog",
                home
            ),
            "~/…/website/content/blog"
        );
        // Short enough to say in full is said in full.
        assert_eq!(short_folder("/home/keith/Documents", home), "~/Documents");
        assert_eq!(short_folder("/home/keith", home), "~");
        // Somewhere else entirely keeps its own root.
        assert_eq!(
            short_folder("/mnt/tank/media/films", home),
            "/…/tank/media/films"
        );
        // A home directory that merely shares a prefix is not this one.
        assert_eq!(
            short_folder("/home/keith2/Documents", home),
            "/home/keith2/Documents"
        );
    }

    #[test]
    fn two_restores_in_the_same_minute_are_titled_apart() {
        // 2026-06-28 09:00:00 and 09:00:23 UTC.
        let first = 1_782_637_200;
        assert_ne!(
            stash_group_title(first, &utc()),
            stash_group_title(first + 23, &utc()),
            "a heading that cannot tell two restores apart has stopped grouping"
        );
    }

    #[test]
    fn a_restore_to_somewhere_else_names_its_own_folder() {
        // 2026-06-28 09:00:00 UTC.
        let taken = 1_782_637_200;
        assert_eq!(
            restored_folder_name("website", taken, &utc()),
            "Restored website 28 Jun 2026"
        );
        // Dated by the backup, not by today: which version you have is what
        // you will want to know in a week.
        assert_ne!(
            restored_folder_name("website", taken, &utc()),
            restored_folder_name("website", taken + 86_400, &utc())
        );
    }

    #[test]
    fn a_restored_folder_name_is_a_name_and_not_a_path() {
        // It is joined onto a directory the user picked, so a separator in it
        // would put the restore somewhere they did not choose.
        let taken = 1_782_637_200;
        for awkward in ["a/b", "/leading", "trailing/"] {
            let made = restored_folder_name(awkward, taken, &utc());
            assert!(!made.contains('/'), "{awkward} produced {made}");
        }
        // The top of the tree has no name of its own to use.
        assert_eq!(
            restored_folder_name("/", taken, &utc()),
            "Restored backup 28 Jun 2026"
        );
    }

    #[test]
    fn neither_version_line_dates_the_backup() {
        // The title dates the backup, from the archive. A row that dates it
        // again, from a file's modification time, gives one screen two answers
        // and no way to tell which is the lie.
        let (disk, backup) = version_lines(&entry(), &utc());
        assert_eq!(disk, "Modified 19 Sep 2026, 15:57 · 318 bytes");
        assert_eq!(backup, "Modified 22 Jul 2026, 15:57 · 228 bytes");
        assert!(
            !backup.contains("backup"),
            "a row must not date the backup: {backup}"
        );
    }

    #[test]
    fn a_change_of_type_says_which_types() {
        let mut changed = entry();
        changed.class = "type-changed".to_string();
        changed.disk_kind = "dir".to_string();
        changed.disk_size = 0;
        let (disk, backup) = version_lines(&changed, &utc());
        assert_eq!(disk, "Folder, modified 19 Sep 2026, 15:57");
        assert_eq!(backup, "Modified 22 Jul 2026, 15:57 · 228 bytes");
        assert!(!disk.contains('—'), "a folder is named, not sized: {disk}");
    }

    #[test]
    fn a_review_row_is_named_relative_to_the_folder_being_restored() {
        let target = "home/keith/Projects/website";
        assert_eq!(review_name(&entry().path, target), "content/home.md");
        assert_eq!(
            review_name("home/keith/Projects/website/README.md", target),
            "README.md"
        );
        // A neighbour that merely shares the prefix is not inside the folder,
        // and must not be named as though it were.
        assert_eq!(
            review_name("home/keith/Projects/website-old/README.md", target),
            "README.md"
        );
        // Restoring a single file: the target is the path itself, and naming
        // it relative to itself would leave the row blank.
        assert_eq!(review_name(target, target), "website");
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
                "214 items are identical — left alone",
                "6 files will be replaced (3 newer on your disk)",
                "2 items exist only in the backup — will be added",
                "4 items exist only on your disk — kept. Nothing is deleted.",
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
                "1 item is identical — left alone",
                "1 file will be replaced",
                "1 item exists only in the backup — will be added",
                "1 item exists only on your disk — kept. Nothing is deleted.",
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
            "1 item has changed type — review before replacing"
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
    fn a_backup_that_does_not_have_the_file_says_so_rather_than_reassuring() {
        let said = not_in_this_backup("report.odt");
        assert!(said.contains("report.odt"));
        assert!(
            !said.to_lowercase().contains("up to date"),
            "the one thing it must never say: {said}"
        );
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
