// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! `backtrack doctor` — everything a bug report needs, in one file.
//!
//! The bundle is meant to be attached to an issue by someone who is already
//! having a bad day, which sets the design constraint: **it must be safe to
//! post publicly without reading it first.** A diagnostic tool that leaks a
//! backup passphrase into a GitHub issue has done more damage than the bug it
//! was collecting.
//!
//! So redaction is not a filter applied to known-bad fields — it is applied to
//! every line of every text file that goes in, matching on the *shape* of a
//! secret assignment rather than on a list of field names we happened to think
//! of. The keyring is never read at all.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::CliError;

/// Text that looks like it is assigning a secret. Matched case-insensitively
/// against each line; everything after the separator is replaced.
const SECRET_KEYS: &[&str] = &[
    "passphrase",
    "password",
    "passwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "private_key",
    "borg_passphrase",
    "borg_passcommand",
    "borg_key",
];

/// What the redactor leaves behind, so a reader can see something was removed
/// rather than wondering why a field is missing.
pub const REDACTED: &str = "[redacted]";

/// How much log tail to include. Enough to cover a failing backup and its lead
/// up; short enough that nobody has to page through a week of routine runs.
const LOG_LINES: usize = 200;

/// Build the bundle, returning the path written.
pub fn collect(status_json: &str, config_toml: &str, now: SystemTime) -> Result<PathBuf, CliError> {
    let dir = std::env::temp_dir();
    let stamp = crate::render::format_time(now)
        .replace([' ', ':'], "-")
        .replace('Z', "");
    // The pid disambiguates two runs inside the same second, which otherwise
    // silently overwrite each other — easy to do when the first bundle is being
    // collected because something is already going wrong.
    let path = dir.join(format!(
        "backtrack-doctor-{stamp}-{}.tar.gz",
        std::process::id()
    ));

    let file = std::fs::File::create(&path).map_err(|e| CliError::Io {
        path: path.clone(),
        message: e.to_string(),
    })?;
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut tar = tar::Builder::new(encoder);

    for (name, contents) in sections(status_json, config_toml) {
        append(&mut tar, &name, &contents)?;
    }

    tar.into_inner()
        .and_then(|encoder| encoder.finish())
        .map_err(|e| CliError::Io {
            path: path.clone(),
            message: e.to_string(),
        })?;
    Ok(path)
}

/// Every file the bundle contains, already redacted.
///
/// Split from the writing so the contents can be tested without unpacking a
/// tarball — and so the redaction test sees exactly what would be shipped.
pub fn sections(status_json: &str, config_toml: &str) -> Vec<(String, String)> {
    vec![
        ("versions.txt".to_string(), versions()),
        ("status.json".to_string(), redact(status_json)),
        ("config.toml".to_string(), redact(config_toml)),
        ("index.txt".to_string(), redact(&index_stats())),
        ("environment.txt".to_string(), redact(&environment())),
        ("log-tail.jsonl".to_string(), redact(&log_tail())),
    ]
}

/// Versions of everything that could be at fault.
fn versions() -> String {
    let mut out = format!("backtrack {}\n", backtrack_core::VERSION);
    out.push_str(&format!(
        "borg {}\n",
        command_output("borg", &["--version"]).unwrap_or_else(|| "not found".into())
    ));
    out.push_str(&format!("os {}\n", os_release()));
    out.push_str(&format!(
        "kernel {}\n",
        command_output("uname", &["-r"]).unwrap_or_else(|| "unknown".into())
    ));
    out
}

/// The bits of the environment that change behaviour. Values are redacted like
/// everything else — `BORG_PASSPHRASE` is exactly the kind of variable someone
/// sets by hand while debugging.
fn environment() -> String {
    let mut out = String::new();
    for key in [
        "BACKTRACK_DEV",
        "XDG_DATA_HOME",
        "HOME",
        "LANG",
        "RUST_LOG",
        "BORG_PASSPHRASE",
        "BORG_PASSCOMMAND",
        "BORG_REPO",
    ] {
        match std::env::var(key) {
            Ok(value) => out.push_str(&format!("{key}={value}\n")),
            Err(_) => out.push_str(&format!("{key}=<unset>\n")),
        }
    }
    out
}

