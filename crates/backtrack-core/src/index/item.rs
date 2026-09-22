// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The unit of ingest: a single entry from a Borg archive listing, parsed into
//! the shape the index stores.
//!
//! Listings are read with `borg list --format` ([`ITEM_FORMAT`]) rather than
//! `--json-lines`, and the reason is the timestamp. Borg's JSON renders `mtime`
//! as a naive local-time string with no offset recorded, so a catalogue built
//! from it holds every file's modification time shifted by whatever the
//! machine's UTC offset happened to be. `{mtime:%s.%f}` asks Borg's own Python
//! for the epoch it already holds. [`BorgItem::from_json_line`] is kept for
//! reading captured listings.
//!
//! Borg's `list` output carries no per-file content hash, so [`BorgItem::chunk_hash`]
//! is populated only when a future source provides one; change detection at
//! ingest falls back to size + mtime, exactly as the architecture intends.

use serde::Deserialize;

/// What kind of filesystem object a version is. Mirrors Borg's single-character
/// `type` field, collapsing the device/fifo/socket zoo into [`Kind::Other`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Other,
}

impl Kind {
    /// Map Borg's `type` character (`"-"`, `"d"`, `"l"`, …) to a [`Kind`].
    pub fn from_borg(type_field: &str) -> Kind {
        match type_field {
            "-" => Kind::File,
            "d" => Kind::Dir,
            "l" => Kind::Symlink,
            _ => Kind::Other,
        }
    }

    /// The token stored in `versions.kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::File => "file",
            Kind::Dir => "dir",
            Kind::Symlink => "symlink",
            Kind::Other => "other",
        }
    }

    /// Inverse of [`Kind::as_str`]: reconstruct a [`Kind`] from a stored token.
    /// Unknown tokens (only possible from a corrupt row) map to [`Kind::Other`].
    pub fn from_token(token: &str) -> Kind {
        match token {
            "file" => Kind::File,
            "dir" => Kind::Dir,
            "symlink" => Kind::Symlink,
            _ => Kind::Other,
        }
    }
}

/// Which repository an archive belongs to. Stored in `archives.repo`; drives the
/// "on this computer" badge and unified restores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repo {
    Primary,
    Spool,
    FsSnapshot,
}

impl Repo {
    /// The token stored in `archives.repo` (matches the schema CHECK constraint).
    pub fn as_str(self) -> &'static str {
        match self {
            Repo::Primary => "primary",
            Repo::Spool => "spool",
            Repo::FsSnapshot => "fs-snapshot",
        }
    }
}

/// Metadata for the archive being ingested. `seq` is assigned by the writer;
/// this is the caller-supplied identity and timestamp.
#[derive(Debug, Clone)]
pub struct ArchiveMeta {
    /// Borg's archive id (hex), or `None` for synthetic/test archives.
    pub borg_id: Option<String>,
    /// Human-facing archive name (e.g. `home-2026-05-01`).
    pub name: String,
    /// Archive creation time, epoch seconds.
    pub ts: i64,
}

/// One parsed entry from a Borg archive listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BorgItem {
    /// Archive-relative path, `/`-separated, exactly as Borg emits it.
    pub path: String,
    pub kind: Kind,
    pub size: i64,
    /// Modification time, epoch **microseconds** (Borg's resolution).
    pub mtime: i64,
    /// Unix permission + special bits parsed from Borg's mode string.
    pub mode: i64,
    /// Content hash when a source provides one; `None` from `borg list`.
    pub chunk_hash: Option<String>,
}

/// Errors from parsing a Borg listing line.
#[derive(Debug, thiserror::Error)]
pub enum ItemParseError {
    #[error("malformed borg list JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unparseable mtime {0:?}")]
    Mtime(String),
    #[error("unparseable mode {0:?}")]
    Mode(String),
    #[error("malformed borg list line {0:?}")]
    Format(String),
}

/// The raw JSON shape Borg emits, before domain conversion. Fields we do not use
/// are ignored by serde.
#[derive(Deserialize)]
struct RawBorgItem {
    #[serde(rename = "type")]
    kind: String,
    mode: String,
    path: String,
    #[serde(default)]
    size: i64,
    mtime: String,
}

