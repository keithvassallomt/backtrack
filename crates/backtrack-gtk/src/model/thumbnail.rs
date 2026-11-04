// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Which files in Rewind's front window show a thumbnail, and where it comes
//! from (S18-T4).
//!
//! The desktop keeps a thumbnail of each file it has shown, in the cache the
//! freedesktop thumbnail specification describes: a PNG named by the MD5 of
//! the file's URI, under `~/.cache/thumbnails/<size>/`, which records the
//! modification time of the file it was made from (`Thumb::MTime`). Where that
//! time is the backed-up version's, the thumbnail is of that version, so it is
//! shown as it is. Where it is not (the file changed since, or was deleted, or
//! the desktop never showed it), an image's thumbnail is made from the
//! backup's own copy, fetched only for the window in front and only as it
//! comes into sight; the folder as it is now is read from the disk. Every
//! other file shows its icon.

use std::path::{Path, PathBuf};

/// Where an entry's picture comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The desktop's thumbnail of this very version.
    Cache(PathBuf),
    /// Made from the backup's copy of the file.
    Backup,
    /// Made from the file on the disk (the folder as it is now).
    Disk,
    /// The icon for its type.
    Icon,
}

/// What is known about an entry, as far as its thumbnail goes.
#[derive(Debug, Clone, Copy)]
pub struct Facts<'a> {
    pub name: &'a str,
    /// A file, rather than a folder, a link or anything else.
    pub file: bool,
    pub size: i64,
    /// Modification time, in microseconds, as the catalogue has it.
    pub mtime: i64,
    /// In the window in front, where a thumbnail may be fetched or made.
    pub front: bool,
    /// The folder as it is now, read from the disk rather than a backup.
    pub now: bool,
}

/// A thumbnail in the desktop's cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cached {
    pub path: PathBuf,
    /// The modification time of the file it was made from, in seconds.
    pub mtime: i64,
}

/// The largest image a thumbnail is made from: a bigger one costs more to
/// fetch and decode than its thumbnail is worth.
pub const LARGEST: i64 = 40 * 1024 * 1024;

/// The cache's sizes, the best for an icon of 64 pixels at a scale of 2
/// first.
const SIZES: [&str; 4] = ["large", "normal", "x-large", "xx-large"];

/// Image types a thumbnail is made of, by their names' endings: the ones
/// GTK's loaders read on every desktop Backtrack runs on.
const IMAGES: [&str; 11] = [
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "avif", "heic", "jxl",
];

/// Whether `name` is of an image a thumbnail can be made of.
pub fn is_image(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, ending)| {
        !stem.is_empty() && IMAGES.contains(&ending.to_ascii_lowercase().as_str())
    })
}

/// Where an entry's picture comes from, `cached` being what the desktop's
/// cache holds for its path, if anything.
pub fn source(facts: Facts, cached: Option<&Cached>) -> Source {
    if !facts.file {
        return Source::Icon;
    }
    if let Some(cached) = cached {
        if cached.mtime == facts.mtime.div_euclid(1_000_000) {
            return Source::Cache(cached.path.clone());
        }
    }
    if !facts.front || !is_image(facts.name) || facts.size > LARGEST {
        return Source::Icon;
    }
    if facts.now {
        Source::Disk
    } else {
        Source::Backup
    }
}

/// The thumbnail the desktop's cache, at `root`, holds for the file at `uri`,
/// at the best size it has one.
pub fn cached(root: &Path, uri: &str) -> Option<Cached> {
    let name = format!(
        "{}.png",
        glib::compute_checksum_for_string(glib::ChecksumType::Md5, uri)?
    );
    SIZES.iter().find_map(|size| {
        let path = root.join(size).join(&name);
        let png = std::fs::read(&path).ok()?;
        // A thumbnail of another file whose URI has the same MD5 is not this
        // one's.
        if text(&png, "Thumb::URI").is_some_and(|recorded| recorded != uri) {
            return None;
        }
        let mtime = text(&png, "Thumb::MTime")?.trim().parse().ok()?;
        Some(Cached { path, mtime })
    })
}

