// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The preview cache behind `PreviewFile`.
//!
//! Previewing a file means extracting it from the repository, which is slow
//! enough that doing it twice for the same file is unacceptable — arrow-keying
//! through a folder would re-extract every time you stepped back. Extractions
//! are therefore cached on disk under `<data_dir>/cache`, capped at 1 GB, and
//! evicted least-recently-used.
//!
//! Entries are keyed by `(archive, path)`. Because an archive is immutable once
//! written, that pair identifies exactly one byte sequence forever — a cache hit
//! can never be stale, so there is nothing to invalidate.
//!
//! The daemon hands the client a file descriptor rather than a path. That keeps
//! the cache directory private to the daemon, and it is what lets a sandboxed
//! GUI read a file it has no filesystem permission to open for itself.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tracing::{debug, warn};

/// The cache ceiling. Generous enough to hold a working set of photographs,
/// small enough that it cannot quietly eat a small SSD.
pub const CAPACITY_BYTES: u64 = 1024 * 1024 * 1024;

/// An on-disk, size-capped cache of extracted files.
#[derive(Debug, Clone)]
pub struct PreviewCache {
    dir: PathBuf,
    capacity: u64,
}

impl PreviewCache {
    /// A cache rooted at `dir`, holding at most [`CAPACITY_BYTES`].
    pub fn new(dir: PathBuf) -> PreviewCache {
        PreviewCache {
            dir,
            capacity: CAPACITY_BYTES,
        }
    }

    /// A cache with an explicit ceiling, for tests.
    #[cfg(test)]
    pub fn with_capacity(dir: PathBuf, capacity: u64) -> PreviewCache {
        PreviewCache { dir, capacity }
    }

    /// Where the extraction of `path` from `archive` lives, hit or miss.
    ///
    /// The name is a hash rather than the path itself: archive paths are
    /// arbitrarily long, contain separators, and are not guaranteed to be valid
    /// UTF-8 on every filesystem, none of which survives being used as a
    /// filename.
    pub fn entry_path(&self, archive: &str, path: &str) -> PathBuf {
        let mut hasher = DefaultHasher::new();
        archive.hash(&mut hasher);
        // Hash the separator too, so ("a", "b/c") and ("a/b", "c") cannot
        // collide into one entry.
        0u8.hash(&mut hasher);
        path.hash(&mut hasher);
        self.dir.join(format!("{:016x}", hasher.finish()))
    }

    /// Whether this extraction is already cached.
    pub fn contains(&self, archive: &str, path: &str) -> bool {
        self.entry_path(archive, path).is_file()
    }

    /// Mark an entry as just used, so eviction sees it as recent.
    ///
    /// Recency is the file's own mtime — the filesystem is already keeping that
    /// record, and a separate index would be one more thing to corrupt, lock,
    /// and keep in step with the directory it describes.
    pub fn touch(&self, entry: &Path) {
        if let Err(e) = set_mtime_now(entry) {
            // Losing a touch costs an early eviction, nothing more.
            debug!(path = %entry.display(), "could not touch cache entry: {e}");
        }
    }

    /// Create the cache directory if it is not there yet.
    pub fn ensure_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)
    }

    /// Evict least-recently-used entries until the cache fits its ceiling.
    ///
    /// Called after each insertion. Best-effort throughout: a preview that fails
    /// to evict is still a working preview, so no error here is worth failing a
    /// user's request over.
    pub fn evict_to_fit(&self) {
        let mut entries = match self.scan() {
            Ok(entries) => entries,
            Err(e) => {
                warn!(dir = %self.dir.display(), "cannot read preview cache: {e}");
                return;
            }
        };

        let mut total: u64 = entries.iter().map(|e| e.size).sum();
        if total <= self.capacity {
            return;
        }

        // Oldest first — the least recently used are evicted before anything
        // that has been looked at lately.
        entries.sort_by_key(|e| e.used);
        for entry in entries {
            if total <= self.capacity {
                break;
            }
            match std::fs::remove_file(&entry.path) {
                Ok(()) => {
                    total = total.saturating_sub(entry.size);
                    debug!(path = %entry.path.display(), size = entry.size, "evicted preview");
                }
                Err(e) => warn!(path = %entry.path.display(), "cannot evict: {e}"),
            }
        }
    }

    /// Current size of the cache in bytes.
    #[cfg(test)]
    pub fn size_bytes(&self) -> u64 {
        self.scan()
            .map(|entries| entries.iter().map(|e| e.size).sum())
            .unwrap_or(0)
    }

    fn scan(&self) -> std::io::Result<Vec<CacheEntry>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            entries.push(CacheEntry {
                path: entry.path(),
                size: meta.len(),
                used: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
        Ok(entries)
    }
}

