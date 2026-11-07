// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Reading a GVDB file, the format dconf keeps its databases in.
//!
//! Inside the Flatpak there is no dconf to ask (see `super::dconf`), so the
//! desktop's settings are read from dconf's own files. GLib reads the format
//! itself but does not offer it, and it is small: a header pointing at a hash
//! table, whose items each name a key (as a suffix of a parent item's) and
//! point at a serialized GVariant or at another table. Lookups here walk every
//! item rather than hashing, which for a database of a few hundred keys is
//! nothing, and leaves no hash function to get wrong.
//!
//! The layout is GLib's `gvdb/gvdb-format.h`. Only little-endian files are
//! read, which is every file dconf writes on the machines Backtrack runs on.

use std::collections::HashMap;

use gtk4::glib;

const SIGNATURE: &[u8; 8] = b"GVariant";
const HEADER: usize = 24;
const ITEM: usize = 24;
const NO_PARENT: u32 = u32::MAX;

/// A table of a GVDB file: its keys, each with a value or a further table.
#[derive(Debug, Clone, Default)]
pub struct Table {
    items: HashMap<String, Item>,
}

#[derive(Debug, Clone)]
enum Item {
    Value(glib::Variant),
    Table(Table),
    /// A list of a folder's children, which dconf keeps for listing and
    /// nothing here needs.
    Other,
}

impl Table {
    /// The value stored at `key`, unwrapped from the variant dconf stores it
    /// in.
    pub fn value(&self, key: &str) -> Option<glib::Variant> {
        match self.items.get(key)? {
            Item::Value(value) => Some(value.clone()),
            _ => None,
        }
    }

    /// The table stored at `key`, as dconf keeps a system database's locks
    /// at `.locks`.
    pub fn table(&self, key: &str) -> Option<&Table> {
        match self.items.get(key)? {
            Item::Table(table) => Some(table),
            _ => None,
        }
    }

    /// Whether `key` is in this table at all, whatever it holds.
    pub fn contains(&self, key: &str) -> bool {
        self.items.contains_key(key)
    }
}

/// Why a file could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreadable(pub &'static str);

/// Read the root table of a GVDB file's bytes.
pub fn parse(bytes: &[u8]) -> Result<Table, Unreadable> {
    if bytes.len() < HEADER || &bytes[..8] != SIGNATURE {
        return Err(Unreadable("not a GVDB file, or not a little-endian one"));
    }
    if u32_at(bytes, 8)? != 0 {
        return Err(Unreadable("a GVDB version this does not know"));
    }
    let (start, end) = pointer_at(bytes, 16)?;
    table(bytes, start, end, 0)
}

/// Tables nest only one deep in dconf (the locks); this keeps a file that
/// points a table at itself from recursing for ever.
const DEEPEST: u32 = 4;

