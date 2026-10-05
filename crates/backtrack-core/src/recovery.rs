// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Disaster recovery: what "restore this computer" means for one backup.
//!
//! A backup stores the paths of the computer it came from. A new computer's
//! home folder may be somewhere else entirely, under another name, so the
//! first question is which folder of the backup *was* the home folder. That
//! folder's contents come back into this computer's home, one step at a time:
//! each visible folder on its own, and the hidden folders and loose files
//! together as one last step. The steps are what the progress window ticks
//! off and what "Restore selected folders…" offers to choose from.
//!
//! The daemon and the window both work this out, the daemon to run it and the
//! window to offer it, from the same catalogue through the same function, so
//! the two cannot disagree about what a step contains.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::index::{IndexReader, Kind};

/// The key of the step holding the hidden folders and loose files.
///
/// No folder can be called `.`, so it cannot collide with one that is.
pub const THE_REST: &str = ".";

/// Folders inside a home folder that a recovery never restores: Backtrack's
/// own data, release and development. The daemon doing the restoring owns its
/// copy, and the old computer's catalogue, settings and bookkeeping written
/// over it would describe a computer that no longer exists.
const NEVER: &[&str] = &[".local/share/backtrack", ".local/share/backtrack-dev"];

/// One step of a recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// The folder's name, or [`THE_REST`].
    pub key: String,
    /// The archive paths it brings back.
    pub members: Vec<String>,
    /// Bytes of the files in it, from the catalogue.
    pub bytes: u64,
    /// How many files it holds.
    pub files: u64,
}

/// What restoring this computer from one backup would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// The archive path that stands for the home folder.
    pub source: String,
    /// The steps, in the order they run.
    pub steps: Vec<Step>,
    /// Archive paths below `source` that are never restored.
    pub never: Vec<String>,
}

impl Layout {
    /// Bytes across every step.
    pub fn bytes(&self) -> u64 {
        self.steps.iter().map(|s| s.bytes).sum()
    }

    /// Only the steps whose keys are in `keys`, in their usual order.
    pub fn only(mut self, keys: &[String]) -> Layout {
        self.steps.retain(|step| keys.contains(&step.key));
        self
    }
}

/// Which folder of a backup was the home folder, given what it was asked to
/// back up (`roots`, archive paths), this computer's home folder in archive
/// form, and the name of the person using it.
///
/// In order of how sure the answer is:
///
/// 1. A root inside this computer's own home folder, or one holding it. The
///    same person on a computer set up the same way, which is the usual case.
/// 2. The folder named after this person: `home/keith` for `keith`, wherever
///    it is (`var/home/keith` on Silverblue).
/// 3. Some other person's folder in `home/`: the backups came from a
///    computer where the account had another name.
/// 4. Whatever folder holds every root, or for a single root, the folder
///    above it, so that the folder that was backed up arrives as a folder.
///
/// `None` only when nothing was backed up.
pub fn source_home(roots: &[String], home: &str, user: &str) -> Option<String> {
    let roots: Vec<&str> = roots
        .iter()
        .map(|root| root.trim_matches('/'))
        .filter(|root| !root.is_empty())
        .collect();
    if roots.is_empty() {
        return None;
    }

    let home = home.trim_matches('/');
    if !home.is_empty()
        && roots
            .iter()
            .any(|root| within(root, home) || within(home, root))
    {
        return Some(home.to_string());
    }

    // The home folder each root is in, by the rules that recognise one; the
    // one most of the roots agree on wins.
    let mut votes: Vec<(String, usize)> = Vec::new();
    for root in &roots {
        if let Some(found) = named_home(root, user).or_else(|| home_shaped(root)) {
            match votes.iter_mut().find(|(home, _)| *home == found) {
                Some((_, count)) => *count += 1,
                None => votes.push((found, 1)),
            }
        }
    }
    if let Some((found, _)) = votes.iter().max_by_key(|(_, count)| *count) {
        return Some(found.clone());
    }

    let shared = common_folder(&roots);
    if roots.len() == 1 {
        return Some(parent(&shared).unwrap_or_default());
    }
    Some(shared)
}

/// The deepest folder of `root` named after `user`.
fn named_home(root: &str, user: &str) -> Option<String> {
    if user.is_empty() {
        return None;
    }
    let parts: Vec<&str> = root.split('/').collect();
    let at = parts.iter().rposition(|part| *part == user)?;
    Some(parts[..=at].join("/"))
}

