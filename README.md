# Backtrack

Backtrack is a Time Machine-class backup and restore application for desktop
Linux, built on [Borg Backup](https://www.borgbackup.org/). It is restore-first:
you browse your folders back through time and bring files back, while the backups
themselves stay quietly out of the way.

## Installing

From the first release:

| Where | How |
|---|---|
| Any distribution | The Flatpak, from FriendlyHub: [packaging/flatpak/FRIENDLYHUB.md](packaging/flatpak/FRIENDLYHUB.md) |
| Fedora 44 and later | COPR: `sudo dnf copr enable keithvassallo/backtrack`, then `sudo dnf install backtrack` |
| Ubuntu 26.04 and later | PPA: `sudo add-apt-repository ppa:keithvassallomt/backtrack`, then `sudo apt install backtrack` |
| Arch | AUR: `backtrack` |

The native packages have the file managers' integrations as `backtrack-nautilus`
and `backtrack-dolphin`; the Flatpak installs them from Preferences.

## Building from source

Backtrack needs GTK 4.22 and libadwaita 1.9 (GNOME 50), so it builds on Fedora
44, Ubuntu 26.04 and Arch, or newer, and uses
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
