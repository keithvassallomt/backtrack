// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

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
    ArchiveId, BackupEngine, CheckLevel, CreateSpec, EngineError, JobEvent, JobStream, Presence,
    PrunePolicy, RepoInfo, RepoSpec, RepoStats, Result,
};
use crate::index::{ArchiveMeta, BorgItem};
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

    /// What is at this repository's location, before anything is created there.
    ///
    /// Asked of Borg rather than worked out from the filesystem, because the
    /// same question has to be answered for an SSH server, where there is no
    /// filesystem to look at, and some hosts (BorgBase among them) run nothing
    /// but `borg serve`.
    ///
    /// No passphrase is offered. An encrypted repository then refuses with
    /// `PassphraseWrong`, and that refusal is the answer: something is there.
    pub async fn presence(&self) -> Result<Presence> {
        let local = Path::new(&self.repo);
        if local.is_absolute() {
            if let Some(presence) = local_presence(local) {
                return Ok(presence);
            }
        }

        let mut cmd = base_command(&self.bin, "");
        // Nobody is at a terminal to answer a password or host-key question,
        // and ssh asks on the terminal rather than on stdin, so without this a
        // connection test from a shell would hang on a prompt nobody can see.
        // A person who has set their own BORG_RSH has already made this choice.
        if std::env::var_os("BORG_RSH").is_none() {
            cmd.env("BORG_RSH", "ssh -o BatchMode=yes -o ConnectTimeout=15");
        }
        cmd.arg("list").arg("--json").arg(&self.repo);
        cmd.stdout(Stdio::null()).stderr(Stdio::piped());
        let out = cmd.output().await.map_err(|e| EngineError::BorgFailed {
            code: -1,
            stderr: format!("running borg: {e}"),
        })?;
        let code = out.status.code().unwrap_or(-1);
        if classify::classify_exit(code) != classify::ExitClass::Error {
            // An unencrypted repository opens without a passphrase.
            return Ok(Presence::Existing);
        }

        let errors = collect_json_errors(&out.stderr);
        let said = |msgid: &str| errors.iter().any(|e| e.msgid.as_deref() == Some(msgid));
        if said("PassphraseWrong") {
            return Ok(Presence::Existing);
        }
        if said("Repository.DoesNotExist") {
            return Ok(Presence::Empty);
        }
        if said("Repository.InvalidRepository") {
            return Ok(Presence::Occupied);
        }
        // Whether the server refused us or could not be reached at all is in
        // ssh's own words, which Borg passes on as warnings rather than errors.
        let mut evidence = errors;
        evidence.extend(remote_warnings(&out.stderr));
        Err(classify::classify(code, &evidence))
    }
}

/// The answers about a local path that Borg gets wrong or cannot give.
///
/// Borg calls an empty folder "not a valid repository", which is true, but
/// `borg init` is perfectly happy to create one there, so for the question
/// being asked it is empty. And a folder nobody may write into is worth
/// refusing now rather than when the repository is created, which is several
/// questions later in the wizard.
fn local_presence(path: &Path) -> Option<Presence> {
    if let Ok(mut listing) = std::fs::read_dir(path) {
        if listing.next().is_none() {
            return Some(if writable(path) {
                Presence::Empty
            } else {
                Presence::Unwritable
            });
        }
        return None;
    }
    if path.exists() {
        return None;
    }
    // `borg init --make-parent-dirs` creates whatever is missing, so what
    // matters is the nearest folder that already exists.
    let existing = path.ancestors().skip(1).find(|p| p.is_dir())?;
    (!writable(existing)).then_some(Presence::Unwritable)
}

fn writable(dir: &Path) -> bool {
    rustix::fs::access(dir, rustix::fs::Access::WRITE_OK).is_ok()
}

/// The `Remote:` warnings in captured `--log-json` stderr: ssh speaking,
/// relayed by Borg, and the only place that says "Permission denied".
fn remote_warnings(stderr: &[u8]) -> Vec<classify::ErrLine> {
    use crate::engine::LogLevel;
    use logjson::{parse_log_line, Parsed};
    String::from_utf8_lossy(stderr)
        .lines()
        .filter_map(|l| match parse_log_line(l) {
            Parsed::Log {
                level: LogLevel::Warning,
                msgid,
                message,
            } if message.starts_with("Remote:") => Some(classify::ErrLine { msgid, message }),
            _ => None,
        })
        .collect()
}

