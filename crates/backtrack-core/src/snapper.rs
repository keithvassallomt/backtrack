// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Snapper's snapshots, found where Snapper keeps them (Stage 17).
//!
//! Snapper takes btrfs snapshots on a schedule of its own, and goes on taking
//! them through the hours Backtrack does not back up: on battery, and while
//! backups are paused. Where it covers a backed-up folder, the timeline shows
//! its snapshots among the backups. Backtrack reads them, and never creates,
//! changes or removes one.
//!
//! A Snapper configuration keeps its snapshots in a `.snapshots` folder at the
//! top of the subvolume it covers, each as `<N>/snapshot/`, the copy, with
//! `<N>/info.xml` beside it. That is all that is read: no configuration under
//! `/etc`, and no call to Snapper's service, so the Flatpak needs no
//! permission it does not already have.
//!
//! `.snapshots` belongs to root, and others may not read it. A configuration
//! naming a user in `ALLOW_USERS`, with `SYNC_ACL=yes`, gives that user an ACL
//! entry on it; until then it is [`Found::Unreadable`], which the window
//! reports as something to change (S17-T4).

use std::path::{Path, PathBuf};

use tracing::debug;

use crate::btrfs;
use crate::index::parse_borg_mtime;

/// The folder at the top of a subvolume holding Snapper's snapshots of it.
pub const FOLDER: &str = ".snapshots";

/// Snapper's snapshots of one subvolume holding backed-up folders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapper {
    /// The subvolume they copy.
    pub subvolume: PathBuf,
    /// Its `.snapshots` folder.
    pub folder: PathBuf,
    pub found: Found,
}

/// What a `.snapshots` folder gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// The snapshots in it that can be read, by number.
    Snapshots(Vec<Snapshot>),
    /// It is there, and this user may not read it.
    Unreadable,
}

/// One of Snapper's snapshots, as its `info.xml` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub number: u64,
    pub kind: Type,
    /// When it was taken, epoch seconds.
    pub date: i64,
    /// Empty when Snapper was given none.
    pub description: String,
    /// The copy of the subvolume, `<.snapshots>/<N>/snapshot`.
    pub path: PathBuf,
}

/// A snapshot's type, as Snapper names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    /// Taken on its own: on Snapper's timeline, or by hand.
    Single,
    /// Taken before a change, such as a package update.
    Pre,
    /// Taken after that change, paired with the one before it.
    Post,
}

/// How an archive name names one of Snapper's snapshots: `snapper:` and its
/// folder, `<.snapshots>/<N>`, which no other snapshot on the machine shares.
pub const ARCHIVE_PREFIX: &str = "snapper:";

/// The archive name `snapshot` is catalogued under.
pub fn archive_name(snapshot: &Snapshot) -> String {
    let folder = snapshot.path.parent().unwrap_or(&snapshot.path);
    format!("{ARCHIVE_PREFIX}{}", folder.display())
}

/// For the archive name of one of Snapper's snapshots, its copy
/// (`<.snapshots>/<N>/snapshot`) and the subvolume it is a snapshot of (the
/// folder `.snapshots` is in). None for any other name.
pub fn copy_of(archive: &str) -> Option<(PathBuf, PathBuf)> {
    let folder = Path::new(archive.strip_prefix(ARCHIVE_PREFIX)?);
    let number = folder.file_name()?.to_str()?;
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let snapshots = folder.parent()?;
    if snapshots.file_name()? != FOLDER {
        return None;
    }
    Some((folder.join("snapshot"), snapshots.parent()?.to_path_buf()))
}

impl Snapper {
    /// The snapshots that could be read; none, when the folder could not be.
    pub fn snapshots(&self) -> &[Snapshot] {
        match &self.found {
            Found::Snapshots(snapshots) => snapshots,
            Found::Unreadable => &[],
        }
    }
}

/// Snapper's snapshots of each subvolume holding one of `sources`, in the
/// order of the sources and each subvolume once. A source not on btrfs, or in
/// a subvolume with no `.snapshots` folder, gives nothing.
pub fn find(sources: &[PathBuf]) -> Vec<Snapper> {
    let mut found: Vec<Snapper> = Vec::new();
    for source in sources {
        let Some(subvolume) = btrfs::containing_subvolume(source) else {
            continue;
        };
        if found.iter().any(|s| s.subvolume == subvolume.root) {
            continue;
        }
        found.extend(read(&subvolume.root));
    }
    found
}