impl BorgItem {
    /// Parse one `borg list --json-lines` line.
    ///
    /// Retained for reading captured listings, but **not** what the engine uses:
    /// Borg's JSON renders `mtime` as a naive local-time string with no offset
    /// recorded, so reading it back yields a timestamp shifted by whatever the
    /// machine's UTC offset happened to be. See [`BorgItem::from_format_line`].
    pub fn from_json_line(line: &str) -> Result<BorgItem, ItemParseError> {
        let raw: RawBorgItem = serde_json::from_str(line)?;
        Ok(BorgItem {
            kind: Kind::from_borg(&raw.kind),
            mode: parse_mode_string(&raw.mode)?,
            mtime: parse_borg_mtime(&raw.mtime)?,
            size: raw.size,
            path: raw.path,
            chunk_hash: None,
        })
    }

    /// Parse one line of [`ITEM_FORMAT`] output:
    /// `mode \t size \t epoch.micros \t isomtime \t path`.
    ///
    /// This is how listings are actually read, for the same reason archive
    /// timestamps are read with `{time:%s}`: Borg's JSON carries local time
    /// with no offset, so a catalogue built from it holds every file's
    /// modification time shifted by the machine's UTC offset — an hour out for
    /// half the year in most of Europe, and shifted by a different amount again
    /// if the repository is ever read somewhere else. Asking Borg's own Python
    /// for the epoch it already holds is exact and costs nothing.
    ///
    /// It also happens to be the difference between the offline spool working
    /// and not working at all: change detection compares an indexed timestamp
    /// against one read from the live filesystem, and those two can only ever
    /// agree if the indexed one is a real epoch.
    ///
    /// The path is last because an imported repository's member paths are
    /// whatever was on somebody's disk, including tabs.
    pub fn from_format_line(line: &str) -> Result<BorgItem, ItemParseError> {
        let err = || ItemParseError::Format(line.to_string());
        let mut fields = line.splitn(5, '\t');
        let mode = fields.next().ok_or_else(err)?;
        let size = fields.next().ok_or_else(err)?.trim();
        let epoch = fields.next().ok_or_else(err)?.trim();
        let iso = fields.next().ok_or_else(err)?.trim();
        let path = fields
            .next()
            .ok_or_else(err)?
            .trim_end_matches(['\r', '\n']);
        if path.is_empty() {
            return Err(err());
        }
        Ok(BorgItem {
            // The mode string's leading character carries the object type, so
            // no separate field is needed for it.
            kind: Kind::from_borg(&mode[..1.min(mode.len())]),
            mode: parse_mode_string(mode)?,
            mtime: parse_epoch_micros(epoch)
                // `%s` is a glibc extension rather than C89. A platform that
                // does not honour it leaves the field unparseable, and falling
                // back to the ISO string keeps the file catalogued — with the
                // timezone caveat above, which is a far smaller problem than
                // losing the entry.
                .or_else(|| parse_borg_mtime(iso).ok())
                .ok_or_else(err)?,
            size: size.parse().map_err(|_| err())?,
            path: path.to_string(),
            chunk_hash: None,
        })
    }
}

/// The `--format` string [`BorgItem::from_format_line`] reads.
pub const ITEM_FORMAT: &str = "{mode}\t{size}\t{mtime:%s.%f}\t{isomtime}\t{path}{NL}";

/// Parse `seconds.microseconds` into epoch microseconds.
fn parse_epoch_micros(text: &str) -> Option<i64> {
    let (secs, micros) = match text.split_once('.') {
        Some((s, m)) => (s, m),
        None => (text, "0"),
    };
    let secs: i64 = secs.parse().ok()?;
    let digits: String = micros.chars().take_while(|c| c.is_ascii_digit()).collect();
    let padded = format!("{digits:0<6}");
    let micros: i64 = padded.get(..6)?.parse().ok()?;
    secs.checked_mul(1_000_000)?.checked_add(micros)
}

