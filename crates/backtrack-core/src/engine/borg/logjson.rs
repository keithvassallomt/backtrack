// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Parse one `borg --log-json` stderr line into a [`Parsed`] record.
//!
//! Mapping rules (per the Stage 2 research):
//! - `progress_percent` → [`Parsed::Progress`]; a negative `total` means the
//!   denominator is unknown (`None`). Never trusted for partial extracts —
//!   honest restore % is computed from index byte totals downstream.
//! - `archive_progress` → [`Parsed::Progress`] keyed on `original_size`.
//! - `file_status` → [`Parsed::ItemDone`].
//! - `log_message` → [`Parsed::Log`] (retained for error classification).
//! - anything else / non-JSON → [`Parsed::Ignore`].

use serde::Deserialize;

use crate::engine::LogLevel;

/// A single classified log line. Consumed by classification and `BorgCli`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Progress {
        current: u64,
        total: Option<u64>,
        phase: String,
    },
    ItemDone {
        path: String,
    },
    Log {
        level: LogLevel,
        msgid: Option<String>,
        message: String,
    },
    Ignore,
}

#[derive(Deserialize)]
struct Raw {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    msgid: Option<String>,
    #[serde(default)]
    levelname: Option<String>,
    #[serde(default)]
    current: Option<i64>,
    #[serde(default)]
    total: Option<i64>,
    #[serde(default)]
    original_size: Option<i64>,
    #[serde(default)]
    path: Option<String>,
}

fn level_from(name: Option<&str>) -> LogLevel {
    match name.unwrap_or("INFO") {
        "DEBUG" => LogLevel::Debug,
        "WARNING" => LogLevel::Warning,
        "ERROR" | "CRITICAL" => LogLevel::Error,
        _ => LogLevel::Info,
    }
}

/// Parse one stderr line. Never fails: unrecognised input is [`Parsed::Ignore`].
pub fn parse_log_line(line: &str) -> Parsed {
    let raw: Raw = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(_) => return Parsed::Ignore,
    };
    match raw.kind.as_str() {
        "progress_percent" => Parsed::Progress {
            current: raw.current.unwrap_or(0).max(0) as u64,
            total: match raw.total {
                Some(t) if t >= 0 => Some(t as u64),
                _ => None,
            },
            phase: raw.message.unwrap_or_default(),
        },
        "archive_progress" => Parsed::Progress {
            current: raw.original_size.unwrap_or(0).max(0) as u64,
            total: None,
            phase: "archiving".to_string(),
        },
        "file_status" => match raw.path {
            Some(path) => Parsed::ItemDone { path },
            None => Parsed::Ignore,
        },
        "log_message" => Parsed::Log {
            level: level_from(raw.levelname.as_deref()),
            msgid: raw.msgid,
            message: raw.message.unwrap_or_default(),
        },
        _ => Parsed::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::LogLevel;

    #[test]
    fn parses_progress_percent_with_total() {
        let line = r#"{"type":"progress_percent","message":"Calculating","current":40,"total":100,"finished":false,"msgid":"extract"}"#;
        assert_eq!(
            parse_log_line(line),
            Parsed::Progress {
                current: 40,
                total: Some(100),
                phase: "Calculating".into()
            }
        );
    }

    #[test]
    fn progress_percent_negative_total_is_unknown() {
        let line =
            r#"{"type":"progress_percent","message":"","current":5,"total":-1,"finished":false}"#;
        assert_eq!(
            parse_log_line(line),
            Parsed::Progress {
                current: 5,
                total: None,
                phase: "".into()
            }
        );
    }

    #[test]
    fn parses_archive_progress_as_progress() {
        let line = r#"{"type":"archive_progress","original_size":2048,"compressed_size":1024,"nfiles":3,"path":"home/a"}"#;
        assert_eq!(
            parse_log_line(line),
            Parsed::Progress {
                current: 2048,
                total: None,
                phase: "archiving".into()
            }
        );
    }

    #[test]
    fn parses_file_status_as_item_done() {
        let line = r#"{"type":"file_status","status":"A","path":"home/report.odt"}"#;
        assert_eq!(
            parse_log_line(line),
            Parsed::ItemDone {
                path: "home/report.odt".into()
            }
        );
    }

    #[test]
    fn parses_error_log_message_with_msgid() {
        let line = r#"{"type":"log_message","levelname":"ERROR","name":"borg","message":"Repository does not exist.","msgid":"Repository.DoesNotExist"}"#;
        assert_eq!(
            parse_log_line(line),
            Parsed::Log {
                level: LogLevel::Error,
                msgid: Some("Repository.DoesNotExist".into()),
                message: "Repository does not exist.".into(),
            }
        );
    }

    #[test]
    fn file_status_without_path_is_ignored() {
        let line = r#"{"type":"file_status","status":"A"}"#;
        assert_eq!(parse_log_line(line), Parsed::Ignore);
    }

    #[test]
    fn unknown_type_and_garbage_are_ignored() {
        assert_eq!(
            parse_log_line(r#"{"type":"question_prompt"}"#),
            Parsed::Ignore
        );
        assert_eq!(parse_log_line("not json"), Parsed::Ignore);
    }
}
