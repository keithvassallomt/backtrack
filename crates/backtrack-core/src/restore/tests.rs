// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The restore engine against real directory trees.
//!
//! The classification matrix is tested as a matrix in `classify`; these are
//! the tests that need a filesystem — that the walk finds what is there, that
//! the moves land where they should, and that the three promises the engine
//! makes hold when something goes wrong.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::*;

/// A staging tree, a destination, and somewhere to put safety copies.
struct Fixture {
    _dir: tempfile::TempDir,
    staging: PathBuf,
    dest: PathBuf,
    stash: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let staging = dir.path().join("staging");
    let dest = dir.path().join("dest");
    let stash = dir.path().join("replaced");
    for path in [&staging, &dest, &stash] {
        std::fs::create_dir_all(path).unwrap();
    }
    Fixture {
        _dir: dir,
        staging,
        dest,
        stash,
    }
}

/// Some fixed instant, so "newer" and "older" are decided by the test rather
/// than by how long it took to run.
fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + seconds)
}

fn write(root: &Path, relative: &str, contents: &str, when: SystemTime) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, contents).unwrap();
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_times(
        std::fs::FileTimes::new()
            .set_modified(when)
            .set_accessed(when),
    )
    .unwrap();
}

fn link(root: &Path, relative: &str, target: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(target, path).unwrap();
}

/// Every file under `root`, by relative path, with its contents — the shape a
/// test can compare before and after.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, String> {
    let mut found = BTreeMap::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        let Ok(listing) = std::fs::read_dir(root.join(&relative)) else {
            continue;
        };
        for item in listing.flatten() {
            let child = relative.join(item.file_name());
            let path = root.join(&child);
            let facts = std::fs::symlink_metadata(&path).unwrap();
            if facts.file_type().is_symlink() {
                found.insert(
                    child,
                    format!("-> {}", std::fs::read_link(&path).unwrap().display()),
                );
            } else if facts.is_dir() {
                pending.push(child);
            } else {
                found.insert(child, std::fs::read_to_string(&path).unwrap_or_default());
            }
        }
    }
    found
}

/// What a test asked for: the top-level names in the staging tree, which is
/// what "restore everything in this subtree" amounts to.
fn asked_for(staging: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(staging)
        .unwrap()
        .flatten()
        .map(|entry| PathBuf::from(entry.file_name()))
        .collect()
}

fn class_of<'a>(plan: &'a RestorePlan, relative: &str) -> &'a Entry {
    plan.entries
        .iter()
        .find(|e| e.path == Path::new(relative))
        .unwrap_or_else(|| panic!("{relative} not in the plan: {:?}", plan.entries))
}

// ── The compare pass ────────────────────────────────────────────────────────

#[test]
fn every_classification_shows_up_in_a_real_tree() {
    // A folder restore, which is the case where the whole subtree on disk is
    // in scope and "only on your disk — kept" has a meaning.
    let f = fixture();
    // Identical, by metadata alone.
    write(&f.staging, "Projects/same.txt", "hello", at(0));
    write(&f.dest, "Projects/same.txt", "hello", at(0));
    // Identical, settled by reading: same size, different time.
    write(&f.staging, "Projects/touched.txt", "hello", at(0));
    write(&f.dest, "Projects/touched.txt", "hello", at(500));
    // A real edit, newer on disk.
    write(&f.staging, "Projects/edited.txt", "old text", at(0));
    write(&f.dest, "Projects/edited.txt", "new longer text", at(500));
    // A real edit, newer in the backup.
    write(
        &f.staging,
        "Projects/reverted.txt",
        "restored text",
        at(500),
    );
    write(&f.dest, "Projects/reverted.txt", "stale", at(0));
    // Only one side.
    write(&f.staging, "Projects/added.txt", "from the backup", at(0));
    write(&f.dest, "Projects/mine.txt", "only on disk", at(0));
    // A type change.
    write(&f.staging, "Projects/swapped", "now a file", at(0));
    std::fs::create_dir_all(f.dest.join("Projects/swapped")).unwrap();

    let plan = plan("snapshot-01", &f.staging, &f.dest, &asked_for(&f.staging)).unwrap();

    assert_eq!(class_of(&plan, "Projects/same.txt").class, Class::Identical);
    assert_eq!(
        class_of(&plan, "Projects/touched.txt").class,
        Class::Identical
    );
    assert_eq!(
        class_of(&plan, "Projects/edited.txt").class,
        Class::Conflict { disk_newer: true }
    );
    assert_eq!(
        class_of(&plan, "Projects/reverted.txt").class,
        Class::Conflict { disk_newer: false }
    );
    assert_eq!(
        class_of(&plan, "Projects/added.txt").class,
        Class::OnlyInBackup
    );
    assert_eq!(
        class_of(&plan, "Projects/mine.txt").class,
        Class::OnlyOnDisk
    );
    assert_eq!(
        class_of(&plan, "Projects/swapped").class,
        Class::TypeChanged
    );

    let counts = plan.counts();
    // `Projects` itself is a directory on both sides, so it counts as
    // identical alongside the two files that are.
    assert_eq!(counts.identical, 3);
    assert_eq!(counts.conflicts, 2);
    assert_eq!(counts.disk_newer, 1);
    assert_eq!(counts.only_in_backup, 1);
    assert_eq!(counts.only_on_disk, 1);
    assert_eq!(counts.type_changed, 1);
}

