// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>

//! The real [`BackupEngine`](super::BackupEngine): `BorgCli`. Spawns
//! `borg --log-json` and parses its stderr JSONL into [`JobEvent`](super::JobEvent)s.

mod classify;
mod invoke;
mod logjson;

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;

use crate::engine::{
    ArchiveId, BackupEngine, CheckLevel, CreateSpec, EngineError, JobEvent, JobStream, PrunePolicy,
    RepoInfo, RepoSpec, Result,
};
use crate::index::BorgItem;
use crate::secret::SecretStore;

use invoke::{base_command, probe_version, spawn_streamed};

/// The v1 backup engine: drives the `borg` CLI (1.2/1.4).
pub struct BorgCli {
    repo: String,
    repo_id: String,
    secrets: Arc<dyn SecretStore>,
    bin: PathBuf,
}

impl BorgCli {
    /// Construct against `repo` (local path / `ssh://` / mounted share), keyed by
    /// `repo_id` for passphrase lookup. Probes `borg --version` and enforces the
    /// `>=1.2` floor.
    pub async fn new(
        repo: String,
        repo_id: String,
        secrets: Arc<dyn SecretStore>,
    ) -> Result<BorgCli> {
        let bin = PathBuf::from("borg");
        probe_version(&bin).await?;
        Ok(BorgCli {
            repo,
            repo_id,
            secrets,
            bin,
        })
    }

    /// A borg command with the passphrase + environment applied.
    async fn cmd(&self) -> Result<Command> {
        let pass = self.secrets.get(&self.repo_id).await?;
        Ok(base_command(&self.bin, &pass))
    }

    fn archive_ref(&self, id: &ArchiveId) -> String {
        format!("{}::{}", self.repo, id.0)
    }
}

#[async_trait]
impl BackupEngine for BorgCli {
    async fn init_repo(&self, spec: &RepoSpec) -> Result<()> {
        let mut cmd = self.cmd().await?;
        cmd.arg("init")
            .arg("--encryption")
            .arg(spec.encryption.as_borg_arg())
            .arg(&spec.path);
        // init is short; run it as a streamed job and drain to its outcome.
        drain_to_result(spawn_streamed(cmd)?).await
    }

    async fn repo_info(&self) -> Result<RepoInfo> {
        let mut cmd = self.cmd().await?;
        cmd.arg("info").arg("--json").arg(&self.repo);
        let out = run_json(cmd).await?;
        let repository_id = out
            .get("repository")
            .and_then(|r| r.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let archive_count = out
            .get("archives")
            .and_then(|a| a.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        Ok(RepoInfo {
            repository_id,
            archive_count,
        })
    }

    async fn key_export(&self) -> Result<String> {
        let mut cmd = self.cmd().await?;
        cmd.arg("key").arg("export").arg(&self.repo); // to stdout
        run_stdout_string(cmd).await
    }

    async fn create(&self, spec: &CreateSpec) -> Result<JobStream> {
        let mut cmd = self.cmd().await?;
        cmd.arg("create")
            .arg("--json")
            .arg("--list")
            .arg("--filter")
            .arg("AME")
            .arg("--compression")
            .arg(spec.compression.as_borg_arg());
        if spec.one_file_system {
            cmd.arg("--one-file-system");
        }
        for ex in &spec.excludes {
            cmd.arg("--exclude").arg(ex);
        }
        cmd.arg(format!("{}::{}", self.repo, spec.archive_name));
        for src in &spec.sources {
            cmd.arg(src);
        }
        spawn_streamed(cmd)
    }

    async fn list_archive(&self, id: &ArchiveId) -> Result<BoxStream<'static, Result<BorgItem>>> {
        let mut cmd = self.cmd().await?;
        cmd.arg("list")
            .arg("--json-lines")
            .arg(self.archive_ref(id));
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| EngineError::BorgFailed {
            code: -1,
            stderr: format!("spawning borg list: {e}"),
        })?;
        let stdout = child.stdout.take().expect("stdout piped");
        let lines = BufReader::new(stdout).lines();

        // Own the child in the stream state so it lives as long as the stream
        // (kill_on_drop reaps it if the consumer drops early).
        let stream = stream::unfold((lines, child), |(mut lines, mut child)| async move {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let item =
                        BorgItem::from_json_line(&line).map_err(|e| EngineError::BorgFailed {
                            code: -1,
                            stderr: format!("parsing borg list line: {e}"),
                        });
                    Some((item, (lines, child)))
                }
                Ok(None) => {
                    let _ = child.wait().await;
                    None
                }
                Err(e) => Some((
                    Err(EngineError::BorgFailed {
                        code: -1,
                        stderr: e.to_string(),
                    }),
                    (lines, child),
                )),
            }
        });
        Ok(stream.boxed())
    }

    async fn extract(&self, id: &ArchiveId, paths: &[String], dest: &Path) -> Result<JobStream> {
        let mut cmd = self.cmd().await?;
        cmd.current_dir(dest)
            .arg("extract")
            .arg("--list")
            .arg(self.archive_ref(id));
        for p in paths {
            cmd.arg(p);
        }
        spawn_streamed(cmd)
    }

    async fn extract_stdout(
        &self,
        id: &ArchiveId,
        path: &str,
    ) -> Result<Pin<Box<dyn AsyncRead + Send>>> {
        let mut cmd = self.cmd().await?;
        cmd.arg("extract")
            .arg("--stdout")
            .arg(self.archive_ref(id))
            .arg(path)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| EngineError::BorgFailed {
            code: -1,
            stderr: format!("spawning borg extract --stdout: {e}"),
        })?;
        let stdout = child.stdout.take().expect("stdout piped");
        // Keep the child alive alongside its stdout; kill_on_drop reaps it.
        Ok(Box::pin(ChildStdoutReader {
            _child: child,
            stdout,
        }))
    }

    async fn prune(&self, policy: &PrunePolicy) -> Result<JobStream> {
        let mut cmd = self.cmd().await?;
        cmd.arg("prune")
            .arg("--list")
            .arg(format!("--keep-hourly={}", policy.keep_hourly))
            .arg(format!("--keep-daily={}", policy.keep_daily))
            .arg(format!("--keep-weekly={}", policy.keep_weekly))
            .arg(format!("--keep-monthly={}", policy.keep_monthly))
            .arg(&self.repo);
        spawn_streamed(cmd)
    }

    async fn compact(&self) -> Result<JobStream> {
        let mut cmd = self.cmd().await?;
        cmd.arg("compact").arg(&self.repo);
        spawn_streamed(cmd)
    }

    async fn check(&self, level: CheckLevel) -> Result<JobStream> {
        let mut cmd = self.cmd().await?;
        cmd.arg("check");
        match level {
            CheckLevel::Repository => {
                cmd.arg("--repository-only");
            }
            CheckLevel::Archives => {
                cmd.arg("--archives-only");
            }
            CheckLevel::Full => {}
        }
        cmd.arg(&self.repo);
        spawn_streamed(cmd)
    }
}

