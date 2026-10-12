// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! A Flatpak install's own copies of the file-manager integrations (S14-T4).
//!
//! Native packages install the Nautilus extension and the Dolphin menus
//! system-wide. A Flatpak cannot put files outside itself, but it can write
//! to the user's data folder, where both file managers also look, so
//! Preferences installs copies there: the shipped files, with the two things a
//! Flatpak install does differently rewritten. Its window opens with
//! `flatpak run`, and its daemon publishes the backed-up folders under
//! `~/.var/app`. The same rewrite as the development recipes make, so the
//! copy is the file that ships.
//!
//! A copy is recognised as ours by the line that opens the Flatpak, so
//! removing or refreshing one never touches a package's files or a
//! development copy.

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::prefs::Plugin;

const NAUTILUS: &str = include_str!("../../../../integrations/nautilus/backtrack.py");
const DOLPHIN_FILE: &str = include_str!("../../../../integrations/dolphin/backtrack.desktop");
const DOLPHIN_FOLDER: &str =
    include_str!("../../../../integrations/dolphin/backtrack-folder.desktop");

/// How a Flatpak copy opens the window.
fn flatpak_run() -> String {
    format!("flatpak run {}", backtrack_core::dbus::APP_ID)
}

/// One file of a copy: where it goes under the user's data folder, what it
/// holds, and whether it is marked executable (KIO runs a service menu from
/// the home folder only if it is).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopiedFile {
    pub path: &'static str,
    pub contents: String,
    pub executable: bool,
}

impl Plugin {
    /// The line a Flatpak copy opens the window with, written into every copy
    /// and looked for to recognise one. Each integration spells the command
    /// its own way: a Python tuple, a desktop file's `Exec`.
    fn flatpak_line(self) -> String {
        match self {
            Plugin::Nautilus => format!(
                "COMMAND = (\"flatpak\", \"run\", \"{}\")",
                backtrack_core::dbus::APP_ID
            ),
            Plugin::Dolphin => format!("Exec={} ", flatpak_run()),
        }
    }

    /// The Flatpak's copy of this integration, reading the backed-up folders
    /// from `roots`.
    pub fn flatpak_copy(self, roots: &Path) -> Vec<CopiedFile> {
        match self {
            Plugin::Nautilus => {
                let contents = NAUTILUS
                    .replacen("COMMAND = (\"backtrack-gtk\",)", &self.flatpak_line(), 1)
                    .replacen(
                        "ROOTS = None",
                        &format!("ROOTS = {:?}", roots.display().to_string()),
                        1,
                    );
                vec![CopiedFile {
                    path: "nautilus-python/extensions/backtrack.py",
                    contents,
                    executable: false,
                }]
            }
            Plugin::Dolphin => [
                ("kio/servicemenus/backtrack.desktop", DOLPHIN_FILE),
                ("kio/servicemenus/backtrack-folder.desktop", DOLPHIN_FOLDER),
            ]
            .into_iter()
            .map(|(path, shipped)| CopiedFile {
                path,
                contents: shipped.replace(
                    "\nExec=backtrack-gtk ",
                    &format!("\n{}", self.flatpak_line()),
                ),
                executable: true,
            })
            .collect(),
        }
    }

    /// Whether the Flatpak's copy is installed under `data_home`.
    pub fn has_flatpak_copy(self, data_home: &Path) -> bool {
        self.flatpak_copy(Path::new(""))
            .iter()
            .any(|copy| self.is_ours(&data_home.join(copy.path)))
    }

    /// Install the Flatpak's copy under `data_home`, replacing an older one.
    pub fn install_flatpak_copy(self, data_home: &Path, roots: &Path) -> io::Result<()> {
        for copy in self.flatpak_copy(roots) {
            write(&data_home.join(copy.path), &copy)?;
        }
        Ok(())
    }

