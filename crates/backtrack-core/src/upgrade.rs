// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Learning that Backtrack has been upgraded under a running program, the
//! daemon or the tray, and, inside a Flatpak, starting the new version
//! (S18-T13).
//!
//! Installed from a package, the program is replaced on disk, which
//! [`host::wait_until_replaced`] notices: the daemon exits for systemd to
//! start the new one, and the tray starts the new one in its own place.
//! Inside a Flatpak, `/app` stays as it was for as long as the instance runs,
//! so the update is learnt from Flatpak itself, and the daemon's supervisor,
//! which stands in for systemd there, and the tray each ask the Flatpak
//! portal to start the new version in a sandbox of its own, then leave. A
//! sandbox does not end with its first process: on the Plasma VM the tray,
//! the daemon's child, ran on in the old one, holding its bus name, after the
//! daemon had gone.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

use crate::host::{self, Replaced};
use futures::StreamExt;
use tracing::{debug, info};

/// Where Flatpak describes the sandbox to the processes in it.
const FLATPAK_INFO: &str = "/.flatpak-info";

/// Passed on to the new version, which would otherwise have the portal's
/// environment rather than this one's.
const PASSED_ON: [&str; 2] = ["BACKTRACK_DEV", "RUST_LOG"];

/// What becomes of this program: uninstalled or upgraded, as
/// [`host::wait_until_replaced`] says, or inside a Flatpak upgraded once a
/// newer version has been installed. Never returns where neither can be told.
pub async fn replaced() -> Replaced {
    if host::in_flatpak() {
        updated().await;
        return Replaced::Upgraded;
    }
    match std::env::current_exe() {
        Ok(program) => host::wait_until_replaced(program, host::LOOK_FOR_PROGRAM_EVERY).await,
        Err(_) => std::future::pending().await,
    }
}

/// The deployment this sandbox runs, as `/.flatpak-info` gives it.
#[derive(Debug, PartialEq, Eq)]
struct Deployment {
    /// The commit running.
    commit: String,
    /// The link naming the commit installed now, beside the running one's
    /// directory. Flatpak hides its installations from a sandbox, even one
    /// with the host's files; the manifest grants Backtrack's own, read-only,
    /// both per user and system-wide.
    active: PathBuf,
}

fn deployment(flatpak_info: &str) -> Option<Deployment> {
    let mut instance = false;
    let (mut commit, mut path) = (None, None);
    for line in flatpak_info.lines().map(str::trim) {
        if line.starts_with('[') {
            instance = line == "[Instance]";
        } else if instance {
            if let Some(value) = line.strip_prefix("app-commit=") {
                commit = Some(value.to_string());
            } else if let Some(value) = line.strip_prefix("app-path=") {
                path = Some(PathBuf::from(value));
            }
        }
    }
    // `app-path` is `…/<commit>/files`.
    let active = path?.parent()?.parent()?.join("active");
    Some(Deployment {
        commit: commit?,
        active,
    })
}

