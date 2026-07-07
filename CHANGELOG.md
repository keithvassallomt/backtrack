<!--
  Changelog rules (keep this block):
  - Format follows Keep a Changelog (https://keepachangelog.com/en/1.1.0/).
  - Categories, in this order: Added, Changed, Deprecated, Removed, Fixed, Security.
  - Entries are user-facing sentences describing the change, not commit subjects.
  - AI/contributors add entries under [Unreleased] ONLY. A human moves them into a
    versioned section at release time via `just bump-version` (see VERSIONING.md).
-->

# Changelog

All notable changes to Backtrack are documented here. This project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) and the
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format.

## [Unreleased]

### Added
- Project bootstrap: Cargo workspace (core library plus daemon, GTK app, and CLI
  binaries), structured logging with JSONL rotation, developer task runner,
  versioning policy, and continuous integration.

## [0.1.0] - TBD

Initial development version.

[Unreleased]: https://github.com/keithvassallomt/backtrack/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/keithvassallomt/backtrack/releases/tag/v0.1.0
