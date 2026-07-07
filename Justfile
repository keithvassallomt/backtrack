# Backtrack — task runner.
#
# `just` with no arguments lists all recipes. Dev recipes run with
# BACKTRACK_DEV=1 so the daemon uses ~/.local/share/backtrack-dev/ paths and
# never touches real backups or logs.

# Development runs never touch the real backup/log location.
export BACKTRACK_DEV := "1"

# Show available recipes.
default:
    @just --list

# ─── Setup ──────────────────────────────────────────────────────────────────

# Install system deps (Fedora/Ubuntu) + Rust if absent, then run checks. Idempotent.
setup:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ ! -r /etc/os-release ]]; then
        echo "Cannot detect distro (/etc/os-release missing)." >&2
        exit 1
    fi
    # shellcheck disable=SC1091
    source /etc/os-release
    echo "Detected: ${NAME} ${VERSION_ID:-}"

    if ! command -v cargo >/dev/null 2>&1; then
        echo "Installing Rust via rustup…"
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        # shellcheck disable=SC1091
        source "${HOME}/.cargo/env"
    fi

    case "${ID}" in
        fedora|rhel|centos|rocky|almalinux)
            echo "Installing dependencies with dnf…"
            sudo dnf install -y \
                gcc pkgconf-pkg-config \
                gtk4-devel libadwaita-devel sqlite-devel dbus-devel \
                borgbackup flatpak-builder python3-gobject
            ;;
        ubuntu|debian|pop|linuxmint)
            echo "Installing dependencies with apt…"
            sudo apt-get update
            sudo apt-get install -y \
                build-essential pkg-config \
                libgtk-4-dev libadwaita-1-dev libsqlite3-dev libdbus-1-dev \
                borgbackup flatpak-builder python3-gi
            ;;
        *)
            echo "Unsupported distro '${ID}'. Install the GTK4/libadwaita/sqlite/dbus" >&2
            echo "dev packages plus borgbackup and flatpak-builder manually." >&2
            exit 1
            ;;
    esac

    echo "Dependencies installed. Running checks…"
    just check

# ─── Build ──────────────────────────────────────────────────────────────────

# Debug build of the whole workspace.
build:
    cargo build --workspace

# Optimised release build.
build-release:
    cargo build --workspace --release

# ─── Quality gate ───────────────────────────────────────────────────────────

# Format check + clippy (warnings are errors) + unit tests + license headers. Mirrors CI.
check:
    cargo fmt --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    just check-license-headers

# Fail if any Rust source file under crates/ lacks an SPDX license header.
check-license-headers:
    #!/usr/bin/env bash
    set -euo pipefail
    missing=()
    while IFS= read -r f; do
        if ! head -n 3 "$f" | grep -q 'SPDX-License-Identifier: GPL-3.0-or-later'; then
            missing+=("$f")
        fi
    done < <(find crates -name '*.rs')
    if (( ${#missing[@]} > 0 )); then
        printf 'Missing SPDX header: %s\n' "${missing[@]}" >&2
        exit 1
    fi
    echo "All crate source files carry an SPDX license header."

# Unit tests only.
test:
    cargo test --workspace

# Integration tests (real borg); skipped with a warning if borg is absent.
test-integration:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v borg >/dev/null 2>&1; then
        echo "borg not found — skipping integration tests." >&2
        exit 0
    fi
    cargo test --workspace --features integration

# ─── Versioning (human-only — see VERSIONING.md) ────────────────────────────

# Set the version across all tracked locations. Human-only; does NOT commit.
bump-version NEW_VERSION:
    #!/usr/bin/env bash
    set -euo pipefail
    new="{{NEW_VERSION}}"
    semver='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$'
    if ! [[ "$new" =~ $semver ]]; then
        echo "Not a valid SemVer version: '$new'" >&2
        exit 1
    fi
    old="$(awk '/^\[workspace\.package\]/{p=1} p&&/^version = /{gsub(/[",]/,"",$3); print $3; exit}' Cargo.toml)"
    if [[ "$old" == "$new" ]]; then
        echo "Version is already $new; nothing to do."
        exit 0
    fi
    # Update only the version line inside [workspace.package]; crates inherit it.
    awk -v new="$new" '
        /^\[workspace\.package\]/ { inpkg = 1 }
        /^\[/ && $0 !~ /^\[workspace\.package\]/ { inpkg = 0 }
        inpkg && /^version = / { sub(/"[^"]*"/, "\"" new "\"") }
        { print }
    ' Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml
    # Refresh the lockfile so the workspace crates record the new version.
    cargo update --workspace --quiet
    echo "Bumped version: $old -> $new"
    echo "Locations updated: Cargo.toml (+ Cargo.lock). Review and commit manually:"
    git --no-pager diff --stat

# Fail if version locations disagree (Cargo.toml vs the workspace crates in the lockfile).
verify-version:
    #!/usr/bin/env bash
    set -euo pipefail
    python3 - <<'PY'
    import sys, tomllib
    with open("Cargo.toml", "rb") as f:
        want = tomllib.load(f)["workspace"]["package"]["version"]
    with open("Cargo.lock", "rb") as f:
        lock = tomllib.load(f)
    crates = {"backtrack-core", "backtrackd", "backtrack-gtk", "backtrack-cli"}
    bad = [(p["name"], p["version"]) for p in lock["package"]
           if p["name"] in crates and p["version"] != want]
    if bad:
        print(f"Version mismatch (workspace is {want}): {bad}", file=sys.stderr)
        sys.exit(1)
    print(f"verify-version OK: all workspace crates at {want}")
    PY

# ─── Run ────────────────────────────────────────────────────────────────────

# Run the daemon with debug logging.
run-daemon:
    RUST_LOG=debug cargo run -p backtrackd

# Run the GTK app with debug logging (talks to the session daemon).
run-app:
    RUST_LOG=debug cargo run -p backtrack-gtk

# Generate a demo Borg repo + index for development. Implemented in Stage 1.
demo-repo:
    @echo "demo-repo: implemented in Stage 1 (S01-T7)."

# Remove build artifacts.
clean:
    cargo clean
    rm -rf .flatpak-builder

# ─── GitHub board (progress.md is the source of truth) ──────────────────────

# Reconcile the board FROM progress.md (dry run — shows changes).
sync-board:
    python3 scripts/board.py sync

# Apply the reconciliation to GitHub (open/close issues, move Status columns).
sync-board-apply:
    python3 scripts/board.py sync --execute

# Create any missing issues/milestones for tasks in progress.md (dry run).
provision-board:
    python3 scripts/board.py provision

# Apply provisioning to GitHub (idempotent; skips issues that already exist).
provision-board-apply:
    python3 scripts/board.py provision --execute
