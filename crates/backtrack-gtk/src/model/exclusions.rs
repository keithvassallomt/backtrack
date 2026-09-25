// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The exclusion list as a person reads it.
//!
//! The configuration holds Borg patterns, and several of the defaults are one
//! idea spelled several ways: caches are `.cache`, `Cache` and `Caches`, and a
//! virtual machine's disk is a `.qcow2`, a `.vdi` or a `.vmdk`. The list shows
//! the idea, as mockup 16 does ("Caches", "VM images"), and removing a row
//! removes every pattern behind it. Anything else is shown as the pattern
//! itself, minus the `**/` that means "anywhere", which is noise to anybody
//! not writing Borg patterns for a living.

/// The groups that read as one row, when every pattern in the group is there.
const GROUPS: &[(&str, &[&str])] = &[
    ("Trash", &["**/.local/share/Trash"]),
    ("Caches", &["**/.cache", "**/Cache", "**/Caches"]),
    ("VM images", &["**/*.qcow2", "**/*.vdi", "**/*.vmdk"]),
];

/// One row of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub label: String,
    /// What removing the row removes.
    pub patterns: Vec<String>,
}

/// The list as rows, in the order the patterns first appear.
pub fn rows(patterns: &[String]) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let mut used = vec![false; patterns.len()];
    for (index, pattern) in patterns.iter().enumerate() {
        if used[index] {
            continue;
        }
        let group = GROUPS.iter().find(|(_, members)| {
            members.contains(&pattern.as_str())
                && members.iter().all(|m| patterns.iter().any(|p| p == m))
        });
        match group {
            Some((label, members)) => {
                for (other, flag) in patterns.iter().zip(used.iter_mut()) {
                    if members.contains(&other.as_str()) {
                        *flag = true;
                    }
                }
                rows.push(Row {
                    label: label.to_string(),
                    patterns: members.iter().map(|m| m.to_string()).collect(),
                });
            }
            None => {
                used[index] = true;
                rows.push(Row {
                    label: readable(pattern),
                    patterns: vec![pattern.clone()],
                });
            }
        }
    }
    rows
}

fn readable(pattern: &str) -> String {
    pattern.strip_prefix("**/").unwrap_or(pattern).to_string()
}

/// Turn what somebody typed into "Add Exclusion…" into a pattern, or `None`
/// if there is nothing there.
///
/// A bare name or wildcard (`node_modules`, `*.tmp`) means "anywhere", which
/// is what a person typing it means, so it gets the `**/` the defaults use.
/// An absolute path means that folder and nothing else, which Borg calls a
/// path prefix. Anything already written as a pattern, with a slash in it or
/// one of Borg's style prefixes, is taken as written.
pub fn from_input(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if input.starts_with('/') {
        return Some(format!("pp:{}", input.trim_end_matches('/')));
    }
    let styled = ["fm:", "sh:", "re:", "pp:", "pf:"]
        .iter()
        .any(|style| input.starts_with(style));
    if styled || input.contains('/') {
        return Some(input.to_string());
    }
    Some(format!("**/{input}"))
}

/// The list with `row` taken out.
pub fn without(patterns: &[String], row: &Row) -> Vec<String> {
    patterns
        .iter()
        .filter(|p| !row.patterns.contains(p))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use backtrack_core::config::Config;

    fn labels(patterns: &[String]) -> Vec<String> {
        rows(patterns).into_iter().map(|r| r.label).collect()
    }

    #[test]
    fn the_defaults_read_the_way_the_mockup_lists_them() {
        let defaults = Config::default().backup.exclude;
        assert_eq!(
            labels(&defaults),
            vec![
                "Caches",
                "Trash",
                "node_modules",
                "target/debug",
                "*.iso",
                "VM images"
            ]
        );
    }

    #[test]
    fn a_group_missing_a_member_is_shown_pattern_by_pattern() {
        // Somebody removed `.vdi` by hand; "VM images" would now claim more
        // than the list holds.
        let patterns = vec!["**/*.qcow2".to_string(), "**/*.vmdk".to_string()];
        assert_eq!(labels(&patterns), vec!["*.qcow2", "*.vmdk"]);
    }

    #[test]
    fn removing_a_group_removes_every_pattern_behind_it() {
        let defaults = Config::default().backup.exclude;
        let caches = rows(&defaults)
            .into_iter()
            .find(|r| r.label == "Caches")
            .unwrap();
        let left = without(&defaults, &caches);
        assert!(!left.iter().any(|p| p.contains("ache")), "{left:?}");
        assert_eq!(left.len(), defaults.len() - 3);
    }

    #[test]
    fn what_is_typed_becomes_the_pattern_that_was_meant() {
        assert_eq!(from_input(" *.tmp ").as_deref(), Some("**/*.tmp"));
        assert_eq!(
            from_input("node_modules").as_deref(),
            Some("**/node_modules")
        );
        assert_eq!(
            from_input("/home/k/Videos/").as_deref(),
            Some("pp:/home/k/Videos")
        );
        assert_eq!(from_input("**/build/out").as_deref(), Some("**/build/out"));
        assert_eq!(from_input("sh:**/*.log").as_deref(), Some("sh:**/*.log"));
        assert_eq!(from_input("   "), None);
    }
}