fn table(bytes: &[u8], start: usize, end: usize, depth: u32) -> Result<Table, Unreadable> {
    if depth > DEEPEST {
        return Err(Unreadable("tables nested too deep"));
    }
    let region = bytes
        .get(start..end)
        .ok_or(Unreadable("a table outside the file"))?;
    let bloom_words = (u32_at(region, 0)? & ((1 << 27) - 1)) as usize;
    let buckets = u32_at(region, 4)? as usize;
    let items_at = 8usize
        .checked_add(4 * bloom_words)
        .and_then(|n| n.checked_add(4 * buckets))
        .filter(|n| *n <= region.len())
        .ok_or(Unreadable("a table's header overruns it"))?;
    let count = (region.len() - items_at) / ITEM;

    struct Raw<'a> {
        parent: u32,
        key: &'a str,
        kind: u8,
        value: (usize, usize),
    }
    let mut raw = Vec::with_capacity(count);
    for i in 0..count {
        let at = items_at + i * ITEM;
        let parent = u32_at(region, at + 4)?;
        let key_start = u32_at(region, at + 8)? as usize;
        let key_size = u16::from_le_bytes([region[at + 12], region[at + 13]]) as usize;
        let key = bytes
            .get(key_start..key_start + key_size)
            .and_then(|k| std::str::from_utf8(k).ok())
            .ok_or(Unreadable("a key outside the file, or not UTF-8"))?;
        raw.push(Raw {
            parent,
            key,
            kind: region[at + 14],
            value: pointer_at(region, at + 16)?,
        });
    }

    // A key is its parent's key followed by its own part.
    let full = |mut i: usize| -> Result<String, Unreadable> {
        let mut parts = Vec::new();
        for _ in 0..=count {
            parts.push(raw[i].key);
            match raw[i].parent {
                NO_PARENT => {
                    parts.reverse();
                    return Ok(parts.concat());
                }
                parent if (parent as usize) < count => i = parent as usize,
                _ => return Err(Unreadable("a key's parent is not in its table")),
            }
        }
        Err(Unreadable("a key is its own ancestor"))
    };

    let mut items = HashMap::with_capacity(count);
    for (i, item) in raw.iter().enumerate() {
        let (value_start, value_end) = item.value;
        let held = match item.kind {
            b'v' => {
                let data = bytes
                    .get(value_start..value_end)
                    .ok_or(Unreadable("a value outside the file"))?;
                let boxed = glib::Variant::from_bytes_with_type(
                    &glib::Bytes::from(data),
                    glib::VariantTy::VARIANT,
                );
                match boxed.as_variant() {
                    Some(value) => Item::Value(value),
                    None => continue,
                }
            }
            b'H' => Item::Table(table(bytes, value_start, value_end, depth + 1)?),
            _ => Item::Other,
        };
        items.insert(full(i)?, held);
    }
    Ok(Table { items })
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, Unreadable> {
    let word = bytes
        .get(at..at + 4)
        .ok_or(Unreadable("the file ends too soon"))?;
    Ok(u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
}

fn pointer_at(bytes: &[u8], at: usize) -> Result<(usize, usize), Unreadable> {
    let start = u32_at(bytes, at)? as usize;
    let end = u32_at(bytes, at + 4)? as usize;
    if start > end {
        return Err(Unreadable("a pointer that ends before it starts"));
    }
    Ok((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Table {
        let path = format!(
            "{}/src/model/wallpaper/testdata/dconf/{name}.gvdb",
            env!("CARGO_MANIFEST_DIR")
        );
        parse(&std::fs::read(path).unwrap()).unwrap()
    }

    fn string(table: &Table, key: &str) -> Option<String> {
        table.value(key).and_then(|v| v.str().map(str::to_owned))
    }

    #[test]
    fn a_user_database_gives_back_what_dconf_was_given() {
        let user = fixture("user");
        assert_eq!(
            string(&user, "/org/gnome/desktop/background/picture-uri").as_deref(),
            Some("file:///home/keith/Pictures/lake.jpg")
        );
        assert_eq!(
            string(&user, "/org/gnome/desktop/background/picture-options").as_deref(),
            Some("scaled")
        );
        assert_eq!(
            string(&user, "/org/gnome/desktop/interface/color-scheme").as_deref(),
            Some("prefer-dark")
        );
        // A folder is in the table, as a list, but has no value.
        assert!(user.contains("/org/gnome/desktop/background/"));
        assert_eq!(user.value("/org/gnome/desktop/background/"), None);
    }

    #[test]
    fn a_key_nobody_set_is_not_there() {
        let user = fixture("user-elsewhere");
        assert_eq!(
            user.value("/org/gnome/desktop/background/picture-uri"),
            None
        );
        assert!(user.contains("/org/gnome/desktop/interface/color-scheme"));
    }

    #[test]
    fn a_system_database_carries_its_locks_in_a_table_of_their_own() {
        let site = fixture("site");
        let locks = site.table(".locks").unwrap();
        assert!(locks.contains("/org/gnome/desktop/background/picture-uri"));
        assert!(!locks.contains("/org/gnome/desktop/background/picture-options"));
        assert!(fixture("local").table(".locks").is_none());
    }

    #[test]
    fn what_is_not_a_gvdb_file_is_refused_rather_than_misread() {
        assert!(parse(b"").is_err());
        assert!(parse(b"GVariant").is_err());
        assert!(parse(&[0u8; 64]).is_err());
        // A real file cut short, so that its table runs past the end.
        let path = format!(
            "{}/src/model/wallpaper/testdata/dconf/user.gvdb",
            env!("CARGO_MANIFEST_DIR")
        );
        let whole = std::fs::read(path).unwrap();
        assert!(parse(&whole[..40]).is_err());
    }
}
