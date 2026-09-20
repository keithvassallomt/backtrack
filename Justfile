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
    #!/usr/bin/env bash
    set -euo pipefail
    # Forcing a colour scheme means wanting to see it, and on a desktop that
    # has customised its colours the scheme alone cannot deliver that: a
    # ~/.config/gtk-4.0/gtk.css redefining the libadwaita palette loads above
    # the theme stylesheet and keeps its own colours whichever scheme is set.
    # So a run that forces a scheme also runs with an empty config directory.
    # Backtrack's own data is under XDG_DATA_HOME and is untouched by this.
    if [[ -n "${BACKTRACK_THEME:-}" ]]; then
        clean="$(mktemp -d)"
        trap 'rm -rf "${clean}"' EXIT
        export XDG_CONFIG_HOME="${clean}"
    fi
    RUST_LOG="${RUST_LOG:-debug}" cargo run -p backtrack-gtk -- {{ARGS}}

# Check the window in one of the two colour schemes. A thin name for
# `BACKTRACK_THEME=<scheme> just run-app`, which is what does the work.
theme-check SCHEME="light" *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{SCHEME}}" in
        dark|light) ;;
        *) echo "usage: just theme-check dark|light [app args]" >&2; exit 2 ;;
    esac
    BACKTRACK_THEME={{SCHEME}} just run-app {{ARGS}}

# Generate a demo Borg repo + index for development (scripted 30-snapshot
# history under ~/.local/share/backtrack-dev/). Idempotent; rebuilds from scratch.
demo-repo:
    cargo run --quiet -p xtask