/// `home/<name>` or `var/home/<name>`, if `root` is inside one.
fn home_shaped(root: &str) -> Option<String> {
    let parts: Vec<&str> = root.split('/').collect();
    match parts.as_slice() {
        ["home", name, ..] => Some(format!("home/{name}")),
        ["var", "home", name, ..] => Some(format!("var/home/{name}")),
        _ => None,
    }
}

/// The deepest folder that is, or holds, every one of `roots`.
fn common_folder(roots: &[&str]) -> String {
    let mut shared: Vec<&str> = roots[0].split('/').collect();
    for root in &roots[1..] {
        let parts: Vec<&str> = root.split('/').collect();
        let same = shared
            .iter()
            .zip(&parts)
            .take_while(|(a, b)| a == b)
            .count();
        shared.truncate(same);
    }
    shared.join("/")
}

fn parent(path: &str) -> Option<String> {
    path.rsplit_once('/').map(|(parent, _)| parent.to_string())
}

/// Whether `path` is `folder` or inside it. The empty folder is the root of
/// the archive, which holds everything.
pub fn within(path: &str, folder: &str) -> bool {
    folder.is_empty() || path == folder || path.starts_with(&format!("{folder}/"))
}

/// `path` with `folder` taken off the front, or `None` if it is not inside.
pub fn relative<'a>(path: &'a str, folder: &str) -> Option<&'a str> {
    if folder.is_empty() {
        return Some(path);
    }
    if path == folder {
        return Some("");
    }
    path.strip_prefix(folder)?.strip_prefix('/')
}

/// Work out the recovery of archive `seq` into the home folder `home`, for
/// the person called `user`.
///
/// `None` when the backup holds nothing at all.
pub fn layout(
    reader: &IndexReader,
    seq: i64,
    home: &Path,
    user: &str,
) -> crate::index::Result<Option<Layout>> {
    let roots = reader.backed_up_roots(seq)?;
    let home = home.to_string_lossy();
    let Some(source) = source_home(&roots, &home, user) else {
        return Ok(None);
    };

    let never: Vec<String> = NEVER.iter().map(|tail| join(&source, tail)).collect();
    let mut hidden = Step {
        key: THE_REST.to_string(),
        members: Vec::new(),
        bytes: 0,
        files: 0,
    };
    let mut folders = Vec::new();
    for entry in reader.sizes_inside(&source, seq)? {
        let member = join(&source, &entry.name);
        if entry.kind == Kind::Dir && !entry.name.starts_with('.') {
            folders.push(Step {
                key: entry.name,
                members: vec![member],
                bytes: entry.bytes,
                files: entry.files,
            });
        } else {
            hidden.members.push(member);
            hidden.bytes += entry.bytes;
            hidden.files += entry.files;
        }
    }
    // What is never restored is not counted either, or the bar would stop
    // short of the end by exactly that much.
    for skipped in &never {
        let (bytes, files) = reader.size_of(skipped, seq)?;
        hidden.bytes = hidden.bytes.saturating_sub(bytes);
        hidden.files = hidden.files.saturating_sub(files);
    }

    // Smallest first: most folders are back soonest, and the ones people
    // want first, documents above all, are rarely the big ones.
    folders.sort_by(|a, b| a.bytes.cmp(&b.bytes).then_with(|| a.key.cmp(&b.key)));
    let mut steps = folders;
    if !hidden.members.is_empty() {
        steps.push(hidden);
    }
    Ok(Some(Layout {
        source,
        steps,
        never,
    }))
}

/// What to back up on this computer once it has been restored: everything the
/// old one backed up that is inside its home folder, at the same place inside
/// this one's.
pub fn backups_after(roots: &[String], source: &str, home: &Path) -> Vec<PathBuf> {
    let mut include: Vec<PathBuf> = roots
        .iter()
        .filter_map(|root| relative(root.trim_matches('/'), source))
        .map(|inside| {
            if inside.is_empty() {
                home.to_path_buf()
            } else {
                home.join(inside)
            }
        })
        .collect();
    include.sort();
    include.dedup();
    include
}

