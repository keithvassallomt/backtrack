// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! `backtrackd --supervise`: inside a Flatpak, what the systemd unit's
//! `Restart=on-failure` and `RestartSec=5s` do for a packaged install
//! (S14-T7).
//!
//! A Flatpak has no unit, so a daemon that crashed would stay down until
//! something started it again, and no backups would run meanwhile. Both ways
//! in, the Background portal's autostart command and the Flatpak's D-Bus
//! activation file, start the daemon through this instead. It runs the daemon
//! as a child and, as systemd does, starts it again after a failure and not
//! after a clean exit: a daemon that lost the single-instance race, or left
//! with no window open and no job running, exits 0, and that decision is the
//! daemon's to make.

use std::os::unix::process::ExitStatusExt;
use std::process::{ExitCode, ExitStatus};
use std::time::Duration;

use tokio::process::{Child, Command};
use tokio::signal::unix::{signal, SignalKind};
use tracing::{error, info, warn};

/// The unit's `RestartSec=`; `units.rs` checks that the two agree.
pub const RESTART_DELAY: Duration = Duration::from_secs(5);

/// The signals systemd counts as a clean stop rather than a failure:
/// SIGHUP, SIGINT, SIGPIPE and SIGTERM.
const CLEAN_SIGNALS: [i32; 4] = [1, 2, 13, 15];

/// What to do once the daemon has exited.
#[derive(Debug, PartialEq, Eq)]
pub enum Next {
    Restart,
    Stop,
}

/// `Restart=on-failure`: start it again after a non-zero exit or death by a
/// signal, unless that signal is one systemd counts as a clean stop.
pub fn after(status: ExitStatus) -> Next {
    // The one place this differs from the unit: a daemon that lost its bus
    // ran on a sandbox's proxy to it, which does not come back. Started again,
    // it could only fail to connect every five seconds; the next login's
    // autostart starts one in a new sandbox instead.
    if status.success() || status.code() == Some(i32::from(crate::daemon::BUS_LOST)) {
        return Next::Stop;
    }
    match status.signal() {
        Some(signal) if CLEAN_SIGNALS.contains(&signal) => Next::Stop,
        _ => Next::Restart,
    }
}

/// Run the daemon, and run it again whenever it fails, until it stops
/// cleanly or this process is asked to stop.
pub async fn run() -> ExitCode {
    let daemon = match std::env::current_exe() {
        Ok(daemon) => daemon,
        Err(error) => {
            error!(%error, "the daemon's own program could not be found");
            return ExitCode::FAILURE;
        }
    };
    let mut terminate = match signal(SignalKind::terminate()) {
        Ok(terminate) => terminate,
        Err(error) => {
            error!(%error, "SIGTERM could not be watched for");
            return ExitCode::FAILURE;
        }
    };
    loop {
        let mut child = match Command::new(&daemon).spawn() {
            Ok(child) => child,
            Err(error) => {
                error!(%error, path = %daemon.display(), "the daemon could not be started");
                return ExitCode::FAILURE;
            }
        };
        info!(pid = child.id(), "supervising the daemon");
        let status = tokio::select! {
            status = child.wait() => status,
            _ = terminate.recv() => return stop(&mut child).await,
        };
        let status = match status {
            Ok(status) => status,
            Err(error) => {
                error!(%error, "the daemon's exit could not be read");
                return ExitCode::FAILURE;
            }
        };
        match after(status) {
            Next::Stop => {
                info!(%status, "the daemon stopped, and so does its supervisor");
                return ExitCode::SUCCESS;
            }
            Next::Restart => {
                warn!(%status, delay = ?RESTART_DELAY, "the daemon failed; starting it again");
                tokio::select! {
                    () = tokio::time::sleep(RESTART_DELAY) => {}
                    _ = terminate.recv() => return ExitCode::SUCCESS,
                }
            }
        }
    }
}

/// Asked to stop: pass it on to the daemon, and stop once it has.
async fn stop(child: &mut Child) -> ExitCode {
    if let Some(pid) = child
        .id()
        .and_then(|pid| rustix::process::Pid::from_raw(pid as i32))
    {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
    }
    let _ = child.wait().await;
    info!("asked to stop; the daemon has stopped");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exited(code: i32) -> ExitStatus {
        ExitStatus::from_raw(code << 8)
    }

    fn killed(signal: i32) -> ExitStatus {
        ExitStatus::from_raw(signal)
    }

    #[test]
    fn a_clean_exit_is_the_daemons_decision() {
        assert_eq!(after(exited(0)), Next::Stop);
    }

    #[test]
    fn a_failure_is_started_again() {
        assert_eq!(after(exited(1)), Next::Restart);
        assert_eq!(after(killed(9)), Next::Restart);
        assert_eq!(after(killed(11)), Next::Restart);
        assert_eq!(after(killed(6)), Next::Restart);
    }

    #[test]
    fn a_daemon_that_lost_its_bus_is_left_to_the_next_login() {
        assert_eq!(
            after(exited(i32::from(crate::daemon::BUS_LOST))),
            Next::Stop
        );
    }

    #[test]
    fn the_signals_systemd_calls_clean_are_not() {
        for signal in [1, 2, 13, 15] {
            assert_eq!(after(killed(signal)), Next::Stop, "signal {signal}");
        }
    }
}