#[test]
fn restoring_one_file_does_not_claim_the_rest_of_the_folder_was_kept() {
    // Alice restores `report.odt`. The fifty other files in her Documents
    // folder are not part of the restore, and a summary announcing that fifty
    // files were kept would be alarming nonsense.
    let f = fixture();
    write(&f.staging, "report.odt", "the backup version", at(0));
    write(&f.dest, "report.odt", "the version on disk", at(500));
    write(&f.dest, "unrelated.txt", "nothing to do with it", at(0));
    write(&f.dest, "notes/deep.txt", "nor this", at(0));

    let plan = plan("snapshot-01", &f.staging, &f.dest, &asked_for(&f.staging)).unwrap();
    assert_eq!(plan.entries.len(), 1, "{:?}", plan.entries);
    assert_eq!(plan.counts().only_on_disk, 0);
}

#[test]
fn the_destination_is_only_walked_where_the_restore_reaches() {
    // A restore of Documents has no opinion about Pictures, and must not spend
    // time forming one.
    let f = fixture();
    write(&f.staging, "Documents/report.odt", "backup", at(0));
    write(&f.dest, "Documents/report.odt", "backup", at(0));
    write(&f.dest, "Pictures/holiday.jpg", "not involved", at(0));

    let plan = plan("snapshot-01", &f.staging, &f.dest, &asked_for(&f.staging)).unwrap();
    assert!(
        !plan.entries.iter().any(|e| e.path.starts_with("Pictures")),
        "Pictures should not appear in a restore of Documents: {:?}",
        plan.entries
    );
}

#[test]
fn a_restore_with_nothing_to_do_says_so() {
    let f = fixture();
    write(&f.staging, "same.txt", "hello", at(0));
    write(&f.dest, "same.txt", "hello", at(0));
    write(&f.dest, "mine.txt", "only on disk", at(0));
    assert!(
        plan("snapshot-01", &f.staging, &f.dest, &asked_for(&f.staging))
            .unwrap()
            .is_a_no_op()
    );
}

// ── The three promises ──────────────────────────────────────────────────────

#[test]
fn a_file_that_exists_only_on_disk_survives_every_decision() {
    // The number one fear about restoring a folder, and the property that
    // answers it. No combination of answers may remove or rename `mine.txt`.
    for decision in [Decision::Replace, Decision::KeepBoth, Decision::Skip] {
        let f = fixture();
        write(&f.staging, "shared.txt", "from the backup", at(0));
        write(&f.dest, "shared.txt", "mine, edited", at(500));
        write(&f.dest, "mine.txt", "only on disk", at(0));
        write(&f.dest, "deep/also-mine.txt", "only on disk too", at(0));

        let plan = plan("snapshot-01", &f.staging, &f.dest, &asked_for(&f.staging)).unwrap();
        execute(&plan, &Decisions::all(decision), &f.stash, at(900)).unwrap();

        let after = snapshot(&f.dest);
        assert_eq!(
            after.get(Path::new("mine.txt")).map(String::as_str),
            Some("only on disk"),
            "{decision:?} disturbed a disk-only file"
        );
        assert_eq!(
            after
                .get(Path::new("deep/also-mine.txt"))
                .map(String::as_str),
            Some("only on disk too"),
            "{decision:?} disturbed a nested disk-only file"
        );
    }
}