# Stage a folder that needs every kind of restore decision, so the folder
# summary and its review list have something real to show. Idempotent. Run it
# after `just demo-repo`, which wipes demo-src and takes this with it.
#
# The folder is built, backed up for real, and only then edited on disk. That
# order is the whole point: a conflict is the backup and the disk genuinely
# disagreeing about a file, so there is no way to write one straight into the
# fixture — it has to be lived through.
#
# `just --list` shows the attribute below, not the last line of this block.
[doc("Stage a folder that needs every kind of restore decision.")]
demo-conflicts:
    #!/usr/bin/env bash
    set -euo pipefail

    data="${HOME}/.local/share/backtrack-dev"
    folder="${data}/demo-src/home/Projects/website"
    cli="{{justfile_directory()}}/target/debug/backtrack"

    # This asks the daemon to back up whatever it is configured to back up, so
    # it checks first that the answer is the demo fixture and not a real home
    # directory.
    if ! grep -q 'demo-src' "${data}/config.toml" 2>/dev/null; then
        echo "Refusing: ${data}/config.toml does not back up demo-src." >&2
        echo "Run 'just demo-repo' first, then point the dev config at it." >&2
        exit 1
    fi

    cargo build --quiet -p backtrack-cli

    # Four dates, because "newer" has to be unambiguous on screen. Most of the
    # folder is two months old. The three files the person had just edited when
    # the backup ran are two days old, and they are what produces a conflict
    # where the *backup* is the newer side — the case a fixture built only by
    # editing things afterwards can never reach.
    long_ago="$(date -d '60 days ago' +%s)"
    lately="$(date -d '2 days ago' +%s)"
    yesterday="$(date -d '1 day ago' +%s)"
    way_back="$(date -d '40 days ago' +%s)"

    # Revision 2 of a file is always longer than revision 1, so a changed file
    # differs in size as well as in time. That is deliberate: it exercises the
    # path where the compare settles it from metadata without reading bytes.
    body() {
        local line
        printf 'Acme Tooling website — %s (revision %s)\n' "$1" "$2"
        for (( line = 0; line < ${#1} + $2 * 9; line++ )); do
            printf '%s\n' "$1"
        done
    }
    # Real folders do not have every file modified in the same minute, and a
    # dialog full of one repeated timestamp reads as a mock-up rather than as a
    # fixture. Each file steps back a further 4177 seconds — deterministic, so
    # two runs of this recipe produce the same folder, and small enough that it
    # never reorders the four dates against each other.
    step=0
    dated() {
        touch -d "@$(( $1 - step * 4177 ))" "${folder}/$2"
        step=$(( step + 1 ))
    }
    seed() {
        mkdir -p "${folder}/$(dirname "$1")"
        body "$1" 1 > "${folder}/$1"
    }
    seed_at() {
        seed "$2"
        dated "$1" "$2"
    }
    revise() {
        body "$2" 2 > "${folder}/$2"
        dated "$1" "$2"
    }

    # ── The folder as the backup will find it ──
    untouched=(index.html about.html css/print.css js/analytics.js img/hero.jpg
               content/faq.md content/blog/2026-01-launch.md data/team.json
               LICENSE deploy.sh config/site.toml)
    edited_since=(contact.html css/main.css js/app.js content/home.md README.md)
    rolled_back=(content/pricing.md data/menu.json content/blog/2026-06-pricing.md)
    type_changed=(config/redirects.txt)
    deleted_since=(content/blog/2026-03-redesign.md img/team.jpg img/logo.png notes.txt)
    written_since=(drafts/newsletter.md css/dark.css js/vendor.js)

    rm -rf "${folder}"
    for file in "${untouched[@]}" "${edited_since[@]}" "${type_changed[@]}" \
                "${deleted_since[@]}"; do
        seed_at "${long_ago}" "${file}"
    done
    for file in "${rolled_back[@]}"; do
        seed_at "${lately}" "${file}"
    done

    # Counted before anything is disturbed. A directory that exists on both
    # sides is nothing to restore, but it is still an entry in the plan.
    folders_in_backup="$(find "${folder}" -type d | wc -l)"

    # ── Back it up, and wait for the archive to be catalogued ──
    was="$("${cli}" status --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["last_backup"] or 0)')"
    echo "Backing the folder up…"
    "${cli}" backup-now > /dev/null

    settled() {
        "${cli}" status --json | python3 -c 'import json, sys; s = json.load(sys.stdin); sys.exit(0 if s["active_job"] is None and (s["last_backup"] or 0) > int(sys.argv[1]) else 1)' "$1"
    }
    deadline=$(( SECONDS + 180 ))
    until settled "${was}"; do
        if (( SECONDS > deadline )); then
            echo "The backup has not finished after three minutes; try 'backtrack status'." >&2
            exit 1
        fi
        sleep 1
    done

    # ── What happened to the folder afterwards ──

    # Worked on since the backup: the copy on disk is the newer side. This is
    # the risky direction, and the one the dialog has to say loudest.
    for file in "${edited_since[@]}"; do
        revise "${yesterday}" "${file}"
    done

    # Pulled back from somewhere older — a stale copy off a memory stick, a sync
    # that went the wrong way. Here the backup is the newer side.
    for file in "${rolled_back[@]}"; do
        revise "${way_back}" "${file}"
    done

    # A file in the backup, a directory on disk. Never settled by a blanket
    # answer: it starts unticked in the review list and needs its own decision.
    for file in "${type_changed[@]}"; do
        rm -f "${folder}/${file}"
        mkdir -p "${folder}/${file}"
    done

    # Deleted since. These come back.
    for file in "${deleted_since[@]}"; do
        rm -f "${folder}/${file}"
    done

    # Written since. These are kept — the row the summary exists to show.
    for file in "${written_since[@]}"; do
        seed_at "${yesterday}" "${file}"
    done

    # Counted from the lists above rather than written out, so the recipe and
    # the dialog can be held against each other. If they disagree, one of them
    # is wrong, and that is worth knowing.
    echo
    echo "Staged: ${folder}"
    echo
    echo "Restoring that folder from the newest backup should report"
    echo "  $(( ${#untouched[@]} + folders_in_backup )) identical (${#untouched[@]} files and ${folders_in_backup} folders)"
    echo "  $(( ${#edited_since[@]} + ${#rolled_back[@]} )) to replace, ${#edited_since[@]} of them newer on disk"
    echo "  ${#deleted_since[@]} only in the backup, to be added"
    echo "  $(( ${#written_since[@]} + 1 )) only on disk, kept"
    echo "  ${#type_changed[@]} changed type, which needs its own answer"
    echo
    echo "Look at it with"
    echo "  just run-app --path '${folder}'"
    echo "and press Ctrl+R with nothing selected."

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
