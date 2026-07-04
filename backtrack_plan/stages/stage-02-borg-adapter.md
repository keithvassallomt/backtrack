# Stage 2 — Borg Adapter

## Objective
`backtrack-core` can drive Borg 1.2/1.4 for every operation the product needs,
with typed errors, streamed progress, and the passphrase coming from the system
keyring. Everything Borg-specific lives behind one trait so Borg 2 later is a
second implementation, not a rewrite.

## Prerequisites
Stages 0–1. Read [../reference/stack.md](../reference/stack.md) §4–5 and
[../reference/open-questions.md](../reference/open-questions.md) Q3.

## Tasks

### S02-T1 — Trait + errors
`trait BackupEngine` (async): `create(spec) -> JobStream`, `list_archive(id) ->
impl Stream<Item=BorgItem>`, `extract(archive, paths, dest) -> JobStream`,
`extract_stdout(archive, path) -> impl AsyncRead`, `prune(policy)`, `compact()`,
`check(level)`, `init_repo(spec)`, `key_export() -> String`, `repo_info()`.
`JobStream` yields typed events: `Progress{current,total,phase}`, `Log{level,msg}`,
`ItemDone{path}`, `Finished{summary}`.
Error taxonomy (thiserror): `RepoUnreachable`, `PassphraseMissing`,
`PassphraseWrong`, `AuthFailed`, `DestinationFull`, `LocalDiskFull`,
`RepoCorrupt`, `LockedByOther`, `BorgMissing{needed,found}`, `BorgFailed{code,stderr}`.
These map 1:1 to the failure catalogue in [../reference/health.md](../reference/health.md).
**Accept:** trait compiles with a `MockEngine` used by daemon/GUI tests later;
every health.md catalogue row has a corresponding error variant (unit test asserts
the mapping table is exhaustive).

### S02-T2 — BorgCli implementation
Spawn `borg` (tokio process) with `--log-json`; parse stderr JSONL into JobStream
events (`archive_progress`, `progress_percent`, `progress_message`, `log_message`).
Rules from research: never trust `progress_percent` for partial extracts (byte
totals come from the index); `list` uses `--json-lines` with an explicit
`--format` including size, mtime, mode, and content metadata for change detection.
Environment: `BORG_PASSPHRASE` from the provider (T3), `BORG_RELOCATED_REPO_ACCESS_IS_OK=no`,
locale pinned. Version probe at startup: require ≥1.2, error `BorgMissing` otherwise.
Exit-code + stderr-pattern → error taxonomy mapping table (documented in code).
**Accept:** integration tests (feature-gated, borg in CI): init temp repo → create
2 archives from a fixture tree → list streams correct items → extract single file
via stdout matches source bytes → prune+compact runs; wrong passphrase yields
`PassphraseWrong`; unplugged path yields `RepoUnreachable`.

### S02-T3 — Passphrase provider
`trait SecretStore` with `oo7` (Secret Service / keyring) implementation:
get/set/delete under a stable attribute set (app-id + repo-id). Missing entry →
`PassphraseMissing` (NOT a prompt — prompting is UI's job, per health.md).
Dev mode (`BACKTRACK_DEV=1`) uses a file-backed store to keep CI headless.
**Accept:** round-trip test against a mock/file store; daemon integration test:
delete key → next backup fails with `PassphraseMissing` (this exact path powers
mockup 24 later).

### S02-T4 — Repo lifecycle ops
`init_repo` (encryption repokey-blake2, on local path or ssh:// or file path on a
mounted share), `import_repo` (open existing, verify passphrase, read archive list),
`key_export` (text form for the wizard's Save/Print recovery key). SMB/NFS
destinations are used as mounted paths (GIO/gvfs mount handling is Stage 9's
problem; core just takes a path).
**Accept:** integration test: init → key_export non-empty → import same repo with
right/wrong passphrase behaves correctly.

### S02-T5 — CI integration suite
`just test-integration` runs the real-borg tests; CI job includes them (borg
installed in the Fedora container). Keep each test < 30 s; parallel temp dirs.
**Accept:** suite green in CI two consecutive runs (flake check).

## Definition of Done
All accept criteria pass; `MockEngine` exported for later stages; progress.md
updated; decision notes (exact borg flags chosen, format string) appended to
progress.md Notes.