#[test]
fn replace_puts_the_file_it_overwrites_in_the_stash() {
    let f = fixture();
    write(&f.staging, "report.odt", "the backup version", at(0));
    write(&f.dest, "report.odt", "the version on disk", at(500));

    let plan = plan("snapshot-01", &f.staging, &f.dest, &asked_for(&f.staging)).unwrap();
    let report = execute(&plan, &Decisions::all(Decision::Replace), &f.stash, at(900)).unwrap();

    assert_eq!(report.restored, 1);
    assert_eq!(
        std::fs::read_to_string(f.dest.join("report.odt")).unwrap(),
        "the backup version"
    );
    let Some(Move::Replaced { stashed, .. }) = report.log.moves.first() else {
        panic!("expected a replacement, got {:?}", report.log.moves);
    };
    assert_eq!(
        std::fs::read_to_string(stashed).unwrap(),
        "the version on disk",
        "the overwritten file has to be recoverable"
    );
}

#[test]
fn keep_both_gives_the_backup_the_real_name() {
    // The inverse of a copy dialog: in a restore tool the user asked for the
    // backup version, so it takes the name.
    let f = fixture();
    write(&f.staging, "report.odt", "the backup version", at(0));
    write(&f.dest, "report.odt", "the version on disk", at(500));

    let plan = plan("snapshot-01", &f.staging, &f.dest, &asked_for(&f.staging)).unwrap();
    execute(
        &plan,
        &Decisions::all(Decision::KeepBoth),
        &f.stash,
        at(900),
    )
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(f.dest.join("report.odt")).unwrap(),
        "the backup version"
    );
    assert_eq!(
        std::fs::read_to_string(f.dest.join("report (current).odt")).unwrap(),
        "the version on disk"
    );
}

#[test]
fn a_second_keep_both_does_not_overwrite_the_first() {
    let f = fixture();
    write(&f.dest, "report (current).odt", "kept earlier", at(0));
    let chosen = keep_both_name(&f.dest.join("report.odt"), |p| p.exists());
    assert_eq!(chosen, f.dest.join("report (current 2).odt"));
}

#[test]
fn keep_both_handles_names_without_an_extension() {
    let taken = |_: &Path| false;
    assert_eq!(
        keep_both_name(Path::new("/tmp/Makefile"), taken),
        PathBuf::from("/tmp/Makefile (current)")
    );
    assert_eq!(
        keep_both_name(Path::new("/tmp/archive.tar.gz"), taken),
        PathBuf::from("/tmp/archive.tar (current).gz"),
    );
}

// ── Undo ────────────────────────────────────────────────────────────────────

#[test]
fn undo_puts_the_tree_back_exactly_as_it_was() {
    let f = fixture();
    write(&f.staging, "report.odt", "the backup version", at(0));
    write(&f.staging, "added.txt", "new from the backup", at(0));
    write(&f.staging, "kept/both.txt", "backup copy", at(0));
    write(&f.dest, "report.odt", "the version on disk", at(500));
    write(&f.dest, "kept/both.txt", "disk copy", at(500));
    write(&f.dest, "mine.txt", "only on disk", at(0));

    let before = snapshot(&f.dest);

    let plan = plan("snapshot-01", &f.staging, &f.dest, &asked_for(&f.staging)).unwrap();
    let decisions = Decisions::all(Decision::Replace).except("kept/both.txt", Decision::KeepBoth);
    let report = execute(&plan, &decisions, &f.stash, at(900)).unwrap();
    assert_ne!(
        snapshot(&f.dest),
        before,
        "the restore should have changed something"
    );

    let undone = undo(&report.log);
    assert!(undone.failures.is_empty(), "{:?}", undone.failures);
    assert_eq!(
        snapshot(&f.dest),
        before,
        "undo has to put back everything the restore moved"
    );
}

// ── Safety ──────────────────────────────────────────────────────────────────

