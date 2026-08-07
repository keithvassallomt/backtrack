// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Borg's exclusion patterns, evaluated locally.
//!
//! Borg normally does this matching itself, and for an ordinary backup it still
//! does — the excludes are handed to `borg create` and never come near this
//! module. The offline spool is the exception, and the reason is arithmetic.
//!
//! A spool run has to know *which files changed* before it can ask Borg for
//! anything, because the whole point is to archive a small delta rather than
//! walk the world. If that walk ignored exclusions, every file under `~/.cache`
//! would look changed every hour — those paths are excluded from the primary
//! backup, so the catalogue has never heard of them, so they can only compare as
//! new. The resulting file list would be tens of thousands of entries of pure
//! churn, and handing that to Borg costs either an argument list that does not
//! fit or a pattern file whose evaluation is quadratic in the size of the tree.
//!
//! So the walk prunes as it goes, and that requires understanding the same
//! patterns Borg does.
//!
//! ## What is supported, and what happens when it is not
//!
//! | prefix | meaning | supported |
//! |---|---|---|
//! | *(none)* / `fm:` | fnmatch: `*` and `?` cross `/` | yes |
//! | `sh:` | shell: `*`/`?` stop at `/`, `**/` spans directories | yes |
//! | `pp:` | path prefix — the path itself and everything under it | yes |
//! | `pf:` | exact path | yes |
//! | `re:` | regular expression | **no** |
//!
//! An unsupported pattern matches nothing, and the caller is expected to say so
//! once. That direction is deliberate: failing to exclude means a file the user
//! did not want archived is held in the local spool — encrypted with their own
//! key, on their own machine, inside a size cap, and expired after 30 days.
//! Failing the other way would mean silently *not* protecting files during an
//! offline window, which is the one thing the spool exists to do. Over-inclusion
//! is recoverable; under-protection is not.
//!
//! Semantics verified against borg 1.4.5 rather than read off the manual: `*`
//! genuinely does cross `/` in the default style (`*/keep.txt` excludes
//! `a/b/keep.txt`), `sh:*/keep.txt` genuinely does not, patterns are matched
//! against the archive-relative path rather than the basename, and a pattern
//! that matches a directory removes everything beneath it.

use std::path::Path;

/// A compiled set of exclusion patterns.
#[derive(Debug, Default)]
pub struct ExcludeSet {
    patterns: Vec<Pattern>,
    /// Patterns this build cannot evaluate, kept so the caller can name them.
    unsupported: Vec<String>,
}

#[derive(Debug)]
enum Pattern {
    /// Wildcards cross `/` (Borg's default style).
    Fnmatch(Vec<Token>),
    /// Wildcards stop at `/`; `**/` spans directories.
    Shell(Vec<Token>),
    /// The path itself, or anything beneath it.
    PathPrefix(String),
    /// Exactly this path.
    PathFull(String),
}

