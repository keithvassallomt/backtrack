// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>

//! `xtask` — Backtrack's development tooling.
//!
//! Its one job today is `demo-repo`: build a real Borg repository with a scripted
//! 30-snapshot history over a fake home (files appearing, changing, and being
//! deleted on known dates) and ingest it into a fresh index under the dev data
//! directory. This is the dataset every GUI stage develops and screenshots
//! against.
//!
//! Run it with `just demo-repo` (which sets `BACKTRACK_DEV=1`, so it writes to
//! `~/.local/share/backtrack-dev/`).

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use backtrack_core::index::{parse_borg_mtime, ArchiveMeta, BorgItem, IndexWriter, Repo};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// What a demo build produced, for reporting and testing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Summary {
    archives: usize,
    versions: i64,
    /// The acceptance signal: `old-client-folder` (deleted after snapshot 15) is
    /// flagged deleted-after when viewed at snapshot 10.
    old_client_deleted_at_10: bool,
}

fn main() -> Result<()> {
    let dir = data_dir();
    println!("Building demo repo + index under {}", dir.display());
    let summary = run(&dir)?;
    println!(
        "Done: {} snapshots, {} version rows.",
        summary.archives, summary.versions
    );
    println!(
        "  old-client-folder shows a 'deleted after this' badge at snapshot 10: {}",
        if summary.old_client_deleted_at_10 {
            "yes"
        } else {
            "NO — acceptance check failed"
        }
    );
    if !summary.old_client_deleted_at_10 {
        return Err("demo verification failed: old-client-folder not flagged deleted".into());
    }
    Ok(())
}

/// Build (from scratch) a real Borg repo and index at `dir`.
fn run(dir: &Path) -> Result<Summary> {
    let repo = dir.join("demo-repo");
    let src = dir.join("demo-src");
    let index_path = dir.join("index.db");

    // Start fresh so the recipe is idempotent.
    fs::create_dir_all(dir)?;
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&src);
    for suffix in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(dir.join(format!("index.db{suffix}")));
    }
    fs::create_dir_all(src.join("home"))?;

    borg(&["init", "-e", "none", &repo.to_string_lossy()])?;

    // Seed the history: mutate the fake home incrementally (so unchanged files
    // keep their mtime and Borg dedups), one snapshot per day, dated 1–30 June.
    let script = history();
    let mut prev = BTreeMap::new();
    for (i, day_files) in script.iter().enumerate() {
        let day = i + 1;
        apply_day(&src.join("home"), &prev, day_files)?;
        let date = format!("2026-06-{day:02}T12:00:00");
        borg_create(&src, &repo, &format!("snapshot-{day:02}"), &date)?;
        prev = day_files.clone();
        print!("\r  seeded snapshot {day}/{}", script.len());
    }
    println!();

    // Ingest every archive in chronological order. With no removals, the total
    // version-row count is just the sum of new versions opened.
    let mut writer = IndexWriter::open(&index_path)?;
    let archives = list_archives(&repo)?;
    let mut versions: i64 = 0;
    for (name, id, ts) in &archives {
        let items = list_items(&repo, name)?;
        let stats = writer.ingest_archive(
            &ArchiveMeta {
                borg_id: Some(id.clone()),
                name: name.clone(),
                ts: *ts,
            },
            Repo::Primary,
            items.into_iter(),
        )?;
        versions += stats.new_versions as i64;
    }
    drop(writer);

    let old_client_deleted_at_10 = verify_old_client_deleted(&index_path)?;
    Ok(Summary {
        archives: archives.len(),
        versions,
        old_client_deleted_at_10,
    })
}

/// The scripted 30-day history: for each day (1-based), the file tree relative to
/// `home/`, mapping path to content. Files appear, change, and disappear on known
/// dates; names echo the mockups.
fn history() -> Vec<BTreeMap<String, String>> {
    (1..=30)
        .map(|day| {
            let mut files = BTreeMap::new();

            // report.odt — three distinct versions (distinct lengths, so the
            // change is visible by size regardless of mtime).
            let report = if day < 8 {
                "report draft"
            } else if day < 20 {
                "report reviewed copy"
            } else {
                "report final signed version!!"
            };
            files.insert("Documents/report.odt".to_string(), report.to_string());

            // notes.txt — one change, late.
            let notes = if day < 25 {
                "meeting notes"
            } else {
                "meeting notes plus action items"
            };
            files.insert("Documents/notes.txt".to_string(), notes.to_string());

            // invoice appears mid-month and stays.
            if day >= 5 {
                files.insert(
                    "Documents/invoice-may.pdf".to_string(),
                    "invoice may 2026 total due".to_string(),
                );
            }
            // a photo shows up on day 10.
            if day >= 10 {
                files.insert(
                    "Pictures/vacation.jpg".to_string(),
                    "JPEG-BINARY".to_string(),
                );
            }
            // old-client-folder exists days 1–15, then is deleted.
            if day <= 15 {
                files.insert(
                    "old-client-folder/contract.pdf".to_string(),
                    "signed contract".to_string(),
                );
                files.insert(
                    "old-client-folder/proposal.odt".to_string(),
                    "project proposal".to_string(),
                );
            }
            files
        })
        .collect()
}

