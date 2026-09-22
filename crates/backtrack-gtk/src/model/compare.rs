// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! What two versions of a file differ by — worked out without a display.
//!
//! The compare window is two panes of text with some of the lines coloured,
//! and the part most likely to be wrong is which lines those are. Deciding it
//! here, over bytes and strings, means a test can ask "what does comparing
//! these two files look like" without a compositor, a theme or a font.
//!
//! The vocabulary is the mockup's, and it is one-sided on purpose: the left
//! pane is the backup and the right is today, so a line that only the backup
//! has was **removed since** (red, on the left) and a line only today has was
//! **added since** (green, on the right). "Added" and "removed" always mean
//! what happened *to the file on the computer*, never what the diff algorithm
//! did.

use similar::{ChangeTag, TextDiff};

/// What one line in one pane is, which is the whole of what the window needs
/// to know in order to draw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// Present in both versions: drawn plainly.
    Same,
    /// On this side only. Red in the backup pane, green in the today pane.
    Changed,
    /// Nothing here — the other side has a line and this one does not. Drawn
    /// as blank space, so the two panes stay in step with each other going
    /// down the window.
    Gap,
}

/// One line of one pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub mark: Mark,
}

/// A comparison of two text files, laid out as two columns of equal height.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextComparison {
    pub backup: Vec<Line>,
    pub today: Vec<Line>,
    /// How many runs of consecutive changed lines there are, counting a
    /// replacement — lines gone from one side and lines arrived on the other,
    /// in the same place — as one.
    ///
    /// This is the footer's "N sections differ", and a section is what a person
    /// would point at and call one edit, not a line count. Rewriting a
    /// paragraph is one difference, and saying "6 sections differ" about it
    /// would be arithmetic rather than information.
    pub sections: usize,
}

/// Compare two texts line by line.
pub fn text(backup: &str, today: &str) -> TextComparison {
    let diff = TextDiff::from_lines(backup, today);

    let mut comparison = TextComparison::default();
    // A section is open while consecutive ops keep changing things; the first
    // unchanged line closes it. Deletions followed immediately by insertions
    // are a rewrite, and a rewrite is one section.
    let mut in_section = false;

    for change in diff.iter_all_changes() {
        let text = change.value().trim_end_matches(['\r', '\n']).to_string();
        match change.tag() {
            ChangeTag::Equal => {
                in_section = false;
                comparison.backup.push(Line {
                    text: text.clone(),
                    mark: Mark::Same,
                });
                comparison.today.push(Line {
                    text,
                    mark: Mark::Same,
                });
            }
            ChangeTag::Delete => {
                if !in_section {
                    comparison.sections += 1;
                    in_section = true;
                }
                comparison.backup.push(Line {
                    text,
                    mark: Mark::Changed,
                });
            }
            ChangeTag::Insert => {
                if !in_section {
                    comparison.sections += 1;
                    in_section = true;
                }
                comparison.today.push(Line {
                    text,
                    mark: Mark::Changed,
                });
            }
        }
    }

    pad_to_equal_height(&mut comparison);
    comparison
}

/// Make the two columns the same height by filling the shorter side with gaps.
///
/// Done per section rather than at the end, so a change stays beside the change
/// it corresponds to: a paragraph replaced halfway down a file must not push
/// the rest of one pane out of step with the other for the remaining length of
/// the document.
fn pad_to_equal_height(comparison: &mut TextComparison) {
    let mut backup = Vec::with_capacity(comparison.backup.len());
    let mut today = Vec::with_capacity(comparison.today.len());
    let (mut b, mut t) = (0, 0);

    while b < comparison.backup.len() || t < comparison.today.len() {
        // A run of changed lines on either side is one section; the two runs
        // sit beside each other and the shorter is padded out.
        let b_run = run_of_changes(&comparison.backup, b);
        let t_run = run_of_changes(&comparison.today, t);

        if b_run == 0 && t_run == 0 {
            if b < comparison.backup.len() {
                backup.push(comparison.backup[b].clone());
                b += 1;
            }
            if t < comparison.today.len() {
                today.push(comparison.today[t].clone());
                t += 1;
            }
            continue;
        }

        let height = b_run.max(t_run);
        for step in 0..height {
            backup.push(match step < b_run {
                true => comparison.backup[b + step].clone(),
                false => gap(),
            });
            today.push(match step < t_run {
                true => comparison.today[t + step].clone(),
                false => gap(),
            });
        }
        b += b_run;
        t += t_run;
    }

    comparison.backup = backup;
    comparison.today = today;
}

/// How many changed lines start at `from`.
fn run_of_changes(lines: &[Line], from: usize) -> usize {
    lines[from.min(lines.len())..]
        .iter()
        .take_while(|line| line.mark == Mark::Changed)
        .count()
}

fn gap() -> Line {
    Line {
        text: String::new(),
        mark: Mark::Gap,
    }
}

/// The footer's summary of a comparison.
pub fn sections_differ(sections: usize) -> String {
    match sections {
        0 => "No differences".to_string(),
        1 => "1 section differs".to_string(),
        n => format!("{n} sections differ"),
    }
}