#[derive(Debug, PartialEq, Eq)]
enum Token {
    Literal(char),
    /// `?`
    Any,
    /// `*`
    Star,
    /// `**/` in shell style: zero or more whole directory levels.
    AnyDirs,
    Class {
        negated: bool,
        items: Vec<ClassItem>,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum ClassItem {
    Char(char),
    Range(char, char),
}

impl ExcludeSet {
    /// Compile `patterns`, in Borg's `--exclude` syntax.
    pub fn compile(patterns: &[String]) -> ExcludeSet {
        let mut set = ExcludeSet::default();
        for raw in patterns {
            match Pattern::parse(raw) {
                Some(pattern) => set.patterns.push(pattern),
                None => set.unsupported.push(raw.clone()),
            }
        }
        set
    }

    /// Patterns that were not compiled, so the caller can report them once
    /// rather than silently doing the wrong thing.
    pub fn unsupported(&self) -> &[String] {
        &self.unsupported
    }

    /// Whether `path` is excluded.
    ///
    /// `path` is archive-relative — the form Borg stores and the index holds,
    /// with no leading `/`. A directory that matches is excluded along with
    /// everything under it, which callers get by pruning the walk rather than by
    /// testing every descendant.
    pub fn excludes(&self, path: &str) -> bool {
        self.patterns.iter().any(|p| p.matches(path))
    }

    /// Whether the set has anything to say at all.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }
}

/// Strip a leading `/` so an absolute pattern lines up with the archive-relative
/// paths Borg stores. Borg does the same, which is why an exclude written as
/// `/home/k/.cache` works against a stored `home/k/.cache`.
fn relative(text: &str) -> &str {
    text.trim_start_matches('/')
}

impl Pattern {
    fn parse(raw: &str) -> Option<Pattern> {
        let (style, body) = match raw.split_once(':') {
            // Only the styles Borg defines are prefixes; anything else is a
            // pattern that happens to contain a colon.
            Some((style, body)) if matches!(style, "fm" | "sh" | "pp" | "pf" | "re") => {
                (style, body)
            }
            _ => ("fm", raw),
        };
        let body = relative(body);
        match style {
            "fm" => Some(Pattern::Fnmatch(tokenise(body, false))),
            "sh" => Some(Pattern::Shell(tokenise(body, true))),
            "pp" => Some(Pattern::PathPrefix(body.trim_end_matches('/').to_string())),
            "pf" => Some(Pattern::PathFull(body.to_string())),
            // `re:` needs a regular-expression engine. See the module note on
            // why not matching is the safe direction.
            _ => None,
        }
    }

    fn matches(&self, path: &str) -> bool {
        let path = relative(path);
        match self {
            Pattern::Fnmatch(tokens) => glob_match(tokens, path, true),
            Pattern::Shell(tokens) => glob_match(tokens, path, false),
            Pattern::PathPrefix(prefix) => {
                path == prefix
                    || (path.len() > prefix.len()
                        && path.starts_with(prefix)
                        && path.as_bytes()[prefix.len()] == b'/')
            }
            Pattern::PathFull(full) => path == full,
        }
    }
}

/// Split a pattern into tokens. `shell` enables `**/` as a distinct token.
fn tokenise(pattern: &str, shell: bool) -> Vec<Token> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' if shell && chars.get(i + 1) == Some(&'*') => {
                // `**/` spans whole directory levels, including none at all.
                // A bare trailing `**` is just a star that crosses `/`.
                if chars.get(i + 2) == Some(&'/') {
                    tokens.push(Token::AnyDirs);
                    i += 3;
                } else {
                    tokens.push(Token::AnyDirs);
                    i += 2;
                }
            }
            '*' => {
                tokens.push(Token::Star);
                i += 1;
            }
            '?' => {
                tokens.push(Token::Any);
                i += 1;
            }
            '[' => match parse_class(&chars, i) {
                Some((token, next)) => {
                    tokens.push(token);
                    i = next;
                }
                // An unterminated `[` is a literal bracket, as in fnmatch.
                None => {
                    tokens.push(Token::Literal('['));
                    i += 1;
                }
            },
            c => {
                tokens.push(Token::Literal(c));
                i += 1;
            }
        }
    }
    tokens
}

/// Parse a `[...]` class beginning at `start`, returning it and the index just
/// past its closing bracket.
fn parse_class(chars: &[char], start: usize) -> Option<(Token, usize)> {
    let mut i = start + 1;
    let negated = matches!(chars.get(i), Some('!') | Some('^'));
    if negated {
        i += 1;
    }
    let mut items = Vec::new();
    // A `]` immediately after the (possibly negated) opening bracket is a
    // literal, not a terminator — the fnmatch convention.
    let mut first = true;
    while i < chars.len() {
        let c = chars[i];
        if c == ']' && !first {
            return Some((Token::Class { negated, items }, i + 1));
        }
        first = false;
        if chars.get(i + 1) == Some(&'-') && chars.get(i + 2).is_some_and(|e| *e != ']') {
            items.push(ClassItem::Range(c, chars[i + 2]));
            i += 3;
        } else {
            items.push(ClassItem::Char(c));
            i += 1;
        }
    }
    None
}

fn class_matches(negated: bool, items: &[ClassItem], c: char) -> bool {
    let hit = items.iter().any(|item| match item {
        ClassItem::Char(x) => *x == c,
        ClassItem::Range(lo, hi) => *lo <= c && c <= *hi,
    });
    hit != negated
}

