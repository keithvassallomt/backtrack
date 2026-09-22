// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The words on a search result, worked out where a test can read them.
//!
//! Charlie's story is the one this serves: he downloaded something last week,
//! it is not where he left it, and he does not remember which folder that was.
//! What a result has to tell him is therefore not "here is a file" but "here
//! is the file you mean, this is where it lived, this is when it existed, and
//! no, it is not on your computer any more".
//!
//! The lifespan line is the load-bearing part. "Existed: 26 Jun to 1 Jul" is
//! how a person recognises the thing they lost, far more reliably than a path
//! they have already forgotten.

use backtrack_core::dbus::SearchResult;
use backtrack_core::index::Kind;
use gtk4::glib;

use crate::model::format;

/// One result, as the card draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    /// Archive-relative path, which is what the actions act on.
    pub path: String,
    pub name: String,
    pub kind: Kind,
    /// The folders above the file, as the card shows them: "Documents › Clients".
    pub breadcrumb: String,
    /// "Existed: 26 Jun – 1 Jul · 8 versions · 214 KB".
    pub lifespan: String,
    /// Whether to show the "no longer on your disk" tag.
    pub gone: bool,
}

/// Turn what the daemon answered into what the window draws.
pub fn cards(
    hits: &[SearchResult],
    home: Option<&str>,
    now: i64,
    tz: &glib::TimeZone,
) -> Vec<Card> {
    hits.iter().map(|hit| card(hit, home, now, tz)).collect()
}

fn card(hit: &SearchResult, home: Option<&str>, now: i64, tz: &glib::TimeZone) -> Card {
    let kind = Kind::from_token(&hit.kind);
    Card {
        breadcrumb: breadcrumb(&hit.path, home),
        lifespan: lifespan(hit, kind, now, tz),
        gone: hit.gone_from_disk,
        path: hit.path.clone(),
        name: hit.name.clone(),
        kind,
    }
}

/// The folders holding `path`, with the home directory left off.
///
/// A catalogued path is absolute — `home/keith/Documents/Clients/x.odt` — so
/// every result would otherwise open with the same two components, which is
/// two words of nothing on every card. The same shorthand the breadcrumb in
/// the header bar uses, for the same reason.
pub fn breadcrumb(path: &str, home: Option<&str>) -> String {
    let inside_home = home
        .filter(|h| !h.is_empty())
        .and_then(|h| path.strip_prefix(h))
        .map(|rest| rest.trim_start_matches('/'))
        .filter(|rest| !rest.is_empty());
    let path = inside_home.unwrap_or(path);

    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() <= 1 {
        return String::new();
    }
    // Everything but the file itself, and never more than the last three
    // folders: beyond that the line wraps and stops being scannable.
    let folders = &parts[..parts.len() - 1];
    let shown = &folders[folders.len().saturating_sub(3)..];
    let mut crumbs: Vec<String> = shown.iter().map(|part| part.to_string()).collect();
    if folders.len() > shown.len() {
        crumbs.insert(0, "…".to_string());
    }
    crumbs.push(parts[parts.len() - 1].to_string());
    crumbs.join(" › ")
}

/// "Existed: 26 Jun – 1 Jul · 8 versions · 214 KB".
///
/// The end of the range reads "today" when the file is still in the newest
/// backup, because a date there invites the reader to work out whether it is
/// recent, and the answer they want is simply yes.
fn lifespan(hit: &SearchResult, kind: Kind, now: i64, tz: &glib::TimeZone) -> String {
    let day = |ts: i64| format::at(ts, tz, "%-d %b");
    let ends_today = same_day(hit.last_ts, now, tz);

    let mut parts = vec![format!(
        "Existed: {} – {}",
        day(hit.first_ts),
        if ends_today {
            "today".to_string()
        } else {
            day(hit.last_ts)
        }
    )];

    parts.push(match hit.versions {
        1 => "1 version".to_string(),
        n => format!("{n} versions"),
    });

    // A folder has no size worth reporting: Borg records the directory entry,
    // not what is inside it, so the number would be a few kilobytes of
    // metadata presented as though it were the contents.
    if kind != Kind::Dir {
        parts.push(format::size(hit.size, kind));
    } else {
        parts.push("folder".to_string());
    }

    parts.join(" · ")
}

fn same_day(ts: i64, now: i64, tz: &glib::TimeZone) -> bool {
    format::at(ts, tz, "%F") == format::at(now, tz, "%F")
}

/// "3 files matched across 47 backups".
///
/// Both halves matter. The first says how much there is to read; the second
/// says how much was looked at, which is the thing that separates this from
/// the search box in a file manager.
pub fn caption(files: usize, backups: usize) -> String {
    format!(
        "{files} file{} matched across {backups} backup{}",
        if files == 1 { "" } else { "s" },
        if backups == 1 { "" } else { "s" },
    )
}

