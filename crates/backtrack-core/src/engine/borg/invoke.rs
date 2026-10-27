// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Process plumbing for `BorgCli`: environment, the version probe, and the
//! stderr-reader task that turns `--log-json` output into a [`JobStream`].

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::Stdio;
use std::task::{ready, Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader, ReadBuf};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::engine::borg::classify::{classify, classify_exit, is_evidence, ErrLine, ExitClass};
use crate::engine::borg::logjson::{parse_log_line, Parsed};
use crate::engine::{EngineError, JobEvent, JobStream, JobSummary, Result};

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

/// Grace period for borg to checkpoint and exit after SIGTERM before we SIGKILL.
const CANCEL_GRACE: Duration = Duration::from_secs(10);

/// Ask the borg child to terminate gracefully (SIGTERM) so it can write a
/// checkpoint and release the repository lock. Best-effort: a missing pid
/// means the child has already exited.
fn send_sigterm(child: &Child) {
    if let Some(pid) = child.id() {
        if let Some(pid) = rustix::process::Pid::from_raw(pid as i32) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        }
    }
}

/// A Borg let go of while it still runs: asked to stop with SIGTERM rather
/// than killed outright, and killed only if it has not gone after a grace
/// period.
///
/// Borg gives up the repository's lock as it stops. Killed while taking or
/// giving up the lock (which it does even to read, briefly holding
/// `lock.exclusive` to edit the roster), it leaves the lock behind, and every
/// Borg after it is refused as LockedByOther until the lock is broken. A
/// listing read only part way, or a preview nobody wants any more, lets go of
/// its Borg this way; a job stops its own (see [`spawn`]).
pub(super) struct Stopping {
    child: Option<Child>,
    grace: Duration,
}

impl Stopping {
    /// `child`, which must have been spawned without `kill_on_drop`.
    pub(super) fn new(child: Child) -> Stopping {
        Stopping::with_grace(child, CANCEL_GRACE)
    }

    fn with_grace(child: Child, grace: Duration) -> Stopping {
        Stopping {
            child: Some(child),
            grace,
        }
    }

    pub(super) fn child(&mut self) -> &mut Child {
        self.child.as_mut().expect("held until dropped")
    }
}

impl Drop for Stopping {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        send_sigterm(&child);
        let grace = self.grace;
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(async move {
                    if tokio::time::timeout(grace, child.wait()).await.is_err() {
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                    }
                });
            }
            // No runtime left to wait for it on: killing it is all there is.
            Err(_) => {
                let _ = child.start_kill();
            }
        }
    }
}

/// Spawn a job-style borg command (progress on stderr) and return its stream.
/// Stdout is discarded; the stderr reader forwards events and, on exit,
/// classifies failure into the terminal [`JobEvent::Finished`].
pub(super) fn spawn_streamed(cmd: Command) -> Result<JobStream> {
    spawn(cmd, Warnings::Tolerated)
}

/// [`spawn_streamed`] for `borg check`, which reports damage by exiting in
/// the *warning* band: a repository with a corrupt segment exits 1 with
/// "errors found", even under `BORG_EXIT_CODES=modern`. Read as every other
/// command's warnings are, that is a clean bill of health for a damaged
/// repository.
pub(super) fn spawn_check(cmd: Command) -> Result<JobStream> {
    spawn(cmd, Warnings::MeanDamage)
}

/// What a warning exit means for the command that produced it.
#[derive(Clone, Copy)]
enum Warnings {
    /// The command reached its normal end; see [`classify_exit`].
    Tolerated,
    /// The command found something wrong with the repository.
    MeanDamage,
}

fn spawn(mut cmd: Command, warnings: Warnings) -> Result<JobStream> {
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
                    send_sigterm(&child);
                    break;
                }
                next = lines.next_line() => match next {
                    Ok(Some(line)) => forward_line(&line, &tx, &mut errbuf, &task_cancel).await,
                    Ok(None) | Err(_) => break,
                }
            }
        }
        let result = if cancelled {
            // Give borg a grace period to checkpoint and exit after SIGTERM,
            // then escalate to SIGKILL. The outcome is Cancelled regardless of
            // exit status, so a cancel never reads as a borg failure.
            if tokio::time::timeout(CANCEL_GRACE, child.wait())
                .await
                .is_err()
            {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
            Err(EngineError::Cancelled)
        } else {
            // Drain any remaining lines after the child closes stderr.
            while let Ok(Some(line)) = lines.next_line().await {
                forward_line(&line, &tx, &mut errbuf, &task_cancel).await;
            }
            match child.wait().await {
                Ok(status) => {
                    let code = status.code().unwrap_or(-1);
                    match classify_exit(code) {
                        ExitClass::Success => Ok(JobSummary::default()),
                        // The operation reached its normal end. The individual
                        // warnings have already gone out as `JobEvent::Log`, so
                        // this line is only the fact that the run as a whole
                        // ended in the warning band.
                        ExitClass::Warning => match warnings {
                            Warnings::Tolerated => {
                                tracing::info!(code, "borg finished with warnings");
                                Ok(JobSummary::default())
                            }
                            Warnings::MeanDamage => Err(EngineError::RepoCorrupt),
                        },
                        ExitClass::Error => Err(classify(code, &errbuf)),
                    }
                }
                Err(e) => Err(EngineError::BorgFailed {
                    code: -1,
                    stderr: e.to_string(),
                }),
            }
        };
        let _ = tx.send(JobEvent::Finished(result)).await;
    });

    Ok(JobStream::new(rx, cancel))
}