/// Parse Borg's naive timestamp (`YYYY-MM-DDTHH:MM:SS[.ffffff]`) to epoch
/// microseconds. Dependency-free and deterministic; the fractional part is
/// optional and padded/truncated to microseconds.
///
/// Reads the string as UTC, which Borg's own timestamps are **not**: they are
/// rendered in the machine's zone with no offset written down. This is for
/// reading a timestamp already known to be UTC, and for the fallback in
/// `parse_archive_line` when Borg declines to give an epoch at all. Anything
/// ingesting a real listing wants [`ITEM_FORMAT`] and
/// [`BorgItem::from_format_line`] instead.
pub fn parse_borg_mtime(s: &str) -> Result<i64, ItemParseError> {
    let err = || ItemParseError::Mtime(s.to_string());
    let (date, rest) = s.split_once('T').ok_or_else(err)?;
    let (time, frac) = match rest.split_once('.') {
        Some((t, f)) => (t, Some(f)),
        None => (rest, None),
    };

    let mut d = date.splitn(3, '-');
    let year: i64 = d.next().ok_or_else(err)?.parse().map_err(|_| err())?;
    let month: i64 = d.next().ok_or_else(err)?.parse().map_err(|_| err())?;
    let day: i64 = d.next().ok_or_else(err)?.parse().map_err(|_| err())?;

    let mut t = time.splitn(3, ':');
    let hour: i64 = t.next().ok_or_else(err)?.parse().map_err(|_| err())?;
    let min: i64 = t.next().ok_or_else(err)?.parse().map_err(|_| err())?;
    let sec: i64 = t.next().ok_or_else(err)?.parse().map_err(|_| err())?;

    let micros = match frac {
        None => 0,
        Some(f) => {
            let digits: String = f.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                return Err(err());
            }
            let padded = format!("{digits:0<6}");
            padded[..6].parse::<i64>().map_err(|_| err())?
        }
    };

    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3_600 + min * 60 + sec;
    Ok(secs * 1_000_000 + micros)
}

