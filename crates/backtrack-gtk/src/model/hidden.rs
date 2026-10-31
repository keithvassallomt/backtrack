// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Hidden files (S18-T11): names that begin with a dot, which file managers
//! leave out unless asked to show them, and which Backtrack now leaves out
//! too, in the file pane and in Rewind.
//!
//! Whether to show them starts from the desktop's own choice: GTK's file
//! chooser setting, which Nautilus shares. Show Hidden Files (Ctrl+H) changes
//! it for Backtrack.

use gtk4::glib;

use crate::path;

/// Where the desktop keeps whether hidden files are shown.
pub const SCHEMA: &str = "org.gtk.gtk4.Settings.FileChooser";
pub const PATH: &str = "/org/gtk/gtk4/settings/file-chooser/";
pub const KEY: &str = "show-hidden";

/// Whether a name is a hidden file's.
pub fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// What the desktop's setting says; not to show them when it has none.
pub fn from_desktop(value: Option<glib::Variant>) -> bool {
    value.and_then(|v| v.get::<bool>()).unwrap_or(false)
}

/// The names directly inside `folder` that are, or lead down to, one of
/// `roots` (the folders backed up). One of those is never hidden: hiding it
/// would leave no way to what the person chose to back up.
pub fn ways_down(folder: &str, roots: &[String]) -> Vec<String> {
    let mut names: Vec<String> = roots
        .iter()
        .filter(|root| *root != folder && (folder.is_empty() || path::is_within(root, folder)))
        .filter_map(|root| {
            let rest = if folder.is_empty() {
                root.as_str()
            } else {
                root.get(folder.len() + 1..)?
            };
            Some(rest.split('/').next()?.to_owned())
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Whether the entry `name` is shown.
pub fn shown(name: &str, show_hidden: bool, ways_down: &[String]) -> bool {
    show_hidden || !is_hidden(name) || ways_down.iter().any(|way| way == name)
}

#[cfg(test)]
mod tests {
    use super::super::wallpaper::{dconf, gvdb};
    use super::*;

    fn layers(names: &[&str]) -> dconf::Layers {
        dconf::Layers {
            databases: names
                .iter()
                .map(|name| {
                    let path = format!(
                        "{}/src/model/wallpaper/testdata/dconf/{name}.gvdb",
                        env!("CARGO_MANIFEST_DIR")
                    );
                    let table = gvdb::parse(&std::fs::read(path).unwrap()).unwrap();
                    (dconf::Source::User((*name).to_owned()), table)
                })
                .collect(),
        }
    }

    fn desktop(names: &[&str]) -> bool {
        from_desktop(
            layers(names)
                .read_from(&format!("{PATH}{KEY}"))
                .map(|(_, value)| value),
        )
    }

    #[test]
    fn the_desktops_choice_is_read_as_dconf_keeps_it() {
        assert!(desktop(&["show-hidden"]), "turned on");
        assert!(!desktop(&["user"]), "never set: not shown");
        assert!(!from_desktop(None), "no such setting on this desktop");
        assert!(
            !from_desktop(Some(glib::Variant::from("yes"))),
            "not a boolean"
        );
    }

    #[test]
    fn a_dot_file_is_hidden_unless_hidden_files_are_shown() {
        assert!(is_hidden(".bashrc") && is_hidden(".config"));
        assert!(!is_hidden("Documents") && !is_hidden("notes.txt."));
        assert!(!shown(".bashrc", false, &[]));
        assert!(shown(".bashrc", true, &[]));
        assert!(shown("Documents", false, &[]));
    }

    #[test]
    fn a_hidden_folder_that_leads_to_a_backed_up_one_is_shown() {
        let roots = vec![
            "home/keith/.config/app".to_owned(),
            "home/keith/.ssh".to_owned(),
            "home/keith/Documents".to_owned(),
            "home/other/.local".to_owned(),
        ];
        let ways = ways_down("home/keith", &roots);
        assert_eq!(ways, [".config", ".ssh", "Documents"]);
        assert!(
            shown(".config", false, &ways),
            "the way down to .config/app"
        );
        assert!(shown(".ssh", false, &ways), "backed up itself");
        assert!(!shown(".cache", false, &ways));
        assert_eq!(ways_down("", &roots), ["home"]);
        assert!(ways_down("home/keith/Documents", &roots).is_empty());
    }
}
