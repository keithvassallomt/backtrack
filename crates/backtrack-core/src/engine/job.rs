// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>

//! Streamed job events and the [`JobStream`] that carries them. A job's terminal
//! outcome is delivered in-band as the final [`JobEvent::Finished`], because a
//! Borg process only fails *after* the `create()` future has already returned.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::EngineError;

/// Severity of a `log_message` line from Borg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

/// A summary of a finished job. Minimal in Stage 2; extended with byte/file stats
/// when the backup pipeline (Stage 4) needs them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobSummary {
    pub archive_id: Option<String>,
}

/// One event from a running Borg job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobEvent {
    /// Progress tick. `total` is `None` when Borg cannot give a denominator
    /// (e.g. partial extracts — never trust `progress_percent` there).
    Progress {
        current: u64,
        total: Option<u64>,
        phase: String,
    },
    /// A forwarded `log_message`.
    Log { level: LogLevel, msg: String },
    /// One item finished (from `file_status`).
    ItemDone { path: String },
    /// Terminal event: the job's outcome. The stream ends after this.
    Finished(std::result::Result<JobSummary, EngineError>),
}

/// A live Borg job as a stream of [`JobEvent`]s. Owns the Borg child (via the
/// spawner) and a [`CancellationToken`]; [`JobStream::cancel`] and `Drop` both
/// trip the token, which the reader task turns into a SIGTERM/kill.
pub struct JobStream {
    rx: mpsc::Receiver<JobEvent>,
    cancel: CancellationToken,
}

impl JobStream {
    /// Construct from a channel + token. Used by [`BorgCli`](super) once it has
    /// spawned the reader task.
    pub(crate) fn new(rx: mpsc::Receiver<JobEvent>, cancel: CancellationToken) -> JobStream {
        JobStream { rx, cancel }
    }

    /// Build a stream that yields a fixed sequence of events. For mocks and tests
    /// (used by `backtrack-testkit`); requires a Tokio runtime.
    pub fn from_events(events: impl IntoIterator<Item = JobEvent> + Send + 'static) -> JobStream {
        // Collected eagerly: the bound above only guarantees `events` itself is
        // `Send`, not that its `IntoIter` is too, and `tokio::spawn` needs a
        // `Send` future. `Vec<JobEvent>`'s `IntoIter` is `Send` since `JobEvent`
        // is `Send`.
        let events: Vec<JobEvent> = events.into_iter().collect();
        let (tx, rx) = mpsc::channel(64);
        let cancel = CancellationToken::new();
        let tc = cancel.clone();
        tokio::spawn(async move {
            for ev in events {
                tokio::select! {
                    _ = tc.cancelled() => break,
                    r = tx.send(ev) => {
                        if r.is_err() {
                            break;
                        }
                    }
                }
            }
        });
        JobStream::new(rx, cancel)
    }

    /// Request cancellation: trips the token so the reader task kills the child.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

impl Drop for JobStream {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Stream for JobStream {
    type Item = JobEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<JobEvent>> {
        self.rx.poll_recv(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn from_events_yields_each_event_then_ends() {
        let mut s = JobStream::from_events(vec![
            JobEvent::Progress {
                current: 1,
                total: Some(2),
                phase: "archiving".into(),
            },
            JobEvent::ItemDone {
                path: "home/a".into(),
            },
            JobEvent::Finished(Ok(JobSummary::default())),
        ]);
        assert!(matches!(
            s.next().await,
            Some(JobEvent::Progress { current: 1, .. })
        ));
        assert!(matches!(s.next().await, Some(JobEvent::ItemDone { .. })));
        assert!(matches!(s.next().await, Some(JobEvent::Finished(Ok(_)))));
        assert!(s.next().await.is_none());
    }

    #[tokio::test]
    async fn cancel_stops_the_stream_early() {
        let mut s = JobStream::from_events((0..1000).map(|i| JobEvent::ItemDone {
            path: format!("f{i}"),
        }));
        assert!(s.next().await.is_some());
        s.cancel();
        // Drain: cancellation makes the feeder stop; the stream ends.
        while s.next().await.is_some() {}
    }
}
