// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The published shape of `org.backtrack.Daemon1`, pinned.
//!
//! The interface is a contract with the GUI, the CLI, and any file-manager
//! plugin: renaming a method or changing an argument's type breaks them at run
//! time, where a compiler cannot help. The snapshot below is compared against
//! what the daemon actually exports, so any such change has to be made
//! deliberately, here, in the same commit that causes it.
//!
//! The rendering comes from the interface itself rather than from a live bus,
//! so this runs anywhere — including a CI container with no session bus.

/// The expected method set: name and D-Bus signature, sorted.
///
/// Signature codes: `s` string, `t` u64, `u` u32, `x` i64, `b` bool, `y` byte,
/// `h` file descriptor, `as` string array, `ay` byte array, `()` struct.
pub const METHODS: &[(&str, &str, &str)] = &[
    // (name, argument signature, return signature)
    ("BackupNow", "", "t"),
    ("CancelJob", "t", ""),
    ("Compact", "", "t"),
    ("CompareFile", "ss", "t"),
    ("DiscardRestore", "t", ""),
    ("ExecuteRestore", "tsa(ss)", "t"),
    ("GetConfig", "", "s"),
    ("GetConfigKey", "s", "s"),
    ("GetRestorePreview", "t", "(ssuuuuuua(ssbtxtxss)a(ss)as)"),
    ("GetStatus", "", "(sttbtsuuttb)"),
    ("ImportRepo", "ss", ""),
    ("ListReplaced", "u", "a(sstxx)"),
    ("LiveFile", "s", "h"),
    ("PathsOnDisk", "as", "ay"),
    ("Pause", "t", ""),
    ("PauseJob", "t", ""),
    ("PrepareRestore", "sass", "t"),
    ("PreviewFile", "ss", "h"),
    ("Prune", "", "t"),
    ("PutBackReplaced", "s", "s"),
    ("RestoreEverything", "ss", "t"),
    ("RestoreFiles", "sasss", "t"),
    ("RestoreInto", "sass", "t"),
    ("Resume", "", ""),
    ("ResumeJob", "t", ""),
    ("SearchFiles", "s", "a(sssxxxxuxb)"),
    ("SetConfig", "ss", ""),
    ("SetupRepo", "ss", ""),
    ("UndoRestore", "t", "t"),
    ("Verify", "", "t"),
];

