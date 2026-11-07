// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! A desktop setting read the way dconf reads it, from dconf's own files.
//!
//! Natively Backtrack asks GSettings. Inside the Flatpak, GSettings reads the
//! sandbox's own settings, the GNOME runtime has no dconf module to reach the
//! host's, and the Settings portal does not offer the wallpaper (S18-T2, found
//! on the GNOME VM, 2026-10-20). What `--filesystem=host` does reach is dconf's
//! files, so they are read in dconf's order: the profile names the user's
//! database and the system ones after it; the first that has the key gives its
//! value, except that a key a system database locks is never read from the
//! user's. A key in none of them falls to its schema's default, which the
//! caller reads from the compiled schemas.

use gtk4::glib;

use super::gvdb::Table;

/// A line of a dconf profile, naming a database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `user-db:NAME`, the user's own: `NAME` in `~/.config/dconf/`.
    User(String),
    /// `system-db:NAME`, in `/etc/dconf/db/`.
    System(String),
    /// `file-db:PATH`, a database at a path of its own.
    File(String),
}

/// The databases a dconf profile names, in its order. A missing profile is
/// dconf's default, the user's database alone; lines this does not read
/// (`service-db:`, comments) are passed over.
pub fn profile(text: Option<&str>) -> Vec<Source> {
    let Some(text) = text else {
        return vec![Source::User("user".to_owned())];
    };
    text.lines()
        .map(str::trim)
        .filter_map(|line| {
            let (kind, name) = line.split_once(':')?;
            let name = name.trim().to_owned();
            match kind.trim() {
                "user-db" => Some(Source::User(name)),
                "system-db" => Some(Source::System(name)),
                "file-db" => Some(Source::File(name)),
                _ => None,
            }
        })
        .collect()
}

/// The databases a profile names, read, in its order. One that could not be
/// read is left out, as dconf leaves it out.
#[derive(Debug, Clone, Default)]
pub struct Layers {
    pub databases: Vec<(Source, Table)>,
}

impl Layers {
    /// The value of `key` (a dconf path, as `/org/gnome/desktop/background/
    /// picture-uri`) as the desktop sees it, or None for its schema's default.
    /// Also the database it came from.
    pub fn read_from(&self, key: &str) -> Option<(&Source, glib::Variant)> {
        let locked = self.databases.iter().any(|(source, table)| {
            !matches!(source, Source::User(_))
                && table
                    .table(".locks")
                    .is_some_and(|locks| locks.contains(key))
        });
        self.databases
            .iter()
            .filter(|(source, _)| !(locked && matches!(source, Source::User(_))))
            .find_map(|(source, table)| Some((source, table.value(key)?)))
    }
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::User(name) => write!(f, "the user's database ({name})"),
            Source::System(name) => write!(f, "the system database {name}"),
            Source::File(path) => write!(f, "the database {path}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::gvdb;
    use super::*;

    fn fixture(name: &str) -> Table {
        let path = format!(
            "{}/src/model/wallpaper/testdata/dconf/{name}.gvdb",
            env!("CARGO_MANIFEST_DIR")
        );
        gvdb::parse(&std::fs::read(path).unwrap()).unwrap()
    }

    fn layers(names: &[(&str, bool)]) -> Layers {
        Layers {
            databases: names
                .iter()
                .map(|(name, user)| {
                    let source = if *user {
                        Source::User("user".to_owned())
                    } else {
                        Source::System((*name).to_owned())
                    };
                    (source, fixture(name))
                })
                .collect(),
        }
    }

    fn read(layers: &Layers, key: &str) -> Option<String> {
        layers
            .read_from(&format!("/org/gnome/desktop/background/{key}"))
            .and_then(|(_, v)| v.str().map(str::to_owned))
    }

    fn source(layers: &Layers, key: &str) -> Option<String> {
        layers
            .read_from(&format!("/org/gnome/desktop/background/{key}"))
            .map(|(source, _)| source.to_string())
    }

    #[test]
    fn fedoras_profile_is_read_in_its_order() {
        // /etc/dconf/profile/user on the Fedora 45 GNOME VM, 2026-10-27.
        let text = "user-db:user\nsystem-db:local\nsystem-db:site\nsystem-db:distro\n";
        assert_eq!(
            profile(Some(text)),
            [
                Source::User("user".into()),
                Source::System("local".into()),
                Source::System("site".into()),
                Source::System("distro".into()),
            ]
        );
        assert_eq!(profile(None), [Source::User("user".into())]);
        assert_eq!(
            profile(Some(
                "# a comment\nservice-db:keyfile/user\nfile-db:/srv/db\n"
            )),
            [Source::File("/srv/db".into())]
        );
    }

    #[test]
    fn the_users_own_setting_comes_first() {
        let all = layers(&[("user", true), ("local", false)]);
        assert_eq!(
            read(&all, "picture-uri-dark").as_deref(),
            Some("file:///home/keith/Pictures/lake%20at%20night.jpg")
        );
        assert_eq!(
            source(&all, "picture-uri-dark").as_deref(),
            Some("the user's database (user)")
        );
    }

    #[test]
    fn a_key_the_user_never_set_comes_from_a_system_database() {
        let all = layers(&[("user-elsewhere", true), ("local", false), ("site", false)]);
        assert_eq!(
            read(&all, "picture-uri-dark").as_deref(),
            Some("file:///usr/share/backgrounds/local-night.png")
        );
        assert_eq!(read(&all, "picture-options").as_deref(), Some("stretched"));
        // In none of them: the schema's default, which is the caller's to read.
        assert_eq!(read(&all, "primary-color"), None);
    }

    #[test]
    fn a_key_a_system_database_locks_is_never_read_from_the_users() {
        let all = layers(&[("user", true), ("site", false)]);
        assert_eq!(
            read(&all, "picture-uri").as_deref(),
            Some("file:///usr/share/backgrounds/company/office.png")
        );
        assert_eq!(
            source(&all, "picture-uri").as_deref(),
            Some("the system database site")
        );
        // The lock is on that key alone.
        assert_eq!(read(&all, "picture-options").as_deref(), Some("scaled"));
    }
}