/// The value of a PNG's `tEXt` chunk named `key`.
pub fn text(png: &[u8], key: &str) -> Option<String> {
    let mut rest = png.strip_prefix(b"\x89PNG\r\n\x1a\n")?;
    while rest.len() >= 12 {
        let length = u32::from_be_bytes(rest[..4].try_into().ok()?) as usize;
        let kind = &rest[4..8];
        let data = rest.get(8..8 + length)?;
        if kind == b"tEXt" {
            if let Some(at) = data.iter().position(|b| *b == 0) {
                if &data[..at] == key.as_bytes() {
                    // Latin-1, which is Unicode's first 256 code points.
                    return Some(data[at + 1..].iter().map(|b| char::from(*b)).collect());
                }
            }
        }
        if kind == b"IEND" {
            break;
        }
        rest = rest.get(12 + length..)?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(name: &str) -> Facts<'_> {
        Facts {
            name,
            file: true,
            size: 2_000_000,
            mtime: 1_700_000_000_250_000,
            front: true,
            now: false,
        }
    }

    fn cached_at(mtime: i64) -> Cached {
        Cached {
            path: PathBuf::from("/c/large/x.png"),
            mtime,
        }
    }

    #[test]
    fn the_desktops_thumbnail_of_this_version_is_shown_front_or_behind() {
        let this = cached_at(1_700_000_000);
        assert_eq!(
            source(facts("report.pdf"), Some(&this)),
            Source::Cache(this.path.clone()),
            "any type the desktop has a thumbnail of"
        );
        let behind = Facts {
            front: false,
            ..facts("site-visit.jpg")
        };
        assert_eq!(
            source(behind, Some(&this)),
            Source::Cache(this.path.clone())
        );
    }

    #[test]
    fn another_versions_thumbnail_is_not() {
        let newer = cached_at(1_700_000_001);
        assert_eq!(
            source(facts("site-visit.jpg"), Some(&newer)),
            Source::Backup,
            "an image: made from the backup's copy"
        );
        assert_eq!(source(facts("report.pdf"), Some(&newer)), Source::Icon);
    }

    #[test]
    fn only_the_window_in_front_fetches() {
        let behind = Facts {
            front: false,
            ..facts("site-visit.jpg")
        };
        assert_eq!(source(behind, None), Source::Icon);
        assert_eq!(source(facts("site-visit.jpg"), None), Source::Backup);
        let now = Facts {
            now: true,
            ..facts("site-visit.jpg")
        };
        assert_eq!(source(now, None), Source::Disk, "now: from the disk");
    }

    #[test]
    fn folders_large_images_and_other_types_show_their_icons() {
        let folder = Facts {
            file: false,
            ..facts("Photos.png")
        };
        assert_eq!(
            source(folder, Some(&cached_at(1_700_000_000))),
            Source::Icon
        );
        let large = Facts {
            size: LARGEST + 1,
            ..facts("panorama.tiff")
        };
        assert_eq!(source(large, None), Source::Icon);
        assert_eq!(source(facts("notes.txt"), None), Source::Icon);
        assert!(is_image("IMG_0001.JPG"));
        assert!(!is_image(".png"), "a hidden file named only by its ending");
        assert!(!is_image("png"));
    }

    /// A PNG of chunks only, as far as `text` reads: the signature, then each
    /// chunk's length, type, data and a CRC it does not check.
    fn png(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        for (kind, data) in chunks {
            out.extend((data.len() as u32).to_be_bytes());
            out.extend(*kind);
            out.extend(*data);
            out.extend([0; 4]);
        }
        out
    }

    #[test]
    fn a_pngs_text_is_read_by_its_key() {
        let file = png(&[
            (b"IHDR", &[0; 13]),
            (b"tEXt", b"Thumb::URI\0file:///home/k/caf\xe9.jpg"),
            (b"tEXt", b"Thumb::MTime\x001700000000"),
            (b"IDAT", &[1, 2, 3]),
            (b"IEND", b""),
        ]);
        assert_eq!(text(&file, "Thumb::MTime").as_deref(), Some("1700000000"));
        assert_eq!(
            text(&file, "Thumb::URI").as_deref(),
            Some("file:///home/k/café.jpg"),
            "Latin-1"
        );
        assert_eq!(text(&file, "Thumb::Size"), None);
        assert_eq!(text(b"GIF89a", "Thumb::MTime"), None);
        assert_eq!(
            text(&file[..40], "Thumb::MTime"),
            None,
            "cut short, nothing past the cut"
        );
    }

    #[test]
    fn the_cache_is_found_by_the_md5_of_the_uri() {
        let root = tempfile::tempdir().unwrap();
        let uri = "file:///home/k/Pictures/site-visit.jpg";
        let md5 = glib::compute_checksum_for_string(glib::ChecksumType::Md5, uri).unwrap();
        std::fs::create_dir_all(root.path().join("normal")).unwrap();
        let path = root.path().join("normal").join(format!("{md5}.png"));
        std::fs::write(
            &path,
            png(&[
                (b"tEXt", format!("Thumb::URI\0{uri}").as_bytes()),
                (b"tEXt", b"Thumb::MTime\x001700000000"),
                (b"IEND", b""),
            ]),
        )
        .unwrap();
        assert_eq!(
            cached(root.path(), uri),
            Some(Cached {
                path: path.clone(),
                mtime: 1_700_000_000
            })
        );
        assert_eq!(cached(root.path(), "file:///home/k/other.jpg"), None);

        // Under this URI's name, but recorded as another file's.
        std::fs::write(
            &path,
            png(&[
                (b"tEXt", b"Thumb::URI\0file:///home/k/elsewhere.jpg"),
                (b"tEXt", b"Thumb::MTime\x001700000000"),
                (b"IEND", b""),
            ]),
        )
        .unwrap();
        assert_eq!(cached(root.path(), uri), None);
    }
}