#[test]
fn a_symlink_in_the_archive_cannot_write_outside_the_destination() {
    // The archive carries `data -> ../outside` and a file inside it. Following
    // that link would write into a directory the restore was never aimed at.
    let f = fixture();
    let outside = f.dest.parent().unwrap().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("passwd"), "untouched").unwrap();

    link(&f.dest, "data", outside.to_str().unwrap());
    write(&f.staging, "data/passwd", "owned", at(0));

    let plan = plan("snapshot-01", &f.staging, &f.dest, &asked_for(&f.staging)).unwrap();
    let report = execute(&plan, &Decisions::all(Decision::Replace), &f.stash, at(900)).unwrap();

    assert_eq!(
        std::fs::read_to_string(outside.join("passwd")).unwrap(),
        "untouched",
        "the restore escaped the destination"
    );
    assert!(
        !report.failures.is_empty() || !plan.refused.is_empty(),
        "escaping must be reported, not silently skipped"
    );
}

#[test]
fn a_symlink_is_restored_as_a_link_and_never_followed() {
    let f = fixture();
    write(&f.dest, "target.txt", "the real file", at(0));
    link(&f.staging, "shortcut", "target.txt");

    let plan = plan("snapshot-01", &f.staging, &f.dest, &asked_for(&f.staging)).unwrap();
    execute(&plan, &Decisions::all(Decision::Replace), &f.stash, at(900)).unwrap();

    let restored = f.dest.join("shortcut");
    let facts = std::fs::symlink_metadata(&restored).unwrap();
    assert!(facts.file_type().is_symlink(), "restored as a real file");
    assert_eq!(
        std::fs::read_link(&restored).unwrap(),
        Path::new("target.txt")
    );
    assert_eq!(
        std::fs::read_to_string(f.dest.join("target.txt")).unwrap(),
        "the real file",
        "restoring the link must not have written through it"
    );
}

#[test]
fn one_unwritable_file_does_not_abandon_the_rest_of_the_restore() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture();
    write(&f.staging, "locked/report.odt", "the backup version", at(0));
    write(&f.staging, "fine.txt", "also from the backup", at(0));
    write(&f.dest, "locked/report.odt", "the version on disk", at(500));
    // A directory nothing may be written into.
    std::fs::set_permissions(
        f.dest.join("locked"),
        std::fs::Permissions::from_mode(0o500),
    )
    .unwrap();

    let plan = plan("snapshot-01", &f.staging, &f.dest, &asked_for(&f.staging)).unwrap();
    let report = execute(&plan, &Decisions::all(Decision::Replace), &f.stash, at(900)).unwrap();

    // Put it back so the temporary directory can be cleaned up.
    std::fs::set_permissions(
        f.dest.join("locked"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(f.dest.join("fine.txt")).unwrap(),
        "also from the backup",
        "the rest of the restore should still have happened"
    );
    assert_eq!(report.failures.len(), 1, "{:?}", report.failures);
    assert!(report.failures[0].0.ends_with("report.odt"));
}