    /// Remove the Flatpak's copy from `data_home`, leaving any file that is
    /// not ours where it is.
    pub fn remove_flatpak_copy(self, data_home: &Path) -> io::Result<()> {
        for copy in self.flatpak_copy(Path::new("")) {
            let path = data_home.join(copy.path);
            if self.is_ours(&path) {
                std::fs::remove_file(&path)?;
            }
        }
        if self == Plugin::Nautilus {
            remove_python_cache(&data_home.join("nautilus-python/extensions"));
        }
        Ok(())
    }

    /// Rewrite the Flatpak's copy where one is installed and differs from
    /// what this version would install: an update to the Flatpak updates it.
    /// Returns whether anything was written.
    pub fn refresh_flatpak_copy(self, data_home: &Path, roots: &Path) -> io::Result<bool> {
        if !self.has_flatpak_copy(data_home) {
            return Ok(false);
        }
        let stale = self.flatpak_copy(roots).iter().any(|copy| {
            std::fs::read_to_string(data_home.join(copy.path))
                .ok()
                .as_deref()
                != Some(copy.contents.as_str())
        });
        if stale {
            self.install_flatpak_copy(data_home, roots)?;
        }
        Ok(stale)
    }

    /// Whether the file at `path` is a Flatpak copy: it opens the window with
    /// `flatpak run`, which neither a package's file nor a development copy
    /// does.
    fn is_ours(self, path: &Path) -> bool {
        std::fs::read_to_string(path).is_ok_and(|text| text.contains(&self.flatpak_line()))
    }
}

/// Written beside its final name and moved into place, so a file manager
/// reading the folder never finds half a file.
fn write(path: &Path, copy: &CopiedFile) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let partial = path.with_extension("partial");
    std::fs::write(&partial, &copy.contents)?;
    let mode = if copy.executable { 0o755 } else { 0o644 };
    std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(mode))?;
    std::fs::rename(&partial, path)
}

/// Python's compiled copies of the extension, and their folder if nothing
/// else is in it.
fn remove_python_cache(extensions: &Path) {
    let cache = extensions.join("__pycache__");
    if let Ok(entries) = std::fs::read_dir(&cache) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with("backtrack.")
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let _ = std::fs::remove_dir(&cache);
}

/// Where the file managers on the host load integrations from, as seen from
/// inside the Flatpak: the user's data folder, then the host's own system
/// folders, which `--filesystem=host` shows under `/run/host`.
pub fn host_data_dirs(data_home: PathBuf) -> Vec<PathBuf> {
    vec![
        data_home,
        PathBuf::from("/run/host/usr/local/share"),
        PathBuf::from("/run/host/usr/share"),
    ]
}

/// Where the host's programs are, as seen from inside the Flatpak.
pub fn host_program_dirs() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/run/host/usr/local/bin"),
        PathBuf::from("/run/host/usr/bin"),
    ]
}

/// Whether the host has nautilus-python, without which Nautilus loads no
/// Python extension at all. Its library is in a different folder on each
/// distribution, so each of their layouts is looked in.
pub fn host_has_nautilus_python() -> bool {
    has_nautilus_python(Path::new("/run/host/usr"))
}

