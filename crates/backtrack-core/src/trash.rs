// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The desktop's Trash, read as the freedesktop.org Trash specification lays
//! it out (Stage 16).
//!
//! A file created at 12:15 and moved to the Trash at 12:45 is in no backup, and
//! one that lived a few hours drops out of the history once hourly backups are
//! thinned. While it is still in the Trash, it can be shown and restored. This
//! module finds what is there and says which items hold something no backup
//! has; the daemon watches it (S16-T2) and the window shows it (S16-T3).
//!
//! A Trash is a folder holding `files/<name>`, the item itself, and
//! `info/<name>.trashinfo` beside it, which says where it came from and when it
//! was moved there. GNOME and KDE both write it. There is one in the home folder
//! (`$XDG_DATA_HOME/Trash`), and one at the top of each other drive, for what
//! was deleted from that drive: `$topdir/.Trash/$uid`, which an administrator
//! sets up, or `$topdir/.Trash-$uid`, which the desktop makes.
//!
//! Nothing here writes to a Trash. A malformed `.trashinfo`, or one whose item
//! is not there, is skipped with a debug line: the Trash belongs to the desktop,
//! and an entry it would not show either is no business of Backtrack's.

use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use tracing::debug;

use crate::index::{IndexReader, Kind, LiveEntry, Result};
use crate::pattern::{is_within, ExcludeSet};
use crate::walk::{self, WalkSpec};

/// One Trash: the folder holding `files/` and `info/`, and the folder a
/// relative `Path=` in it is relative to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trash {
    pub root: PathBuf,
    /// "The directory in which the trash directory resides": the data home for
    /// the home Trash, the drive's top for a drive's.
    base: PathBuf,
}

/// Something moved to the Trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Where it was, absolute.
    pub original: PathBuf,
    /// It, in the Trash: `<trash>/files/<name>`.
    pub copy: PathBuf,
    /// Its `<trash>/info/<name>.trashinfo`.
    pub info: PathBuf,
    /// When it was moved there, epoch seconds.
    pub deleted_at: i64,
    pub kind: Kind,
    /// Bytes, for a file; 0 for a folder.
    pub size: i64,
    /// Its modification time, epoch microseconds, as the catalogue keeps them.
    pub mtime: i64,
}

impl Item {
    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Dir
    }
}

/// How an archive name names something in the Trash, for the calls that read
/// an archive's files (`PreviewFile`, `CompareFile`): `trash:` and the item's
/// `.trashinfo`, which is unique even when the same file was trashed twice.
pub const ARCHIVE_PREFIX: &str = "trash:";

/// The archive name `item` is read through.
pub fn archive_name(item: &Item) -> String {
    format!("{ARCHIVE_PREFIX}{}", item.info.display())
}

/// The `.trashinfo` an archive name names, if it names one.
pub fn info_of(archive: &str) -> Option<&Path> {
    archive.strip_prefix(ARCHIVE_PREFIX).map(Path::new)
}

/// Where the file at archive-relative `path` is, inside the Trash's copy of
/// `item`: the copy itself, or a file below it for a trashed folder. None for
/// a path outside the item, or one that climbs with `..`.
pub fn copy_of(item: &Item, path: &str) -> Option<PathBuf> {
    let original = walk::archive_path(&item.original);
    let rest = if path == original {
        ""
    } else {
        path.strip_prefix(&original)?.strip_prefix('/')?
    };
    if rest.is_empty() {
        // Joined, an empty path would add a trailing slash, which names a
        // folder: a trashed file would then fail to open.
        return Some(item.copy.clone());
    }
    let rest = Path::new(rest);
    if rest
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    Some(item.copy.join(rest))
}

impl Trash {
    /// The home Trash of `data_home`, `<data_home>/Trash`. The daemon's is the
    /// host's ([`crate::paths::host_data_home`]): inside the Flatpak, the
    /// sandbox's own data home is somewhere else.
    pub fn in_data_home(data_home: &Path) -> Trash {
        Trash {
            root: data_home.join("Trash"),
            base: data_home.to_path_buf(),
        }
    }

