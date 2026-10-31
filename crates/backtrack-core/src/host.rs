// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Facts about the computer Backtrack runs on, read the same way inside a
//! Flatpak as outside one.

use std::path::{Path, PathBuf};
use std::time::Duration;

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

/// How often a running program looks at itself; see [`wait_until_replaced`].
pub const LOOK_FOR_PROGRAM_EVERY: Duration = Duration::from_secs(30);

/// How long after a change the program is looked at again.
const SECOND_LOOK_AFTER: Duration = Duration::from_secs(5);

/// What has become of a running program's file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replaced {
    /// Gone: the package it came with has been uninstalled.
    Uninstalled,
    /// Another file in its place: the package has been upgraded (S18-T13).
    Upgraded,
}

/// Return once `program`, this process's own, has been uninstalled or
/// upgraded under it, as the file there then is from the one there now.
///
/// Neither ends the processes a package started. On Arch nothing stops a
/// user's daemon at all, so after an uninstall the daemon and the tray ran on
/// from deleted programs until the user's systemd stopped; after an upgrade
/// they ran on as the old version, and the tray could no longer read the new
/// daemon's status. A package manager writes the new program beside the old
/// one and renames it over, and the old one stays on disk, under a new name
/// or none, while it runs, so the two are never the same file. A change is
/// looked at again a few seconds later, so a package manager mid-write is
/// taken for neither. A Flatpak's `/app` stays while the instance runs, so
/// inside one this never returns.
pub fn wait_until_replaced(
    program: PathBuf,
    every: Duration,
) -> impl std::future::Future<Output = Replaced> {
    // Which file is running, taken now rather than at the first look.
    let running = identity(&program);
    async move {
        let Some(running) = running else {
            return std::future::pending().await;
        };
        let mut looks = tokio::time::interval(every);
        loop {
            looks.tick().await;
            let first = identity(&program);
            if first == Some(running) {
                continue;
            }
            tokio::time::sleep(SECOND_LOOK_AFTER).await;
            match identity(&program) {
                None if first.is_none() => return Replaced::Uninstalled,
                Some(second) if first == Some(second) => return Replaced::Upgraded,
                _ => {}
            }
        }
    }
}

/// Which file is at `path`: its device and inode.
fn identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVERY: Duration = Duration::from_secs(60);

    #[tokio::test(start_paused = true)]
    async fn a_program_uninstalled_is_noticed() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("backtrack-tray");
        std::fs::write(&program, b"old").unwrap();
        let replaced = tokio::spawn(wait_until_replaced(program.clone(), EVERY));

        tokio::time::sleep(Duration::from_secs(600)).await;
        assert!(
            !replaced.is_finished(),
            "ten minutes installed, and still there"
        );

        std::fs::remove_file(&program).unwrap();
        let replaced = tokio::time::timeout(2 * EVERY, replaced)
            .await
            .expect("noticed within a couple of looks")
            .unwrap();
        assert_eq!(replaced, Replaced::Uninstalled);
    }

    #[tokio::test(start_paused = true)]
    async fn an_upgrade_is_noticed_as_one() {
        // A package manager writes the new program beside the old one and
        // renames it over, so the path is there before, during and after.
        // The old one stays open while it runs, as it would.
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("backtrack-tray");
        std::fs::write(&program, b"old").unwrap();
        let _running = std::fs::File::open(&program).unwrap();
        let replaced = tokio::spawn(wait_until_replaced(program.clone(), EVERY));

        let new = dir.path().join(".backtrack-tray.new");
        std::fs::write(&new, b"new").unwrap();
        std::fs::rename(&new, &program).unwrap();
        let replaced = tokio::time::timeout(2 * EVERY, replaced)
            .await
            .expect("noticed within a couple of looks")
            .unwrap();
        assert_eq!(
            replaced,
            Replaced::Upgraded,
            "an upgrade is not an uninstall"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_program_missing_for_a_moment_is_not_an_uninstall() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("backtrack-tray");
        std::fs::write(&program, b"old").unwrap();
        let _running = std::fs::File::open(&program).unwrap();
        let replaced = tokio::spawn(wait_until_replaced(program.clone(), EVERY));
        tokio::time::sleep(Duration::from_secs(1)).await;
        // Missing at a look, and back before the second.
        std::fs::remove_file(&program).unwrap();
        tokio::time::sleep(EVERY).await;
        std::fs::write(&program, b"new").unwrap();
        let replaced = tokio::time::timeout(2 * EVERY, replaced)
            .await
            .expect("noticed")
            .unwrap();
        assert_eq!(replaced, Replaced::Upgraded, "back within seconds, and new");
    }

    #[tokio::test(start_paused = true)]
    async fn a_program_not_there_when_started_is_never_reported() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("backtrack-tray");
        let replaced = tokio::spawn(wait_until_replaced(program.clone(), EVERY));
        std::fs::write(&program, b"new").unwrap();
        tokio::time::sleep(10 * EVERY).await;
        assert!(!replaced.is_finished(), "nothing to compare with");
    }

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