#[async_trait]
impl BackupEngine for BorgCli {
    async fn init_repo(&self, spec: &RepoSpec) -> Result<()> {
        let mut cmd = self.cmd().await?;
        // --make-parent-dirs because the wizard places a repository in a
        // folder of its own on the chosen drive, and that folder is not
        // there yet the first time.
        cmd.arg("init")
            .arg("--make-parent-dirs")
            .arg("--encryption")
            .arg(spec.encryption.as_borg_arg())
            .arg(&spec.path);
        // init is short; run it as a streamed job and drain to its outcome.
        drain_to_result(spawn_streamed(cmd)?).await
    }

    async fn repo_info(&self) -> Result<RepoInfo> {
        // `borg list --json`, not `borg info --json`: the latter reports cache
        // and encryption detail but carries no archive list, so counting from it
        // reports zero for every repository, however full. Both carry the
        // repository id, so one invocation answers both questions.
        let mut cmd = self.cmd().await?;
        cmd.arg("list").arg("--json").arg(&self.repo);
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

    async fn change_passphrase(&self, old: &str, new: &str) -> Result<()> {
        // Both passphrases travel in the child's environment, as the
        // passphrase always does, and never on its command line, where any
        // other user of the machine could read them.
        let mut cmd = base_command(&self.bin, old);
        cmd.env("BORG_NEW_PASSPHRASE", new)
            .arg("key")
            .arg("change-passphrase")
            .arg(&self.repo);
        run_to_completion(cmd).await.map(|_| ())
    }

    async fn repo_stats(&self) -> Result<RepoStats> {
        let mut cmd = self.cmd().await?;
        cmd.arg("info").arg("--json").arg(&self.repo);
        let out = run_json(cmd).await?;
        let stat = |name: &str| {
            out.get("cache")
                .and_then(|c| c.get("stats"))
                .and_then(|s| s.get(name))
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
        };
        Ok(RepoStats {
            encryption: out
                .get("encryption")
                .and_then(|e| e.get("mode"))
                .and_then(|m| m.as_str())
                .unwrap_or_default()
                .to_string(),
            stored_bytes: stat("unique_csize"),
            original_bytes: stat("total_size"),
        })
    }

    async fn create(&self, spec: &CreateSpec) -> Result<JobStream> {
        let mut cmd = self.cmd().await?;
        cmd.arg("create")
            // Without --progress, borg emits no `archive_progress` messages at
            // all, so every progress bar in the application would sit still for
            // the whole backup.
            .arg("--progress")
            .arg("--json")
            .arg("--list")
            .arg("--filter")
            .arg("AME")
            .arg("--compression")
            .arg(spec.compression.as_borg_arg());
        if spec.one_file_system {
            cmd.arg("--one-file-system");
        }
        if let Some(limit) = spec.upload_limit_kib.filter(|kib| *kib > 0) {
            cmd.arg("--upload-ratelimit").arg(limit.to_string());
        }
        for ex in &spec.excludes {
            cmd.arg("--exclude").arg(ex);
        }
        cmd.arg(format!("{}::{}", self.repo, spec.archive_name));
        // An explicit list replaces the sources rather than adding to them: the
        // caller has already decided what this archive holds. Borg processes
        // exactly the paths given and does not recurse, which is why the walk
        // emits files and symlinks and never directories.
        let targets = if spec.paths.is_empty() {
            &spec.sources
        } else {
            &spec.paths
        };
        for target in targets {
            cmd.arg(target);
        }
        spawn_streamed(cmd)
    }

    async fn list_archives(&self) -> Result<Vec<ArchiveMeta>> {
        // `--format` rather than `--json`, for one field: `{time:%s}`. Borg's
        // JSON reports archive times as a naive ISO string in the machine's
        // *local* time, with no offset recorded, so reading it back costs a
        // timezone database and still guesses across a DST boundary. `%s` asks
        // Borg's own Python for the epoch seconds it already holds, which is
        // exact and needs nothing.
        //
        // The name goes last and the fields are tab-separated, because an
        // archive name is user-supplied text and an imported repository's could
        // contain anything — including a tab.
        let mut cmd = self.cmd().await?;
        cmd.arg("list")
            .arg("--format")
            .arg("{id}\t{time:%s}\t{time}\t{barchive}{NL}")
            .arg(&self.repo);
        let text = run_stdout_string(cmd).await?;
        Ok(text.lines().filter_map(parse_archive_line).collect())
    }

    async fn list_archive(&self, id: &ArchiveId) -> Result<BoxStream<'static, Result<BorgItem>>> {
        // `--format` rather than `--json-lines`, for the timestamp — the same
        // reason `list_archives` uses `{time:%s}`. Borg's JSON renders `mtime`
        // as naive local time with no offset recorded, so a catalogue built
        // from it holds every file's modification time shifted by the machine's
        // UTC offset. See `BorgItem::from_format_line`.
        let mut cmd = self.cmd().await?;
        cmd.arg("list")
            .arg("--format")
            .arg(crate::index::ITEM_FORMAT)
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
            // Loops rather than yielding per line, so a blank line — which
            // `--format` produces for an item whose fields are all empty, and
            // which the `{NL}` at the end of the format can leave trailing — is
            // skipped instead of being reported as a malformed item.
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) if line.trim().is_empty() => continue,
                    Ok(Some(line)) => {
                        let item = BorgItem::from_format_line(&line).map_err(|e| {
                            EngineError::BorgFailed {
                                code: -1,
                                stderr: format!("parsing borg list line: {e}"),
                            }
                        });
                        return Some((item, (lines, child)));
                    }
                    Ok(None) => {
                        let _ = child.wait().await;
                        return None;
                    }
                    Err(e) => {
                        return Some((
                            Err(EngineError::BorgFailed {
                                code: -1,
                                stderr: e.to_string(),
                            }),
                            (lines, child),
                        ))
                    }
                }
            }
        });
        Ok(stream.boxed())
    }

    async fn extract(&self, id: &ArchiveId, paths: &[String], dest: &Path) -> Result<JobStream> {
        let mut cmd = self.cmd().await?;
        cmd.current_dir(dest)
            .arg("extract")
            .arg("--progress")
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
            .arg("--progress")
            .arg("--list")
            .arg(format!("--keep-hourly={}", policy.keep_hourly))
            .arg(format!("--keep-daily={}", policy.keep_daily))
            .arg(format!("--keep-weekly={}", policy.keep_weekly))
            .arg(format!("--keep-monthly={}", policy.keep_monthly))
            .arg(&self.repo);
        spawn_streamed(cmd)
    }

    async fn delete_archives(&self, ids: &[ArchiveId]) -> Result<JobStream> {
        if ids.is_empty() {
            // Never spawn `borg delete` with no archives named: the first
            // positional argument is the repository, so an empty list is the
            // command for deleting the entire repository.
            return Ok(JobStream::from_events(vec![JobEvent::Finished(Ok(
                Default::default(),
            ))]));
        }
        let mut cmd = self.cmd().await?;
        cmd.arg("delete").arg("--progress").arg("--list");
        // `borg delete repo::first second third` — the repository and the first
        // archive share an argument, the rest are bare names.
        cmd.arg(self.archive_ref(&ids[0]));
        for id in &ids[1..] {
            cmd.arg(&id.0);
        }
        spawn_streamed(cmd)
    }

    async fn compact(&self) -> Result<JobStream> {
        let mut cmd = self.cmd().await?;
        cmd.arg("compact").arg("--progress").arg(&self.repo);
        spawn_streamed(cmd)
    }

    async fn check(&self, level: CheckLevel) -> Result<JobStream> {
        let mut cmd = self.cmd().await?;
        cmd.arg("check").arg("--progress");
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

/// Parse one line of the archive listing, or skip it.
///
/// Checkpoint archives are dropped here, at the single point every caller goes
/// through. Borg writes `<name>.checkpoint` when a `create` is interrupted, and
/// the next run consumes it: it is scaffolding, not a snapshot, and showing one
/// in the timeline would offer the user a restore from a backup that was never
/// finished.
fn parse_archive_line(line: &str) -> Option<ArchiveMeta> {
    let mut fields = line.splitn(4, '\t');
    let id = fields.next()?.trim();
    let epoch = fields.next()?.trim();
    let iso = fields.next()?.trim();
    let name = fields.next()?.trim_end_matches(['\r', '\n']);
    if name.is_empty() || is_checkpoint(name) {
        return None;
    }

    // `%s` is a glibc/musl strftime extension rather than C89, so a platform
    // that does not honour it would hand back something unparseable. Falling
    // back to the ISO string keeps the archive visible; the note is that the
    // fallback reads a local timestamp as if it were UTC, so a foreign
    // repository's dates could be out by the machine's offset. Ordering, which
    // is what the catalogue depends on, survives either way.
    let ts = match epoch.parse::<i64>() {
        Ok(ts) => ts,
        Err(_) => {
            tracing::warn!(
                archive = name,
                "borg did not report an epoch timestamp; falling back to its local-time string"
            );
            crate::index::parse_borg_mtime(iso).ok()? / 1_000_000
        }
    };

    Some(ArchiveMeta {
        borg_id: (!id.is_empty()).then(|| id.to_string()),
        name: name.to_string(),
        ts,
    })
}

/// Whether this is one of Borg's interrupted-run checkpoint archives.
pub fn is_checkpoint(name: &str) -> bool {
    name.ends_with(".checkpoint") || name.contains(".checkpoint.")
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
async fn run_json(cmd: Command) -> Result<serde_json::Value> {
    let out = run_to_completion(cmd).await?;
    serde_json::from_slice(&out).map_err(|e| EngineError::BorgFailed {
        code: -1,
        stderr: format!("parsing borg --json: {e}"),
    })
}

/// Run a borg command and return its stdout as a UTF-8 string (e.g. key export).
async fn run_stdout_string(cmd: Command) -> Result<String> {
    Ok(String::from_utf8_lossy(&run_to_completion(cmd).await?).into_owned())
}

/// Run a borg command to completion and hand back its stdout, failing only on a
/// genuine error exit.
///
/// A warning exit keeps its output: `borg list` that warns about something is
/// still a listing, and discarding it would lose the catalogue over a message.
/// See [`classify::classify_exit`] for why "non-zero" and "failed" are not the
/// same question.
async fn run_to_completion(mut cmd: Command) -> Result<Vec<u8>> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = cmd.output().await.map_err(|e| EngineError::BorgFailed {
        code: -1,
        stderr: format!("running borg: {e}"),
    })?;
    let code = out.status.code().unwrap_or(-1);
    match classify::classify_exit(code) {
        classify::ExitClass::Success => {}
        classify::ExitClass::Warning => tracing::info!(code, "borg finished with warnings"),
        classify::ExitClass::Error => {
            return Err(classify::classify(code, &collect_json_errors(&out.stderr)))
        }
    }
    Ok(out.stdout)
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

#[cfg(test)]
mod archive_listing_tests {
    use super::*;

    #[test]
    fn parses_a_real_borg_listing_line() {
        let line =
            "830239d8c5e10c\t1786085752\t2026-08-07T07:55:52.000000\tbt-thinkpad-20260807T065552Z";
        let meta = parse_archive_line(line).expect("a well-formed line");
        assert_eq!(meta.name, "bt-thinkpad-20260807T065552Z");
        assert_eq!(meta.borg_id.as_deref(), Some("830239d8c5e10c"));
        assert_eq!(
            meta.ts, 1_786_085_752,
            "the epoch comes from borg, not from re-reading its local-time string"
        );
    }

    #[test]
    fn checkpoint_archives_never_reach_the_catalogue() {
        // Borg's scaffolding from an interrupted run. Offering one in the
        // timeline would offer a restore from a backup that never finished.
        assert!(parse_archive_line("id\t100\tiso\tbt-host-20260807T065552Z.checkpoint").is_none());
        // Borg 1.2 numbers them when several accumulate.
        assert!(parse_archive_line("id\t100\tiso\tbt-host-x.checkpoint.1").is_none());
        assert!(is_checkpoint("anything.checkpoint"));
        assert!(!is_checkpoint("holiday-checkpoint-photos"));
    }

    #[test]
    fn an_archive_name_containing_a_tab_survives() {
        // Only ours are named to a scheme; an imported repository's names are
        // whatever somebody typed, which is why the name is the last field.
        let meta = parse_archive_line("id\t100\tiso\todd\tname").expect("parses");
        assert_eq!(meta.name, "odd\tname");
    }

    #[test]
    fn an_unparseable_epoch_falls_back_to_the_iso_string() {
        // If a platform's strftime does not honour `%s`, the archive stays
        // visible rather than vanishing from the catalogue.
        let meta = parse_archive_line("id\t%s\t1970-01-02T00:00:00.000000\tarch").expect("parses");
        assert_eq!(meta.ts, 86_400);
    }

    #[test]
    fn blank_and_malformed_lines_are_skipped() {
        assert!(parse_archive_line("").is_none());
        assert!(parse_archive_line("no tabs here").is_none());
        assert!(parse_archive_line("id\t100\tiso\t").is_none());
    }
}
