// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The Dolphin switch in Preferences → General.
//!
//! Backtrack's Dolphin items are two fixed service-menu files
//! (`integrations/dolphin`), which cannot read a setting the way the Nautilus
//! extension reads `roots.json`. Dolphin keeps its own list of service-menu
//! actions to leave out, though: `kservicemenurc`, which its Settings →
//! Context Menu page writes, and which KIO reads each time it builds a menu.
//! An action listed as `false` under `[Show]` is not offered. The switch
//! writes those entries for Backtrack's two actions, and takes them out again
//! when it is turned back on.
//!
//! ```ini
//! [Show]
//! backtrackPreviousVersion=false
//! backtrackThisFolder=false
//! ```

use std::path::Path;

/// The action names in `integrations/dolphin/*.desktop`, which are the keys
/// Dolphin lists them under.
pub const ACTIONS: [&str; 2] = ["backtrackPreviousVersion", "backtrackThisFolder"];

const GROUP: &str = "[Show]";

/// `text`, the contents of a `kservicemenurc`, with Backtrack's actions
/// hidden, or no longer hidden. Every other line is kept as it was.
pub fn with_shown(text: &str, shown: bool) -> String {
    let hide: Vec<String> = ACTIONS.iter().map(|key| format!("{key}=false")).collect();
    let mut lines: Vec<String> = Vec::new();
    let mut in_group = false;
    let mut hidden = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_group = trimmed == GROUP;
            lines.push(line.to_string());
            if in_group && !shown && !hidden {
                lines.extend(hide.iter().cloned());
                hidden = true;
            }
        } else if !(in_group && is_ours(trimmed)) {
            lines.push(line.to_string());
        }
    }
    if !shown && !hidden {
        if lines.last().is_some_and(|line| !line.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push(GROUP.to_string());
        lines.extend(hide);
    }
    if lines.is_empty() {
        return String::new();
    }
    lines.join("\n") + "\n"
}

/// Whether `line` is the entry for one of Backtrack's actions. KConfig may
/// write a key with flags in brackets, `key[$i]=false`.
fn is_ours(line: &str) -> bool {
    let key = line.split('=').next().unwrap_or_default();
    let key = key.split('[').next().unwrap_or_default().trim();
    ACTIONS.contains(&key)
}

/// Hide Backtrack's actions in the `kservicemenurc` at `path`, or stop hiding
/// them. Returns whether the file was written: it is left alone when it
/// already says this, and not created only to say that the items are shown.
///
/// Written through a temporary file and a rename, so Dolphin never reads half
/// of one.
pub fn apply(path: &Path, shown: bool) -> std::io::Result<bool> {
    let current = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let wanted = with_shown(&current, shown);
    if wanted == current {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("backtrack.tmp");
    std::fs::write(&tmp, wanted)?;
    std::fs::rename(&tmp, path)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hiding_into_no_file_writes_the_group() {
        assert_eq!(
            with_shown("", false),
            "[Show]\nbacktrackPreviousVersion=false\nbacktrackThisFolder=false\n"
        );
    }

    #[test]
    fn showing_with_no_file_writes_nothing() {
        assert_eq!(with_shown("", true), "");
    }

    #[test]
    fn hiding_joins_the_existing_group_and_keeps_everything_else() {
        let before = "[Show]\ncompressfileitemaction=false\n\n[Other]\nkey=value\n";
        assert_eq!(
            with_shown(before, false),
            "[Show]\nbacktrackPreviousVersion=false\nbacktrackThisFolder=false\n\
             compressfileitemaction=false\n\n[Other]\nkey=value\n"
        );
    }

    #[test]
    fn hiding_adds_the_group_after_the_others() {
        let before = "[Other]\nkey=value\n";
        assert_eq!(
            with_shown(before, false),
            "[Other]\nkey=value\n\n[Show]\nbacktrackPreviousVersion=false\nbacktrackThisFolder=false\n"
        );
    }

    #[test]
    fn showing_removes_only_backtrack_s_entries() {
        let before = "[Show]\nbacktrackPreviousVersion=false\ncompressfileitemaction=false\n\
                      backtrackThisFolder[$i]=false\n";
        assert_eq!(
            with_shown(before, true),
            "[Show]\ncompressfileitemaction=false\n"
        );
    }

    #[test]
    fn a_key_of_the_same_name_in_another_group_is_not_ours() {
        let before = "[Other]\nbacktrackThisFolder=false\n";
        assert_eq!(with_shown(before, true), before);
    }

    #[test]
    fn hiding_twice_changes_nothing_the_second_time() {
        let once = with_shown("[Show]\nother=true\n", false);
        assert_eq!(with_shown(&once, false), once);
    }

    #[test]
    fn hiding_and_showing_again_gives_back_what_was_there() {
        let before = "[Show]\ncompressfileitemaction=false\n\n[Other]\nkey=value\n";
        assert_eq!(with_shown(&with_shown(before, false), true), before);
    }

    #[test]
    fn apply_writes_only_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/kservicemenurc");
        assert!(!apply(&path, true).unwrap(), "nothing to say");
        assert!(!path.exists());

        assert!(apply(&path, false).unwrap());
        let hidden = std::fs::read_to_string(&path).unwrap();
        assert!(hidden.contains("backtrackPreviousVersion=false"));
        assert!(!apply(&path, false).unwrap(), "already hidden");

        assert!(apply(&path, true).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[Show]\n");
    }

    #[test]
    fn the_action_names_are_the_ones_the_menus_declare() {
        // The keys only hide anything while they match the shipped files.
        let menus = concat!(
            include_str!("../../../integrations/dolphin/backtrack.desktop"),
            include_str!("../../../integrations/dolphin/backtrack-folder.desktop"),
        );
        for action in ACTIONS {
            assert!(
                menus.contains(&format!("[Desktop Action {action}]")),
                "{action} is not an action in integrations/dolphin"
            );
        }
    }
}
