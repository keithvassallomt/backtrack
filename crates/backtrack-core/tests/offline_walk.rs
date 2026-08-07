// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Does the offline walk agree with Borg?
//!
//! The spool decides what to protect by walking the sources itself and
//! comparing against the catalogue. Two things have to hold for that to be
//! worth anything, and neither can be established by reasoning about it:
//!
//! 1. The walk must select **exactly** the files Borg would have archived.
//!    Backtrack evaluates the exclusion patterns locally (see
//!    `backtrack_core::pattern` for why), so this is two independent
//!    implementations of the same rules and the only honest check is to run
//!    both and compare.
//! 2. An unchanged tree must produce an **empty** change set. Path form and
//!    timestamp resolution both have to be right to the microsecond; if either
//!    is off, every file compares as modified and the first offline tick tries
//!    to spool the entire home directory.
//!
//! Run via `just test-integration`; skipped otherwise.
#![cfg(feature = "integration")]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use backtrack_core::engine::{
    ArchiveId, BackupEngine, BorgCli, Compression, CreateSpec, Encryption, JobEvent, RepoSpec,
};
use backtrack_core::index::{ArchiveMeta, IndexReader, IndexWriter, Kind, Repo};
use backtrack_core::pattern::ExcludeSet;
use backtrack_core::secret::{FileSecretStore, SecretStore};
use backtrack_core::walk::{archive_path, walk, WalkSpec};
use futures::StreamExt;

const PASS: &str = "offline-walk-passphrase";

/// The exclusions the wizard seeds. Testing against these rather than a
/// convenient subset is the point: they are what nearly every user will run.
fn shipped_exclusions() -> Vec<String> {
    backtrack_core::config::Config::default().backup.exclude
}

/// A tree with the shapes that catch mistakes: excluded directories at several
/// depths, an excluded file kind, a symlink, near-miss names, and a file whose
/// name would break a naive matcher.
fn build_tree(root: &std::path::Path) {
    let dirs = [
        "docs/deep/deeper",
        ".cache/chromium/Default",
        "src/app/node_modules/pkg",
        "src/app/target/debug/incremental",
        "src/app/target/release",
        ".local/share/Trash/files",
        "media/Caches",
        "notes",
    ];
    for d in dirs {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    let files = [
        "docs/report.odt",
        "docs/deep/deeper/buried.txt",
        "notes/cached-thoughts.txt",
        "notes/target-practice.md",
        "src/app/main.rs",
        "src/app/target/release/keepme.txt",
        "media/holiday.jpg",
        "media/ubuntu.iso",
        "vm.qcow2",
        ".cache/chromium/Default/blob",
        "src/app/node_modules/pkg/index.js",
        "src/app/target/debug/incremental/junk.bin",
        ".local/share/Trash/files/deleted.txt",
        "media/Caches/thumb.png",
        "odd:name.txt",
    ];
    for (n, f) in files.iter().enumerate() {
        std::fs::write(root.join(f), vec![b'x'; n + 1]).unwrap();
    }
    std::os::unix::fs::symlink("docs/report.odt", root.join("shortcut")).unwrap();
}

struct Fixture {
    _dir: tempfile::TempDir,
    src: PathBuf,
    engine: BorgCli,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo").to_str().unwrap().to_string();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    build_tree(&src);

    let store = FileSecretStore::new(dir.path().join("secrets.json"));
    store.set("test", PASS).await.unwrap();
    let secrets: Arc<dyn SecretStore> = Arc::new(store);

    let engine = BorgCli::new(repo.clone(), "test".into(), secrets)
        .await
        .expect("borg >= 1.2 available");
    engine
        .init_repo(&RepoSpec {
            path: repo,
            encryption: Encryption::RepokeyBlake2,
        })
        .await
        .expect("repo created");

    Fixture {
        _dir: dir,
        src,
        engine,
    }
}

async fn create(f: &Fixture, name: &str) {
    let spec = CreateSpec {
        archive_name: name.to_string(),
        sources: vec![f.src.clone()],
        excludes: shipped_exclusions(),
        compression: Compression::Zstd,
        one_file_system: false,
        created_at: std::time::SystemTime::now(),
        paths: Vec::new(),
    };
    let mut stream = f.engine.create(&spec).await.expect("create starts");
    while let Some(event) = stream.next().await {
        if let JobEvent::Finished(result) = event {
            result.expect("create succeeds");
        }
    }
}

/// Every non-directory member of `archive`, as Borg stored it.
async fn archived(f: &Fixture, archive: &str) -> BTreeSet<String> {
    let mut items = f
        .engine
        .list_archive(&ArchiveId(archive.to_string()))
        .await
        .expect("listing");
    let mut out = BTreeSet::new();
    while let Some(item) = items.next().await {
        let item = item.expect("item parses");
        if item.kind != Kind::Dir {
            out.insert(item.path);
        }
    }
    out
}

fn walked(f: &Fixture) -> BTreeSet<String> {
    let result = walk(&WalkSpec {
        sources: vec![f.src.clone()],
        excludes: ExcludeSet::compile(&shipped_exclusions()),
        one_file_system: false,
        never: Vec::new(),
    });
    assert_eq!(result.unreadable, 0, "nothing in the fixture is unreadable");
    result
        .entries
        .iter()
        .map(|e| e.path.to_string_lossy().to_string())
        .collect()
}

#[tokio::test]
async fn the_walk_selects_exactly_what_borg_would_archive() {
    let f = fixture().await;
    create(&f, "reference").await;

    let borg = archived(&f, "reference").await;
    let ours = walked(&f);

    let missing: Vec<_> = borg.difference(&ours).collect();
    let extra: Vec<_> = ours.difference(&borg).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "the walk and borg disagree.\n  borg archived but we skipped: {missing:?}\n  \
         we selected but borg excluded: {extra:?}"
    );
    // And the comparison is not vacuous: the tree really does contain both
    // things to keep and things to drop.
    assert!(borg.len() >= 8, "the fixture kept {} files", borg.len());
    assert!(
        borg.iter().any(|p| p.ends_with("report.odt")),
        "a document survived"
    );
    assert!(
        !borg.iter().any(|p| p.contains("/.cache/")),
        "the caches were dropped by both"
    );
}