#[test]
fn a_move_that_fails_leaves_the_file_that_was_there() {
    // Atomicity as it is actually experienced: whatever goes wrong, the path
    // holds a whole file — the old one or the new one, never a piece of either.
    use std::os::unix::fs::PermissionsExt;
    let f = fixture();
    write(&f.staging, "locked/report.odt", "the backup version", at(0));
    write(&f.dest, "locked/report.odt", "the version on disk", at(500));
    std::fs::set_permissions(
        f.dest.join("locked"),
        std::fs::Permissions::from_mode(0o500),
    )
    .unwrap();

    let plan = plan("snapshot-01", &f.staging, &f.dest, &asked_for(&f.staging)).unwrap();
    execute(&plan, &Decisions::all(Decision::Replace), &f.stash, at(900)).unwrap();

    std::fs::set_permissions(
        f.dest.join("locked"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(f.dest.join("locked/report.odt")).unwrap(),
        "the version on disk",
        "a failed restore must not have damaged what was there"
    );
    assert!(
        snapshot(&f.dest)
            .keys()
            .all(|p| !p.to_string_lossy().contains("backtrack-partial")),
        "a partial file was left behind"
    );
}

#[test]
fn a_type_change_is_never_resolved_by_a_blanket_answer() {
    // "Replace changed files" must not turn a directory into a file behind the
    // user's back; that needs its own tick in the review list.
    let f = fixture();
    write(&f.staging, "swapped", "now a file", at(0));
    std::fs::create_dir_all(f.dest.join("swapped")).unwrap();
    write(&f.dest, "swapped/inside.txt", "still here", at(0));

    let plan = plan("snapshot-01", &f.staging, &f.dest, &asked_for(&f.staging)).unwrap();
    execute(&plan, &Decisions::all(Decision::Replace), &f.stash, at(900)).unwrap();

    assert!(
        f.dest.join("swapped").is_dir(),
        "a blanket Replace turned a directory into a file"
    );
    assert_eq!(
        std::fs::read_to_string(f.dest.join("swapped/inside.txt")).unwrap(),
        "still here"
    );
}

#[test]
fn a_type_change_is_carried_out_when_it_is_asked_for_by_name() {
    let f = fixture();
    write(&f.staging, "swapped", "now a file", at(0));
    std::fs::create_dir_all(f.dest.join("swapped")).unwrap();

    let plan = plan("snapshot-01", &f.staging, &f.dest, &asked_for(&f.staging)).unwrap();
    let decisions = Decisions::all(Decision::Skip).except("swapped", Decision::Replace);
    execute(&plan, &decisions, &f.stash, at(900)).unwrap();

    assert_eq!(
        std::fs::read_to_string(f.dest.join("swapped")).unwrap(),
        "now a file"
    );
}

// ── Space ───────────────────────────────────────────────────────────────────

#[test]
fn the_space_a_restore_needs_is_counted_before_it_starts() {
    let f = fixture();
    write(&f.staging, "big.bin", &"x".repeat(1000), at(0));
    write(&f.dest, "big.bin", &"y".repeat(400), at(500));
    write(&f.staging, "new.bin", &"z".repeat(50), at(0));

    let plan = plan("snapshot-01", &f.staging, &f.dest, &asked_for(&f.staging)).unwrap();

    let replacing = plan.space_needed(&Decisions::all(Decision::Replace));
    assert_eq!(replacing.dest, 1050, "both incoming files");
    assert_eq!(replacing.stash, 400, "only the one being overwritten");

    let skipping = plan.space_needed(&Decisions::all(Decision::Skip));
    assert_eq!(skipping.dest, 50, "the addition happens regardless");
    assert_eq!(skipping.stash, 0, "nothing is overwritten");
}

// ── The merge property ──────────────────────────────────────────────────────

#[test]
fn no_combination_of_answers_can_disturb_a_file_that_is_only_on_disk() {
    // S07-T2's property, swept rather than sampled: three conflicting files,
    // every one of the 27 ways of answering them, and after each the files that
    // exist only on disk must still be exactly where they were, byte for byte.
    //
    // A merge that never deletes is the first thing people want to be told
    // about folder restores, and the only convincing way to say it is to have
    // tried every way of getting it wrong.
    let conflicts = ["Projects/a.txt", "Projects/b.txt", "Projects/c.txt"];
    let untouchable = [
        ("Projects/mine.txt", "only on disk"),
        ("Projects/deep/also-mine.txt", "nested, only on disk"),
        (
            "Projects/deep/deeper/still-mine.txt",
            "deeply, only on disk",
        ),
    ];
    let answers = [Decision::Replace, Decision::KeepBoth, Decision::Skip];

    for first in answers {
        for second in answers {
            for third in answers {
                let f = fixture();
                for name in conflicts {
                    write(&f.staging, name, "from the backup", at(0));
                    write(&f.dest, name, "edited on disk", at(500));
                }
                for (name, contents) in untouchable {
                    write(&f.dest, name, contents, at(0));
                }

                let plan =
                    plan("snapshot-01", &f.staging, &f.dest, &asked_for(&f.staging)).unwrap();
                let decisions = Decisions::all(Decision::Skip)
                    .except(conflicts[0], first)
                    .except(conflicts[1], second)
                    .except(conflicts[2], third);
                execute(&plan, &decisions, &f.stash, at(900)).unwrap();

                let after = snapshot(&f.dest);
                for (name, contents) in untouchable {
                    assert_eq!(
                        after.get(Path::new(name)).map(String::as_str),
                        Some(contents),
                        "{first:?}/{second:?}/{third:?} disturbed {name}"
                    );
                }
                // And nothing was renamed aside that was not a conflict.
                let strays: Vec<_> = after
                    .keys()
                    .filter(|p| p.to_string_lossy().contains("(current)"))
                    .filter(|p| {
                        !conflicts.iter().any(|c| {
                            p.starts_with(Path::new(c).parent().unwrap())
                                && p.to_string_lossy()
                                    .contains(Path::new(c).file_stem().unwrap().to_str().unwrap())
                        })
                    })
                    .collect();
                assert!(
                    strays.is_empty(),
                    "{first:?}/{second:?}/{third:?} renamed something it should not have: {strays:?}"
                );
            }
        }
    }
}

#[test]
fn restoring_one_deep_file_does_not_walk_everything_above_it() {
    // Extracting `a/b/c/one.txt` recreates every directory above it, so the
    // staging tree's top-level name is `a` — and taking *that* as the scope
    // means walking all of `dest/a` to restore one file. On a real machine
    // that was four million files and ninety seconds, reported as "kept".
    let f = fixture();
    write(&f.staging, "a/b/c/one.txt", "from the backup", at(0));
    write(&f.dest, "a/b/c/one.txt", "on disk", at(500));
    // Plenty of neighbours at every level, none of them part of the restore.
    for neighbour in ["a/sibling.txt", "a/b/nephew.txt", "a/b/c/cousin.txt"] {
        write(&f.dest, neighbour, "nothing to do with it", at(0));
    }

    let plan = plan(
        "snapshot-01",
        &f.staging,
        &f.dest,
        &[PathBuf::from("a/b/c/one.txt")],
    )
    .unwrap();

    assert_eq!(
        plan.counts().only_on_disk,
        0,
        "neighbours are not part of this restore: {:?}",
        plan.entries
            .iter()
            .filter(|e| e.class == Class::OnlyOnDisk)
            .map(|e| &e.path)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        class_of(&plan, "a/b/c/one.txt").class,
        Class::Conflict { disk_newer: true }
    );
}

#[test]
fn restoring_a_folder_still_covers_everything_inside_it() {
    // The other half of the same rule: ask for a folder and its whole subtree
    // is in scope, disk-only files included.
    let f = fixture();
    write(&f.staging, "a/b/c/one.txt", "from the backup", at(0));
    write(&f.dest, "a/b/c/one.txt", "from the backup", at(0));
    write(&f.dest, "a/b/c/cousin.txt", "only on disk", at(0));
    write(&f.dest, "a/sibling.txt", "outside the restore", at(0));

    let plan = plan(
        "snapshot-01",
        &f.staging,
        &f.dest,
        &[PathBuf::from("a/b/c")],
    )
    .unwrap();
    assert_eq!(plan.counts().only_on_disk, 1, "{:?}", plan.entries);
    assert!(
        !plan.entries.iter().any(|e| e.path.ends_with("sibling.txt")),
        "a sibling of the restored folder is not in it"
    );
}

#[test]
fn the_directories_a_file_lives_in_are_not_counted_as_files_being_added() {
    // Extracting `a/b/c/one.txt` recreates `a`, `a/b` and `a/b/c`. Counting
    // those would tell someone restoring one file that four were added.
    let f = fixture();
    write(&f.staging, "a/b/c/one.txt", "from the backup", at(0));

    let plan = plan(
        "snapshot-01",
        &f.staging,
        &f.dest,
        &[PathBuf::from("a/b/c/one.txt")],
    )
    .unwrap();

    assert_eq!(plan.counts().only_in_backup, 1, "{:?}", plan.entries);
    assert_eq!(plan.entries.len(), 1);

    // And they are still made, because the file has to land somewhere.
    execute(&plan, &Decisions::all(Decision::Replace), &f.stash, at(900)).unwrap();
    assert_eq!(
        std::fs::read_to_string(f.dest.join("a/b/c/one.txt")).unwrap(),
        "from the backup"
    );
}

#[test]
fn a_folder_that_was_asked_for_is_not_treated_as_scaffolding() {
    // The folder itself is the thing being restored, so it counts.
    let f = fixture();
    write(&f.staging, "a/b/one.txt", "from the backup", at(0));

    let plan = plan("snapshot-01", &f.staging, &f.dest, &[PathBuf::from("a/b")]).unwrap();
    let names: Vec<String> = plan
        .entries
        .iter()
        .map(|e| e.path.to_string_lossy().to_string())
        .collect();
    assert_eq!(
        names,
        ["a/b", "a/b/one.txt"],
        "`a` is scaffolding; `a/b` is not"
    );
}