/// Apply the difference between `prev` and `cur` to the on-disk `home` tree:
/// write added/changed files, delete removed ones, then prune emptied
/// directories so a deleted folder actually disappears from the next snapshot.
fn apply_day(
    home: &Path,
    prev: &BTreeMap<String, String>,
    cur: &BTreeMap<String, String>,
) -> Result<()> {
    for rel in prev.keys() {
        if !cur.contains_key(rel) {
            let _ = fs::remove_file(home.join(rel));
        }
    }
    for (rel, content) in cur {
        if prev.get(rel) != Some(content) {
            let path = home.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, content)?;
        }
    }
    prune_empty_dirs(home)?;
    Ok(())
}

/// Remove empty subdirectories, bottom-up (never removing `dir` itself).
fn prune_empty_dirs(dir: &Path) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            prune_empty_dirs(&path)?;
            if fs::read_dir(&path)?.next().is_none() {
                fs::remove_dir(&path)?;
            }
        }
    }
    Ok(())
}

/// Open the index and check the acceptance signal.
fn verify_old_client_deleted(index_path: &Path) -> Result<bool> {
    use backtrack_core::index::IndexReader;
    let reader = IndexReader::open(index_path)?;
    let at_10 = reader.folder_at("home", 10)?;
    Ok(at_10
        .iter()
        .any(|e| e.name == "old-client-folder" && e.deleted_after))
}

// ── Borg helpers ────────────────────────────────────────────────────────────

fn borg(args: &[&str]) -> Result<()> {
    let status = Command::new("borg")
        .args(args)
        .env("BORG_UNKNOWN_UNENCRYPTED_REPO_ACCESS_IS_OK", "yes")
        .status()?;
    if !status.success() {
        return Err(format!("borg {args:?} exited with {status}").into());
    }
    Ok(())
}

fn borg_create(src: &Path, repo: &Path, name: &str, date: &str) -> Result<()> {
    let target = format!("{}::{name}", repo.to_string_lossy());
    let status = Command::new("borg")
        .current_dir(src)
        .args(["create", "--timestamp", date, &target, "home"])
        .env("BORG_UNKNOWN_UNENCRYPTED_REPO_ACCESS_IS_OK", "yes")
        .status()?;
    if !status.success() {
        return Err(format!("borg create {name} exited with {status}").into());
    }
    Ok(())
}

/// (name, borg id, ts-epoch-seconds) for every archive, chronological order.
fn list_archives(repo: &Path) -> Result<Vec<(String, String, i64)>> {
    let out = borg_output(&["list", "--json", &repo.to_string_lossy()])?;
    let value: serde_json::Value = serde_json::from_slice(&out)?;
    let mut archives = Vec::new();
    for a in value["archives"]
        .as_array()
        .ok_or("borg list: no archives")?
    {
        let name = a["name"]
            .as_str()
            .ok_or("archive without name")?
            .to_string();
        let id = a["id"].as_str().unwrap_or_default().to_string();
        let time = a["time"].as_str().ok_or("archive without time")?;
        let ts = parse_borg_mtime(time).map_err(|e| e.to_string())? / 1_000_000;
        archives.push((name, id, ts));
    }
    Ok(archives)
}

fn list_items(repo: &Path, name: &str) -> Result<Vec<BorgItem>> {
    let target = format!("{}::{name}", repo.to_string_lossy());
    let out = borg_output(&["list", "--json-lines", &target])?;
    let text = String::from_utf8(out)?;
    let mut items = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        items.push(BorgItem::from_json_line(line).map_err(|e| e.to_string())?);
    }
    Ok(items)
}