/// Snapper's snapshots of the subvolume at `root`, or None when it has no
/// `.snapshots` folder.
pub fn read(root: &Path) -> Option<Snapper> {
    let folder = root.join(FOLDER);
    // Snapper makes a folder (a subvolume, or a mount on openSUSE's root), so
    // a link or a file by that name is not its.
    if !std::fs::symlink_metadata(&folder).ok()?.is_dir() {
        return None;
    }
    let found = match std::fs::read_dir(&folder) {
        Ok(listing) => {
            let mut snapshots: Vec<Snapshot> = listing
                .flatten()
                .filter_map(|entry| snapshot(&folder, &entry.file_name()))
                .collect();
            snapshots.sort_by_key(|s| s.number);
            Found::Snapshots(snapshots)
        }
        Err(e) => {
            debug!(folder = %folder.display(), "Snapper's snapshots cannot be read: {e}");
            Found::Unreadable
        }
    };
    Some(Snapper {
        subvolume: root.to_path_buf(),
        folder,
        found,
    })
}

/// The snapshot `<folder>/<name>`, or None for anything else in the folder,
/// for snapshot 0, and for one that cannot be read whole.
fn snapshot(folder: &Path, name: &std::ffi::OsStr) -> Option<Snapshot> {
    let number: u64 = name
        .to_str()
        .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))?
        .parse()
        .ok()?;
    // Snapper's name for the live filesystem, never a copy of it.
    if number == 0 {
        return None;
    }
    let dir = folder.join(name);
    let info = dir.join("info.xml");
    let skipped = |why: &str| {
        debug!(snapshot = %dir.display(), "a Snapper snapshot is left out: {why}");
        None
    };
    let Ok(text) = std::fs::read_to_string(&info) else {
        return skipped("its info.xml cannot be read");
    };
    let Some(parsed) = parse_info(&text) else {
        return skipped("its info.xml is not as Snapper writes it");
    };
    if parsed.number != number {
        return skipped("its info.xml gives another number");
    }
    let path = dir.join("snapshot");
    if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
        return skipped("it has no snapshot folder");
    }
    Some(Snapshot {
        number,
        kind: parsed.kind,
        date: parsed.date,
        description: parsed.description,
        path,
    })
}

/// What an `info.xml` says.
#[derive(Debug, PartialEq, Eq)]
struct Info {
    number: u64,
    kind: Type,
    date: i64,
    description: String,
}

/// An `info.xml`, as Snapper writes it through libxml2: a `<snapshot>`
/// element holding one element per field, text only but for `<userdata>`.
/// None when `<num>`, `<type>` or `<date>` is missing or unreadable.
fn parse_info(text: &str) -> Option<Info> {
    let body = element(text, "snapshot")?;
    let field = |name: &str| children(body).find(|(n, _)| *n == name).map(|(_, v)| v);
    let number = unescape(field("num")?)?.trim().parse().ok()?;
    let kind = match unescape(field("type")?)?.trim() {
        "single" => Type::Single,
        "pre" => Type::Pre,
        "post" => Type::Post,
        _ => return None,
    };
    // `YYYY-MM-DD hh:mm:ss`, in UTC whatever the machine's zone: a snapshot
    // taken with TZ=Asia/Tokyo at 08:49:39 UTC is written 08:49:39.
    let date = unescape(field("date")?)?;
    let date = parse_borg_mtime(&date.trim().replacen(' ', "T", 1)).ok()? / 1_000_000;
    let description = match field("description") {
        Some(value) => unescape(value)?,
        None => String::new(),
    };
    Some(Info {
        number,
        kind,
        date,
        description,
    })
}

/// What is between `<name>` and `</name>`, the first such element in `text`.
fn element<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = text.find(&open)? + open.len();
    let end = start + text[start..].find(&format!("</{name}>"))?;
    Some(&text[start..end])
}

