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

use crate::engine::EngineError;

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
