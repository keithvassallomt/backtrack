// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! How much memory cataloguing an archive takes (S18-T21): ingests a
//! generated listing of `FILES` files in `FOLDERS` folders twice, as the
//! first backup and as the next one, into a catalogue in a temporary folder,
//! and prints each ingest's peak resident memory.
//!
//!     cargo run --release -p backtrack-core --example ingest_memory [FILES FOLDERS]
//!
//! The listing is made as it is read, so it takes no memory of its own. A
//! command run from a terminal, whose output is what it is for.

use std::time::Instant;

use backtrack_core::index::{ArchiveMeta, BorgItem, IndexWriter, Kind, Repo};

/// A line of `/proc/self/status`, in kB.
fn status_kb(field: &str) -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find_map(|line| line.strip_prefix(field)?.trim().strip_suffix("kB"))
        .and_then(|kb| kb.trim().parse().ok())
        .unwrap_or(0)
}

/// Start the peak resident memory again from what is resident now.
fn reset_peak() {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
}

/// The listing: each folder, then its files, the folders two deep under 250
/// at the top, as a home folder's photos and documents might be.
fn listing(files: u64, folders: u64, mtime: i64) -> impl Iterator<Item = BorgItem> {
    let per_folder = files / folders;
    let tops = 250.min(folders);
    let item = move |path: String, kind: Kind, size: i64| BorgItem {
        path,
        kind,
        size,
        mtime,
        mode: if kind == Kind::Dir { 0o755 } else { 0o644 },
        chunk_hash: None,
    };
    let top_dirs = (0..tops).map(move |t| item(format!("home/k/top-{t:03}"), Kind::Dir, 0));
    let rest = (0..folders).flat_map(move |f| {
        let folder = format!("home/k/top-{:03}/folder-{f:05}", f % tops);
        let dir = std::iter::once(item(folder.clone(), Kind::Dir, 0));
        let files = (0..per_folder).map(move |n| {
            item(
                format!("{folder}/IMG_2026{:04}_{n:06}.jpg", f % 10_000),
                Kind::File,
                (n * 4096) as i64,
            )
        });
        dir.chain(files)
    });
    top_dirs.chain(rest)
}

fn main() {
    let args: Vec<u64> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let files = args.first().copied().unwrap_or(2_500_000);
    let folders = args.get(1).copied().unwrap_or(50_000);
    let dir = tempfile::tempdir().expect("a temporary folder");
    let mut writer = IndexWriter::open(&dir.path().join("index.db")).expect("a catalogue");
    println!("{files} files in {folders} folders");
    for (n, what) in ["first backup", "next backup"].iter().enumerate() {
        let meta = ArchiveMeta {
            borg_id: None,
            name: format!("generated-{n}"),
            ts: 1_800_000_000 + n as i64 * 3600,
        };
        reset_peak();
        let before = status_kb("VmRSS:");
        let started = Instant::now();
        let stats = writer
            .ingest_archive(
                &meta,
                Repo::Primary,
                listing(files, folders, 1_700_000_000_000_000),
            )
            .expect("the ingest");
        let peak = status_kb("VmHWM:");
        println!(
            "{what}: {} items ({} new versions, {} carried on) in {:.1} s; resident {} MB before, peak {} MB (+{} MB)",
            stats.items,
            stats.new_versions,
            stats.extended,
            started.elapsed().as_secs_f64(),
            before / 1024,
            peak / 1024,
            peak.saturating_sub(before) / 1024
        );
    }
}
