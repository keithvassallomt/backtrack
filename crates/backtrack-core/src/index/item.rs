// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>

//! The unit of ingest: a single entry from `borg list --json-lines`, parsed
//! into the shape the index stores.
//!
//! Borg emits one JSON object per line, e.g.
//! ```text
//! {"type": "-", "mode": "-rw-r--r--", "path": "home/user/report.odt",
//!  "size": 12345, "mtime": "2026-05-01T12:00:00.489198", ...}
//! ```
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
}

/// Parse Borg's naive-UTC timestamp (`YYYY-MM-DDTHH:MM:SS[.ffffff]`) to epoch
/// microseconds. Dependency-free and deterministic; the fractional part is
/// optional and padded/truncated to microseconds.
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
    fn rejects_malformed_json() {
        assert!(matches!(
            BorgItem::from_json_line("not json"),
            Err(ItemParseError::Json(_))
        ));
    }
}