/// Whether the installation's link names another commit than the one running.
fn installed_since(deployment: &Deployment) -> bool {
    std::fs::read_link(&deployment.active)
        .ok()
        .and_then(|target| {
            target
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .is_some_and(|installed| installed != deployment.commit)
}

/// Return once a newer version of this Flatpak has been installed: from its
/// installation's link where that can be read, within a look, or from the
/// Flatpak portal, which looks twice an hour.
async fn updated() {
    let read = std::fs::read_to_string(FLATPAK_INFO).ok();
    let Some(deployment) = read.as_deref().and_then(deployment) else {
        debug!("this sandbox does not say what it runs; an update is noticed at the next login");
        return std::future::pending().await;
    };
    tokio::select! {
        () = link_changed(&deployment) => {}
        () = portal_says(&deployment.commit) => {}
    }
    info!(running = %deployment.commit, "a newer version of Backtrack has been installed");
}

async fn link_changed(deployment: &Deployment) {
    if std::fs::symlink_metadata(&deployment.active).is_err() {
        debug!(
            link = %deployment.active.display(),
            "the installation cannot be read from here; the Flatpak portal says when it changes"
        );
        return std::future::pending().await;
    }
    let mut looks = tokio::time::interval(host::LOOK_FOR_PROGRAM_EVERY);
    loop {
        looks.tick().await;
        if installed_since(deployment) {
            return;
        }
    }
}

async fn portal_says(running: &str) {
    let monitor = async {
        let flatpak = ashpd::flatpak::Flatpak::new().await?;
        flatpak.create_update_monitor(Default::default()).await
    };
    let monitor = match monitor.await {
        Ok(monitor) => monitor,
        Err(error) => {
            debug!(%error, "the Flatpak portal will not say when Backtrack is updated");
            return std::future::pending().await;
        }
    };
    let mut updates = match monitor.receive_update_available().await {
        Ok(updates) => updates,
        Err(error) => {
            debug!(%error, "the Flatpak portal's updates cannot be listened to");
            return std::future::pending().await;
        }
    };
    // It also says when a newer version is only available; only one
    // installed is a reason to restart.
    while let Some(update) = updates.next().await {
        if update.local_commit() != running {
            return;
        }
    }
    std::future::pending().await
}

/// Start the newest installed version of `program`, with `args`, in a sandbox
/// of its own, through the Flatpak portal; the process asking can leave once
/// this returns, as the new one is not tied to it.
pub async fn start_latest(program: &Path, args: &[&str]) -> ashpd::Result<u32> {
    let flatpak = ashpd::flatpak::Flatpak::new().await?;
    let mut argv = vec![program.to_path_buf()];
    argv.extend(args.iter().map(PathBuf::from));
    let passed: Vec<(&str, String)> = PASSED_ON
        .iter()
        .filter_map(|name| std::env::var(name).ok().map(|value| (*name, value)))
        .collect();
    let envs: HashMap<&str, &str> = passed
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    flatpak
        .spawn(
            "/",
            &argv,
            HashMap::<u32, OwnedFd>::new(),
            envs,
            ashpd::flatpak::SpawnFlags::LatestVersion.into(),
            ashpd::flatpak::SpawnOptions::default(),
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped as a sandbox's own on Keith's laptop (Flatseal's, 2026-10-31).
    const INFO: &str = "[Application]\n\
        name=tools.backtrack.Backtrack\n\
        runtime=runtime/org.gnome.Platform/x86_64/50\n\
        \n\
        [Instance]\n\
        instance-id=1234567890\n\
        instance-path=/home/k/.var/app/tools.backtrack.Backtrack\n\
        app-path=/home/k/.local/share/flatpak/app/tools.backtrack.Backtrack/x86_64/master/ef9f/files\n\
        app-commit=ef9f\n\
        branch=master\n\
        \n\
        [Context]\n\
        app-commit=not-this-one\n";

    #[test]
    fn the_running_commit_and_the_installations_link_are_read() {
        assert_eq!(
            deployment(INFO),
            Some(Deployment {
                commit: "ef9f".into(),
                active: PathBuf::from(
                    "/home/k/.local/share/flatpak/app/tools.backtrack.Backtrack/x86_64/master/active"
                ),
            })
        );
        assert_eq!(deployment("[Application]\nname=x\n"), None);
    }

    #[test]
    fn a_newer_commit_installed_is_noticed_and_the_same_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let active = dir.path().join("active");
        let running = Deployment {
            commit: "ef9f".into(),
            active: active.clone(),
        };
        assert!(!installed_since(&running), "no link: nothing to say");
        std::os::unix::fs::symlink("ef9f", &active).unwrap();
        assert!(!installed_since(&running), "the commit running");
        std::fs::remove_file(&active).unwrap();
        std::os::unix::fs::symlink("a1b2", &active).unwrap();
        assert!(installed_since(&running), "a newer one installed");
    }
}