/// Match `tokens` against `text`.
///
/// `cross` says whether `*` and `?` may span `/`, which is the whole difference
/// between Borg's default style and `sh:`.
///
/// Iterative with backtracking rather than plain recursion: a pattern like
/// `*a*a*a*a*a*` against a long path is a stack overflow waiting to happen in
/// the recursive form, and exclusion patterns come from a configuration file
/// that a person edits.
fn glob_match(tokens: &[Token], text: &str, cross: bool) -> bool {
    let text: Vec<char> = text.chars().collect();
    let (mut t, mut p) = (0usize, 0usize);
    // Where to resume from if the current wildcard's guess turns out wrong.
    let mut star: Option<(usize, usize)> = None;

    while t < text.len() {
        let matched = match tokens.get(p) {
            Some(Token::Literal(c)) => *c == text[t],
            Some(Token::Any) => cross || text[t] != '/',
            Some(Token::Class { negated, items }) => {
                (cross || text[t] != '/') && class_matches(*negated, items, text[t])
            }
            Some(Token::Star) | Some(Token::AnyDirs) => {
                // Consume nothing for now; if that fails we come back and eat
                // one more character.
                star = Some((p, t));
                p += 1;
                continue;
            }
            None => false,
        };

        if matched {
            p += 1;
            t += 1;
            continue;
        }

        // Backtrack into the most recent wildcard, if it is allowed to swallow
        // the character we are stuck on.
        match star {
            Some((sp, st)) => {
                let swallowing = text[st];
                let may_swallow = match tokens.get(sp) {
                    // `**/` spans directory separators by definition.
                    Some(Token::AnyDirs) => true,
                    _ => cross || swallowing != '/',
                };
                if !may_swallow {
                    return false;
                }
                p = sp + 1;
                t = st + 1;
                star = Some((sp, st + 1));
            }
            None => return false,
        }
    }

    // Trailing wildcards may match nothing at all, which is what lets `**/foo`
    // match a bare `foo`.
    tokens[p.min(tokens.len())..]
        .iter()
        .all(|tok| matches!(tok, Token::Star | Token::AnyDirs))
}