struct CacheEntry {
    path: PathBuf,
    size: u64,
    used: SystemTime,
}

/// Set a file's mtime to now, which is how the cache records "used".
fn set_mtime_now(path: &Path) -> std::io::Result<()> {
    let file = std::fs::OpenOptions::new().write(true).open(path)?;
    file.set_modified(SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn cache(capacity: u64) -> (tempfile::TempDir, PreviewCache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = PreviewCache::with_capacity(dir.path().to_path_buf(), capacity);
        cache.ensure_dir().unwrap();
        (dir, cache)
    }

    /// Write an entry of `size` bytes, last used `age_secs` ago.
    fn write_entry(cache: &PreviewCache, archive: &str, path: &str, size: usize, age_secs: u64) {
        let entry = cache.entry_path(archive, path);
        std::fs::write(&entry, vec![0u8; size]).unwrap();
        let when = SystemTime::now() - Duration::from_secs(age_secs);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&entry)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }

    #[test]
    fn the_same_file_maps_to_the_same_entry() {
        let (_dir, cache) = cache(CAPACITY_BYTES);
        assert_eq!(
            cache.entry_path("archive-1", "home/k/a.txt"),
            cache.entry_path("archive-1", "home/k/a.txt")
        );
    }

    #[test]
    fn different_archives_do_not_share_an_entry() {
        let (_dir, cache) = cache(CAPACITY_BYTES);
        assert_ne!(
            cache.entry_path("archive-1", "home/k/a.txt"),
            cache.entry_path("archive-2", "home/k/a.txt"),
            "the same path in two archives is two different files"
        );
    }

    #[test]
    fn the_separator_is_part_of_the_key() {
        let (_dir, cache) = cache(CAPACITY_BYTES);
        assert_ne!(
            cache.entry_path("a", "b/c"),
            cache.entry_path("a/b", "c"),
            "concatenating without a separator would collide these"
        );
    }

    #[test]
    fn contains_reports_what_is_there() {
        let (_dir, cache) = cache(CAPACITY_BYTES);
        assert!(!cache.contains("a1", "f.txt"));
        write_entry(&cache, "a1", "f.txt", 10, 0);
        assert!(cache.contains("a1", "f.txt"));
    }

    #[test]
    fn a_cache_within_its_ceiling_is_left_alone() {
        let (_dir, cache) = cache(1000);
        write_entry(&cache, "a1", "one", 100, 0);
        write_entry(&cache, "a1", "two", 100, 0);
        cache.evict_to_fit();
        assert!(cache.contains("a1", "one"));
        assert!(cache.contains("a1", "two"));
    }

    #[test]
    fn eviction_removes_the_least_recently_used_first() {
        let (_dir, cache) = cache(250);
        write_entry(&cache, "a1", "oldest", 100, 3_000);
        write_entry(&cache, "a1", "middle", 100, 2_000);
        write_entry(&cache, "a1", "newest", 100, 1_000);
        assert_eq!(cache.size_bytes(), 300);

        cache.evict_to_fit();

        assert!(!cache.contains("a1", "oldest"), "oldest must go first");
        assert!(cache.contains("a1", "middle"));
        assert!(cache.contains("a1", "newest"));
        assert!(cache.size_bytes() <= 250);
    }

    #[test]
    fn eviction_keeps_going_until_it_fits() {
        let (_dir, cache) = cache(150);
        for i in 0..5 {
            write_entry(&cache, "a1", &format!("f{i}"), 100, 5_000 - i * 100);
        }
        cache.evict_to_fit();
        assert!(
            cache.size_bytes() <= 150,
            "still over ceiling: {}",
            cache.size_bytes()
        );
        // The most recent survivor is the one that was used last.
        assert!(cache.contains("a1", "f4"));
    }

    #[test]
    fn touching_an_entry_saves_it_from_the_next_eviction() {
        let (_dir, cache) = cache(150);
        write_entry(&cache, "a1", "old", 100, 3_000);
        write_entry(&cache, "a1", "new", 100, 1_000);

        // Read the old one — now it is the recently used one.
        cache.touch(&cache.entry_path("a1", "old"));
        cache.evict_to_fit();

        assert!(
            cache.contains("a1", "old"),
            "the touched entry must survive"
        );
        assert!(!cache.contains("a1", "new"));
    }

    #[test]
    fn a_missing_cache_directory_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PreviewCache::with_capacity(dir.path().join("absent"), 100);
        // Never created: eviction and sizing must cope.
        cache.evict_to_fit();
        assert_eq!(cache.size_bytes(), 0);
    }

    #[test]
    fn the_default_ceiling_is_one_gigabyte() {
        assert_eq!(CAPACITY_BYTES, 1_073_741_824);
    }
}
