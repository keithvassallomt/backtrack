// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Facts about the computer Backtrack runs on, read the same way inside a
//! Flatpak as outside one.

use std::path::Path;

/// Inside a Flatpak, `/etc/os-release` describes the runtime, and Flatpak puts
/// the host's at `/run/host/os-release`. Outside one, only the second exists.
const OS_RELEASE: [&str; 2] = ["/run/host/os-release", "/etc/os-release"];

/// Flatpak writes this into every sandbox; its `runtime=` names the runtime.
const FLATPAK_INFO: &str = "/.flatpak-info";

/// The host distribution's `os-release`, or an empty string if there is none.
pub fn os_release() -> String {
    first_readable(&OS_RELEASE)
}

/// Whether this process runs inside a Flatpak.
pub fn in_flatpak() -> bool {
    Path::new(FLATPAK_INFO).exists()
}

/// The Flatpak runtime this process runs on (`runtime/org.gnome.Platform/x86_64/51`),
/// or `None` outside a Flatpak.
pub fn flatpak_runtime() -> Option<String> {
    runtime_of(&std::fs::read_to_string(FLATPAK_INFO).ok()?)
}

fn first_readable(paths: &[impl AsRef<Path>]) -> String {
    paths
        .iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default()
}

fn runtime_of(flatpak_info: &str) -> Option<String> {
    flatpak_info
        .lines()
        .find_map(|line| line.strip_prefix("runtime="))
        .map(|runtime| runtime.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hosts_file_is_looked_for_first() {
        assert_eq!(OS_RELEASE, ["/run/host/os-release", "/etc/os-release"]);
    }

    #[test]
    fn the_hosts_file_wins_over_the_runtimes() {
        let dir = tempfile::tempdir().unwrap();
        let host = dir.path().join("host");
        let own = dir.path().join("own");
        std::fs::write(&host, "ID=fedora\n").unwrap();
        std::fs::write(&own, "ID=org.gnome.Platform\n").unwrap();
        assert_eq!(first_readable(&[&host, &own]), "ID=fedora\n");
    }

    #[test]
    fn outside_a_flatpak_the_systems_own_file_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let own = dir.path().join("own");
        std::fs::write(&own, "ID=arch\n").unwrap();
        assert_eq!(
            first_readable(&[&dir.path().join("absent"), &own]),
            "ID=arch\n"
        );
    }

    #[test]
    fn with_neither_file_there_is_nothing_to_say() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(first_readable(&[dir.path().join("absent")]), "");
    }

    #[test]
    fn the_runtime_is_read_from_flatpak_info() {
        let info = "[Application]\nname=tools.backtrack.Backtrack\n\
                    runtime=runtime/org.gnome.Platform/x86_64/51\n";
        assert_eq!(
            runtime_of(info).as_deref(),
            Some("runtime/org.gnome.Platform/x86_64/51")
        );
        assert_eq!(runtime_of("[Application]\nname=x\n"), None);
    }
}