#[tokio::test]
async fn an_unchanged_tree_has_nothing_to_spool() {
    // The check that keeps the spool small. Path form and timestamp resolution
    // must both match what Borg stored, to the microsecond — if either is off,
    // every file reads as modified and the first offline tick tries to archive
    // the whole home directory.
    let f = fixture().await;
    create(&f, "baseline").await;

    let dir = tempfile::tempdir().unwrap();
    let index_path = dir.path().join("index.db");
    {
        let mut writer = IndexWriter::open(&index_path).unwrap();
        let mut items = f
            .engine
            .list_archive(&ArchiveId("baseline".into()))
            .await
            .unwrap();
        let mut listing = Vec::new();
        while let Some(item) = items.next().await {
            listing.push(item.unwrap());
        }
        writer
            .ingest_archive(
                &ArchiveMeta {
                    borg_id: None,
                    name: "baseline".into(),
                    ts: 1_000,
                },
                Repo::Primary,
                listing.into_iter(),
            )
            .unwrap();
    }

    let reader = IndexReader::open(&index_path).unwrap();
    let live = walk(&WalkSpec {
        sources: vec![f.src.clone()],
        excludes: ExcludeSet::compile(&shipped_exclusions()),
        one_file_system: false,
        never: Vec::new(),
    });
    let changed = reader.changed_since(1, live.entries).unwrap();
    assert!(
        changed.is_empty(),
        "an untouched tree must have nothing to spool, got {changed:?}"
    );
}

