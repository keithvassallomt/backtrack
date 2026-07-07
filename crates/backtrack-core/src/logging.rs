//! Structured logging shared by every Backtrack binary.
//!
//! Two layers are installed: a pretty console layer on stderr (respecting
//! `RUST_LOG`, default `info`) and a daily-rotated JSONL file layer under
//! `~/.local/share/backtrack/logs/<binary>.jsonl`. A panic hook records panics
//! through `tracing` before the process aborts, and logs older than 14 days are
//! pruned on startup.
//!
//! Levels follow a consistent convention across the codebase:
//! `error` = user-visible failure, `warn` = recovered/degraded, `info` = state
//! changes (backup started/finished, job lifecycle), `debug` = per-operation
//! detail, `trace` = per-file detail.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter, Layer};

const LOG_RETENTION_DAYS: u64 = 14;
const SECONDS_PER_DAY: u64 = 86_400;

/// Guard that must be kept alive for the lifetime of the process. Dropping it
/// flushes and joins the non-blocking file-writer thread, so binaries bind it in
/// `main` (`let _guard = logging::init(...)`).
#[must_use = "dropping the guard stops the JSONL writer from flushing"]
pub struct LogGuard(#[allow(dead_code)] WorkerGuard);

/// Initialise logging for `binary` (e.g. `"backtrackd"`). Call once, early in
/// `main`, and keep the returned guard alive.
pub fn init(binary: &str) -> LogGuard {
    let dir = log_dir();
    let _ = std::fs::create_dir_all(&dir);
    prune_old_logs(&dir, LOG_RETENTION_DAYS, SystemTime::now());

    let file_appender = tracing_appender::rolling::daily(&dir, format!("{binary}.jsonl"));
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let console = fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(env_filter());
    let json = fmt::layer()
        .json()
        .flatten_event(true)
        .with_writer(non_blocking)
        .with_filter(env_filter());

    tracing_subscriber::registry()
        .with(console)
        .with(json)
        .init();
    install_panic_hook();

    LogGuard(guard)
}

/// Console/file filter: `RUST_LOG` if set, otherwise `info`.
fn env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Log directory: `<data_dir>/logs`.
fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

/// Base data directory, honoring `BACKTRACK_DEV` so development never writes to
/// the real backup/log location.
fn data_dir() -> PathBuf {
    data_dir_from(
        std::env::var_os("BACKTRACK_DEV").is_some(),
        std::env::var_os("XDG_DATA_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

/// Pure path resolution, split out so it can be tested without touching the
/// process environment.
fn data_dir_from(dev: bool, xdg_data_home: Option<&OsStr>, home: Option<&OsStr>) -> PathBuf {
    let leaf = if dev { "backtrack-dev" } else { "backtrack" };
    let base = xdg_data_home
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(leaf)
}

/// Record panics through `tracing` (so they land in the JSONL log) before
/// deferring to the previous hook, which aborts/unwinds as usual.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| l.to_string())
            .unwrap_or_else(|| "<unknown>".to_string());
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".to_string());
        tracing::error!(target: "panic", location = %location, "panic: {message}");
        previous(info);
    }));
}

/// Delete rotated log files whose modification time is older than `max_days`.
/// Best-effort: unreadable entries and IO errors are ignored.
fn prune_old_logs(dir: &Path, max_days: u64, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if is_expired(modified, now, max_days) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Whether `modified` is more than `max_days` before `now`.
fn is_expired(modified: SystemTime, now: SystemTime, max_days: u64) -> bool {
    now.duration_since(modified)
        .map(|age| age > Duration::from_secs(max_days * SECONDS_PER_DAY))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    /// A `MakeWriter` that appends to a shared buffer, so a test can capture the
    /// exact bytes the JSONL layer emits.
    #[derive(Clone)]
    struct BufWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for BufWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> fmt::MakeWriter<'a> for BufWriter {
        type Writer = BufWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn jsonl_line_parses_and_has_required_fields() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let layer = fmt::layer()
            .json()
            .flatten_event(true)
            .with_writer(BufWriter(buf.clone()));
        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "acceptance_target", "structured startup line");
        });

        let bytes = buf.lock().unwrap();
        let line = std::str::from_utf8(&bytes)
            .unwrap()
            .lines()
            .next()
            .expect("one JSONL line");
        let value: serde_json::Value = serde_json::from_str(line).expect("valid JSON");

        assert!(value.get("timestamp").is_some(), "has timestamp");
        assert_eq!(value["level"], "INFO");
        assert_eq!(value["target"], "acceptance_target");
        assert_eq!(value["message"], "structured startup line");
    }

    #[test]
    fn data_dir_honors_dev_flag_and_xdg() {
        let real = data_dir_from(false, Some(OsStr::new("/x/data")), None);
        assert_eq!(real, PathBuf::from("/x/data/backtrack"));

        let dev = data_dir_from(true, Some(OsStr::new("/x/data")), None);
        assert_eq!(dev, PathBuf::from("/x/data/backtrack-dev"));

        let from_home = data_dir_from(false, None, Some(OsStr::new("/home/u")));
        assert_eq!(from_home, PathBuf::from("/home/u/.local/share/backtrack"));

        let empty_xdg = data_dir_from(false, Some(OsStr::new("")), Some(OsStr::new("/home/u")));
        assert_eq!(empty_xdg, PathBuf::from("/home/u/.local/share/backtrack"));
    }

    #[test]
    fn expiry_boundary() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100 * SECONDS_PER_DAY);
        let fresh = now - Duration::from_secs(13 * SECONDS_PER_DAY);
        let old = now - Duration::from_secs(15 * SECONDS_PER_DAY);
        assert!(!is_expired(fresh, now, LOG_RETENTION_DAYS));
        assert!(is_expired(old, now, LOG_RETENTION_DAYS));
    }

    #[test]
    fn prune_keeps_fresh_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("backtrackd.jsonl.2026-07-07");
        std::fs::write(&path, b"{}\n").unwrap();
        prune_old_logs(dir.path(), LOG_RETENTION_DAYS, SystemTime::now());
        assert!(path.exists(), "recently written log must survive pruning");
    }
}
