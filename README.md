# Backtrack

Backtrack is a Time Machine-class backup and restore application for desktop
Linux, built on [Borg Backup](https://www.borgbackup.org/). It is restore-first:
you browse your folders back through time and bring files back, while the backups
themselves stay quietly out of the way.

## Building from source

Backtrack targets Fedora and Ubuntu (24.04 LTS or newer) and uses
[`just`](https://github.com/casey/just) as its task runner.

```sh
git clone https://github.com/keithvassallomt/backtrack.git
cd backtrack
just setup    # installs system deps (GTK4, libadwaita, SQLite, D-Bus, borg) + Rust
just check    # fmt, clippy, and tests — the full quality gate
```

`just` on its own lists every recipe. Development runs (`just run-daemon`,
`just run-app`) use `BACKTRACK_DEV=1` so they never touch real backups or logs.

## Documentation

The full implementation plan, architecture, and design docs live in
[`backtrack_plan/`](backtrack_plan/README.md). Start with the
[product brief](backtrack_plan/reference/brief.md) and the
[architecture](backtrack_plan/reference/stack.md); task status is tracked in
[`backtrack_plan/progress.md`](backtrack_plan/progress.md).

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
