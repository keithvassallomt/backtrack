// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Index reads, kept off the main loop.
//!
//! Every query the timeline makes is an indexed lookup against a local SQLite
//! file, so they are all fast — and none of them is guaranteed to be. The
//! database may be on a spinning disk, on NFS, or being written to by the
//! daemon at that exact moment, and a query that usually takes 200 µs will
//! occasionally take 200 ms. On the main loop that is a visible stutter while
//! arrow-keying through time, which is the one interaction the whole app is
//! built around.
//!
//! So the reader lives on a thread of its own and the main loop talks to it
//! with futures. The reader is opened lazily there too: a corrupt or missing
//! index then arrives as an error on the first query, where the window can show
//! it, rather than as a failure before there is a window to show it in.

use std::path::PathBuf;
use std::thread;

use backtrack_core::index::IndexReader;
use tracing::{debug, error};

/// A query, already carrying the channel it answers on.
type Job = Box<dyn FnOnce(&Result<IndexReader, String>) + Send>;

/// A handle onto the index worker. Cheap to clone; every clone talks to the
/// same reader.
#[derive(Clone)]
pub struct Index {
    jobs: async_channel::Sender<Job>,
}

impl Index {
    /// Start the worker for the index at `path`. Returns immediately; the file
    /// is opened on the worker thread when the first query arrives.
    pub fn spawn(path: PathBuf) -> Index {
        let (jobs, queue) = async_channel::unbounded::<Job>();
        thread::Builder::new()
            .name("backtrack-index".to_string())
            .spawn(move || {
                let reader = IndexReader::open(&path).map_err(|e| {
                    error!(path = %path.display(), error = %e, "the index could not be opened");
                    e.to_string()
                });
                if reader.is_ok() {
                    debug!(path = %path.display(), "index opened for reading");
                }
                while let Ok(job) = queue.recv_blocking() {
                    job(&reader);
                }
                debug!("index worker finished");
            })
            .expect("the index worker thread could not be started");
        Index { jobs }
    }

    /// Run `query` against the reader and await its result.
    ///
    /// The error is already a sentence, because by the time it reaches a widget
    /// there is nothing left to match on — it goes into a status page either
    /// way.
    pub async fn query<T, F>(&self, query: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&IndexReader) -> backtrack_core::index::Result<T> + Send + 'static,
    {
        let (tx, rx) = async_channel::bounded(1);
        let job: Job = Box::new(move |reader| {
            let answer = match reader {
                Ok(reader) => query(reader).map_err(|e| e.to_string()),
                Err(message) => Err(message.clone()),
            };
            // The receiver is gone when the caller was dropped mid-query, which
            // is the normal end of a superseded request.
            let _ = tx.send_blocking(answer);
        });
        self.jobs
            .send(job)
            .await
            .map_err(|_| "the index reader has stopped".to_string())?;
        rx.recv()
            .await
            .map_err(|_| "the index reader dropped the request".to_string())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use backtrack_core::index::{ArchiveMeta, BorgItem, IndexWriter, Kind, Repo};
    use gtk4::glib;

    fn item(path: &str, kind: Kind) -> BorgItem {
        BorgItem {
            path: path.to_string(),
            kind,
            size: 10,
            mtime: 1_781_438_400_000_000,
            mode: 0o644,
            chunk_hash: None,
        }
    }

    /// Build a one-archive index in a temporary directory and hand back its path.
    fn fixture(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("index.db");
        let mut writer = IndexWriter::open(&path).unwrap();
        writer
            .ingest_archive(
                &ArchiveMeta {
                    borg_id: None,
                    name: "snapshot-01".to_string(),
                    ts: 1_781_438_400,
                },
                Repo::Primary,
                vec![item("home", Kind::Dir), item("home/notes.txt", Kind::File)].into_iter(),
            )
            .unwrap();
        path
    }

    #[test]
    fn a_query_runs_on_the_worker_and_comes_back_with_its_answer() {
        let dir = tempfile::tempdir().unwrap();
        let index = Index::spawn(fixture(dir.path()));
        let answer = glib::MainContext::new()
            .block_on(index.query(|reader| reader.archives_overview()))
            .unwrap();
        assert_eq!(answer.len(), 1);
        assert_eq!(answer[0].name, "snapshot-01");
    }

    #[test]
    fn the_worker_runs_somewhere_other_than_the_caller() {
        let dir = tempfile::tempdir().unwrap();
        let index = Index::spawn(fixture(dir.path()));
        let here = thread::current().id();
        let there = glib::MainContext::new()
            .block_on(index.query(move |_| Ok(thread::current().id())))
            .unwrap();
        assert_ne!(
            here, there,
            "the query must not have run on the main thread"
        );
    }

    #[test]
    fn an_index_that_cannot_be_opened_answers_every_query_with_why() {
        let dir = tempfile::tempdir().unwrap();
        let index = Index::spawn(dir.path().join("does-not-exist.db"));
        let error = glib::MainContext::new()
            .block_on(index.query(|reader| reader.archives_overview()))
            .unwrap_err();
        assert!(!error.is_empty(), "the failure has to say something");
    }
}