/// Spawn a `borg extract --stdout` and return a reader of the file it writes.
///
/// The reader ends cleanly only if Borg says the extraction succeeded. When
/// Borg fails (the repository unreachable, the archive not there, the
/// passphrase wrong) it reports on stderr and simply closes stdout, which on
/// its own reads exactly like a complete, empty file. So the end of stdout is
/// not the end of the read: the reader waits for Borg to exit, and a failure
/// becomes the read's error, carrying the classified [`EngineError`] for
/// [`EngineError::from_io`] to recover.
pub(super) fn spawn_stdout(mut cmd: Command) -> Result<Pin<Box<dyn AsyncRead + Send>>> {
    // Stopped by `Stopping` if the reader is dropped early, not killed.
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| EngineError::BorgFailed {
        code: -1,
        stderr: format!("spawning borg extract --stdout: {e}"),
    })?;
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let mut child = Stopping::new(child);

    // Read while the file streams rather than after it: a Borg with a full
    // stderr pipe would wait on us while we waited on its stdout.
    let errors = tokio::spawn(async move {
        let mut errors = Vec::new();
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Parsed::Log {
                level,
                msgid,
                message,
            } = parse_log_line(&line)
            {
                if is_evidence(level, &message) {
                    errors.push(ErrLine { msgid, message });
                }
            }
        }
        errors
    });
    let verdict = async move {
        let errors = errors.await.unwrap_or_default();
        let status = child
            .child()
            .wait()
            .await
            .map_err(|e| EngineError::BorgFailed {
                code: -1,
                stderr: e.to_string(),
            })?;
        let code = status.code().unwrap_or(-1);
        match classify_exit(code) {
            // A warning is Borg reaching its normal end, as for every other
            // command that tolerates them.
            ExitClass::Success | ExitClass::Warning => Ok(()),
            ExitClass::Error => Err(classify(code, &errors)),
        }
    };
    Ok(Box::pin(BorgStdout {
        stdout,
        verdict: Some(Box::pin(verdict)),
    }))
}

/// The reader [`spawn_stdout`] returns.
struct BorgStdout {
    stdout: tokio::process::ChildStdout,
    /// Borg's exit, awaited once stdout has ended, and `None` once it has
    /// been. It owns the child, so a reader dropped early still has Borg
    /// stopped.
    verdict: Option<Pin<Box<dyn Future<Output = Result<()>> + Send>>>,
}

impl AsyncRead for BorgStdout {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let before = buf.filled().len();
        ready!(Pin::new(&mut this.stdout).poll_read(cx, buf))?;
        if buf.filled().len() > before {
            return Poll::Ready(Ok(()));
        }
        // Stdout has ended. Whether that is the whole file is Borg's to say.
        let Some(verdict) = this.verdict.as_mut() else {
            return Poll::Ready(Ok(()));
        };
        let outcome = ready!(verdict.as_mut().poll(cx));
        this.verdict = None;
        Poll::Ready(outcome.map_err(std::io::Error::other))
    }
}

async fn forward_line(
    line: &str,
    tx: &mpsc::Sender<JobEvent>,
    errbuf: &mut Vec<ErrLine>,
    cancel: &CancellationToken,
) {
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
            if is_evidence(level, &message) {
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
    // Observe cancellation even while the channel is full, so a cancelled
    // job's reader never blocks indefinitely on backpressure (mirrors
    // JobStream::from_events).
    tokio::select! {
        _ = cancel.cancelled() => {}
        _ = tx.send(event) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for a Borg in the middle of something: it runs `trap` for
    /// SIGTERM, says it is ready, and goes on until stopped.
    async fn running(trap: &str, marker: &Path) -> Child {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(format!("{trap}; echo ready; while :; do sleep 0.05; done"))
            .env("MARKER", marker)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            // Not the test's own: a stand-in left running by a failing test
            // would hold the test runner's output open.
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut ready = BufReader::new(child.stdout.take().unwrap()).lines();
        assert_eq!(ready.next_line().await.unwrap().as_deref(), Some("ready"));
        child
    }

    fn alive(pid: u32) -> bool {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }

    async fn until(what: impl Fn() -> bool) -> bool {
        for _ in 0..100 {
            if what() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    #[tokio::test]
    async fn a_borg_let_go_of_is_asked_to_stop_rather_than_killed() {
        // Stopped this way, Borg gives up the repository's lock; killed, it
        // can leave the lock behind for every Borg after it.
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("stopped");
        let child = running(r#"trap 'echo asked > "$MARKER"; exit 0' TERM"#, &marker).await;
        drop(Stopping::new(child));
        assert!(
            until(|| marker.exists()).await,
            "the Borg was sent SIGTERM, which it could act on"
        );
    }

    #[tokio::test]
    async fn one_that_will_not_stop_is_killed_once_the_grace_is_up() {
        /// Kills the stand-in however the test ends, a failure included.
        struct Reap(u32);
        impl Drop for Reap {
            fn drop(&mut self) {
                if let Some(pid) = rustix::process::Pid::from_raw(self.0 as i32) {
                    let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
                }
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let child = running("trap '' TERM", &dir.path().join("unused")).await;
        let pid = child.id().unwrap();
        let _reap = Reap(pid);
        drop(Stopping::with_grace(child, Duration::from_millis(300)));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(alive(pid), "given its grace first");
        assert!(until(|| !alive(pid)).await, "and then killed");
    }

    #[tokio::test]
    async fn one_that_has_finished_is_left_alone() {
        let mut child = Command::new("true").spawn().unwrap();
        let _ = child.wait().await;
        let mut stopping = Stopping::new(child);
        assert!(stopping.child().try_wait().unwrap().is_some());
        drop(stopping);
    }

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
