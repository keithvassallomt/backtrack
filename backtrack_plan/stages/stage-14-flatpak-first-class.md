# Stage 14 — Flatpak as a First-Class Build

## Objective
The Flatpak does everything a native install does, and is published on
FriendlyHub rather than Flathub. An audit of the code on 2026-10-12 found six
places that assume they are running on the host, and two native behaviours with
no Flatpak equivalent yet. This stage fixes all eight without changing what the
native builds do.

The Flatpak is the main download. It is the only package that reaches
distributions whose own libraries are older than Backtrack's GNOME 50 floor
(GTK 4.22, libadwaita 1.9): Debian 13 ships GNOME 48 and RHEL 10 GNOME 47, so
neither can build or install a native package, while the Flatpak brings its own
runtime.

## Order of work
Numbered after Stage 13 because Stage 13's issues already exist, but it runs
in the middle of it:
1. S13-T1 and S13-T2: a Flatpak to test in.
2. This stage, T1 to T8, each checked inside that Flatpak and on a native
   install.
3. S13-T3's test matrix, which repeats this stage's checks on GNOME and Plasma.
4. S14-T9: publishing on FriendlyHub, once S13-T3 is green and S13-T5's pipeline
   builds the bundle.

S14-T1 also unblocks S12-T4: the daemon has to be able to start in the sandbox
before it can ask the Background portal for anything.

