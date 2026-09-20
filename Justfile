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

# Install system deps + Rust if absent, then run checks. Idempotent.
setup FAMILY="":
    #!/usr/bin/env bash
    set -euo pipefail

    # FAMILY is the installation type: dnf, apt, or pacman. Given explicitly it
    # overrides detection (`just setup pacman`); left empty it is detected from
    # /etc/os-release, and if that fails you are asked to choose.
    family="{{FAMILY}}"

    if [[ -z "${family}" && -r /etc/os-release ]]; then
        # shellcheck disable=SC1091
        source /etc/os-release
        echo "Detected: ${NAME:-unknown} ${VERSION_ID:-}"
        # ID first, then ID_LIKE so derivatives resolve to their parent family.
        for id in "${ID:-}" ${ID_LIKE:-}; do
            case "${id}" in
                fedora|rhel|centos|rocky|almalinux) family=dnf;    break ;;
                ubuntu|debian|pop|linuxmint)        family=apt;    break ;;
                arch|archlinux|manjaro|endeavouros) family=pacman; break ;;
            esac
        done
    fi

    if [[ -z "${family}" ]]; then
        if [[ ! -r /dev/tty ]]; then
            echo "Could not determine the installation type, and there is no" >&2
            echo "terminal to ask on. Re-run as: just setup dnf|apt|pacman" >&2
            exit 1
        fi
        echo "Could not match this system to a known package manager." >&2
        echo "  1) dnf     — Fedora, RHEL, CentOS, Rocky, Alma" >&2
        echo "  2) apt     — Ubuntu, Debian, Mint, Pop!_OS" >&2
        echo "  3) pacman  — Arch, Manjaro, EndeavourOS" >&2
        read -rp "Choose 1-3 (anything else aborts): " reply < /dev/tty
        case "${reply}" in
            1) family=dnf ;;
            2) family=apt ;;
            3) family=pacman ;;
            *)
                echo "Aborted. Install the GTK4/libadwaita/sqlite/dbus dev packages" >&2
                echo "plus borgbackup and flatpak-builder manually." >&2
                exit 1
                ;;
        esac
    fi

    if ! command -v cargo >/dev/null 2>&1; then
        echo "Installing Rust via rustup…"
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        # shellcheck disable=SC1091
        source "${HOME}/.cargo/env"
    fi

    case "${family}" in
        dnf)
            echo "Installing dependencies with dnf…"
            sudo dnf install -y \
                gcc pkgconf-pkg-config \
                gtk4-devel libadwaita-devel sqlite-devel dbus-devel \
                borgbackup flatpak-builder python3-gobject
            ;;
        apt)
            echo "Installing dependencies with apt…"
            sudo apt-get update
            sudo apt-get install -y \
                build-essential pkg-config \
                libgtk-4-dev libadwaita-1-dev libsqlite3-dev libdbus-1-dev \
                borgbackup flatpak-builder python3-gi
            ;;
        pacman)
            echo "Installing dependencies with pacman…"
            sudo pacman -S --needed --noconfirm \
                gcc pkgconf \
                gtk4 libadwaita sqlite dbus \
                borg flatpak-builder python-gobject
            ;;
        *)
            echo "Unknown installation type '${family}'." >&2
            echo "Expected one of: dnf, apt, pacman." >&2
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
    just check-prints

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

# Fail if the daemon or the library prints instead of logging through tracing.
# Test code and backtrack-cli are exempt; see the script for why.
check-prints:
    python3 scripts/check_prints.py

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
# Arguments are passed through: `just run-app --path /home` opens the demo
# fixture's root. BACKTRACK_THEME=dark|light forces a colour scheme for one run.
run-app *ARGS:
    RUST_LOG=debug cargo run -p backtrack-gtk -- {{ARGS}}

