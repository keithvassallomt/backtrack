// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! `backtrack` — the command-line client, and the debugging tool.
//!
//! Every command is a call to the daemon over `org.backtrack.Daemon1`; nothing
//! here touches a repository directly. That is the point: the CLI exercises the
//! same interface the GUI uses, so anything reproducible here is reproducible
//! there, and `backtrack doctor` describes the system the GUI is actually
//! talking to.
//!
//! Output comes in two forms. The human rendering is free to change whenever it
//! reads better. `--json` is an interface, pinned by a golden test, because
//! somebody will put it in a monitoring script and never look at it again.

mod doctor;
mod proxy;
mod render;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

use clap::{Args, Parser, Subcommand};

/// Anything that stops a command completing.
#[derive(Debug)]
pub enum CliError {
    NoSessionBus(String),
    DaemonUnavailable(String),
    Bus(String),
    /// A named error the daemon returned, with its bus name kept so the message
    /// can be phrased for a person.
    Daemon {
        name: String,
        message: String,
    },
    Io {
        path: PathBuf,
        message: String,
    },
    Usage(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::NoSessionBus(e) => {
                write!(f, "cannot reach the session bus: {e}")
            }
            CliError::DaemonUnavailable(e) => write!(f, "{e}"),
            CliError::Bus(e) => write!(f, "{e}"),
            CliError::Daemon { name, message } => write!(f, "{}", explain(name, message)),
            CliError::Io { path, message } => {
                write!(f, "{}: {message}", path.display())
            }
            CliError::Usage(e) => write!(f, "{e}"),
        }
    }
}

/// Turn a named D-Bus error into advice. The name is the contract, so matching
/// on it — never on the message — is what keeps this working.
fn explain(name: &str, message: &str) -> String {
    let advice = match name.rsplit('.').next().unwrap_or_default() {
        "NotConfigured" => Some("Run the setup wizard, or `backtrack config set storage.repository`."),
        "PassphraseMissing" => {
            Some("The passphrase is not in the keyring. Open Backtrack to re-enter it.")
        }
        "PassphraseWrong" => {
            Some("The stored passphrase no longer opens this repository. Re-enter it, or use your recovery key.")
        }
        "RepoUnreachable" => Some("The destination is not reachable right now."),
        "DestinationFull" => Some("Free space on the destination, or run `backtrack compact`."),
        "LockedByOther" => Some("Another Borg process holds the repository. Try again shortly."),
        "RepoCorrupt" => Some("Run `backtrack verify` to see the extent of the damage."),
        "NotPausable" => Some("Only a full restore can be paused. Cancel it instead."),
        _ => None,
    };
    match advice {
        Some(advice) => format!("{message}\n{advice}"),
        None => message.to_string(),
    }
}

impl From<zbus::Error> for CliError {
    fn from(e: zbus::Error) -> CliError {
        match e {
            zbus::Error::MethodError(name, message, _) => CliError::Daemon {
                name: name.as_str().to_string(),
                message: message.unwrap_or_else(|| name.as_str().to_string()),
            },
            other => CliError::Bus(other.to_string()),
        }
    }
}

/// Backtrack — Time Machine-class backups for Linux.
#[derive(Parser)]
#[command(name = "backtrack", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show whether your files are backed up.
    Status(StatusArgs),
    /// Start a backup now.
    BackupNow,
    /// Pause scheduled backups for a while (e.g. 1h, 30m, 2d).
    Pause { duration: String },
    /// Resume scheduled backups.
    Resume,
    /// Restore files from a snapshot.
    Restore(RestoreArgs),
    /// Search every snapshot by filename.
    Search(SearchArgs),
    /// Apply the retention policy.
    Prune,
    /// Check the repository's integrity.
    Verify,
    /// Reclaim space the repository no longer needs.
    Compact,
    /// Read or change settings.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Collect a diagnostic bundle for a bug report.
    Doctor,
}