    /// The Trashes at the top of a drive, for user `uid`: `.Trash/$uid` if the
    /// administrator's `.Trash` passes the specification's checks (a real
    /// folder, not a link, with the sticky bit set), and `.Trash-$uid` if it is
    /// a folder of this user's.
    pub fn on_drive(topdir: &Path, uid: u32) -> Vec<Trash> {
        let mut found = Vec::new();
        let shared = topdir.join(".Trash");
        if let Ok(meta) = std::fs::symlink_metadata(&shared) {
            let sticky = meta.permissions().mode() & 0o1000 != 0;
            if meta.is_dir() && sticky {
                let own = shared.join(uid.to_string());
                if std::fs::symlink_metadata(&own).is_ok_and(|m| m.is_dir()) {
                    found.push(Trash {
                        root: own,
                        base: topdir.to_path_buf(),
                    });
                }
            } else {
                debug!(path = %shared.display(), "a .Trash that is a link or not sticky is not used");
            }
        }
        let personal = topdir.join(format!(".Trash-{uid}"));
        if std::fs::symlink_metadata(&personal).is_ok_and(|m| m.is_dir() && m.uid() == uid) {
            found.push(Trash {
                root: personal,
                base: topdir.to_path_buf(),
            });
        }
        found
    }

    /// Everything in this Trash that can be read, its deletion dates read in
    /// the local zone, as the desktop wrote them.
    pub fn local_items(&self) -> Vec<Item> {
        self.items(&glib::TimeZone::local())
    }

    /// Everything in this Trash that can be read, in no particular order.
    pub fn items(&self, tz: &glib::TimeZone) -> Vec<Item> {
        let Ok(listing) = std::fs::read_dir(self.root.join("info")) else {
            return Vec::new();
        };
        let mut items = Vec::new();
        for entry in listing.flatten() {
            let info = entry.path();
            let Some(name) = info
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".trashinfo"))
            else {
                continue;
            };
            match self.item(&info, name, tz) {
                Some(item) => items.push(item),
                None => {
                    debug!(info = %info.display(), "a Trash entry that cannot be read is left out")
                }
            }
        }
        items
    }

    fn item(&self, info: &Path, name: &str, tz: &glib::TimeZone) -> Option<Item> {
        let text = std::fs::read_to_string(info).ok()?;
        let (path, date) = parse_info(&text)?;
        let original = self.resolve(&decode(path)?)?;
        let deleted_at = deletion_time(date, tz)?;
        let copy = self.root.join("files").join(name);
        let meta = std::fs::symlink_metadata(&copy).ok()?;
        let kind = walk::kind_of(&meta);
        Some(Item {
            original,
            copy,
            info: info.to_path_buf(),
            deleted_at,
            size: if kind == Kind::Dir {
                0
            } else {
                meta.len() as i64
            },
            mtime: walk::mtime_micros(&meta),
            kind,
        })
    }

    /// The original path a `Path=` names: absolute as it stands, relative to
    /// this Trash's base otherwise. None for one that climbs with `..`, which
    /// the specification forbids, and which could otherwise name anything.
    fn resolve(&self, bytes: &[u8]) -> Option<PathBuf> {
        let path = PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec()));
        if path.as_os_str().is_empty() || path.components().any(|c| c == Component::ParentDir) {
            return None;
        }
        Some(if path.is_absolute() {
            path
        } else {
            self.base.join(path)
        })
    }
}

/// Every Trash that can hold something deleted from `sources`: the home Trash
/// of `data_home`, and the Trashes at the top of each drive a source is on.
pub fn trashes_for(data_home: &Path, sources: &[PathBuf]) -> Vec<Trash> {
    let uid = rustix::process::getuid().as_raw();
    let mut trashes = vec![Trash::in_data_home(data_home)];
    let mut tops: Vec<PathBuf> = sources.iter().filter_map(|s| mount_top(s)).collect();
    tops.sort();
    tops.dedup();
    for top in tops {
        for trash in Trash::on_drive(&top, uid) {
            if !trashes.contains(&trash) {
                trashes.push(trash);
            }
        }
    }
    trashes
}

