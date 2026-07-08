// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>

//! Process plumbing for `BorgCli`: environment, the version probe, and the
//! stderr-reader task that turns `--log-json` output into a [`JobStream`].

use std::path::Path;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::engine::borg::classify::{classify, ErrLine};
use crate::engine::borg::logjson::{parse_log_line, Parsed};
use crate::engine::{EngineError, JobEvent, JobStream, JobSummary, LogLevel, Result};

/// The minimum supported Borg version.
pub(super) const BORG_FLOOR: (u32, u32) = (1, 2);

/// Parse the `borg --version` banner ("borg 1.4.0") into `(major, minor)`.
pub(super) fn parse_borg_version(s: &str) -> Option<(u32, u32)> {
    let ver = s.split_whitespace().nth(1)?;
    let mut parts = ver.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    // Minor may carry a suffix (e.g. "0b14"); take the leading digits.
    let minor_str = parts.next()?;
    let digits: String = minor_str
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let minor: u32 = digits.parse().ok()?;
    Some((major, minor))
}

/// Is this `(major, minor)` at or above the floor?
pub(super) fn meets_floor(v: (u32, u32)) -> bool {
    v >= BORG_FLOOR
}

/// Run `borg --version` and enforce the floor.
pub(super) async fn probe_version(bin: &Path) -> Result<()> {
    let out = Command::new(bin)
        .arg("--version")
        .output()
        .await
        .map_err(|_| EngineError::BorgMissing {
            needed: ">=1.2".into(),
            found: None,
        })?;
    let banner = String::from_utf8_lossy(&out.stdout);
    match parse_borg_version(&banner) {
        Some(v) if meets_floor(v) => Ok(()),
        found => Err(EngineError::BorgMissing {
            needed: ">=1.2".into(),
            found: found.map(|(a, b)| format!("{a}.{b}")),
        }),
    }
}

/// Common environment for every borg invocation. The repository itself is not
/// passed here — each subcommand's args embed it (plain repo path, or
/// `repo::archive`), since some commands (e.g. `key export`) take it directly
/// while others need the `::archive` suffix.
pub(super) fn base_command(bin: &Path, passphrase: &str) -> Command {
    let mut cmd = Command::new(bin);
    cmd.env("BORG_PASSPHRASE", passphrase)
        .env("BORG_RELOCATED_REPO_ACCESS_IS_OK", "no")
        .env("BORG_EXIT_CODES", "modern")
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8")
        .arg("--log-json");
    cmd
}

/// Spawn a job-style borg command (progress on stderr) and return its stream.
/// Stdout is discarded; the stderr reader forwards events and, on exit,
/// classifies failure into the terminal [`JobEvent::Finished`].
pub(super) fn spawn_streamed(mut cmd: Command) -> Result<JobStream> {
    cmd.stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| EngineError::BorgFailed {
        code: -1,
        stderr: format!("spawning borg: {e}"),
    })?;
    let stderr = child.stderr.take().expect("stderr piped");

    let (tx, rx) = mpsc::channel(64);
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();

    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut errbuf: Vec<ErrLine> = Vec::new();
        let mut cancelled = false;
        loop {
            tokio::select! {
                _ = task_cancel.cancelled() => {
                    cancelled = true;
                    let _ = child.start_kill();
                    break;
                }
                next = lines.next_line() => match next {
                    Ok(Some(line)) => forward_line(&line, &tx, &mut errbuf).await,
                    Ok(None) | Err(_) => break,
                }
            }
        }
        // Drain any remaining lines after the child closes stderr (non-cancel path).
        if !cancelled {
            while let Ok(Some(line)) = lines.next_line().await {
                forward_line(&line, &tx, &mut errbuf).await;
            }
        }
        let result = match child.wait().await {
            Ok(status) if status.success() && !cancelled => Ok(JobSummary::default()),
            Ok(status) => Err(classify(status.code().unwrap_or(-1), &errbuf)),
            Err(e) => Err(EngineError::BorgFailed {
                code: -1,
                stderr: e.to_string(),
            }),
        };
        let _ = tx.send(JobEvent::Finished(result)).await;
    });

    Ok(JobStream::new(rx, cancel))
}

async fn forward_line(line: &str, tx: &mpsc::Sender<JobEvent>, errbuf: &mut Vec<ErrLine>) {
    let event = match parse_log_line(line) {
        Parsed::Progress {
            current,
            total,
            phase,
        } => JobEvent::Progress {
            current,
            total,
            phase,
        },
        Parsed::ItemDone { path } => JobEvent::ItemDone { path },
        Parsed::Log {
            level,
            msgid,
            message,
        } => {
            if level == LogLevel::Error {
                errbuf.push(ErrLine {
                    msgid,
                    message: message.clone(),
                });
            }
            JobEvent::Log {
                level,
                msg: message,
            }
        }
        Parsed::Ignore => return,
    };
    let _ = tx.send(event).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_borg_version_banner() {
        assert_eq!(parse_borg_version("borg 1.2.8"), Some((1, 2)));
        assert_eq!(parse_borg_version("borg 1.4.0\n"), Some((1, 4)));
        assert_eq!(parse_borg_version("borg 2.0.0b14"), Some((2, 0)));
        assert_eq!(parse_borg_version("garbage"), None);
    }

    #[test]
    fn version_floor_rejects_pre_1_2() {
        assert!(meets_floor((1, 2)));
        assert!(meets_floor((1, 4)));
        assert!(meets_floor((2, 0)));
        assert!(!meets_floor((1, 1)));
        assert!(!meets_floor((0, 9)));
    }
}