/// A Borg patterns file selecting `whole` (each with everything below it) and
/// `exact` (each path by itself), and never anything at or below `never`.
///
/// `None` when a path cannot be written into one. Borg reads the file a line
/// at a time and trims each line, so a name with a line break in it, or one
/// ending in a space, would select something else or nothing; the caller has
/// to fetch those another way.
pub fn patterns(never: &[String], whole: &[String], exact: &[String]) -> Option<String> {
    let representable = |path: &String| {
        !path.is_empty() && !path.contains(['\n', '\r']) && !path.ends_with(char::is_whitespace)
    };
    if !never.iter().chain(whole).chain(exact).all(representable) {
        return None;
    }
    // First match wins, so what is never restored comes before what is
    // asked for, and the closing line turns down everything else.
    let mut text = String::new();
    for path in never {
        text.push_str(&format!("- pp:{path}\n"));
    }
    for path in whole {
        text.push_str(&format!("+ pp:{path}\n"));
    }
    for path in exact {
        text.push_str(&format!("+ pf:{path}\n"));
    }
    text.push_str("- *\n");
    Some(text)
}

fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_string()
    } else {
        format!("{folder}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{ArchiveMeta, BorgItem, IndexWriter, Repo};

    fn roots(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn the_same_home_on_a_new_computer_is_restored_in_place() {
        assert_eq!(
            source_home(&roots(&["home/keith"]), "/home/keith", "keith").as_deref(),
            Some("home/keith")
        );
        // The usual folders chosen one by one are inside it too.
        assert_eq!(
            source_home(
                &roots(&["home/keith/Documents", "home/keith/Pictures"]),
                "/home/keith",
                "keith"
            )
            .as_deref(),
            Some("home/keith")
        );
    }

    #[test]
    fn a_backup_of_every_home_restores_this_persons() {
        assert_eq!(
            source_home(&roots(&["home"]), "/home/keith", "keith").as_deref(),
            Some("home/keith")
        );
    }

    #[test]
    fn one_folder_backed_up_still_comes_back_into_its_place_in_the_home() {
        // Not into the top of the home folder: Documents arrives as Documents.
        assert_eq!(
            source_home(&roots(&["home/keith/Documents"]), "/home/keith", "keith").as_deref(),
            Some("home/keith")
        );
    }

    #[test]
    fn a_home_somewhere_else_is_found_by_the_persons_name() {
        // Backed up on Silverblue, restored on a scratch home.
        assert_eq!(
            source_home(
                &roots(&["var/home/keith/Documents", "var/home/keith/Music"]),
                "/tmp/drill/home",
                "keith"
            )
            .as_deref(),
            Some("var/home/keith")
        );
    }

    #[test]
    fn a_home_under_another_name_is_still_a_home() {
        assert_eq!(
            source_home(&roots(&["home/kvassallo"]), "/home/keith", "keith").as_deref(),
            Some("home/kvassallo")
        );
    }

    #[test]
    fn folders_from_outside_any_home_arrive_as_folders() {
        // One folder: the folder above it stands for the home, so the folder
        // itself is what lands in it.
        assert_eq!(
            source_home(&roots(&["srv/photos"]), "/home/keith", "keith").as_deref(),
            Some("srv")
        );
        // Two folders: whatever holds both.
        assert_eq!(
            source_home(&roots(&["data/a", "data/b"]), "/home/keith", "keith").as_deref(),
            Some("data")
        );
        assert_eq!(
            source_home(&roots(&["photos"]), "/home/keith", "keith").as_deref(),
            Some("")
        );
    }

    #[test]
    fn a_home_beside_something_else_is_still_the_home() {
        assert_eq!(
            source_home(&roots(&["etc", "home/old"]), "/home/keith", "keith").as_deref(),
            Some("home/old")
        );
    }

    #[test]
    fn nothing_backed_up_is_nothing_to_restore() {
        assert_eq!(source_home(&[], "/home/keith", "keith"), None);
    }

    #[test]
    fn paths_inside_and_outside_a_folder() {
        assert!(within("home/k/Documents", "home/k"));
        assert!(within("home/k", "home/k"));
        assert!(!within("home/kate", "home/k"));
        assert!(within("anything", ""));
        assert_eq!(
            relative("home/k/Documents/a", "home/k"),
            Some("Documents/a")
        );
        assert_eq!(relative("home/k", "home/k"), Some(""));
        assert_eq!(relative("home/kate", "home/k"), None);
    }

    #[test]
    fn what_was_backed_up_is_what_is_backed_up_afterwards() {
        let home = Path::new("/home/new");
        assert_eq!(
            backups_after(
                &roots(&["home/old/Documents", "home/old/Pictures", "etc"]),
                "home/old",
                home
            ),
            vec![
                PathBuf::from("/home/new/Documents"),
                PathBuf::from("/home/new/Pictures")
            ]
        );
        assert_eq!(
            backups_after(&roots(&["home/old"]), "home/old", home),
            vec![PathBuf::from("/home/new")]
        );
    }

    #[test]
    fn a_patterns_file_turns_down_what_is_never_restored_first() {
        let text = patterns(
            &roots(&["home/k/.local/share/backtrack"]),
            &roots(&["home/k/.local"]),
            &roots(&["home/k/Pictures/a b.jpg"]),
        )
        .unwrap();
        assert_eq!(
            text,
            "- pp:home/k/.local/share/backtrack\n\
             + pp:home/k/.local\n\
             + pf:home/k/Pictures/a b.jpg\n\
             - *\n"
        );
    }

    #[test]
    fn a_name_a_patterns_file_cannot_hold_is_refused_rather_than_mangled() {
        for awkward in ["home/k/two\nlines", "home/k/trailing ", "home/k/cr\r"] {
            assert_eq!(patterns(&[], &[], &roots(&[awkward])), None, "{awkward:?}");
        }
    }

    fn item(path: &str, kind: Kind, size: i64) -> BorgItem {
        BorgItem {
            path: path.to_string(),
            kind,
            size,
            mtime: 100,
            mode: 0o644,
            chunk_hash: None,
        }
    }

    fn catalogue(items: Vec<BorgItem>) -> (tempfile::TempDir, IndexReader) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("index.db");
        let mut writer = IndexWriter::open(&path).unwrap();
        writer
            .ingest_archive(
                &ArchiveMeta {
                    borg_id: Some("id".into()),
                    name: "bt-old-1".into(),
                    ts: 1_000,
                },
                Repo::Primary,
                items.into_iter(),
            )
            .unwrap();
        drop(writer);
        let reader = IndexReader::open(&path).unwrap();
        (tmp, reader)
    }

    #[test]
    fn a_home_is_restored_folder_by_folder_smallest_first_and_the_rest_last() {
        let (_tmp, reader) = catalogue(vec![
            item("home/old", Kind::Dir, 0),
            item("home/old/Pictures", Kind::Dir, 0),
            item("home/old/Pictures/beach.jpg", Kind::File, 5_000),
            item("home/old/Documents", Kind::Dir, 0),
            item("home/old/Documents/report.odt", Kind::File, 300),
            item("home/old/Music", Kind::Dir, 0),
            item("home/old/Music/song.ogg", Kind::File, 3_000),
            item("home/old/.config", Kind::Dir, 0),
            item("home/old/.config/app.ini", Kind::File, 40),
            item("home/old/todo.txt", Kind::File, 2),
            item("home/old/.local", Kind::Dir, 0),
            item("home/old/.local/share", Kind::Dir, 0),
            item("home/old/.local/share/backtrack", Kind::Dir, 0),
            item(
                "home/old/.local/share/backtrack/config.toml",
                Kind::File,
                500,
            ),
            item("home/old/.local/share/notes.db", Kind::File, 60),
        ]);

        let layout = layout(&reader, 1, Path::new("/home/new"), "new")
            .unwrap()
            .unwrap();

        assert_eq!(layout.source, "home/old");
        let keys: Vec<&str> = layout.steps.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, ["Documents", "Music", "Pictures", THE_REST]);
        let rest = layout.steps.last().unwrap();
        assert_eq!(
            rest.members,
            ["home/old/.config", "home/old/.local", "home/old/todo.txt"]
        );
        // Backtrack's own settings are inside `.local` and are not counted,
        // because they are never brought back.
        assert_eq!((rest.bytes, rest.files), (102, 3));
        assert_eq!(layout.bytes(), 300 + 3_000 + 5_000 + 102);
        assert!(layout
            .never
            .contains(&"home/old/.local/share/backtrack".to_string()));

        let chosen = layout.only(&["Pictures".to_string(), THE_REST.to_string()]);
        let keys: Vec<&str> = chosen.steps.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, ["Pictures", THE_REST]);
    }
}
