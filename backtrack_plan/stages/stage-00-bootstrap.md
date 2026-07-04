# Stage 0 — Bootstrap

## Objective
A repo where `just setup && just check` works on a fresh Fedora or Ubuntu machine:
four compiling crates, structured logging everywhere, CI green, CHANGELOG and
version policy in place. **Nothing product-specific yet** — this stage is pure
foundations, and skipping any of it (especially logging) is how projects become
undebuggable.

## Prerequisites
None. Read [../reference/stack.md](../reference/stack.md) §1 (layout) and
[../README.md](../README.md) (conventions) first.

## Tasks

### S00-T1 — Repository
Create the repo (this plan's parent folder is docs-only; code lives in a new
`backtrack/` source repo unless the human says otherwise — ask once).
`git init`, `LICENSE` = GPL-3.0-or-later full text, `.gitignore` (Rust +
`/target`, `.flatpak-builder/`, `*.db`), root `README.md` (one paragraph + link to
docs), `CODE_OF_CONDUCT.md` optional — skip unless human asks.
**Accept:** initial commit exists; `git log` shows it; license file is the real GPL-3 text.

### S00-T2 — Cargo workspace
Workspace `Cargo.toml` with members `crates/backtrack-core`, `crates/backtrackd`,
`crates/backtrack-gtk`, `crates/backtrack-cli`. Workspace-level
`[workspace.package]` sets `version = "0.1.0"` (set once, never touched by AI again),
edition, license. All crates inherit via `version.workspace = true`.
Core is a lib; the others are bins that for now just init logging and print a
startup line via `tracing::info!`. Pin key deps at workspace level:
`tokio` (rt-multi-thread, process, sync), `zbus`, `rusqlite` (bundled feature),
`serde`/`serde_json`, `thiserror`, `tracing`, `tracing-subscriber`,
`tracing-appender`, `clap` (cli), `gtk4`+`libadwaita` (gtk crate only), `oo7` (keyring).
**Accept:** `cargo build --workspace` succeeds; `cargo run -p backtrackd` logs a
structured startup line and exits cleanly on SIGTERM.

### S00-T3 — Logging (DAY ONE — non-negotiable)
In backtrack-core, a `logging` module used by every binary:
- `tracing` for all instrumentation. Levels: `error` (user-visible failure),
  `warn` (recovered/degraded), `info` (state changes: backup started/finished,
  job lifecycle), `debug` (per-operation detail), `trace` (per-file detail).
- Two layers: pretty console (respects `RUST_LOG`, default `info`) + JSONL file at
  `~/.local/share/backtrack/logs/<binary>.jsonl` via `tracing-appender` daily
  rotation, keep 14 days (implement a small cleanup on startup).
- Panics logged via a panic hook before abort.
- CI guard (T7) fails on `println!`/`eprintln!`/`dbg!` outside tests and build scripts.
**Accept:** running the daemon writes both console and JSONL output; a test asserts
the JSONL line parses and contains `timestamp`, `level`, `target`, `message`.

### S00-T4 — Justfile
A `Justfile` at repo root. Recipes (all must work on Fedora; use per-distro branches
where needed):
- `setup` — install system deps: Fedora `dnf install` / Ubuntu (22.04+) `apt install`
  detection via `/etc/os-release`. Installs: rust (via rustup if absent), just is
  assumed present, `gtk4-devel libadwaita-devel sqlite-devel dbus-devel`,
  `borgbackup`, `flatpak-builder`, `python3-gobject` (for the nautilus extension
  later). Prints what it's doing; idempotent; ends by running `just check`.
- `build` / `build-release` — cargo build (workspace).
- `check` — `cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo test --workspace`.
- `test` / `test-integration` — unit vs `--features integration` split (integration
  needs borg installed; skip with warning if absent).
- `run-daemon` / `run-app` — run with `RUST_LOG=debug`, app pointed at session daemon.
- `demo-repo` — placeholder now; implemented in Stage 1 T7.
- `clean` — cargo clean + remove `.flatpak-builder`.
- Environment: recipes set `BACKTRACK_DEV=1` (daemon then uses
  `~/.local/share/backtrack-dev/` paths so dev never touches real backups).
**Accept:** on a clean Fedora container/VM, `just setup && just check` passes end-to-end.

### S00-T5 — CHANGELOG + version policy
- `CHANGELOG.md`: Keep a Changelog skeleton with `[Unreleased]` and `[0.1.0] - TBD`.
  Add a comment block at top restating the rules (categories: Added/Changed/
  Deprecated/Removed/Fixed/Security; entries are user-facing sentences; AI adds
  under `[Unreleased]` only).
- `VERSIONING.md`: SemVer, pre-1.0 semantics, and in bold: **version numbers are
  changed only by a human, only via `just bump-version`.** List every file that
  embeds the version (kept current as they appear): workspace Cargo.toml,
  metainfo.xml (Stage 13), flatpak manifest, RPM spec, deb changelog.
**Accept:** both files exist; CHANGELOG has the rules comment; VERSIONING.md lists
current version locations accurately.

### S00-T6 — `just bump-version NEW_VERSION`
Recipe that: validates SemVer format; updates every location listed in
VERSIONING.md (workspace Cargo.toml is the single Rust source of truth —
crates inherit); runs `cargo update -w` to refresh the lockfile; prints a diff
summary; does NOT commit (human reviews). Add a `just verify-version` recipe that
greps all listed locations and fails if they disagree (runs in CI).
**Accept:** `just bump-version 0.1.1` on a scratch branch updates all locations
consistently (`just verify-version` passes); branch discarded (version stays 0.1.0).

### S00-T7 — CI (GitHub Actions)
`.github/workflows/ci.yml`: on push/PR — `just check` inside a Fedora container
(install just + deps in the workflow), plus: println-guard
(`grep -rn 'println!\|eprintln!\|dbg!' crates/ --include='*.rs' | grep -v tests`
must be empty), `just verify-version`, and `cargo deny` or `cargo audit`
(pick `cargo-audit`; add config). Integration tests (borg) run in the same
container (`dnf install borgbackup`).
**Accept:** CI green on the bootstrap commit; deliberately adding a `println!`
in a scratch commit fails CI.

## Definition of Done
All acceptance criteria pass; `progress.md` Stage 0 all `[x]`; CHANGELOG
`[Unreleased]` notes the bootstrap; a fresh-machine walkthrough
(`git clone && just setup && just check`) is documented in the repo README.