## Prerequisites
S13-T7 (the application ID and the daemon's names under backtrack.tools),
S13-T1 (the manifest, with the permissions as corrected on 2026-10-12) and
S13-T2. Every sandbox branch uses `ashpd::is_sandboxed()`, as
`background::apply` already does (S12-T4).

## Ground rules
- Native builds behave exactly as before unless a task says otherwise. Every
  task's acceptance includes a native check: the workspace tests, and the GNOME
  or Plasma VM wherever the behaviour can be seen.
- No escapes to the host: no `--talk-name=org.freedesktop.Flatpak`, no
  `flatpak-spawn --host`, no `--device=all`. Backtrack never mounts a repository
  (it calls `create`, `extract`, `list`, `info`, `prune`, `compact`, `check`,
  `delete`, `init` and `key`), so it needs none of the FUSE plumbing that Pika
  Backup's manifest carries those permissions for.

## Tasks

### S14-T1 — Daemon activation inside the Flatpak
The window relies on the session bus to start the daemon
(`crates/backtrack-gtk/src/daemon.rs`), and without it reports that the service
is not installed. Flatpak exports a D-Bus activation file only when its name
starts with the application ID, which S13-T7 arranges by moving the daemon to
`tools.backtrack.Backtrack.Daemon1`. Starting the daemon from the window instead
is no substitute: every process in a Flatpak instance ends when the instance's
main process does, so the daemon would stop when the window closed.
- The Flatpak installs its own activation file: `Exec=` runs the daemon through
  S14-T7's supervisor, with no `SystemdService=`, since the sandbox has no
  systemd. Flatpak rewrites the `Exec` line to `flatpak run` when it exports the
  file.
- The manifest needs no `--own-name`: names under the application ID belong to
  the sandbox by default.
- Native: the packaged activation file with `SystemdService=` stays as it is.

**Accept:** in the Flatpak, with no daemon running, opening the window starts
the daemon through the bus, and with "Run in background" on the daemon keeps
running after the window closes. Native: unchanged on the GNOME VM.

### S14-T2 — The window opens through the session bus
Inside the sandbox, two callers open the window in ways that do not work there.
The daemon opens it from a notification click through `systemd-run`
(`crates/backtrackd/src/notify.rs`), and the sandbox has no systemd to ask. The
tray starts `backtrack-gtk` as its own child
(`crates/backtrack-gtk/src/bin/backtrack-tray/tray.rs`), which ties the window
to the tray's instance. Both ask the session bus instead, which starts the
window in an instance of its own.
- Every build installs an activation file for
  `tools.backtrack.Backtrack`, which runs the window as a GApplication
  service (`--gapplication-service`). Flatpak exports it as it exports the
  launcher.
- What the command line already accepts (`--fix <reason>`, `--path DIR`,
  `--select FILE`) also becomes application actions, so a caller can ask for any
  of it with `org.freedesktop.Application.ActivateAction`. The options stay, and
  map onto the same actions.
- Inside the sandbox, notification clicks and the tray use those actions.
  Native builds keep `systemd-run` for notification clicks and keep the tray's
  spawn. Moving native builds to the bus as well would also give the window a
  unit of its own; that is a later decision, not part of this task.

**Accept:** in the Flatpak, clicking a health notification opens the window at
the fix, and restarting the daemon afterwards leaves the window open; the
tray's Open opens the window, or raises the one already open. Native: the same
clicks behave as before on both VMs.

### S14-T3 — Tray icon under Flatpak
The Background portal gives an application one autostart entry, and
Backtrack's is the daemon (S12-T4). Flatpak does not export
`/etc/xdg/autostart` files, so nothing would start the tray.
- Inside the sandbox, the daemon starts `backtrack-tray` on desktops other than
  GNOME (the rule the autostart file states with `NotShowIn=GNOME;`), and does
  not start a second one after S14-T7 restarts it.
- Inside the sandbox, the tray registers without a well-known name, through
  `ksni`'s `disable_dbus_name(true)`, which the crate's documentation asks of
  sandboxed programs. The manifest gains
  `--talk-name=org.kde.StatusNotifierWatcher`.
- Native: unchanged; the autostart file still starts the tray.

**Accept:** in the Flatpak on the Plasma VM, after logging in, the tray icon is
there and shows the daemon's state, and one tray process runs after the daemon
is killed and restarted; on the GNOME VM, no tray process runs. Native:
unchanged on both VMs.

### S14-T4 — File-manager integration for Flatpak installs
`reference/stack.md` §8 accepted no file-manager plugins for the Flatpak. That
is reversed (Keith, 2026-10-12): a Flatpak install integrates as well as a
native one.
- Preferences → General installs per-user copies: the Nautilus extension into
  `~/.local/share/nautilus-python/extensions/` and the Dolphin service menus
  into `~/.local/share/kio/servicemenus/`, the latter marked executable, which
  KIO requires of per-user service menus. On the way in, the copies are
  rewritten to launch the window with `flatpak run tools.backtrack.Backtrack`
  in place of `backtrack-gtk`, as
  `just install-units` rewrites the units. Removing them is offered too.
- The Nautilus copy reads `roots.json` where the sandboxed daemon writes it,
  `~/.var/app/tools.backtrack.Backtrack/data/backtrack/roots.json`.
  Without that, the extension finds no backed-up folders and shows nothing.
- The window rewrites the copies whenever they differ from what the running
  version would install, so an update to the Flatpak updates them.
- Detection (`crates/backtrack-gtk/src/model/prefs.rs`) also looks in the
  per-user directories, and inside the sandbox in the host's system directories
  under `/run/host` (granting `--filesystem=host-os:ro` if `host` does not
  already expose them), so a native plugin package is still recognised.
- Nautilus still needs nautilus-python from the distribution, and the hint goes
  on naming its package.
- Native packages keep installing the system-wide copies (S13-T4).

**Accept:** in the Flatpak, on the GNOME VM with nautilus-python installed and
on the Plasma VM, installing from Preferences and restarting the file manager
gives both actions, each opening the window at the right folder and file;
removing them takes them away. Native: the plugin packages are still detected,
and behave as before.

### S14-T5 — The host's os-release
Inside a Flatpak, `/etc/os-release` describes the runtime rather than the
distribution. The Fix page (`crates/backtrack-gtk/src/ui/fix/mod.rs`) would give
instructions for the wrong distribution, and `backtrack doctor`
(`crates/backtrack-cli/src/doctor.rs`) would report the runtime. Both read
`/run/host/os-release` first, which Flatpak provides, and fall back to
`/etc/os-release`, which is all a native install has. One helper in
`backtrack-core` serves both.
**Accept:** a unit test pins the order; in the Flatpak on the GNOME VM the Fix
page names Fedora, and `flatpak run --command=backtrack
tools.backtrack.Backtrack doctor` reports the host's distribution.
Native: unchanged.

### S14-T6 — One keyring path in every build
Inside a sandbox, `oo7::Keyring::new()`
(`crates/backtrack-core/src/secret/keyring.rs`) switches to oo7's file backend:
the passphrase goes to a file under `~/.var/app`, unlocked through the Secret
portal, rather than to GNOME Keyring or KWallet. Backtrack uses the Secret
Service over D-Bus (`oo7::dbus`) in every build instead, and the manifest gains
`--talk-name=org.freedesktop.secrets`.

Why: one behaviour everywhere, so "Remember passphrase", the recovery flows and
the documentation all describe the same thing; KWallet answers the Secret
Service today, whereas the Secret portal on Plasma is not something to depend
on; the portal's isolation buys little for an application that can already
read every file the user can.
**Accept:** in the Flatpak on both VMs, a remembered passphrase shows in Seahorse
or KWallet as Backtrack's, is read back after a daemon restart, and is removed
when forgotten. Native: unchanged, through the existing tests and the Plasma
VM's KWallet path.

### S14-T7 — Restart after a failure, as the unit does
The native unit has `Restart=on-failure` and `RestartSec=5s`. A Flatpak has no
unit, so a daemon that crashes stays down until something starts it again, and
no backups run in the meantime.
- Inside the sandbox, both ways in (the Background portal's autostart command
  and S14-T1's activation file) start the daemon under a small supervisor that
  does what the unit does. After an exit with an error, or death by a signal,
  it starts the daemon again 5 seconds later. After a clean exit (the
  single-instance race lost, or the daemon leaving with no window open and no
  job running) it stops.
- A supervising mode of `backtrackd` or a separate small binary, whichever
  comes out smaller.
- Native: systemd goes on supervising, and the supervisor is never used.

**Accept:** a unit test pins the restart decision; in the Flatpak, killing the
daemon with SIGKILL brings it back within about 5 seconds with its state
intact, a clean exit is not restarted, and a second supervisor started while one
runs exits cleanly.

### S14-T8 — Btrfs snapshots inside the Flatpak
The local safety net tries a btrfs snapshot before falling back to the spool
(`crates/backtrackd/src/snapshot.rs`, `crates/backtrackd/src/pipeline.rs`), and
does it with the `btrfs` command, which the GNOME runtime lacks. The manifest
builds btrfs-progs (the command only), so the probe behaves as it does on a
native install: it works on a subvolume the user owns, and falls back to the
spool everywhere else. If the Flatpak's seccomp filter refuses the snapshot
anyway, the spool is what the user gets, and that is recorded rather than
worked around.
**Accept:** inside the Flatpak, on a btrfs subvolume the test user owns, a local
snapshot is made and removed; on a home that is not such a subvolume, the spool
is used, as on a native install. Native: unchanged.

### S14-T9 — Publish on FriendlyHub
Backtrack is published on FriendlyHub, not Flathub (Keith, 2026-10-12).
`packaging/flatpak/FRIENDLYHUB.md` covers what FriendlyHub needs from the
repository (manifest, metainfo, screenshots, release notes taken from the
CHANGELOG), how a release reaches it, and how a user adds the remote, including
on distributions whose own packages cannot carry Backtrack (Debian 13, RHEL 10
and its rebuilds). `just release` (S13-T9) generates the files and submits
them; this task settles what FriendlyHub takes and how, and the recipe follows
it. This replaces the Flathub checklist S13-T5 used to carry.
**Accept:** the document exists, and following it, a release candidate installs
from FriendlyHub on a clean VM and completes the wizard → backup → restore round
trip.

## Definition of Done
All nine tasks done; S13-T3's matrix green, this stage's rows included; a
native install on both VMs behaves as it did before the stage; stack.md §2, §8
and §9 describe what was built.