#[derive(Args)]
struct StatusArgs {
    /// Emit machine-readable JSON with a stable shape.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct SearchArgs {
    /// What to look for; matches filenames.
    query: String,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct RestoreArgs {
    /// The snapshot to restore from.
    archive: String,
    /// One or more archive-relative paths.
    #[arg(required = true)]
    paths: Vec<String>,
    /// Where to put them. Defaults to the current directory, which never
    /// overwrites the originals by accident.
    #[arg(long)]
    dest: Option<PathBuf>,
    /// What to do about files that are already there.
    #[arg(long, default_value = "ask")]
    policy: String,
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Print one setting, or the whole configuration.
    Get { key: Option<String> },
    /// Change one setting. The value is a TOML literal.
    Set { key: String, value: String },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("backtrack: cannot start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(run(cli.command)) {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("backtrack: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(command: Command) -> Result<String, CliError> {
    let now = SystemTime::now();
    match command {
        Command::Status(args) => {
            let status = proxy::connect().await?.get_status().await?;
            Ok(if args.json {
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&render::status_json(&status, now))
                        .expect("status serialises")
                )
            } else {
                render::status_human(&status, now)
            })
        }

        Command::BackupNow => {
            let job = proxy::connect().await?.backup_now().await?;
            Ok(format!(
                "Backup started (job {job}).\nIt runs in the background; `backtrack status` shows progress.\n"
            ))
        }

        Command::Pause { duration } => {
            let span = parse_duration(&duration)?;
            let until = now + span;
            proxy::connect().await?.pause(to_epoch(until)).await?;
            Ok(format!(
                "Backups paused until {}.\nThey resume by themselves; `backtrack resume` is immediate.\n",
                render::format_time(until)
            ))
        }

        Command::Resume => {
            proxy::connect().await?.resume().await?;
            Ok("Backups resumed.\n".to_string())
        }

        Command::Restore(args) => {
            let dest = match args.dest {
                Some(dest) => dest,
                None => std::env::current_dir().map_err(|e| CliError::Io {
                    path: PathBuf::from("."),
                    message: e.to_string(),
                })?,
            };
            let job = proxy::connect()
                .await?
                .restore_files(
                    &args.archive,
                    &args.paths,
                    &dest.to_string_lossy(),
                    &args.policy,
                )
                .await?;
            Ok(format!(
                "Restoring {} item{} into {} (job {job}).\n",
                args.paths.len(),
                if args.paths.len() == 1 { "" } else { "s" },
                dest.display()
            ))
        }

        Command::Search(args) => {
            let hits = proxy::connect().await?.search_files(&args.query).await?;
            Ok(if args.json {
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&render::search_json(&hits))
                        .expect("results serialise")
                )
            } else {
                render::search_human(&hits)
            })
        }

        Command::Prune => {
            let job = proxy::connect().await?.prune().await?;
            Ok(format!("Applying the retention policy (job {job}).\n"))
        }
        Command::Verify => {
            let job = proxy::connect().await?.verify().await?;
            Ok(format!(
                "Checking the repository (job {job}). This can take a while.\n"
            ))
        }
        Command::Compact => {
            let job = proxy::connect().await?.compact().await?;
            Ok(format!("Reclaiming space (job {job}).\n"))
        }

        Command::Config(ConfigCommand::Get { key }) => {
            let daemon = proxy::connect().await?;
            Ok(match key {
                Some(key) => format!("{}\n", daemon.get_config_key(&key).await?),
                None => daemon.get_config().await?,
            })
        }
        Command::Config(ConfigCommand::Set { key, value }) => {
            proxy::connect().await?.set_config(&key, &value).await?;
            Ok(format!("{key} = {value}\n"))
        }

        Command::Doctor => {
            // Best-effort: a doctor bundle is most wanted precisely when the
            // daemon is not answering, so a failure to reach it becomes part of
            // the report rather than the end of it.
            let (status_json, config_toml) = match proxy::connect().await {
                Ok(daemon) => {
                    let status = match daemon.get_status().await {
                        Ok(status) => {
                            serde_json::to_string_pretty(&render::status_json(&status, now))
                                .unwrap_or_default()
                        }
                        Err(e) => format!("{{\"error\": \"{e}\"}}"),
                    };
                    let config = daemon.get_config().await.unwrap_or_default();
                    (status, config)
                }
                Err(e) => (format!("{{\"error\": \"{e}\"}}"), String::new()),
            };

            let path = doctor::collect(&status_json, &config_toml, now)?;
            Ok(format!(
                "Diagnostic bundle written to:\n  {}\n\nIt contains no passphrases or keys. Attach it to a bug report.\n",
                path.display()
            ))
        }
    }
}