/// The top of the filesystem `path` is on: the highest folder above it on the
/// same device.
fn mount_top(path: &Path) -> Option<PathBuf> {
    let device = std::fs::metadata(path).ok()?.dev();
    let mut top = path.to_path_buf();
    while let Some(parent) = top.parent() {
        match std::fs::metadata(parent) {
            Ok(meta) if meta.dev() == device => top = parent.to_path_buf(),
            _ => break,
        }
    }
    Some(top)
}

/// The items whose original place is inside a backed-up folder and not
/// excluded: what a backup would have held, had it run before they went.
pub fn in_backups(items: Vec<Item>, sources: &[PathBuf], excludes: &ExcludeSet) -> Vec<Item> {
    items
        .into_iter()
        .filter(|item| {
            sources
                .iter()
                .any(|source| is_within(&item.original, source))
        })
        .filter(|item| !excludes.excludes(&walk::archive_path(&item.original)))
        .collect()
}

/// Whether `item` holds something no backup has, measured against the backups
/// the catalogue holds now: a file that is changed from the version at its
/// original place in the last backup before it was deleted, or that the
/// backup does not have; a folder with such a file inside it.
///
/// Measured against the backups as they are now, so a file pruned out of the
/// history (a few hours old, and gone once the hourly backups are thinned)
/// comes back into it from the Trash.
pub fn counts(reader: &IndexReader, item: &Item, excludes: &ExcludeSet) -> Result<bool> {
    let before = reader
        .archives_overview()?
        .into_iter()
        .find(|a| a.catalogued && a.ts <= item.deleted_at);
    let Some(before) = before else {
        // Nothing was backed up before it went, so nothing has it.
        return Ok(true);
    };
    let held = if item.is_dir() {
        let spec = WalkSpec {
            sources: vec![item.copy.clone()],
            excludes: excludes.clone(),
            one_file_system: false,
            never: Vec::new(),
            include_dirs: false,
        };
        walk::walk_as(&spec, &item.copy, &item.original).live_entries()
    } else {
        vec![LiveEntry {
            path: PathBuf::from(walk::archive_path(&item.original)),
            size: item.size,
            mtime: item.mtime,
        }]
    };
    if held.is_empty() {
        return Ok(false);
    }
    Ok(!reader.changed_since(before.seq, held)?.is_empty())
}

/// `Path=` and `DeletionDate=` from a `.trashinfo`: the first of each in its
/// `[Trash Info]` group, as the specification says, or None if either is
/// missing or the group is not there.
fn parse_info(text: &str) -> Option<(&str, &str)> {
    let mut in_group = false;
    let (mut path, mut date) = (None, None);
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with('[') {
            in_group = line == "[Trash Info]";
            continue;
        }
        if !in_group {
            continue;
        }
        if let Some(value) = line.strip_prefix("Path=") {
            path.get_or_insert(value);
        } else if let Some(value) = line.strip_prefix("DeletionDate=") {
            date.get_or_insert(value);
        }
    }
    Some((path?, date?))
}

/// Undo the URL escaping `Path=` is written in, to the file name's own bytes.
/// None for a broken escape. `+` is a plus here, not a space.
fn decode(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(out)
}

/// `at` (epoch seconds) as a `DeletionDate=` value: `YYYY-MM-DDThh:mm:ss` in
/// local time, as the desktop writes it.
pub fn format_deletion_date(at: i64) -> Option<String> {
    let local = glib::DateTime::from_unix_local(at).ok()?;
    local
        .format("%Y-%m-%dT%H:%M:%S")
        .ok()
        .map(|s| s.to_string())
}