# Run the app in a forced colour scheme with no user GTK overrides, to check
# the window in both themes. A ~/.config/gtk-4.0/gtk.css that redefines the
# libadwaita palette loads above the theme stylesheet and keeps the window its
# own colours whatever scheme is asked for, so this runs against an empty
# config directory. Backtrack's own data lives under XDG_DATA_HOME and is
# unaffected.
theme-check SCHEME="light" *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{SCHEME}}" in
        dark|light) ;;
        *) echo "usage: just theme-check dark|light [-- app args]" >&2; exit 2 ;;
    esac
    clean="$(mktemp -d)"
    trap 'rm -rf "${clean}"' EXIT
    XDG_CONFIG_HOME="${clean}" BACKTRACK_THEME={{SCHEME}} \
        RUST_LOG=debug cargo run -p backtrack-gtk -- {{ARGS}}

# Generate a demo Borg repo + index for development (scripted 30-snapshot
# history under ~/.local/share/backtrack-dev/). Idempotent; rebuilds from scratch.
demo-repo:
    cargo run --quiet -p xtask

# ─── systemd / D-Bus units (development install) ────────────────────────────

# Install dev-mode user units so the daemon starts on demand. Idempotent.
install-units:
    #!/usr/bin/env bash
    set -euo pipefail
    root="{{justfile_directory()}}"
    binary="${root}/target/debug/backtrackd"
    systemd_dir="${XDG_CONFIG_HOME:-${HOME}/.config}/systemd/user"
    dbus_dir="${XDG_DATA_HOME:-${HOME}/.local/share}/dbus-1/services"

    echo "Building the daemon so the unit points at something that exists…"
    cargo build -p backtrackd

    mkdir -p "${systemd_dir}" "${dbus_dir}"

    # The shipped units are the real, packaged ones. The dev install rewrites
    # them rather than keeping a second copy in the tree, so the thing being
    # tested here is the file that will actually be packaged.
    #
    # Two substitutions: the binary path, and the bus name — dev mode uses
    # org.backtrack.Daemon1.Dev so a development daemon and an installed one can
    # never answer each other's clients.
    sed -e "s|^ExecStart=.*|ExecStart=${binary}|" \
        -e "s|^BusName=org.backtrack.Daemon1$|BusName=org.backtrack.Daemon1.Dev|" \
        -e "s|^\[Service\]$|[Service]\nEnvironment=BACKTRACK_DEV=1|" \
        "${root}/packaging/systemd/backtrackd.service" \
        > "${systemd_dir}/backtrackd.service"

    sed -e "s|^Exec=.*|Exec=${binary}|" \
        -e "s|^Name=org.backtrack.Daemon1$|Name=org.backtrack.Daemon1.Dev|" \
        "${root}/packaging/dbus/org.backtrack.Daemon1.service" \
        > "${dbus_dir}/org.backtrack.Daemon1.Dev.service"

    systemctl --user daemon-reload
    # Ask the bus to rescan its service directory, so activation works in this
    # session rather than only after the next login.
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus ReloadConfig >/dev/null 2>&1 || true

    echo "Installed:"
    echo "  ${systemd_dir}/backtrackd.service"
    echo "  ${dbus_dir}/org.backtrack.Daemon1.Dev.service"
    echo
    echo "The daemon now starts on demand. Try:"
    echo "  busctl --user call org.backtrack.Daemon1.Dev /org/backtrack/Daemon1 \\"
    echo "      org.backtrack.Daemon1 GetStatus"

# Remove the dev-mode user units.
uninstall-units:
    #!/usr/bin/env bash
    set -euo pipefail
    systemd_dir="${XDG_CONFIG_HOME:-${HOME}/.config}/systemd/user"
    dbus_dir="${XDG_DATA_HOME:-${HOME}/.local/share}/dbus-1/services"

    systemctl --user stop backtrackd.service 2>/dev/null || true
    systemctl --user disable backtrackd.service 2>/dev/null || true
    rm -f "${systemd_dir}/backtrackd.service"
    rm -f "${dbus_dir}/org.backtrack.Daemon1.Dev.service"
    systemctl --user daemon-reload
    echo "Dev units removed."

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