/// The expected signal set: name and argument signature, sorted.
pub const SIGNALS: &[(&str, &str)] = &[
    ("BackupProgress", "tstt"),
    ("IndexingProgress", "su"),
    ("JobFinished", "tss"),
    ("RestoreProgress", "ttt"),
    ("StatusChanged", "s"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::JobRegistry;
    use crate::service::{Daemon1, Shared};
    use backtrack_core::config::Config;
    use backtrack_testkit::MockSecretStore;
    use std::sync::Arc;
    use zbus::object_server::Interface;

    /// Render the interface exactly as the daemon exports it.
    fn introspection_xml() -> String {
        let shared = Shared::new(
            Config::default(),
            JobRegistry::new(),
            Arc::new(MockSecretStore::default()),
        );
        let iface = Daemon1::new(shared);
        let mut xml = String::new();
        iface.introspect_to_writer(&mut xml, 0);
        xml
    }

    /// A method as introspection describes it: name, arguments, return.
    type Method = (String, String, String);
    /// A signal as introspection describes it: name, arguments.
    type Signal = (String, String);

    /// Pull out the methods and signals the interface exports.
    fn parse(xml: &str) -> (Vec<Method>, Vec<Signal>) {
        let mut methods = Vec::new();
        let mut signals = Vec::new();
        let mut current: Option<(String, String, String, bool)> = None;

        for line in xml.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("<method name=\"") {
                let name = rest.split('"').next().unwrap_or_default().to_string();
                current = Some((name, String::new(), String::new(), false));
                if line.ends_with("/>") {
                    let (n, i, o, _) = current.take().unwrap();
                    methods.push((n, i, o));
                }
            } else if let Some(rest) = line.strip_prefix("<signal name=\"") {
                let name = rest.split('"').next().unwrap_or_default().to_string();
                current = Some((name, String::new(), String::new(), true));
                if line.ends_with("/>") {
                    let (n, i, _, _) = current.take().unwrap();
                    signals.push((n, i));
                }
            } else if line.starts_with("<arg ") {
                if let Some((_, ins, outs, _)) = current.as_mut() {
                    let ty = line
                        .split("type=\"")
                        .nth(1)
                        .and_then(|s| s.split('"').next())
                        .unwrap_or_default();
                    if line.contains("direction=\"out\"") {
                        outs.push_str(ty);
                    } else {
                        ins.push_str(ty);
                    }
                }
            } else if line == "</method>" {
                if let Some((n, i, o, _)) = current.take() {
                    methods.push((n, i, o));
                }
            } else if line == "</signal>" {
                if let Some((n, i, _, _)) = current.take() {
                    signals.push((n, i));
                }
            }
        }
        methods.sort();
        signals.sort();
        (methods, signals)
    }

    #[test]
    fn the_exported_methods_match_the_snapshot() {
        let (methods, _) = parse(&introspection_xml());
        let expected: Vec<Method> = METHODS
            .iter()
            .map(|(n, i, o)| (n.to_string(), i.to_string(), o.to_string()))
            .collect();
        assert_eq!(
            methods, expected,
            "the exported method set changed — update METHODS deliberately, \
             and check every client that calls the affected method"
        );
    }

    #[test]
    fn the_exported_signals_match_the_snapshot() {
        let (_, signals) = parse(&introspection_xml());
        let expected: Vec<Signal> = SIGNALS
            .iter()
            .map(|(n, i)| (n.to_string(), i.to_string()))
            .collect();
        assert_eq!(
            signals, expected,
            "the exported signal set changed — update SIGNALS deliberately"
        );
    }

    #[test]
    fn every_method_from_the_architecture_document_is_present() {
        // stack.md §2's table, by name. Pinned separately from the signature
        // snapshot so a *missing* method fails with a message that says which.
        let documented = [
            "BackupNow",
            "Pause",
            "Resume",
            "GetStatus",
            "RestoreFiles",
            "PreviewFile",
            "CompareFile",
            "SearchFiles",
            "RestoreEverything",
            "Prune",
            "Verify",
            "Compact",
            "CancelJob",
            "PauseJob",
            "SetConfig",
            "GetConfig",
            "SetupRepo",
            "ImportRepo",
        ];
        let (methods, _) = parse(&introspection_xml());
        let names: Vec<&str> = methods.iter().map(|(n, _, _)| n.as_str()).collect();
        for method in documented {
            assert!(
                names.contains(&method),
                "stack.md §2 documents {method}, which is not exported. Have: {names:?}"
            );
        }
    }

    #[test]
    fn every_signal_from_the_architecture_document_is_present() {
        let documented = [
            "BackupProgress",
            "RestoreProgress",
            "IndexingProgress",
            "StatusChanged",
        ];
        let (_, signals) = parse(&introspection_xml());
        let names: Vec<&str> = signals.iter().map(|(n, _)| n.as_str()).collect();
        for signal in documented {
            assert!(
                names.contains(&signal),
                "stack.md §2 documents {signal}, which is not exported. Have: {names:?}"
            );
        }
    }

    #[test]
    fn the_interface_is_named_as_documented() {
        assert_eq!(Daemon1::name(), "org.backtrack.Daemon1");
    }

    #[test]
    fn long_running_methods_return_a_job_id() {
        // The interface convention: nothing blocks for the length of a backup.
        for method in [
            "BackupNow",
            "RestoreFiles",
            "RestoreEverything",
            "CompareFile",
            "Prune",
            "Verify",
            "Compact",
        ] {
            let (_, _, out) = METHODS
                .iter()
                .find(|(n, _, _)| *n == method)
                .unwrap_or_else(|| panic!("{method} is in the snapshot"));
            assert_eq!(*out, "t", "{method} must return a job id");
        }
    }
}