#[tokio::test]
async fn an_untouched_tree_of_thousands_of_files_still_has_nothing_to_spool() {
    // The same claim as above, at a scale where a per-file disagreement of one
    // in a hundred would show up rather than hiding behind a lucky run.
    //
    // Borg reports microseconds through a float, so the timestamp it stores and
    // the one read from the filesystem do not always round the same way. This
    // is the test that says how often that matters in practice, and it is why
    // `MTIME_TOLERANCE_MICROS` exists.
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo").to_str().unwrap().to_string();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    for i in 0..3_000 {
        std::fs::write(src.join(format!("f{i}.txt")), format!("{i}")).unwrap();
    }

    let store = FileSecretStore::new(dir.path().join("secrets.json"));
    store.set("test", PASS).await.unwrap();
    let secrets: Arc<dyn SecretStore> = Arc::new(store);
    let engine = BorgCli::new(repo.clone(), "test".into(), secrets)
        .await
        .expect("borg available");
    engine
        .init_repo(&RepoSpec {
            path: repo,
            encryption: Encryption::RepokeyBlake2,
        })
        .await
        .unwrap();

    let spec = CreateSpec {
        archive_name: "bulk".into(),
        sources: vec![src.clone()],
        excludes: vec![],
        compression: Compression::Zstd,
        one_file_system: false,
        created_at: std::time::SystemTime::now(),
        paths: Vec::new(),
    };
    let mut stream = engine.create(&spec).await.unwrap();
    while let Some(event) = stream.next().await {
        if let JobEvent::Finished(r) = event {
            r.expect("create succeeds");
        }
    }

    let index_path = dir.path().join("index.db");
    {
        let mut writer = IndexWriter::open(&index_path).unwrap();
        let mut items = engine
            .list_archive(&ArchiveId("bulk".into()))
            .await
            .unwrap();
        let mut listing = Vec::new();
        while let Some(item) = items.next().await {
            listing.push(item.unwrap());
        }
        assert!(listing.len() >= 3_000, "the whole tree was catalogued");
        writer
            .ingest_archive(
                &ArchiveMeta {
                    borg_id: None,
                    name: "bulk".into(),
                    ts: 1_000,
                },
                Repo::Primary,
                listing.into_iter(),
            )
            .unwrap();
    }

    let reader = IndexReader::open(&index_path).unwrap();
    let live = walk(&WalkSpec {
        sources: vec![src],
        excludes: ExcludeSet::default(),
        one_file_system: false,
        never: Vec::new(),
    });
    let changed = reader.changed_since(1, live.entries).unwrap();
    assert!(
        changed.is_empty(),
        "{} of 3000 untouched files were reported as modified; \
         the first few were {:?}",
        changed.len(),
        changed.iter().take(5).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn only_what_actually_changed_is_reported() {
    let f = fixture().await;
    create(&f, "baseline").await;

    let dir = tempfile::tempdir().unwrap();
    let index_path = dir.path().join("index.db");
    {
        let mut writer = IndexWriter::open(&index_path).unwrap();
        let mut items = f
            .engine
            .list_archive(&ArchiveId("baseline".into()))
            .await
            .unwrap();
        let mut listing = Vec::new();
        while let Some(item) = items.next().await {
            listing.push(item.unwrap());
        }
        writer
            .ingest_archive(
                &ArchiveMeta {
                    borg_id: None,
                    name: "baseline".into(),
                    ts: 1_000,
                },
                Repo::Primary,
                listing.into_iter(),
            )
            .unwrap();
    }

    // Edit one file, add another, and churn something excluded — the last of
    // which must not show up, or every offline tick would archive the caches.
    std::fs::write(f.src.join("docs/report.odt"), b"rewritten, and longer").unwrap();
    std::fs::write(f.src.join("notes/brand-new.txt"), b"new").unwrap();
    std::fs::write(f.src.join(".cache/chromium/Default/blob"), b"churn churn").unwrap();

    let reader = IndexReader::open(&index_path).unwrap();
    let live = walk(&WalkSpec {
        sources: vec![f.src.clone()],
        excludes: ExcludeSet::compile(&shipped_exclusions()),
        one_file_system: false,
        never: Vec::new(),
    });
    let changed: Vec<String> = reader
        .changed_since(1, live.entries)
        .unwrap()
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    let expected: BTreeSet<String> = [
        archive_path(&f.src.join("docs/report.odt")),
        archive_path(&f.src.join("notes/brand-new.txt")),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        changed.into_iter().collect::<BTreeSet<_>>(),
        expected,
        "exactly the edited and the new file, and nothing from the caches"
    );
}