/// Days between 1970-01-01 and the given proleptic-Gregorian date. Howard
/// Hinnant's `days_from_civil`.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parse Borg's symbolic mode string (`-rw-r--r--`, `drwxr-xr-x`, …) into Unix
/// permission and special bits. The leading type character is ignored — [`Kind`]
/// already carries the object type.
pub fn parse_mode_string(s: &str) -> Result<i64, ItemParseError> {
    let b = s.as_bytes();
    if b.len() < 10 {
        return Err(ItemParseError::Mode(s.to_string()));
    }
    let mut mode: i64 = 0;
    let rw = |c: u8, bit: i64| if c != b'-' { bit } else { 0 };
    mode |= rw(b[1], 0o400) | rw(b[2], 0o200);
    mode |= match b[3] {
        b'x' => 0o100,
        b's' => 0o100 | 0o4000,
        b'S' => 0o4000,
        _ => 0,
    };
    mode |= rw(b[4], 0o040) | rw(b[5], 0o020);
    mode |= match b[6] {
        b'x' => 0o010,
        b's' => 0o010 | 0o2000,
        b'S' => 0o2000,
        _ => 0,
    };
    mode |= rw(b[7], 0o004) | rw(b[8], 0o002);
    mode |= match b[9] {
        b'x' => 0o001,
        b't' => 0o001 | 0o1000,
        b'T' => 0o1000,
        _ => 0,
    };
    Ok(mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_real_borg_file_line() {
        let line = r#"{"type": "-", "mode": "-rw-r--r--", "user": "keith", "group": "keith", "uid": 1000, "gid": 1000, "path": "home/user/report.txt", "healthy": true, "source": "", "linktarget": "", "flags": 0, "size": 5, "mtime": "2026-07-07T11:01:17.489198"}"#;
        let item = BorgItem::from_json_line(line).unwrap();
        assert_eq!(item.path, "home/user/report.txt");
        assert_eq!(item.kind, Kind::File);
        assert_eq!(item.size, 5);
        assert_eq!(item.mode, 0o644);
        assert_eq!(item.chunk_hash, None);
    }

    #[test]
    fn maps_dir_and_symlink_kinds() {
        let dir = r#"{"type":"d","mode":"drwxr-xr-x","path":"home","size":0,"mtime":"2026-01-01T00:00:00.000000"}"#;
        let link = r#"{"type":"l","mode":"lrwxrwxrwx","path":"home/link","size":10,"mtime":"2026-01-01T00:00:00.000000"}"#;
        assert_eq!(BorgItem::from_json_line(dir).unwrap().kind, Kind::Dir);
        assert_eq!(BorgItem::from_json_line(dir).unwrap().mode, 0o755);
        assert_eq!(BorgItem::from_json_line(link).unwrap().kind, Kind::Symlink);
    }

    #[test]
    fn mtime_epoch_micros_matches_known_instant() {
        // 1970-01-01T00:00:00 is epoch zero.
        assert_eq!(parse_borg_mtime("1970-01-01T00:00:00.000000").unwrap(), 0);
        // 1970-01-01T00:00:01.5 → 1.5s = 1_500_000 µs (fraction padded).
        assert_eq!(
            parse_borg_mtime("1970-01-01T00:00:01.5").unwrap(),
            1_500_000
        );
        // Fractionless is allowed.
        assert_eq!(
            parse_borg_mtime("1970-01-02T00:00:00").unwrap(),
            86_400_000_000
        );
    }

    #[test]
    fn mode_string_special_bits() {
        assert_eq!(parse_mode_string("-rwsr-xr-x").unwrap(), 0o4755);
        assert_eq!(parse_mode_string("drwxrwxrwt").unwrap(), 0o1777);
        assert_eq!(parse_mode_string("----------").unwrap(), 0);
    }

    #[test]
    fn parses_a_real_format_line() {
        // A genuine capture from borg 1.4.5 with ITEM_FORMAT.
        let line = "-rw-r--r--\t5\t1786097862.839217\t2026-08-07T11:17:42.839217\tsrc/f1.txt";
        let item = BorgItem::from_format_line(line).unwrap();
        assert_eq!(item.path, "src/f1.txt");
        assert_eq!(item.kind, Kind::File);
        assert_eq!(item.size, 5);
        assert_eq!(item.mode, 0o644);
        assert_eq!(
            item.mtime, 1_786_097_862_839_217,
            "the epoch comes from borg, not from re-reading its local-time string"
        );
    }

    #[test]
    fn the_epoch_is_preferred_over_the_local_time_string() {
        // The two fields disagree by an hour, which is exactly what a machine on
        // summer time produces. Reading the ISO one would shift every file's
        // recorded time — and would make every file compare as modified against
        // a live filesystem, which is what the offline spool does.
        let line = "-rw-r--r--\t5\t1786097862.839217\t2026-08-07T12:17:42.839217\tf";
        assert_eq!(
            BorgItem::from_format_line(line).unwrap().mtime,
            1_786_097_862_839_217
        );
    }

    #[test]
    fn an_unusable_epoch_falls_back_to_the_local_time_string() {
        // A platform whose strftime does not honour `%s` leaves the field as a
        // literal. Keeping the file catalogued with a shifted time beats losing
        // it from the timeline entirely.
        let line = "-rw-r--r--\t5\t%s.%f\t1970-01-02T00:00:00.000000\tf";
        assert_eq!(
            BorgItem::from_format_line(line).unwrap().mtime,
            86_400_000_000
        );
    }

    #[test]
    fn directories_and_symlinks_are_read_from_the_mode_string() {
        // `--format` has no separate type field; the mode's first character is
        // the same information the JSON `type` carries.
        let dir = "drwxr-xr-x\t0\t100.000000\tiso\thome";
        let link = "lrwxrwxrwx\t10\t100.000000\tiso\thome/link";
        assert_eq!(BorgItem::from_format_line(dir).unwrap().kind, Kind::Dir);
        assert_eq!(BorgItem::from_format_line(dir).unwrap().mode, 0o755);
        assert_eq!(
            BorgItem::from_format_line(link).unwrap().kind,
            Kind::Symlink
        );
    }

    #[test]
    fn a_member_path_containing_a_tab_survives() {
        // Only our own archives are named to a scheme; the files inside anyone's
        // repository are whatever was on their disk.
        let line = "-rw-r--r--\t1\t100.000000\tiso\todd\tname.txt";
        assert_eq!(
            BorgItem::from_format_line(line).unwrap().path,
            "odd\tname.txt"
        );
    }

    #[test]
    fn a_fractionless_epoch_is_accepted() {
        let line = "-rw-r--r--\t1\t1786097862\tiso\tf";
        assert_eq!(
            BorgItem::from_format_line(line).unwrap().mtime,
            1_786_097_862_000_000
        );
    }

    #[test]
    fn malformed_format_lines_are_rejected_rather_than_guessed_at() {
        for line in ["", "not a listing", "-rw-r--r--\t1\t100.0\tiso\t"] {
            assert!(
                BorgItem::from_format_line(line).is_err(),
                "{line:?} should not parse"
            );
        }
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(matches!(
            BorgItem::from_json_line("not json"),
            Err(ItemParseError::Json(_))
        ));
    }
}