/// What the results area says when there is nothing in it.
///
/// Three states, because they want three different things said. Nobody has
/// typed anything yet; what was typed is too short to be answered; and a real
/// query that found nothing.
pub fn empty_state(query: &str, minimum: usize) -> (&'static str, String) {
    let typed = query.trim().chars().count();
    if typed == 0 {
        return (
            "Search every backup",
            "Type part of a file's name. Backtrack looks through every backup it has, \
             including the ones where the file no longer exists."
                .to_string(),
        );
    }
    if typed < minimum {
        return (
            "Keep typing",
            format!("A search needs at least {minimum} characters."),
        );
    }
    (
        "Nothing found",
        format!(
            "No file in any backup has a name matching “{}”.",
            query.trim()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tz() -> glib::TimeZone {
        glib::TimeZone::utc()
    }

    const HOME: Option<&str> = Some("home/keith");

    /// 2026-06-26T12:00:00Z and 2026-07-01T12:00:00Z.
    const JUN_26: i64 = 1_782_475_200;
    const JUL_01: i64 = 1_782_907_200;

    fn hit() -> SearchResult {
        SearchResult {
            path: "home/keith/Downloads/contract-final.pdf".to_string(),
            name: "contract-final.pdf".to_string(),
            kind: "file".to_string(),
            first_seq: 1,
            last_seq: 8,
            first_ts: JUN_26,
            last_ts: JUL_01,
            versions: 8,
            size: 214 * 1000,
            gone_from_disk: true,
        }
    }

    #[test]
    fn a_deleted_file_reads_the_way_the_mockup_says() {
        let card = card(&hit(), HOME, JUL_01 + 86_400 * 3, &tz());
        assert_eq!(card.breadcrumb, "Downloads › contract-final.pdf");
        // A non-breaking space before the unit, which is glib's doing and is
        // right: "214.0" and "kB" must not be split across a line.
        assert_eq!(
            card.lifespan,
            "Existed: 26 Jun – 1 Jul · 8 versions · 214.0\u{a0}kB",
        );
        assert!(card.gone);
    }

    /// A file still in the newest backup ends its range with a word, not a
    /// date somebody has to compare against today's.
    #[test]
    fn a_file_that_is_still_there_ends_today() {
        let mut hit = hit();
        hit.gone_from_disk = false;
        let card = card(&hit, HOME, JUL_01, &tz());
        assert!(
            card.lifespan.starts_with("Existed: 26 Jun – today ·"),
            "{}",
            card.lifespan
        );
        assert!(!card.gone);
    }

    #[test]
    fn one_version_is_not_pluralised() {
        let mut hit = hit();
        hit.versions = 1;
        assert!(card(&hit, HOME, JUL_01, &tz())
            .lifespan
            .contains("· 1 version ·"));
    }

    /// A folder's size is the directory entry's, which says nothing about what
    /// is in it, so the card does not pretend otherwise.
    #[test]
    fn a_folder_says_folder_rather_than_a_misleading_size() {
        let mut hit = hit();
        hit.kind = "dir".to_string();
        hit.name = "old-contracts".to_string();
        hit.path = "home/keith/Documents/Archive/old-contracts".to_string();
        let card = card(&hit, HOME, JUL_01, &tz());
        assert!(card.lifespan.ends_with("· folder"), "{}", card.lifespan);
        assert_eq!(card.breadcrumb, "Documents › Archive › old-contracts");
    }

    /// The home directory is four components every result would share, so the
    /// crumb keeps the end and says it dropped the rest.
    #[test]
    fn a_deep_path_keeps_the_end_and_marks_what_it_dropped() {
        assert_eq!(
            breadcrumb("home/keith/a/b/c/d/report.odt", HOME),
            "… › b › c › d › report.odt",
        );
    }

    #[test]
    fn a_path_with_no_folders_has_no_breadcrumb() {
        assert_eq!(breadcrumb("report.odt", HOME), "");
    }

    #[test]
    fn the_caption_counts_both_sides() {
        assert_eq!(caption(3, 47), "3 files matched across 47 backups");
        assert_eq!(caption(1, 1), "1 file matched across 1 backup");
        assert_eq!(caption(0, 30), "0 files matched across 30 backups");
    }

    #[test]
    fn the_empty_state_says_a_different_thing_for_each_reason() {
        assert_eq!(empty_state("", 2).0, "Search every backup");
        assert_eq!(empty_state(" a ", 2).0, "Keep typing");
        assert_eq!(empty_state("zzz", 2).0, "Nothing found");
        assert!(empty_state("zzz", 2).1.contains("zzz"));
    }
}