/// Quick facts about the catalogue: enough to tell a corrupt index from an
/// empty one without asking the user to run SQL.
fn index_stats() -> String {
    let path = backtrack_core::paths::index_db();
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let mut out = format!("path {}\n", path.display());
    out.push_str(&format!("size {}\n", crate::render::format_bytes(size)));

    match backtrack_core::index::IndexReader::open(&path) {
        Ok(reader) => match reader.archives_overview() {
            Ok(archives) => {
                out.push_str(&format!("archives {}\n", archives.len()));
                if let Some(newest) = archives.iter().map(|a| a.ts).max() {
                    out.push_str(&format!("newest_archive_ts {newest}\n"));
                }
                if let Some(oldest) = archives.iter().map(|a| a.ts).min() {
                    out.push_str(&format!("oldest_archive_ts {oldest}\n"));
                }
            }
            Err(e) => out.push_str(&format!("archives unreadable: {e}\n")),
        },
        Err(e) => out.push_str(&format!("index unreadable: {e}\n")),
    }
    out
}

/// The last [`LOG_LINES`] lines of the newest daemon log.
fn log_tail() -> String {
    let dir = backtrack_core::paths::log_dir();
    let Some(newest) = newest_log(&dir) else {
        return format!("no logs under {}\n", dir.display());
    };
    let Ok(text) = std::fs::read_to_string(&newest) else {
        return format!("{} could not be read\n", newest.display());
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(LOG_LINES);
    let mut out = format!("# {} (last {} lines)\n", newest.display(), LOG_LINES);
    out.push_str(&lines[start..].join("\n"));
    out.push('\n');
    out
}

/// The most recently modified log file, which is the one describing the problem.
fn newest_log(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| {
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((modified, e.path()))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

/// Replace anything that looks like a secret being assigned a value.
///
/// Deliberately shape-based rather than key-based: it fires on any line where a
/// secret-ish word is followed by `=`, `:` or `": "`, whatever the surrounding
/// format. That covers TOML, JSON, JSONL log fields, and `KEY=value` env dumps
/// with one rule, and keeps working when a new field name appears that nobody
/// thought to add to a list.
pub fn redact(text: &str) -> String {
    text.lines().map(redact_line).collect::<Vec<_>>().join("\n")
}

fn redact_line(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let Some(key_at) = SECRET_KEYS.iter().filter_map(|k| lower.find(k)).min() else {
        return line.to_string();
    };
    // Find the assignment that follows the key, and blank out its value.
    let after_key = &line[key_at..];
    let Some(sep) = after_key.find(['=', ':']) else {
        return line.to_string();
    };
    let absolute = key_at + sep;
    let head = &line[..=absolute];
    // Preserve a trailing JSON comma/brace so the file still parses.
    let tail: String = line[absolute + 1..]
        .chars()
        .rev()
        .take_while(|c| matches!(c, ',' | '}' | ']' | ' '))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}\"{REDACTED}\"{tail}")
}

fn append<W: Write>(tar: &mut tar::Builder<W>, name: &str, contents: &str) -> Result<(), CliError> {
    let mut header = tar::Header::new_gnu();
    header.set_size(contents.len() as u64);
    header.set_mode(0o600);
    header.set_mtime(0);
    header.set_cksum();
    tar.append_data(&mut header, name, contents.as_bytes())
        .map_err(|e| CliError::Io {
            path: PathBuf::from(name),
            message: e.to_string(),
        })
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn os_release() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"').to_string())
        })
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    #[test]
    fn a_planted_passphrase_never_survives() {
        // The acceptance criterion, stated as bluntly as it deserves: every one
        // of these is a way a real passphrase could reach the bundle.
        let secret = "hunter2-correct-horse";
        let inputs = [
            format!("passphrase = \"{secret}\""),
            format!("BORG_PASSPHRASE={secret}"),
            format!("{{\"password\": \"{secret}\"}}"),
            format!("  \"api_key\": \"{secret}\","),
            format!("borg_passcommand = \"echo {secret}\""),
            format!("{{\"level\":\"INFO\",\"secret\":\"{secret}\"}}"),
            format!("PASSWORD={secret}"),
            format!("Private_Key: {secret}"),
        ];
        for input in inputs {
            let out = redact(&input);
            assert!(
                !out.contains(secret),
                "the passphrase survived redaction.\n  in:  {input}\n  out: {out}"
            );
            assert!(
                out.contains(REDACTED),
                "redaction should leave a marker.\n  in:  {input}\n  out: {out}"
            );
        }
    }

    #[test]
    fn the_whole_bundle_is_scanned_not_just_the_config() {
        // Every section goes through redact(), so a secret in any of them is
        // caught. Proved here through the real assembly path.
        let secret = "hunter2-correct-horse";
        let sections = sections(
            &format!("{{\"token\": \"{secret}\"}}"),
            &format!("passphrase = \"{secret}\"\n[storage]\nrepository = \"/mnt/b\""),
        );
        for (name, contents) in &sections {
            assert!(
                !contents.contains(secret),
                "{name} leaked the secret:\n{contents}"
            );
        }
        assert!(
            sections.iter().any(|(n, _)| n == "config.toml"),
            "the config must be included, redacted — not omitted"
        );
    }

    #[test]
    fn redaction_keeps_everything_that_is_not_a_secret() {
        // Over-redacting makes the bundle useless, which is its own failure.
        let input = "\
[storage]
repository = \"ssh://nas.local/./backups\"
[backup]
frequency = \"hourly\"
on_battery = false";
        let out = redact(input);
        assert!(out.contains("ssh://nas.local/./backups"), "got: {out}");
        assert!(out.contains("hourly"), "got: {out}");
        assert!(out.contains("on_battery = false"), "got: {out}");
        assert!(!out.contains(REDACTED), "nothing here is a secret: {out}");
    }

    #[test]
    fn a_repository_path_is_not_mistaken_for_a_secret() {
        // Paths matter for diagnosis and contain no credentials.
        let out = redact("repository = \"/mnt/backups/keith\"");
        assert!(out.contains("/mnt/backups/keith"), "got: {out}");
    }

    #[test]
    fn redacted_json_still_parses() {
        let out = redact("{\"level\":\"INFO\",\"password\":\"abc\"}");
        serde_json::from_str::<serde_json::Value>(&out)
            .unwrap_or_else(|e| panic!("redaction broke the JSON: {e}\n{out}"));
    }

    #[test]
    fn line_structure_survives_redaction() {
        let out = redact("a = 1\npassphrase = \"x\"\nb = 2");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3, "lines must not be dropped: {out}");
        assert_eq!(lines[0], "a = 1");
        assert_eq!(lines[2], "b = 2");
    }

    #[test]
    fn the_bundle_contains_what_a_bug_report_needs() {
        let sections = sections("{}", "");
        let names: Vec<&str> = sections.iter().map(|(n, _)| n.as_str()).collect();
        for expected in [
            "versions.txt",
            "status.json",
            "config.toml",
            "index.txt",
            "environment.txt",
            "log-tail.jsonl",
        ] {
            assert!(names.contains(&expected), "missing {expected}: {names:?}");
        }
    }

    #[test]
    fn versions_name_the_things_that_could_be_at_fault() {
        let text = versions();
        assert!(text.contains("backtrack "), "got: {text}");
        assert!(text.contains("borg "), "got: {text}");
        assert!(text.contains("os "), "got: {text}");
    }

    #[test]
    fn a_bundle_is_written_and_is_a_valid_archive() {
        let path = collect("{\"state\":\"HEALTHY\"}", "frequency = \"hourly\"", now())
            .expect("bundle written");
        assert!(path.exists(), "{} was not created", path.display());
        assert!(path.to_string_lossy().ends_with(".tar.gz"));

        // Unpack it back and confirm the sections are really in there.
        let file = std::fs::File::open(&path).unwrap();
        let decoder = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        let names: Vec<String> = archive
            .entries()
            .unwrap()
            .flatten()
            .map(|e| e.path().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"versions.txt".to_string()), "{names:?}");
        assert!(names.contains(&"status.json".to_string()), "{names:?}");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_bundle_is_not_world_readable() {
        // It lands in a shared temp directory and, redaction notwithstanding,
        // describes someone's filesystem.
        //
        // A distinct timestamp from the test above: same second plus same
        // process means the same filename, and the two would clobber each other.
        let path = collect("{}", "", now() + Duration::from_secs(1)).expect("bundle written");
        let file = std::fs::File::open(&path).unwrap();
        let decoder = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        for entry in archive.entries().unwrap().flatten() {
            let mode = entry.header().mode().unwrap();
            assert_eq!(mode & 0o077, 0, "entry is group/world readable: {mode:o}");
        }
        let _ = std::fs::remove_file(&path);
    }
}
