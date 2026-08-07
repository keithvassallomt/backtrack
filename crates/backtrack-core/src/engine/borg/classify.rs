// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Map a finished Borg process (exit code + captured error lines) to an
//! [`EngineError`]. Precedence: specific `msgid`/message patterns first, then a
//! `BorgFailed` fallback. Broad English fragments ("is incorrect", "does not
//! exist", "manifest") are matched only when they CO-OCCUR with a
//! domain-specific token on the same line, so an unrelated borg message cannot
//! be misclassified.
//!
//! | Signal | EngineError |
//! |---|---|
//! | msgid `PassphraseWrong`; a line with both "passphrase" and "is incorrect"; or "wrong passphrase" | `PassphraseWrong` |
//! | "Permission denied", "Authentication failed", "Host key verification failed" | `AuthFailed` |
//! | msgid `Repository.DoesNotExist`; a line with both "repository" and "does not exist"; or ssh "No route to host"/"Connection refused"/"Connection closed"/"Network is unreachable"/"Could not resolve hostname" | `RepoUnreachable` |
//! | "No space left on device", "Errno 28" | `DestinationFull` |
//! | msgid `LockTimeout`, "Failed to create/acquire the lock" | `LockedByOther` |
//! | msgid `Repository.CheckNeeded`; "Inconsistency detected"; "Data integrity error"; a line with both "manifest" and "corrupt" | `RepoCorrupt` |
//! | anything else with a non-zero code | `BorgFailed { code, stderr }` |
//!
//! Classification only applies to codes that mean *failure*. Which those are is
//! [`classify_exit`]'s job, and getting it wrong is expensive — see there.

use crate::engine::EngineError;

/// Borg's first specific-warning exit code. Codes from here to
/// [`SIGNAL_BASE`] are warnings under `BORG_EXIT_CODES=modern`.
const WARNING_BASE: i32 = 100;

/// `128 + N` means the process was killed by signal `N`.
const SIGNAL_BASE: i32 = 128;

/// What a finished Borg process's exit code means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitClass {
    /// Ran to its normal end with nothing to report.
    Success,
    /// **Reached its normal end**, but something is worth mentioning — a file
    /// that vanished while being read, a permission that could not be restored.
    /// The archive exists and is complete for everything that still existed.
    Warning,
    /// Did not reach its normal end.
    Error,
}

/// Which band `code` falls into.
///
/// Borg documents these ranges in its manual and fixes them in its own
/// `EXIT_WARNING_BASE = 100` / `EXIT_SIGNAL_BASE = 128` constants:
///
/// | code | meaning |
/// |---|---|
/// | 0 | success |
/// | 1 | generic warning |
/// | 2..=99 | error (specific under `BORG_EXIT_CODES=modern`) |
/// | 100..=127 | specific warning (modern only) |
/// | 128+N | killed by signal N |
///
/// Treating the warning band as failure is not a cosmetic mistake, which is why
/// this is a function with its own tests rather than a `status.success()` call.
/// `BORG_EXIT_CODES=modern` is set on every invocation, so a single file
/// disappearing mid-backup — a browser cache file, an editor's swap file, a
/// download that finished and got moved — exits 107 (`BackupFileNotFoundError`)
/// with a complete archive written. Reading that as a failed backup means a
/// machine that is being protected perfectly well reports that it is not, and
/// since a failed job records no successful backup, it would drift to `AT_RISK`
/// and raise a banner over an archive that is sitting there intact.
///
/// It matters most to the offline spool, where it stops being an edge case: the
/// spool archives precisely the files that just changed, which are exactly the
/// files most likely to be mid-write or about to be deleted.
///
/// A signal death is an error. `128+N` is unreachable through
/// [`std::process::ExitStatus::code`], which reports `None` for a signalled
/// child, but the band is named here so nobody later reads 137 as a warning.
pub fn classify_exit(code: i32) -> ExitClass {
    match code {
        0 => ExitClass::Success,
        1 => ExitClass::Warning,
        c if (WARNING_BASE..SIGNAL_BASE).contains(&c) => ExitClass::Warning,
        _ => ExitClass::Error,
    }
}

/// A captured error-level line from Borg's `--log-json` stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrLine {
    pub msgid: Option<String>,
    pub message: String,
}

/// True if any error line matches any single needle: msgid equal
/// (case-insensitive) OR message contains it (case-insensitive).
fn any(errors: &[ErrLine], needles: &[&str]) -> bool {
    errors.iter().any(|e| {
        let msg = e.message.to_lowercase();
        needles.iter().any(|n| {
            msg.contains(&n.to_lowercase())
                || e.msgid
                    .as_deref()
                    .map(|m| m.eq_ignore_ascii_case(n))
                    .unwrap_or(false)
        })
    })
}

/// True if any single error line's message contains ALL of the given fragments
/// (case-insensitive). Requires a broad fragment to co-occur with a
/// domain-specific token on the same line.
fn any_line_with_all(errors: &[ErrLine], fragments: &[&str]) -> bool {
    errors.iter().any(|e| {
        let msg = e.message.to_lowercase();
        fragments.iter().all(|f| msg.contains(&f.to_lowercase()))
    })
}

