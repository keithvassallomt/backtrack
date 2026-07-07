# Versioning policy

Backtrack follows [Semantic Versioning 2.0.0](https://semver.org/).

## Pre-1.0 semantics

While the version is `0.y.z`, the public surface is not yet stable:

- **`0.MINOR`** (`0.y`) is bumped for new features **and** breaking changes.
- **`0.MINOR.PATCH`** (`0.y.z`) is bumped for backwards-compatible bug fixes.

From `1.0.0` onward, the usual SemVer rules apply (MAJOR = breaking,
MINOR = additive, PATCH = fixes).

## The one rule

**Version numbers are changed only by a human, and only via `just bump-version`.**
No automated process and no AI contributor may edit a version number by hand.
`just bump-version NEW_VERSION` updates every location below in lockstep;
`just verify-version` (run in CI) fails if they ever disagree.

## Version locations

The workspace `Cargo.toml` is the single Rust source of truth — the four crates
inherit it with `version.workspace = true`. Every other location is derived and
must be kept in sync by `just bump-version`.

| Location | Status | Notes |
|---|---|---|
| `Cargo.toml` (`[workspace.package].version`) | **active** | Source of truth; crates inherit. |
| `packaging/*/io.github.keithvassallomt.Backtrack.metainfo.xml` | planned (Stage 13) | AppStream `<release>` version. |
| `packaging/flatpak/*.yml` manifest | planned (Stage 13) | Bundled build/tag. |
| `packaging/rpm/backtrack.spec` | planned (Stage 13) | `Version:` field. |
| `packaging/deb/changelog` | planned (Stage 13) | Top entry version. |

Only **active** locations are checked by `just verify-version`. As each packaging
file is added in Stage 13, move its row to **active** and extend the
`bump-version` / `verify-version` recipes to cover it, in the same commit.