/// The heading above one pane: "Backup — 12 Jun 2026, 09:00 · 45 KB".
pub fn pane_heading(side: &str, when: i64, size: i64, tz: &gtk4::glib::TimeZone) -> String {
    format!(
        "{side} — {} · {}",
        crate::model::format::modified(when, tz),
        crate::model::format::size(size, backtrack_core::index::Kind::File),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(lines: &[Line]) -> Vec<Mark> {
        lines.iter().map(|line| line.mark).collect()
    }

    #[test]
    fn two_identical_files_have_nothing_to_show() {
        let same = "one\ntwo\nthree\n";
        let comparison = text(same, same);
        assert_eq!(comparison.sections, 0);
        assert!(marks(&comparison.backup).iter().all(|m| *m == Mark::Same));
        assert_eq!(comparison.backup.len(), comparison.today.len());
    }

    /// The acceptance case: a file with two separate edits reads as two
    /// sections, not as however many lines they happen to span.
    #[test]
    fn two_separated_edits_are_two_sections() {
        let backup = "\
title
intro paragraph
marketing spend was higher
product development
closing remarks
thanks
";
        let today = "\
title
intro paragraph
we launched a programme
and expanded integrations
product development
closing remarks
a different sign-off
";
        let comparison = text(backup, today);
        assert_eq!(comparison.sections, 2, "two edits, two sections");
    }

    /// A rewritten paragraph is one difference, however many lines go and
    /// however many arrive.
    #[test]
    fn a_replacement_counts_once_however_lopsided() {
        let comparison = text("a\nb\nc\nd\ne\n", "a\nX\nY\nZ\nW\ne\n");
        assert_eq!(comparison.sections, 1);
    }

    /// The two panes are the same height, so they scroll together and a change
    /// sits beside what replaced it.
    #[test]
    fn the_panes_line_up() {
        let comparison = text("a\nb\nc\nd\ne\n", "a\nX\nY\nZ\nW\ne\n");
        assert_eq!(comparison.backup.len(), comparison.today.len());

        // The one deleted line is padded out to meet the four that replaced it.
        assert_eq!(
            marks(&comparison.backup),
            vec![
                Mark::Same,
                Mark::Changed,
                Mark::Changed,
                Mark::Changed,
                Mark::Gap,
                Mark::Same,
            ],
        );
        assert_eq!(
            marks(&comparison.today),
            vec![
                Mark::Same,
                Mark::Changed,
                Mark::Changed,
                Mark::Changed,
                Mark::Changed,
                Mark::Same,
            ],
        );
    }

    /// A change late in a long file must not knock the rest of it out of step.
    #[test]
    fn an_edit_halfway_down_leaves_the_rest_aligned() {
        let backup = format!("{}gone\n{}", "same\n".repeat(20), "tail\n".repeat(20));
        let today = format!("{}{}", "same\n".repeat(20), "tail\n".repeat(20));
        let comparison = text(&backup, &today);

        assert_eq!(comparison.sections, 1);
        assert_eq!(comparison.backup.len(), comparison.today.len());
        // Every line after the edit is identical on both sides and is drawn as
        // such — not shunted one row apart for the rest of the document.
        for (b, t) in comparison.backup.iter().zip(&comparison.today).skip(21) {
            assert_eq!(b.mark, Mark::Same);
            assert_eq!(t.mark, Mark::Same);
            assert_eq!(b.text, t.text);
        }
    }

    /// Text added at the end of a file is a section, and the backup side is
    /// padded rather than left short.
    #[test]
    fn an_appended_line_is_a_difference() {
        let comparison = text("a\nb\n", "a\nb\nc\n");
        assert_eq!(comparison.sections, 1);
        assert_eq!(comparison.backup.len(), comparison.today.len());
        assert_eq!(comparison.backup.last().unwrap().mark, Mark::Gap);
        assert_eq!(comparison.today.last().unwrap().text, "c");
    }

    /// An empty file against a full one is every line, once.
    #[test]
    fn an_empty_side_is_one_section() {
        let comparison = text("", "a\nb\nc\n");
        assert_eq!(comparison.sections, 1);
        assert_eq!(comparison.today.len(), 3);
        assert!(marks(&comparison.backup).iter().all(|m| *m == Mark::Gap));
    }

    /// Line endings belong to the file format, not to the content a person
    /// reads, so they are not drawn and do not make a line look changed.
    #[test]
    fn line_endings_are_not_content() {
        let comparison = text("a\r\nb\r\n", "a\r\nb\r\n");
        assert_eq!(comparison.sections, 0);
        assert_eq!(comparison.backup[0].text, "a");
    }

    /// The demo fixture's own two-edit file, written out here so the recipe
    /// and this model are held against each other. `just demo-conflicts`
    /// stages `content/about.md` in exactly these two revisions, and the
    /// compare view's acceptance is the number this returns.
    #[test]
    fn the_demo_fixtures_two_edit_file_reads_as_two_sections() {
        let backup = "\
Acme Tooling: about this site

We build tooling for small manufacturing teams.
Founded in 2019, we work with clients across Europe.

Our office is open Monday to Friday, 9 until 5.
Call us on 2100 0000, or write to hello@example.invalid.

Thanks for reading.
";
        let today = "\
Acme Tooling: about this site

We build tooling for small manufacturing teams.
Founded in 2019, we now work with clients worldwide.

Our office is open Monday to Friday, 9 until 5.
Call us on 2100 0000, or write to hello@example.invalid.

Thanks for reading.
We are hiring: see the careers page.
";
        let comparison = text(backup, today);
        assert_eq!(comparison.sections, 2);
        assert_eq!(sections_differ(comparison.sections), "2 sections differ");

        // One line replaced, one line added, and the panes still line up.
        assert_eq!(comparison.backup.len(), comparison.today.len());
        assert_eq!(
            comparison
                .backup
                .iter()
                .filter(|l| l.mark == Mark::Changed)
                .count(),
            1,
        );
        assert_eq!(
            comparison
                .today
                .iter()
                .filter(|l| l.mark == Mark::Changed)
                .count(),
            2,
        );
    }

    #[test]
    fn the_footer_counts_in_words_a_person_would_use() {
        assert_eq!(sections_differ(0), "No differences");
        assert_eq!(sections_differ(1), "1 section differs");
        assert_eq!(sections_differ(4), "4 sections differ");
    }
}