/// A reader that streams a borg-extracted file and keeps the child alive.
struct ChildStdoutReader {
    _child: tokio::process::Child,
    stdout: tokio::process::ChildStdout,
}

impl AsyncRead for ChildStdoutReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdout).poll_read(cx, buf)
    }
}

/// Drain a short job stream to its terminal outcome (used by `init_repo`).
async fn drain_to_result(mut stream: JobStream) -> Result<()> {
    let mut outcome = Err(EngineError::BorgFailed {
        code: -1,
        stderr: "no outcome".into(),
    });
    while let Some(ev) = stream.next().await {
        if let JobEvent::Finished(r) = ev {
            outcome = r.map(|_| ());
        }
    }
    outcome
}

/// Run a borg command that prints one JSON document to stdout; parse it.
async fn run_json(mut cmd: Command) -> Result<serde_json::Value> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = cmd.output().await.map_err(|e| EngineError::BorgFailed {
        code: -1,
        stderr: format!("running borg: {e}"),
    })?;
    if !out.status.success() {
        return Err(classify::classify(
            out.status.code().unwrap_or(-1),
            &collect_json_errors(&out.stderr),
        ));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| EngineError::BorgFailed {
        code: -1,
        stderr: format!("parsing borg --json: {e}"),
    })
}

/// Run a borg command and return its stdout as a UTF-8 string (e.g. key export).
async fn run_stdout_string(mut cmd: Command) -> Result<String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = cmd.output().await.map_err(|e| EngineError::BorgFailed {
        code: -1,
        stderr: format!("running borg: {e}"),
    })?;
    if !out.status.success() {
        return Err(classify::classify(
            out.status.code().unwrap_or(-1),
            &collect_json_errors(&out.stderr),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Extract error-level lines from captured `--log-json` stderr, for the
/// non-streaming (`output()`) commands.
fn collect_json_errors(stderr: &[u8]) -> Vec<classify::ErrLine> {
    use crate::engine::LogLevel;
    use logjson::{parse_log_line, Parsed};
    String::from_utf8_lossy(stderr)
        .lines()
        .filter_map(|l| match parse_log_line(l) {
            Parsed::Log {
                level: LogLevel::Error,
                msgid,
                message,
            } => Some(classify::ErrLine { msgid, message }),
            _ => None,
        })
        .collect()
}
