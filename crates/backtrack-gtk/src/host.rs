// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The desktop's own settings and files, as the host has them, whether
//! Backtrack runs natively or in the Flatpak.
//!
//! Natively GSettings answers. Inside the Flatpak GSettings sees only the
//! sandbox's settings, and the GNOME runtime has no dconf module to reach the
//! host's, so dconf's own files are read instead, in dconf's order, and the
//! host's compiled schemas for the defaults (S18-T2). The desktop's wallpaper
//! (S18-T2) and whether it shows hidden files (S18-T11) are read this way.

use std::path::{Path, PathBuf};

use gtk4::prelude::*;
use gtk4::{gio, glib};
use tracing::debug;

use crate::model::wallpaper::dconf::{self, Layers};
use crate::model::wallpaper::gvdb;

/// The host's files, seen from here: natively where they are; inside the
/// Flatpak, the host's `/usr` and `/etc` under `/run/host`, which
/// `--filesystem=host` reaches, and its home folder where it is.
#[derive(Debug, Clone)]
pub struct Host {
    pub flatpak: bool,
    /// The host's `~/.config` and `~/.local/share`. Inside the Flatpak,
    /// `XDG_CONFIG_HOME` and `XDG_DATA_HOME` are the sandbox's own.
    pub config: PathBuf,
    pub data: PathBuf,
}

impl Host {
    pub fn here() -> Host {
        let flatpak = Path::new("/.flatpak-info").exists();
        let home = glib::home_dir();
        let host_dir = |var: &str, default: &str| {
            std::env::var_os(var)
                .filter(|v| !v.is_empty())
                .map_or_else(|| home.join(default), PathBuf::from)
        };
        if flatpak {
            Host {
                flatpak,
                config: host_dir("HOST_XDG_CONFIG_HOME", ".config"),
                data: host_dir("HOST_XDG_DATA_HOME", ".local/share"),
            }
        } else {
            Host {
                flatpak,
                config: glib::user_config_dir(),
                data: glib::user_data_dir(),
            }
        }
    }

    /// Where a path the desktop names is found from here.
    pub fn path(&self, path: &Path) -> PathBuf {
        if self.flatpak && (path.starts_with("/usr") || path.starts_with("/etc")) {
            Path::new("/run/host").join(path.strip_prefix("/").unwrap_or(path))
        } else {
            path.to_path_buf()
        }
    }

    /// The system's data folders, most important first. Natively
    /// `XDG_DATA_DIRS`; inside the Flatpak that is the sandbox's, so the
    /// host's usual two.
    pub fn data_dirs(&self) -> Vec<PathBuf> {
        self.dirs("XDG_DATA_DIRS", &["/usr/local/share", "/usr/share"])
    }

    /// The system's configuration folders, most important first, as KConfig
    /// reads them. Fedora's Plasma session adds its own profile's, which the
    /// Flatpak cannot see in the environment and so names itself.
    pub fn config_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = self.dirs("XDG_CONFIG_DIRS", &["/etc/xdg"]);
        if self.flatpak {
            dirs.insert(0, self.config.join("kdedefaults"));
            dirs.push(self.path(Path::new("/usr/share/kde-settings/kde-profile/default/xdg")));
        }
        dirs
    }

    /// dconf's databases, as the host's profile names them, read from their
    /// files. One that cannot be read is left out, as dconf leaves it out.
    pub fn dconf_layers(&self) -> Layers {
        let profile = std::iter::once(self.path(Path::new("/etc/dconf/profile/user")))
            .chain(
                self.data_dirs()
                    .into_iter()
                    .map(|d| d.join("dconf/profile/user")),
            )
            .find_map(|path| std::fs::read_to_string(path).ok());
        let databases = dconf::profile(profile.as_deref())
            .into_iter()
            .filter_map(|source| {
                let path = match &source {
                    dconf::Source::User(name) => self.config.join("dconf").join(name),
                    dconf::Source::System(name) => {
                        self.path(&Path::new("/etc/dconf/db").join(name))
                    }
                    dconf::Source::File(path) => self.path(Path::new(path)),
                };
                let table = gvdb::parse(&std::fs::read(&path).ok()?)
                    .inspect_err(|why| debug!(path = %path.display(), "a dconf database is unreadable: {}", why.0))
                    .ok()?;
                Some((source, table))
            })
            .collect();
        Layers { databases }
    }

    /// Where the host's compiled schemas are.
    pub fn schemas(&self) -> PathBuf {
        self.path(Path::new("/usr/share/glib-2.0/schemas"))
    }

    /// The host's schema `id`, from its compiled schemas, for its defaults
    /// (which a distribution's override is compiled into).
    pub fn compiled_schema(&self, id: &str) -> Option<gio::SettingsSchema> {
        gio::SettingsSchemaSource::from_directory(self.schemas(), None, false)
            .ok()
            .and_then(|source| source.lookup(id, false))
    }

    /// The desktop's value of `key` in the schema `id`, kept at the dconf path
    /// `path`: from GSettings natively; inside the Flatpak from dconf's files,
    /// then the schema's default. None when the desktop has no such schema.
    pub fn setting(&self, id: &str, path: &str, key: &str) -> Option<glib::Variant> {
        if !self.flatpak {
            let schema = gio::SettingsSchemaSource::default()?.lookup(id, true)?;
            if !schema.has_key(key) {
                return None;
            }
            let settings = gio::Settings::new_full(&schema, None::<&gio::SettingsBackend>, None);
            return Some(settings.value(key));
        }
        if let Some((_, value)) = self.dconf_layers().read_from(&format!("{path}{key}")) {
            return Some(value);
        }
        let schema = self.compiled_schema(id)?;
        schema.has_key(key).then(|| schema.key(key).default_value())
    }

    fn dirs(&self, var: &str, usual: &[&str]) -> Vec<PathBuf> {
        let from_env = (!self.flatpak)
            .then(|| std::env::var(var).ok())
            .flatten()
            .filter(|v| !v.is_empty());
        match from_env {
            Some(list) => list.split(':').map(PathBuf::from).collect(),
            None => usual.iter().map(|d| self.path(Path::new(d))).collect(),
        }
    }
}