/// `DeletionDate=` as epoch seconds. It is written `YYYY-MM-DDThh:mm:ss` in
/// local time with no zone, so `tz` is the zone it is read in: the local one,
/// but for tests.
fn deletion_time(value: &str, tz: &glib::TimeZone) -> Option<i64> {
    let (date, time) = value.trim().split_once('T')?;
    let mut ymd = date.splitn(3, '-').map(str::parse::<i32>);
    let (year, month, day) = (ymd.next()?.ok()?, ymd.next()?.ok()?, ymd.next()?.ok()?);
    let mut hms = time.splitn(3, ':');
    let hour: i32 = hms.next()?.parse().ok()?;
    let minute: i32 = hms.next()?.parse().ok()?;
    // Seconds may carry a fraction; whole seconds are all that is needed.
    let seconds: f64 = hms.next()?.parse().ok()?;
    let at = glib::DateTime::new(tz, year, month, day, hour, minute, seconds.trunc()).ok()?;
    Some(at.to_unix())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{ArchiveMeta, BorgItem, IndexWriter, Kind, Repo};

    /// A Trash in a temporary folder, as GNOME lays one out.
    struct Fixture {
        dir: tempfile::TempDir,
        trash: Trash,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let data_home = dir.path().join("share");
        let trash = Trash::in_data_home(&data_home);
        std::fs::create_dir_all(trash.root.join("files")).unwrap();
        std::fs::create_dir_all(trash.root.join("info")).unwrap();
        Fixture { dir, trash }
    }

    impl Fixture {
        /// Put `name` in the Trash with this `.trashinfo` text; a file holding
        /// `body`, or a folder when `body` is None.
        fn put(&self, name: &str, info: &str, body: Option<&str>) -> PathBuf {
            let copy = self.trash.root.join("files").join(name);
            match body {
                Some(body) => std::fs::write(&copy, body).unwrap(),
                None => std::fs::create_dir_all(&copy).unwrap(),
            }
            std::fs::write(
                self.trash
                    .root
                    .join("info")
                    .join(format!("{name}.trashinfo")),
                info,
            )
            .unwrap();
            copy
        }
    }

    fn info(path: &str, date: &str) -> String {
        format!("[Trash Info]\nPath={path}\nDeletionDate={date}\n")
    }

    fn utc() -> glib::TimeZone {
        glib::TimeZone::utc()
    }

    /// 2026-10-20T12:45:00 UTC.
    const QUARTER_TO_ONE: i64 = 1_792_500_300;

    #[test]
    fn an_item_says_where_it_came_from_and_when_it_went() {
        let f = fixture();
        let copy = f.put(
            "report.odt",
            &info("/home/k/Documents/report.odt", "2026-10-20T12:45:00"),
            Some("draft"),
        );
        let items = f.trash.items(&utc());
        let meta = std::fs::symlink_metadata(&copy).unwrap();
        assert_eq!(
            items,
            [Item {
                original: PathBuf::from("/home/k/Documents/report.odt"),
                copy,
                info: f.trash.root.join("info/report.odt.trashinfo"),
                deleted_at: QUARTER_TO_ONE,
                kind: Kind::File,
                size: 5,
                mtime: walk::mtime_micros(&meta),
            }]
        );
    }

    #[test]
    fn a_percent_encoded_name_is_decoded_to_its_own_bytes() {
        let f = fixture();
        f.put(
            "r.txt",
            &info(
                "/home/k/r%C3%A9sum%C3%A9%20final+v2.txt",
                "2026-10-20T12:45:00",
            ),
            Some("x"),
        );
        assert_eq!(
            f.trash.items(&utc())[0].original,
            PathBuf::from("/home/k/résumé final+v2.txt")
        );
    }

    #[test]
    fn a_drives_trash_names_its_items_from_the_drives_top() {
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path();
        let uid = rustix::process::getuid().as_raw();
        let personal = top.join(format!(".Trash-{uid}"));
        std::fs::create_dir_all(personal.join("files")).unwrap();
        std::fs::create_dir_all(personal.join("info")).unwrap();
        std::fs::write(personal.join("files/a.jpg"), "jpeg").unwrap();
        std::fs::write(
            personal.join("info/a.jpg.trashinfo"),
            info("photos/2026/a.jpg", "2026-10-20T12:45:00"),
        )
        .unwrap();

        let trashes = Trash::on_drive(top, uid);
        assert_eq!(trashes.len(), 1);
        assert_eq!(
            trashes[0].items(&utc())[0].original,
            top.join("photos/2026/a.jpg")
        );
    }

    #[test]
    fn a_shared_trash_is_used_only_when_it_is_sticky_and_not_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path();
        let uid = rustix::process::getuid().as_raw();
        let shared = top.join(".Trash");
        std::fs::create_dir_all(shared.join(uid.to_string())).unwrap();

        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(Trash::on_drive(top, uid).is_empty(), "not sticky");

        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o1777)).unwrap();
        assert_eq!(Trash::on_drive(top, uid).len(), 1, "sticky");

        let other = tempfile::tempdir().unwrap();
        let elsewhere = other.path().join(".Trash");
        std::fs::create_dir_all(elsewhere.join(uid.to_string())).unwrap();
        std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o1777)).unwrap();
        let linked = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(&elsewhere, linked.path().join(".Trash")).unwrap();
        assert!(Trash::on_drive(linked.path(), uid).is_empty(), "a link");
    }

    #[test]
    fn the_deletion_date_is_read_in_the_local_zone() {
        // Two hours ahead of UTC, as Malta is in summer.
        let plus_two = glib::TimeZone::new(Some("+02:00"));
        assert_eq!(
            deletion_time("2026-10-20T14:45:00", &plus_two),
            Some(QUARTER_TO_ONE)
        );
        assert_eq!(
            deletion_time("2026-10-20T12:45:00.250", &utc()),
            Some(QUARTER_TO_ONE),
            "a fraction of a second is allowed"
        );
        assert_eq!(deletion_time("20 Oct 2026", &utc()), None);
    }

    #[test]
    fn a_deletion_date_written_here_reads_back_as_the_same_moment() {
        let written = format_deletion_date(QUARTER_TO_ONE).unwrap();
        assert_eq!(
            deletion_time(&written, &glib::TimeZone::local()),
            Some(QUARTER_TO_ONE)
        );
    }

    #[test]
    fn entries_that_cannot_be_read_are_left_out() {
        let f = fixture();
        f.put(
            "fine.txt",
            &info("/home/k/fine.txt", "2026-10-20T12:45:00"),
            Some("x"),
        );
        f.put(
            "no-group.txt",
            "Path=/home/k/a\nDeletionDate=2026-10-20T12:45:00\n",
            Some("x"),
        );
        f.put("no-date.txt", "[Trash Info]\nPath=/home/k/b\n", Some("x"));
        f.put(
            "bad-escape.txt",
            &info("/home/k/c%G1", "2026-10-20T12:45:00"),
            Some("x"),
        );
        f.put("bad-date.txt", &info("/home/k/d", "yesterday"), Some("x"));
        f.put(
            "climbs.txt",
            &info("/home/k/../../etc/shadow", "2026-10-20T12:45:00"),
            Some("x"),
        );
        // Orphaned: a .trashinfo with nothing in files/.
        std::fs::write(
            f.trash.root.join("info/gone.txt.trashinfo"),
            info("/home/k/gone.txt", "2026-10-20T12:45:00"),
        )
        .unwrap();
        let originals: Vec<PathBuf> = f
            .trash
            .items(&utc())
            .into_iter()
            .map(|i| i.original)
            .collect();
        assert_eq!(originals, [PathBuf::from("/home/k/fine.txt")]);
    }

    #[test]
    fn an_item_is_named_as_an_archive_and_found_from_its_name() {
        let f = fixture();
        f.put(
            "a.txt",
            &info("/home/k/a.txt", "2026-10-20T12:45:00"),
            Some("x"),
        );
        let item = &f.trash.items(&utc())[0];
        let name = archive_name(item);
        assert!(name.starts_with("trash:"));
        assert_eq!(info_of(&name), Some(item.info.as_path()));
        assert_eq!(info_of("bt-fedora-20261020T085450Z"), None);
    }

    #[test]
    fn a_path_inside_a_trashed_folder_is_found_in_its_copy_and_nothing_outside() {
        let item = Item {
            original: PathBuf::from("/home/k/project"),
            copy: PathBuf::from("/t/files/project"),
            info: PathBuf::new(),
            deleted_at: 0,
            kind: Kind::Dir,
            size: 0,
            mtime: 0,
        };
        // Compared as text: `PathBuf` equality ignores a trailing slash, and a
        // trailing slash is what made a trashed file fail to open.
        let text = |p: Option<PathBuf>| p.map(|p| p.into_os_string().into_string().unwrap());
        assert_eq!(
            text(copy_of(&item, "home/k/project")),
            Some("/t/files/project".to_string())
        );
        assert_eq!(
            text(copy_of(&item, "home/k/project/src/main.rs")),
            Some("/t/files/project/src/main.rs".to_string())
        );
        assert_eq!(copy_of(&item, "home/k/project2/x"), None, "a sibling");
        assert_eq!(copy_of(&item, "home/k/other"), None);
        assert_eq!(copy_of(&item, "home/k/project/../../.ssh/id_ed25519"), None);
    }

    #[test]
    fn the_first_path_and_date_are_the_ones_used() {
        let text = "[Trash Info]\nPath=/first\nDeletionDate=2026-10-20T12:45:00\nPath=/second\n";
        assert_eq!(parse_info(text), Some(("/first", "2026-10-20T12:45:00")));
    }

    #[test]
    fn only_items_from_backed_up_folders_that_are_not_excluded_are_kept() {
        let item = |path: &str| Item {
            original: PathBuf::from(path),
            copy: PathBuf::new(),
            info: PathBuf::new(),
            deleted_at: 0,
            kind: Kind::File,
            size: 0,
            mtime: 0,
        };
        let kept = in_backups(
            vec![
                item("/home/k/Documents/letter.odt"),
                item("/home/k/Downloads/setup.bin"),
                item("/home/k/Documents/disk.iso"),
                item("/home/k/Documents2/x"),
            ],
            &[PathBuf::from("/home/k/Documents")],
            &ExcludeSet::compile(&["**/*.iso".to_string()]),
        );
        let kept: Vec<&Path> = kept.iter().map(|i| i.original.as_path()).collect();
        assert_eq!(kept, [Path::new("/home/k/Documents/letter.odt")]);
    }

    // ── counting ────────────────────────────────────────────────────────────

    /// A catalogue whose archive `n` is at `n * 1000` seconds, from listings
    /// oldest first.
    fn catalogue(dir: &Path, archives: Vec<Vec<BorgItem>>) -> PathBuf {
        let path = dir.join("index.db");
        let mut w = IndexWriter::open(&path).unwrap();
        for (n, items) in archives.into_iter().enumerate() {
            let n = n as i64 + 1;
            let meta = ArchiveMeta {
                borg_id: None,
                name: format!("a{n}"),
                ts: n * 1000,
            };
            w.ingest_archive(&meta, Repo::Primary, items.into_iter())
                .unwrap();
        }
        path
    }

    /// What the catalogue would record for the file at `copy`, as though it
    /// were at `original`.
    fn as_backed_up(copy: &Path, original: &str) -> BorgItem {
        let meta = std::fs::symlink_metadata(copy).unwrap();
        BorgItem {
            path: original.trim_start_matches('/').to_string(),
            kind: Kind::File,
            size: meta.len() as i64,
            mtime: walk::mtime_micros(&meta),
            mode: 0o644,
            chunk_hash: None,
        }
    }

    fn dir_item(path: &str) -> BorgItem {
        BorgItem {
            path: path.to_string(),
            kind: Kind::Dir,
            size: 0,
            mtime: 100,
            mode: 0o755,
            chunk_hash: None,
        }
    }

    fn trashed(copy: PathBuf, original: &str, deleted_at: i64) -> Item {
        let meta = std::fs::symlink_metadata(&copy).unwrap();
        let kind = walk::kind_of(&meta);
        Item {
            original: PathBuf::from(original),
            copy,
            info: PathBuf::new(),
            deleted_at,
            size: if kind == Kind::Dir {
                0
            } else {
                meta.len() as i64
            },
            mtime: walk::mtime_micros(&meta),
            kind,
        }
    }

    fn none() -> ExcludeSet {
        ExcludeSet::compile(&[])
    }

    #[test]
    fn a_file_the_backup_already_has_does_not_count() {
        let f = fixture();
        let copy = f.put("a.txt", "", Some("same"));
        let index = catalogue(
            f.dir.path(),
            vec![vec![dir_item("home"), as_backed_up(&copy, "/home/a.txt")]],
        );
        let reader = IndexReader::open(&index).unwrap();
        assert!(!counts(&reader, &trashed(copy, "/home/a.txt", 1500), &none()).unwrap());
    }

    #[test]
    fn a_file_changed_since_the_backup_or_never_in_it_counts() {
        let f = fixture();
        let older = f.put("older.txt", "", Some("the backed-up text"));
        let backed_up = as_backed_up(&older, "/home/a.txt");
        let changed = f.put("a.txt", "", Some("edited after the backup"));
        let new = f.put("new.txt", "", Some("created after it"));
        let index = catalogue(f.dir.path(), vec![vec![dir_item("home"), backed_up]]);
        let reader = IndexReader::open(&index).unwrap();
        assert!(counts(&reader, &trashed(changed, "/home/a.txt", 1500), &none()).unwrap());
        assert!(counts(&reader, &trashed(new, "/home/new.txt", 1500), &none()).unwrap());
    }

    #[test]
    fn a_file_deleted_before_any_backup_counts() {
        let f = fixture();
        let copy = f.put("a.txt", "", Some("x"));
        let index = catalogue(
            f.dir.path(),
            vec![vec![dir_item("home"), as_backed_up(&copy, "/home/a.txt")]],
        );
        let reader = IndexReader::open(&index).unwrap();
        assert!(counts(&reader, &trashed(copy, "/home/a.txt", 500), &none()).unwrap());
    }

    #[test]
    fn a_folder_counts_through_a_changed_file_inside_it() {
        let f = fixture();
        let folder = f.put("project", "", None);
        std::fs::write(folder.join("same.txt"), "same").unwrap();
        let same = as_backed_up(&folder.join("same.txt"), "/home/project/same.txt");
        let index = catalogue(
            f.dir.path(),
            vec![vec![dir_item("home"), dir_item("home/project"), same]],
        );
        let reader = IndexReader::open(&index).unwrap();
        let item = trashed(folder.clone(), "/home/project", 1500);
        assert!(
            !counts(&reader, &item, &none()).unwrap(),
            "everything in it is backed up"
        );

        std::fs::write(folder.join("notes.txt"), "written after the backup").unwrap();
        assert!(
            counts(&reader, &item, &none()).unwrap(),
            "one file in it is not"
        );
    }

    #[test]
    fn a_file_pruned_out_of_the_history_counts_again() {
        // Created after the first backup, held by the second, trashed after
        // it; then the second is pruned, as thinning does.
        let f = fixture();
        let copy = f.put("a.txt", "", Some("lived a few hours"));
        let dir = tempfile::tempdir().unwrap();
        let index = catalogue(
            dir.path(),
            vec![
                vec![dir_item("home")],
                vec![dir_item("home"), as_backed_up(&copy, "/home/a.txt")],
            ],
        );
        let item = trashed(copy, "/home/a.txt", 2500);
        let reader = IndexReader::open(&index).unwrap();
        assert!(
            !counts(&reader, &item, &none()).unwrap(),
            "the second backup has it"
        );
        drop(reader);

        IndexWriter::open(&index)
            .unwrap()
            .remove_archives(&[2])
            .unwrap();
        let reader = IndexReader::open(&index).unwrap();
        assert!(
            counts(&reader, &item, &none()).unwrap(),
            "and once it is pruned, nothing does"
        );
    }
}