/// Classify a failed Borg invocation. `code` is the process exit code; `errors`
/// are the error-level `log_message` lines captured from stderr.
pub fn classify(code: i32, errors: &[ErrLine]) -> EngineError {
    // Order matters: check the most specific signals before the generic fallback.
    if any(errors, &["PassphraseWrong", "wrong passphrase"])
        || any_line_with_all(errors, &["passphrase", "is incorrect"])
    {
        return EngineError::PassphraseWrong;
    }
    if any(
        errors,
        &[
            "Permission denied",
            "Authentication failed",
            "Host key verification failed",
        ],
    ) {
        return EngineError::AuthFailed;
    }
    if any(
        errors,
        &[
            "Repository.DoesNotExist",
            "No route to host",
            "Connection refused",
            "Connection closed",
            "Network is unreachable",
            "Could not resolve hostname",
        ],
    ) || any_line_with_all(errors, &["repository", "does not exist"])
    {
        return EngineError::RepoUnreachable;
    }
    if any(errors, &["No space left on device", "Errno 28"]) {
        return EngineError::DestinationFull;
    }
    if any(
        errors,
        &["LockTimeout", "Failed to create/acquire the lock"],
    ) {
        return EngineError::LockedByOther;
    }
    if any(
        errors,
        &[
            "Repository.CheckNeeded",
            "Inconsistency detected",
            "Data integrity error",
        ],
    ) || any_line_with_all(errors, &["manifest", "corrupt"])
    {
        return EngineError::RepoCorrupt;
    }
    let stderr = errors
        .iter()
        .map(|e| e.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    EngineError::BorgFailed { code, stderr }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineError;

    fn line(msgid: Option<&str>, msg: &str) -> ErrLine {
        ErrLine {
            msgid: msgid.map(str::to_string),
            message: msg.to_string(),
        }
    }

    #[test]
    fn repo_missing_maps_to_unreachable() {
        let errs = [line(
            Some("Repository.DoesNotExist"),
            "Repository /mnt/x does not exist.",
        )];
        assert_eq!(classify(2, &errs), EngineError::RepoUnreachable);
    }

    #[test]
    fn wrong_passphrase_by_message() {
        let errs = [line(
            None,
            "passphrase supplied in BORG_PASSPHRASE is incorrect.",
        )];
        assert_eq!(classify(2, &errs), EngineError::PassphraseWrong);
    }

    #[test]
    fn lock_timeout_maps_to_locked() {
        let errs = [line(
            Some("LockTimeout"),
            "Failed to create/acquire the lock",
        )];
        assert_eq!(classify(2, &errs), EngineError::LockedByOther);
    }

    #[test]
    fn check_needed_maps_to_corrupt() {
        let errs = [line(
            Some("Repository.CheckNeeded"),
            "Inconsistency detected.",
        )];
        assert_eq!(classify(2, &errs), EngineError::RepoCorrupt);
    }

    #[test]
    fn enospc_maps_to_destination_full() {
        let errs = [line(None, "[Errno 28] No space left on device")];
        assert_eq!(classify(2, &errs), EngineError::DestinationFull);
    }

    #[test]
    fn ssh_auth_maps_to_auth_failed() {
        let errs = [line(None, "Permission denied (publickey).")];
        assert_eq!(classify(2, &errs), EngineError::AuthFailed);
    }

    #[test]
    fn ssh_unreachable_maps_to_unreachable() {
        let errs = [line(
            None,
            "ssh: connect to host nas.local port 22: No route to host",
        )];
        assert_eq!(classify(2, &errs), EngineError::RepoUnreachable);
    }

    #[test]
    fn unrecognised_falls_back_to_borg_failed() {
        let errs = [line(None, "something weird happened")];
        assert_eq!(
            classify(2, &errs),
            EngineError::BorgFailed {
                code: 2,
                stderr: "something weird happened".into()
            }
        );
    }

    #[test]
    fn is_incorrect_without_passphrase_is_not_passphrase_wrong() {
        // A broad fragment alone must not misclassify an unrelated message.
        let errs = [line(None, "the archive name specified is incorrect")];
        assert_eq!(
            classify(2, &errs),
            EngineError::BorgFailed {
                code: 2,
                stderr: "the archive name specified is incorrect".into()
            }
        );
    }

    #[test]
    fn borgs_exit_code_bands_are_read_the_way_borg_documents_them() {
        assert_eq!(classify_exit(0), ExitClass::Success);
        // The generic warning, and the specific band `modern` opts into.
        assert_eq!(classify_exit(1), ExitClass::Warning);
        assert_eq!(classify_exit(100), ExitClass::Warning);
        assert_eq!(classify_exit(127), ExitClass::Warning);
        // Errors either side of the warning band.
        assert_eq!(classify_exit(2), ExitClass::Error);
        assert_eq!(classify_exit(99), ExitClass::Error);
        assert_eq!(classify_exit(128), ExitClass::Error, "killed by a signal");
        assert_eq!(classify_exit(137), ExitClass::Error, "kill -9");
        assert_eq!(classify_exit(-1), ExitClass::Error, "no code at all");
    }

    #[test]
    fn a_file_that_vanished_mid_backup_is_a_warning_not_a_failed_backup() {
        // Verified against borg 1.4.5: listing a path that no longer exists
        // writes the archive with everything that does exist and exits 107
        // (`BackupFileNotFoundError`, a WARNING-level message). Calling that a
        // failed backup would raise a health banner over an intact archive —
        // and on the offline spool, which archives exactly the files that just
        // changed, it would happen most hours.
        assert_eq!(classify_exit(107), ExitClass::Warning);
    }

    #[test]
    fn does_not_exist_without_repository_falls_through() {
        let errs = [line(None, "the requested path does not exist")];
        assert_eq!(
            classify(2, &errs),
            EngineError::BorgFailed {
                code: 2,
                stderr: "the requested path does not exist".into()
            }
        );
    }
}