/// Whether `path` is inside `dir` (or is `dir`).
///
/// Used to keep Backtrack's own data directory out of every walk: the spool
/// repository lives there, and a backup that archived it would be copying the
/// backup into the backup, hourly, forever.
pub fn is_within(path: &Path, dir: &Path) -> bool {
    path == dir || path.starts_with(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(patterns: &[&str]) -> ExcludeSet {
        ExcludeSet::compile(&patterns.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    /// The list the wizard seeds, which is what almost every user will run.
    fn defaults() -> ExcludeSet {
        set(&[
            "**/.cache",
            "**/Cache",
            "**/Caches",
            "**/.local/share/Trash",
            "**/node_modules",
            "**/target/debug",
            "**/*.iso",
            "**/*.qcow2",
            "**/*.vdi",
            "**/*.vmdk",
        ])
    }

    #[test]
    fn the_shipped_exclusions_catch_what_they_are_for() {
        let e = defaults();
        assert!(e.excludes("home/k/.cache"));
        assert!(e.excludes("home/k/.config/app/Cache"));
        assert!(e.excludes("home/k/.local/share/Trash"));
        assert!(e.excludes("home/k/src/app/node_modules"));
        assert!(e.excludes("home/k/src/app/target/debug"));
        assert!(e.excludes("home/k/vm/disk.qcow2"));
        assert!(e.excludes("home/k/images/ubuntu.iso"));
    }

    #[test]
    fn the_shipped_exclusions_leave_real_documents_alone() {
        let e = defaults();
        assert!(!e.excludes("home/k/Documents/report.odt"));
        assert!(!e.excludes("home/k/Pictures/holiday.jpg"));
        // Near misses that must not be swept up with the caches.
        assert!(!e.excludes("home/k/Documents/cached-notes.txt"));
        assert!(!e.excludes("home/k/Music/target/release-notes.txt"));
    }

    #[test]
    fn a_star_crosses_slashes_in_the_default_style() {
        // Verified against borg 1.4.5: `*/keep.txt` excludes `a/b/keep.txt`.
        // Getting this backwards would make `**/.cache` match only a top-level
        // `.cache`, and every user's cache would be spooled every hour.
        let e = set(&["*/keep.txt"]);
        assert!(e.excludes("a/b/keep.txt"));
        assert!(e.excludes("a/keep.txt"));
    }

    #[test]
    fn a_star_stops_at_a_slash_in_shell_style() {
        // Also verified against borg: `sh:*/keep.txt` does *not* exclude
        // `a/b/keep.txt`.
        let e = set(&["sh:*/keep.txt"]);
        assert!(!e.excludes("a/b/keep.txt"));
        assert!(e.excludes("a/keep.txt"));
    }

    #[test]
    fn shell_style_double_star_spans_directories_including_none() {
        let e = set(&["sh:**/keep.txt"]);
        assert!(e.excludes("a/b/c/keep.txt"));
        assert!(e.excludes("a/keep.txt"));
        assert!(
            e.excludes("keep.txt"),
            "`**/` matches zero directory levels too"
        );
        assert!(!e.excludes("a/keep.txt.bak"));
    }

    #[test]
    fn a_question_mark_matches_one_character() {
        assert!(set(&["a?c"]).excludes("abc"));
        assert!(!set(&["a?c"]).excludes("ac"));
        assert!(!set(&["a?c"]).excludes("abbc"));
        // And respects the style's view of `/`.
        assert!(set(&["a?c"]).excludes("a/c"));
        assert!(!set(&["sh:a?c"]).excludes("a/c"));
    }

    #[test]
    fn character_classes_and_ranges_work() {
        assert!(set(&["file[0-9].txt"]).excludes("file3.txt"));
        assert!(!set(&["file[0-9].txt"]).excludes("filex.txt"));
        assert!(set(&["file[!0-9].txt"]).excludes("filex.txt"));
        assert!(!set(&["file[!0-9].txt"]).excludes("file3.txt"));
        assert!(set(&["f[abc]g"]).excludes("fbg"));
    }

    #[test]
    fn path_prefix_takes_the_whole_subtree_and_nothing_beside_it() {
        let e = set(&["pp:home/k/big"]);
        assert!(e.excludes("home/k/big"));
        assert!(e.excludes("home/k/big/inner/file"));
        assert!(
            !e.excludes("home/k/bigger"),
            "a prefix must respect path boundaries"
        );
    }

    #[test]
    fn path_full_matches_exactly_one_path() {
        let e = set(&["pf:home/k/notes.txt"]);
        assert!(e.excludes("home/k/notes.txt"));
        assert!(!e.excludes("home/k/notes.txt.bak"));
        assert!(!e.excludes("other/home/k/notes.txt"));
    }

    #[test]
    fn an_absolute_pattern_lines_up_with_the_stored_relative_path() {
        // Borg strips the leading slash from both sides; a user writing an
        // absolute path in Preferences must get what they expect.
        let e = set(&["/home/k/.cache"]);
        assert!(e.excludes("home/k/.cache"));
        assert!(e.excludes("/home/k/.cache"));
    }

    #[test]
    fn a_regular_expression_pattern_is_reported_rather_than_guessed_at() {
        // The safe direction: it matches nothing, and the caller says so once.
        // Quietly treating it as a literal would exclude the wrong things.
        let e = set(&["re:^home/.*\\.tmp$", "**/.cache"]);
        assert_eq!(e.unsupported(), &["re:^home/.*\\.tmp$".to_string()]);
        assert!(!e.excludes("home/k/x.tmp"));
        assert!(e.excludes("home/k/.cache"), "the rest still work");
    }

    #[test]
    fn a_colon_in_a_pattern_is_not_mistaken_for_a_style() {
        let e = set(&["**/odd:name"]);
        assert!(e.unsupported().is_empty());
        assert!(e.excludes("home/k/odd:name"));
    }

    #[test]
    fn an_empty_set_excludes_nothing() {
        let e = set(&[]);
        assert!(e.is_empty());
        assert!(!e.excludes("anything/at/all"));
    }

    #[test]
    fn a_pathological_pattern_does_not_blow_the_stack() {
        // Exclusion patterns come out of a file a person edits, so the matcher
        // has to survive a bad one rather than take the daemon down.
        let e = set(&["*a*a*a*a*a*a*a*a*a*b"]);
        let text = "a".repeat(2_000);
        assert!(!e.excludes(&text));
    }

    #[test]
    fn our_own_data_directory_is_recognised() {
        let dir = Path::new("/home/k/.local/share/backtrack");
        assert!(is_within(Path::new("/home/k/.local/share/backtrack"), dir));
        assert!(is_within(
            Path::new("/home/k/.local/share/backtrack/spool/data"),
            dir
        ));
        assert!(!is_within(Path::new("/home/k/Documents"), dir));
        assert!(
            !is_within(Path::new("/home/k/.local/share/backtrack-dev"), dir),
            "a sibling with a shared prefix is not inside it"
        );
    }
}