fn borg_output(args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("borg")
        .args(args)
        .env("BORG_UNKNOWN_UNENCRYPTED_REPO_ACCESS_IS_OK", "yes")
        .output()?;
    if !out.status.success() {
        return Err(format!(
            "borg {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(out.stdout)
}

// ── Data-directory resolution (mirrors backtrack_core::logging) ──────────────

fn data_dir() -> PathBuf {
    let dev = std::env::var_os("BACKTRACK_DEV").is_some();
    let leaf = if dev { "backtrack-dev" } else { "backtrack" };
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(leaf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use backtrack_core::index::{IndexReader, Kind};
    use std::collections::BTreeSet;

    /// Convert a day's file tree into `BorgItem`s, including directory entries for
    /// every ancestor (as Borg's listing would). Content-derived mtime means an
    /// unchanged file compares equal across snapshots, so the borg-free build
    /// produces the same interval structure as the real one.
    fn to_borg_items(files: &BTreeMap<String, String>) -> Vec<BorgItem> {
        let mut dirs: BTreeSet<String> = BTreeSet::new();
        dirs.insert("home".to_string());
        let mut items = Vec::new();
        for (rel, content) in files {
            let full = format!("home/{rel}");
            let parts: Vec<&str> = full.split('/').collect();
            for depth in 1..parts.len() {
                dirs.insert(parts[..depth].join("/"));
            }
            let checksum: i64 = content.bytes().map(|b| b as i64).sum();
            items.push(BorgItem {
                path: full,
                kind: Kind::File,
                size: content.len() as i64,
                mtime: checksum,
                mode: 0o644,
                chunk_hash: None,
            });
        }
        for dir in dirs {
            items.push(BorgItem {
                path: dir,
                kind: Kind::Dir,
                size: 0,
                mtime: 0,
                mode: 0o755,
                chunk_hash: None,
            });
        }
        items
    }

    fn index_from_history(writer: &mut IndexWriter) {
        for (i, files) in history().iter().enumerate() {
            let day = i + 1;
            writer
                .ingest_archive(
                    &ArchiveMeta {
                        borg_id: None,
                        name: format!("snapshot-{day:02}"),
                        ts: day as i64 * 86_400,
                    },
                    Repo::Primary,
                    to_borg_items(files).into_iter(),
                )
                .unwrap();
        }
    }

    #[test]
    fn history_spans_30_days_with_known_appearances_and_deletions() {
        let h = history();
        assert_eq!(h.len(), 30);
        // invoice appears on day 5 (index 4).
        assert!(!h[3].contains_key("Documents/invoice-may.pdf"));
        assert!(h[4].contains_key("Documents/invoice-may.pdf"));
        // old-client-folder present through day 15, gone from day 16.
        assert!(h[14].contains_key("old-client-folder/contract.pdf"));
        assert!(!h[15].contains_key("old-client-folder/contract.pdf"));
        // report.odt takes three distinct contents over the month.
        let reports: BTreeSet<_> = h
            .iter()
            .map(|d| d["Documents/report.odt"].clone())
            .collect();
        assert_eq!(reports.len(), 3);
    }

    #[test]
    fn scripted_history_produces_the_expected_index() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("index.db");
        {
            let mut w = IndexWriter::open(&path).unwrap();
            index_from_history(&mut w);
        }
        let r = IndexReader::open(&path).unwrap();

        assert_eq!(r.archives_overview().unwrap().len(), 30);

        // The acceptance signal: at snapshot 10 the folder is present but flagged
        // deleted-after; by snapshot 20 it is gone entirely.
        let at_10 = r.folder_at("home", 10).unwrap();
        let ocf = at_10
            .iter()
            .find(|e| e.name == "old-client-folder")
            .expect("old-client-folder present at snapshot 10");
        assert!(ocf.deleted_after);
        let at_20 = r.folder_at("home", 20).unwrap();
        assert!(!at_20.iter().any(|e| e.name == "old-client-folder"));

        // report.odt accumulated exactly three versions.
        assert_eq!(
            r.file_history("home/Documents/report.odt").unwrap().len(),
            3
        );
    }

    #[cfg(feature = "integration")]
    #[test]
    fn full_demo_roundtrip_via_real_borg() {
        let tmp = tempfile::tempdir().unwrap();
        let summary = run(tmp.path()).expect("demo build");
        assert_eq!(summary.archives, 30);
        assert!(summary.old_client_deleted_at_10);
        assert!(summary.versions > 0);
    }
}