fn to_epoch(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Parse `30m`, `2h`, `1d`. Deliberately small: the menu offers a few fixed
/// pauses and this covers the same ground without a date parser.
fn parse_duration(text: &str) -> Result<Duration, CliError> {
    let text = text.trim();
    let (number, unit) = text.split_at(
        text.find(|c: char| !c.is_ascii_digit())
            .ok_or_else(|| bad_duration(text))?,
    );
    let n: u64 = number.parse().map_err(|_| bad_duration(text))?;
    if n == 0 {
        return Err(CliError::Usage(
            "a pause of zero does nothing; use `backtrack resume` to cancel one".into(),
        ));
    }
    let seconds = match unit {
        "s" | "sec" | "secs" => 1,
        "m" | "min" | "mins" => 60,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3_600,
        "d" | "day" | "days" => 86_400,
        _ => return Err(bad_duration(text)),
    };
    Ok(Duration::from_secs(n * seconds))
}

fn bad_duration(text: &str) -> CliError {
    CliError::Usage(format!(
        "{text:?} is not a duration. Use a number and a unit, like 30m, 2h, or 1d."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_surface_parses() {
        Cli::command().debug_assert();
    }

    #[test]
    fn durations_cover_the_units_people_use() {
        assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1_800));
        assert_eq!(parse_duration("2h").unwrap(), Duration::from_secs(7_200));
        assert_eq!(parse_duration("1d").unwrap(), Duration::from_secs(86_400));
        assert_eq!(parse_duration("45s").unwrap(), Duration::from_secs(45));
        assert!(parse_duration("3 hours").is_err(), "a space is not a unit");
        assert_eq!(
            parse_duration("2hours").unwrap(),
            Duration::from_secs(7_200)
        );
    }

    #[test]
    fn a_nonsense_duration_says_what_a_good_one_looks_like() {
        let err = parse_duration("soon").unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("30m"),
            "the error should show the form: {text}"
        );
    }

    #[test]
    fn a_zero_pause_is_refused_rather_than_silently_doing_nothing() {
        let err = parse_duration("0h").unwrap_err();
        assert!(err.to_string().contains("resume"), "got: {err}");
    }

    #[test]
    fn a_bare_number_is_refused() {
        // "pause 30" is ambiguous — seconds? minutes? Refusing beats guessing.
        assert!(parse_duration("30").is_err());
    }

    #[test]
    fn named_daemon_errors_become_advice() {
        let text = explain("org.backtrack.Error.NotConfigured", "not configured");
        assert!(text.contains("setup wizard"), "got: {text}");

        let text = explain("org.backtrack.Error.PassphraseWrong", "wrong passphrase");
        assert!(text.contains("recovery key"), "got: {text}");
    }

    #[test]
    fn an_unrecognised_error_is_passed_through_unembellished() {
        // Inventing advice for an error we do not understand is worse than
        // repeating what the daemon said.
        let text = explain("org.backtrack.Error.Something", "the message");
        assert_eq!(text, "the message");
    }

    #[test]
    fn restore_requires_at_least_one_path() {
        let parsed = Cli::try_parse_from(["backtrack", "restore", "archive-1"]);
        assert!(parsed.is_err(), "a restore of nothing is a usage error");
    }

    #[test]
    fn status_json_is_opt_in() {
        let cli = Cli::try_parse_from(["backtrack", "status"]).unwrap();
        match cli.command {
            Command::Status(args) => assert!(!args.json),
            _ => panic!("wrong command"),
        }
        let cli = Cli::try_parse_from(["backtrack", "status", "--json"]).unwrap();
        match cli.command {
            Command::Status(args) => assert!(args.json),
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn config_get_takes_an_optional_key() {
        let cli = Cli::try_parse_from(["backtrack", "config", "get"]).unwrap();
        match cli.command {
            Command::Config(ConfigCommand::Get { key }) => assert_eq!(key, None),
            _ => panic!("wrong command"),
        }
        let cli = Cli::try_parse_from(["backtrack", "config", "get", "backup.frequency"]).unwrap();
        match cli.command {
            Command::Config(ConfigCommand::Get { key }) => {
                assert_eq!(key.as_deref(), Some("backup.frequency"))
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn the_default_restore_policy_asks() {
        let cli = Cli::try_parse_from(["backtrack", "restore", "a1", "home/k/f.txt"]).unwrap();
        match cli.command {
            Command::Restore(args) => assert_eq!(args.policy, "ask"),
            _ => panic!("wrong command"),
        }
    }
}
