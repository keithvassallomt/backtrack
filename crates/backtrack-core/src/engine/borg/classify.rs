// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>

//! Map a finished Borg process (exit code + captured error lines) to an
//! [`EngineError`]. Precedence: specific `msgid`/message patterns first, then a
//! `BorgFailed` fallback. The table below is the documented mapping; keep it in
//! sync with health.md.
//!
//! | Signal (msgid or message substring) | EngineError |
//! |---|---|
//! | `Repository.DoesNotExist`, "does not exist", ssh "No route to host"/"Connection refused"/"Connection closed"/"Network is unreachable"/"Could not resolve hostname" | `RepoUnreachable` |
//! | "passphrase … is incorrect"/"wrong passphrase"/`PassphraseWrong` | `PassphraseWrong` |
//! | ssh "Permission denied"/"Authentication failed"/"Host key verification failed" | `AuthFailed` |
//! | "No space left on device"/ENOSPC | `DestinationFull` |
//! | `LockTimeout`, "Failed to create/acquire the lock" | `LockedByOther` |
//! | `Repository.CheckNeeded`, "Inconsistency detected"/"Data integrity error"/"manifest" | `RepoCorrupt` |
//! | anything else with a non-zero code | `BorgFailed { code, stderr }` |

use crate::engine::EngineError;

/// A captured error-level line from Borg's `--log-json` stream.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrLine {
    pub msgid: Option<String>,
    pub message: String,
}

fn any(errors: &[ErrLine], needles: &[&str]) -> bool {
    errors.iter().any(|e| {
        needles.iter().any(|n| {
            e.message.to_lowercase().contains(&n.to_lowercase())
                || e.msgid
                    .as_deref()
                    .map(|m| m.eq_ignore_ascii_case(n))
                    .unwrap_or(false)
        })
    })
}

/// Classify a failed Borg invocation. `code` is the process exit code; `errors`
/// are the error-level `log_message` lines captured from stderr.
#[cfg_attr(not(test), allow(dead_code))]
pub fn classify(code: i32, errors: &[ErrLine]) -> EngineError {
    // Order matters: check the most specific signals before the generic fallback.
    if any(
        errors,
        &[
            "PassphraseWrong",
            "passphrase supplied",
            "is incorrect",
            "wrong passphrase",
        ],
    ) {
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
            "does not exist",
            "No route to host",
            "Connection refused",
            "Connection closed",
            "Network is unreachable",
            "Could not resolve hostname",
        ],
    ) {
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
            "manifest",
        ],
    ) {
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
}