fn has_nautilus_python(usr: &Path) -> bool {
    [
        "lib64/nautilus/extensions-4",
        "lib/nautilus/extensions-4",
        "lib/x86_64-linux-gnu/nautilus/extensions-4",
        "lib/aarch64-linux-gnu/nautilus/extensions-4",
    ]
    .iter()
    .any(|dir| usr.join(dir).join("libnautilus-python.so").is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOTS: &str = "/home/k/.var/app/tools.backtrack.Backtrack/data/backtrack/roots.json";

    #[test]
    fn the_nautilus_copy_opens_the_flatpak_and_reads_its_roots() {
        let copy = &Plugin::Nautilus.flatpak_copy(Path::new(ROOTS))[0];
        assert!(copy
            .contents
            .contains("\nCOMMAND = (\"flatpak\", \"run\", \"tools.backtrack.Backtrack\")\n"));
        assert!(copy.contents.contains(&format!("\nROOTS = \"{ROOTS}\"\n")));
        // Both shipped lines were there to rewrite; a change in their shape
        // must fail here rather than install a copy that opens nothing.
        assert!(!copy.contents.contains("COMMAND = (\"backtrack-gtk\",)"));
        assert!(!copy.contents.contains("\nROOTS = None\n"));
        assert!(!copy.executable);
    }

    #[test]
    fn the_dolphin_copies_open_the_flatpak_and_are_executable() {
        let copies = Plugin::Dolphin.flatpak_copy(Path::new(ROOTS));
        assert_eq!(copies.len(), 2);
        for copy in copies {
            assert!(copy.executable, "{}", copy.path);
            assert!(
                !copy.contents.contains("Exec=backtrack-gtk"),
                "{}",
                copy.path
            );
            assert!(
                copy.contents
                    .contains("\nExec=flatpak run tools.backtrack.Backtrack --"),
                "{}",
                copy.path
            );
        }
    }

    #[test]
    fn install_refresh_and_remove_touch_only_our_copy() {
        for plugin in [Plugin::Nautilus, Plugin::Dolphin] {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path();
            let roots = Path::new(ROOTS);
            assert!(!plugin.has_flatpak_copy(home), "{plugin:?}");

            // What Preferences shows Remove for: the copy it just installed.
            plugin.install_flatpak_copy(home, roots).unwrap();
            assert!(plugin.has_flatpak_copy(home), "{plugin:?}");
            assert!(plugin.installed_in(&[home.to_path_buf()]), "{plugin:?}");
            let copy = &plugin.flatpak_copy(roots)[0];
            let file = home.join(copy.path);
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o111 != 0, copy.executable, "{plugin:?}");

            // Up to date: nothing to write. Stale: rewritten.
            assert!(
                !plugin.refresh_flatpak_copy(home, roots).unwrap(),
                "{plugin:?}"
            );
            std::fs::write(&file, format!("{}\nan older version\n", copy.contents)).unwrap();
            assert!(
                plugin.refresh_flatpak_copy(home, roots).unwrap(),
                "{plugin:?}"
            );
            assert_eq!(std::fs::read_to_string(&file).unwrap(), copy.contents);

            plugin.remove_flatpak_copy(home).unwrap();
            assert!(!file.exists(), "{plugin:?}");
            assert!(!plugin.installed_in(&[home.to_path_buf()]), "{plugin:?}");
        }
    }

    #[test]
    fn a_copy_that_is_not_ours_is_left_alone() {
        // A development install of the extension, or any other file of the
        // same name: neither removed nor refreshed.
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let extension = home.join("nautilus-python/extensions/backtrack.py");
        std::fs::create_dir_all(extension.parent().unwrap()).unwrap();
        std::fs::write(&extension, "COMMAND = (\"/checkout/backtrack-gtk\",)\n").unwrap();
        assert!(!Plugin::Nautilus.has_flatpak_copy(home));
        assert!(!Plugin::Nautilus
            .refresh_flatpak_copy(home, Path::new(ROOTS))
            .unwrap());
        Plugin::Nautilus.remove_flatpak_copy(home).unwrap();
        assert!(extension.exists());
    }

    #[test]
    fn nautilus_python_is_found_in_each_distributions_layout() {
        for layout in [
            "lib64/nautilus/extensions-4",
            "lib/nautilus/extensions-4",
            "lib/x86_64-linux-gnu/nautilus/extensions-4",
        ] {
            let dir = tempfile::tempdir().unwrap();
            assert!(!has_nautilus_python(dir.path()));
            std::fs::create_dir_all(dir.path().join(layout)).unwrap();
            std::fs::write(dir.path().join(layout).join("libnautilus-python.so"), "").unwrap();
            assert!(has_nautilus_python(dir.path()), "{layout}");
        }
    }
}
