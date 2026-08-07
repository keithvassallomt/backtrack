// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The shipped systemd and D-Bus units, checked against the code.
//!
//! Activation only works when four separate things agree: the bus name the
//! daemon claims, the `BusName=` systemd waits for, the `Name=` the bus
//! activates, and the `SystemdService=` that names the unit. Nothing in the
//! build catches a disagreement — the units are data, and the failure shows up
//! as a client hanging on its first method call against a daemon that never
//! starts.
//!
//! So the agreement is asserted here instead.

/// The packaged systemd user unit.
const SYSTEMD_UNIT: &str = include_str!("../../../packaging/systemd/backtrackd.service");

/// The packaged D-Bus activation file.
const DBUS_SERVICE: &str = include_str!("../../../packaging/dbus/org.backtrack.Daemon1.service");

/// The unit's file name, which `SystemdService=` has to match.
const SYSTEMD_UNIT_NAME: &str = "backtrackd.service";

#[cfg(test)]
mod tests {
    use super::*;
    use backtrack_core::dbus;

    /// Read `key=value` from an ini-style file, first match wins.
    fn value(text: &str, key: &str) -> Option<String> {
        text.lines()
            .map(str::trim)
            .filter(|line| !line.starts_with('#'))
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .map(|v| v.trim().to_string())
    }

    #[test]
    fn the_unit_waits_for_the_name_the_daemon_claims() {
        assert_eq!(
            value(SYSTEMD_UNIT, "BusName").as_deref(),
            Some(dbus::BUS_NAME),
            "systemd waits for BusName before calling the service started; if it \
             does not match what the daemon claims, activation hangs until the \
             start timeout and the unit is reported failed"
        );
    }

    #[test]
    fn the_activation_file_names_the_same_service() {
        assert_eq!(
            value(DBUS_SERVICE, "Name").as_deref(),
            Some(dbus::BUS_NAME),
            "the bus activates whatever Name says; a mismatch means the first \
             method call starts nothing at all"
        );
    }

    #[test]
    fn activation_hands_off_to_systemd_rather_than_forking_its_own_daemon() {
        // Without SystemdService the bus would spawn the binary itself, outside
        // the unit — two supervisors for one daemon, and `systemctl --user
        // status` describing a process that is not the one running.
        assert_eq!(
            value(DBUS_SERVICE, "SystemdService").as_deref(),
            Some(SYSTEMD_UNIT_NAME),
        );
    }

    #[test]
    fn both_files_point_at_the_same_binary() {
        let exec_start = value(SYSTEMD_UNIT, "ExecStart").expect("ExecStart present");
        let exec = value(DBUS_SERVICE, "Exec").expect("Exec present");
        assert_eq!(
            exec_start, exec,
            "the unit and the activation file must launch the same binary"
        );
        assert!(
            exec_start.starts_with('/'),
            "an activation Exec must be absolute: {exec_start}"
        );
    }

    #[test]
    fn the_service_is_dbus_activated_not_merely_started() {
        assert_eq!(
            value(SYSTEMD_UNIT, "Type").as_deref(),
            Some("dbus"),
            "Type=dbus is what makes a client's first call wait for a daemon \
             that is genuinely ready, rather than racing one that is still \
             starting"
        );
    }

    #[test]
    fn the_daemon_starts_at_login_as_well_as_on_demand() {
        // Activation alone would mean no backups until something asked for one,
        // which for an hourly backup tool is the same as no backups.
        assert_eq!(
            value(SYSTEMD_UNIT, "WantedBy").as_deref(),
            Some("default.target")
        );
    }

    #[test]
    fn a_clean_stop_is_given_enough_time_to_finish() {
        let timeout = value(SYSTEMD_UNIT, "TimeoutStopSec").expect("TimeoutStopSec present");
        assert_eq!(
            value(SYSTEMD_UNIT, "KillSignal").as_deref(),
            Some("SIGTERM")
        );
        assert!(
            timeout.ends_with('s')
                && timeout.trim_end_matches('s').parse::<u32>().unwrap_or(0) >= 30,
            "a running backup needs room to checkpoint before it is killed, got {timeout}"
        );
    }

    #[test]
    fn the_sandbox_does_not_block_the_daemons_purpose() {
        // A backup daemon reads the whole home directory and restores to
        // anywhere the user can write. These would each break that, so their
        // absence is deliberate and worth pinning.
        for hostile in [
            "ProtectHome",
            "ProtectSystem",
            "ReadOnlyPaths",
            "PrivateUsers",
        ] {
            assert!(
                value(SYSTEMD_UNIT, hostile).is_none(),
                "{hostile} would stop the daemon reading or restoring the user's \
                 files; if it is ever added, it needs an explicit carve-out"
            );
        }
        assert_eq!(
            value(SYSTEMD_UNIT, "NoNewPrivileges").as_deref(),
            Some("yes"),
            "this one costs nothing and is worth keeping"
        );
    }

    #[test]
    fn restarts_are_bounded_and_only_follow_real_failures() {
        // Losing the single-instance race exits 0, so Restart=always would
        // relaunch a daemon that correctly decided not to run.
        assert_eq!(
            value(SYSTEMD_UNIT, "Restart").as_deref(),
            Some("on-failure")
        );
        assert!(value(SYSTEMD_UNIT, "RestartSec").is_some());
    }
}