/// The elements directly inside `body`, as names and their unescaped-to-be
/// contents; an element written `<name/>` has none. Anything that is not an
/// element (the whitespace between them, a comment) is passed over.
fn children(body: &str) -> impl Iterator<Item = (&str, &str)> {
    let mut rest = body;
    std::iter::from_fn(move || loop {
        let open = rest.find('<')?;
        rest = &rest[open + 1..];
        if let Some(after) = rest.strip_prefix("!--") {
            rest = &after[after.find("-->")? + 3..];
            continue;
        }
        let tag_end = rest.find('>')?;
        let tag = &rest[..tag_end];
        rest = &rest[tag_end + 1..];
        if let Some(name) = tag.strip_suffix('/') {
            return Some((name.trim(), ""));
        }
        if tag.starts_with(['/', '?', '!']) {
            continue;
        }
        let name = tag.split_whitespace().next()?;
        let close = format!("</{name}>");
        let end = rest.find(&close)?;
        let value = &rest[..end];
        rest = &rest[end + close.len()..];
        return Some((name, value));
    })
}

/// XML's escapes undone: the five named ones and character references, which
/// libxml2 writes for every character outside ASCII when, as here, the file
/// names no encoding. None for any other `&`.
fn unescape(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at + 1..];
        let end = rest.find(';')?;
        let entity = &rest[..end];
        rest = &rest[end + 1..];
        let c = match entity {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = match entity.strip_prefix("#x") {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => entity.strip_prefix('#')?.parse().ok()?,
                };
                char::from_u32(code)?
            }
        };
        out.push(c);
    }
    out.push_str(rest);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// `info.xml` files copied from snapshots Snapper 0.13.0 took on Fedora 45,
    /// on 2026-10-26, of a configuration made for the purpose.
    const SINGLE: &str = include_str!("snapper/testdata/single.xml");
    const PRE: &str = include_str!("snapper/testdata/pre.xml");
    const POST: &str = include_str!("snapper/testdata/post.xml");
    const TIMELINE: &str = include_str!("snapper/testdata/timeline.xml");
    const ESCAPED: &str = include_str!("snapper/testdata/escaped.xml");
    const BY_A_USER: &str = include_str!("snapper/testdata/by-a-user.xml");
    const NO_DESCRIPTION: &str = include_str!("snapper/testdata/no-description.xml");
    const NON_ASCII: &str = include_str!("snapper/testdata/non-ascii.xml");

    /// 2026-10-26 08:49:39 UTC, when the first five were taken.
    const TAKEN: i64 = 1_793_004_579;

    /// A subvolume's top, in a temporary folder, with a `.snapshots` in it.
    struct Fixture {
        root: tempfile::TempDir,
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(FOLDER)).unwrap();
        Fixture { root }
    }

    impl Fixture {
        fn folder(&self) -> PathBuf {
            self.root.path().join(FOLDER)
        }

        /// Snapshot `name` with this `info.xml`, and its copy holding one file.
        fn put(&self, name: &str, info: &str) -> PathBuf {
            let dir = self.folder().join(name);
            std::fs::create_dir_all(dir.join("snapshot/Documents")).unwrap();
            std::fs::write(dir.join("snapshot/Documents/notes.txt"), "one").unwrap();
            std::fs::write(dir.join("info.xml"), info).unwrap();
            dir
        }

        fn snapshots(&self) -> Vec<Snapshot> {
            match read(self.root.path()).expect("a .snapshots folder").found {
                Found::Snapshots(snapshots) => snapshots,
                Found::Unreadable => panic!("the fixture is readable"),
            }
        }

        fn numbers(&self) -> Vec<u64> {
            self.snapshots().iter().map(|s| s.number).collect()
        }
    }

    /// `xml` as the `info.xml` of snapshot `n`: Snapper's files with the number
    /// changed, so that one can serve for several.
    fn numbered(xml: &str, n: u64) -> String {
        let num = element(xml, "num").unwrap();
        xml.replacen(&format!("<num>{num}</num>"), &format!("<num>{n}</num>"), 1)
    }

    #[test]
    fn a_snapshot_is_read_from_its_info_xml() {
        let f = fixture();
        let dir = f.put("1", SINGLE);
        assert_eq!(
            f.snapshots(),
            [Snapshot {
                number: 1,
                kind: Type::Single,
                date: TAKEN,
                description: "before the edit".to_string(),
                path: dir.join("snapshot"),
            }]
        );
    }

    #[test]
    fn a_snapshot_is_named_by_its_folder() {
        let f = fixture();
        f.put("42", &numbered(TIMELINE, 42));
        let snapshots = f.snapshots();
        assert_eq!(
            archive_name(&snapshots[0]),
            format!("snapper:{}", f.folder().join("42").display())
        );
    }

    #[test]
    fn an_archive_name_gives_back_the_copy_and_its_subvolume() {
        assert_eq!(
            copy_of("snapper:/home/.snapshots/42"),
            Some((
                PathBuf::from("/home/.snapshots/42/snapshot"),
                PathBuf::from("/home")
            ))
        );
        assert_eq!(
            copy_of("snapper:/.snapshots/7"),
            Some((PathBuf::from("/.snapshots/7/snapshot"), PathBuf::from("/")))
        );
        for other in [
            "bt-host-20261026T090000Z",
            "trash:/home/k/.local/share/Trash/info/a.trashinfo",
            "snapper:/home/.snapshots/42/../../etc",
            "snapper:/home/elsewhere/42",
            "snapper:/home/.snapshots/x",
        ] {
            assert_eq!(copy_of(other), None, "{other}");
        }
    }

    #[test]
    fn the_date_is_read_as_utc_which_snapper_writes_whatever_the_zone() {
        // SINGLE was taken by `snapper --no-dbus` run with TZ=Asia/Tokyo, nine
        // hours ahead, at 08:49:39 UTC, and says 08:49:39.
        assert!(SINGLE.contains("<date>2026-10-26 08:49:39</date>"));
        assert_eq!(parse_info(SINGLE).unwrap().date, TAKEN);
        assert_eq!(parse_info(BY_A_USER).unwrap().date, TAKEN + 14);
    }

    #[test]
    fn each_of_snappers_types_is_read() {
        let kinds: Vec<Type> = [SINGLE, PRE, POST, TIMELINE]
            .iter()
            .map(|xml| parse_info(xml).unwrap().kind)
            .collect();
        assert_eq!(kinds, [Type::Single, Type::Pre, Type::Post, Type::Single]);
        let post = parse_info(POST).unwrap();
        assert_eq!(
            (post.number, post.description.as_str()),
            (3, "dnf install gimp"),
            "the userdata after the description is passed over"
        );
    }

    #[test]
    fn a_description_is_unescaped_to_what_was_typed() {
        assert_eq!(
            parse_info(ESCAPED).unwrap().description,
            r#"with <xml> & "quotes""#
        );
        assert_eq!(
            parse_info(NON_ASCII).unwrap().description,
            "résumé – ünïcode ✓",
            "libxml2 writes each character outside ASCII as a reference"
        );
    }

    #[test]
    fn a_snapshot_with_no_description_has_an_empty_one() {
        let info = parse_info(NO_DESCRIPTION).unwrap();
        assert_eq!((info.number, info.description.as_str()), (7, ""));
    }

    #[test]
    fn a_snapshot_a_user_took_reads_the_same() {
        // Through ALLOW_USERS, Snapper adds a <uid> before the description.
        let info = parse_info(BY_A_USER).unwrap();
        assert_eq!(
            (info.number, info.kind, info.description.as_str()),
            (6, Type::Single, "made by keith")
        );
    }

    #[test]
    fn snapshots_come_in_number_order() {
        let f = fixture();
        for n in [10, 2, 1] {
            f.put(&n.to_string(), &numbered(TIMELINE, n));
        }
        assert_eq!(f.numbers(), [1, 2, 10]);
    }

    #[test]
    fn snapshot_zero_is_left_out() {
        let f = fixture();
        f.put("0", &numbered(SINGLE, 0));
        f.put("1", SINGLE);
        assert_eq!(f.numbers(), [1]);
    }

    #[test]
    fn a_snapshot_that_cannot_be_read_whole_is_left_out() {
        let f = fixture();
        f.put("1", SINGLE);
        // No info.xml.
        let dir = f.put("2", PRE);
        std::fs::remove_file(dir.join("info.xml")).unwrap();
        // An info.xml cut short.
        f.put("3", &POST[..POST.find("<date>").unwrap()]);
        // One with no number, and one with no date.
        f.put("4", &TIMELINE.replace("<num>4</num>", ""));
        f.put("5", &numbered(TIMELINE, 5).replace("<date>", "<when>"));
        // One with a type Snapper does not write.
        f.put("6", &numbered(TIMELINE, 6).replace(">single<", ">other<"));
        // One whose info.xml names another snapshot.
        f.put("7", &numbered(TIMELINE, 8));
        // No snapshot folder.
        let dir = f.put("9", &numbered(TIMELINE, 9));
        std::fs::remove_dir_all(dir.join("snapshot")).unwrap();
        // A broken escape in the description.
        f.put("11", &numbered(ESCAPED, 11).replace("&amp;", "&"));
        // Not a snapshot at all.
        std::fs::write(f.folder().join("12"), "a file").unwrap();
        std::fs::create_dir(f.folder().join("bt-probe")).unwrap();
        f.put("13", TIMELINE);
        f.put("14", &numbered(TIMELINE, 14));
        assert_eq!(f.numbers(), [1, 14]);
    }

    #[test]
    fn an_unreadable_snapshots_folder_is_reported_as_unreadable() {
        // CI runs in a container as root, which reads it regardless.
        if rustix::process::geteuid().is_root() {
            eprintln!("skipping: running as root, which bypasses permission checks");
            return;
        }
        let f = fixture();
        f.put("1", SINGLE);
        // As Snapper leaves it without ALLOW_USERS: root's, with no access for
        // others.
        std::fs::set_permissions(f.folder(), std::fs::Permissions::from_mode(0o000)).unwrap();
        let found = read(f.root.path());
        std::fs::set_permissions(f.folder(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            found,
            Some(Snapper {
                subvolume: f.root.path().to_path_buf(),
                folder: f.folder(),
                found: Found::Unreadable,
            })
        );
    }

    #[test]
    fn a_subvolume_with_no_snapshots_folder_gives_nothing() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(read(root.path()), None);
        std::fs::write(root.path().join(FOLDER), "not a folder").unwrap();
        assert_eq!(
            read(root.path()),
            None,
            "a file by that name is not Snapper's"
        );
    }

    #[test]
    fn an_empty_snapshots_folder_is_read_as_no_snapshots() {
        let f = fixture();
        assert_eq!(f.snapshots(), []);
    }

    #[test]
    fn a_folder_not_on_btrfs_has_no_snapper() {
        let f = fixture();
        f.put("1", SINGLE);
        if btrfs::is_btrfs(f.root.path()) {
            // A temporary folder on btrfs is covered by the test below.
            return;
        }
        assert_eq!(find(&[f.root.path().to_path_buf()]), []);
    }

    /// The whole of [`find`] on a real btrfs filesystem: subvolumes this user
    /// makes in their cache folder, as the daemon's snapshot tests do.
    /// Skipped with a warning where that is not btrfs.
    #[cfg(feature = "integration")]
    #[test]
    fn the_snapshots_of_the_subvolume_holding_each_folder_are_found() {
        let Some(base) = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        else {
            return;
        };
        if !btrfs::is_btrfs(&base) {
            eprintln!("skipping: {} is not on btrfs", base.display());
            return;
        }
        // Removed however the test ends, an assertion failing included.
        struct Scratch(PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let scratch = base.join(format!("backtrack-snapper-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).unwrap();
        let _cleanup = Scratch(scratch.clone());
        let subvolume = |at: &Path| {
            std::process::Command::new("btrfs")
                .args(["subvolume", "create"])
                .arg(at)
                .stdout(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        let home = scratch.join("home");
        if !subvolume(&home) {
            eprintln!("skipping: cannot create a subvolume here");
            return;
        }
        let documents = home.join("keith/Documents");
        let pictures = home.join("keith/Pictures");
        std::fs::create_dir_all(&documents).unwrap();
        std::fs::create_dir_all(&pictures).unwrap();
        // A folder that is a subvolume of its own is not in the snapshots of
        // the one around it.
        let projects = home.join("keith/Projects");
        assert!(subvolume(&projects));

        let sources = [documents, pictures, projects.clone()];
        assert_eq!(find(&sources), [], "no .snapshots, no Snapper");

        let folder = home.join(FOLDER);
        std::fs::create_dir_all(folder.join("1/snapshot")).unwrap();
        std::fs::write(folder.join("1/info.xml"), SINGLE).unwrap();
        let found = find(&sources);

        assert_eq!(found.len(), 1, "one subvolume, found once: {found:?}");
        assert_eq!(found[0].subvolume, home);
        assert_eq!(found[0].folder, folder);
        let Found::Snapshots(snapshots) = &found[0].found else {
            panic!("readable");
        };
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].path, folder.join("1/snapshot"));
    }
}
