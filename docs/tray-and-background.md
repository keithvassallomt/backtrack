# The tray icon and background presence

How Backtrack shows that it is there while its window is closed. Desktops
with a system tray (Plasma, Xfce, Cinnamon and the rest) get a tray icon.
GNOME has no tray; there, quick settings' background apps list Backtrack,
and only for the Flatpak build (see [GNOME](#gnome)).

## The tray icon

`backtrack-tray`, a second binary of the `backtrack-gtk` package
(`crates/backtrack-gtk/src/bin/backtrack-tray/`). It is a StatusNotifierItem
(through `ksni`), and talks to the daemon over D-Bus and to nothing else.

### What it shows

| The daemon says | The icon | Its line (menu and tooltip) |
|---|---|---|
| Nothing set up | normal | Backups are not set up yet |
| A backup running | normal | Backing up now… |
| Paused | normal | Backups are paused until today 17:00 (or "until you resume them") |
| `AT_RISK` | attention: `dialog-warning` | Backups need your attention |
| `BROKEN` | attention: `dialog-error` | Backups need your attention |
| Anything else | normal | Last backup: today 16:00 |
| Not running | normal | The Backtrack service is not running |

Attention is the item's `NeedsAttention` status with an attention icon,
which a panel shows in place of the normal one; health.md's other states
stay quiet, as its "silence means safe" asks. The normal icon is the app's
own where a package has installed it, and `document-open-recent` where it has
not, as for a development build. Times are given as a day and a time of day
rather than as "2 hours ago", because a tray menu is built when it is opened
and a time of day cannot go stale in the meantime.

Its menu:

- the line above, greyed out
- **Back Up Now**, while no backup is running
- **Pause Backups ▸** For 1 hour, Until tomorrow, Until I resume: the
  window's own choices, from the same code (`model/pause.rs`)
- **Resume Backups**, while paused
- **Open Backtrack**, which a click on the icon itself also does
- **Quit Tray Icon**, which takes the icon away until the next login and
  leaves backups running

### How it stays current

It reads the daemon's status when the health state changes (`StatusChanged`),
when a job ends (`JobFinished`), when a backup starts (its first
`BackupProgress`), when the daemon starts or leaves the bus, and when its
menu is opened. It reads it without starting the daemon (`NoAutoStart`): with
*Run in background* off, the daemon leaves once nobody needs it, and a tray
that started it again to ask how it was would keep it running all day. The
menu's actions do start it, since those are somebody asking for something.

The daemon announces a pause and a resume at once. It used to leave them to
its once-a-minute reassessment, which the window never noticed because it
reads the status again after its own calls.

One icon per session: the tray claims `org.backtrack.Tray`
(`org.backtrack.Tray.Dev` in development) and a second copy leaves.

### Known limitation: an open window is not brought forward

On Wayland, a click on the icon or on **Open Backtrack** opens the window,
but a window already open behind another one stays behind it. A program may
raise a window only with an activation token from the click that asked for
it. Plasma offers one to a tray icon through `ProvideXdgActivationToken`, a
KDE addition to StatusNotifierItem, and `ksni` 0.3.6 does not implement that
method, so the token is never received; Plasma passes none for a menu click
at all. With one, the tray would hand it to the window in
`XDG_ACTIVATION_TOKEN`, which GTK forwards to the instance already running.
Left as it is for now (Keith's call, 2026-10-06), rather than carrying a
patched copy of `ksni`.

### Why a binary of its own

It runs for the whole session. Started, the release build peaks at 9 MB and
the window's, which links GTK, at 60 to 95 MB before it opens anything
(measured on Arch, 2026-10-06), and running it takes about 20 MB. The two
GLib modules it shares with the window (`model/format.rs` for times,
`model/pause.rs`) are included by path and name `glib` directly, so the tray
links GLib and no GTK.

### Starting it

`packaging/autostart/backtrack-tray.desktop`, which packages install to
`/etc/xdg/autostart` (Stage 13). It has `NotShowIn=GNOME;`, which covers
Ubuntu's session (`ubuntu:GNOME`) as well. On a systemd session the desktop
turns it into a unit, `app-backtrack\x2dtray@autostart.service`.

For development, `just install-tray-dev` installs it to `~/.config/autostart`
with the command rewritten to this checkout's debug build and
`BACKTRACK_DEV=1`, and `just uninstall-tray-dev` removes it. `scripts/dev-machine`
installs it, and on a desktop other than GNOME starts the icon, or restarts
it when it is not the build just made.

### Checking it

`scripts/tray-check`, in a session with a tray, against the development
daemon. On the Plasma development VM:

```sh
just vm-push "fedora45 (KDE)"
scripts/vm run "fedora45 (KDE)" scripts/tray-check
```

It finds the icon the way a panel does (the StatusNotifierWatcher), reads
what a panel would draw, clicks the menu through `com.canonical.dbusmenu`, and
checks each result: a pause made elsewhere and one from the menu, Resume,
Back Up Now, `AT_RISK` and `BROKEN` (forced through the development
interface), the daemon stopped (and not started again by the icon) and
started, Open Backtrack, and Quit Tray Icon. It puts everything back and
leaves the icon showing. A line starting `FAIL` says what did not happen.

What it cannot see is the panel itself. On the VM's screen: the icon is in
the system tray (Plasma may put a new one under the tray's arrow), its tooltip
and menu read as above, and while `BROKEN` is forced the icon is the red error
sign:

```sh
scripts/vm run "fedora45 (KDE)" busctl --user call org.backtrack.Daemon1.Dev \
    /org/backtrack/Daemon1 org.backtrack.Daemon1.Dev ForceHealth ss BROKEN passphrase-missing
scripts/vm run "fedora45 (KDE)" busctl --user call org.backtrack.Daemon1.Dev \
    /org/backtrack/Daemon1 org.backtrack.Daemon1.Dev ForceHealth ss "" ""
```

### Checked

On the Plasma VM (Fedora 45, Plasma 6.7), 2026-10-06: every `tray-check`
step passed, and after a cold boot the icon was started by its autostart
unit and registered with the panel. Keith's look at the panel the same day:
the icon, its tooltip and menu read as above, `BROKEN` showed the red error
sign, a pause from the menu lifted itself after its minute, and a click
opened the window (see the limitation above for one already open).
Screenshots: `backtrack_plan/screenshots/stage-12-tray-menu.png` and
`stage-12-tray-attention.png`.

## GNOME

GNOME 44 and later list applications running without a window under quick
settings → **Background Apps**, and choosing one there opens it. That is
Backtrack's presence on GNOME: one click opens the window, whose status line
and banner say how backups are doing.

The list holds **Flatpak instances only**. gnome-shell reads it from
xdg-desktop-portal's `org.freedesktop.background.Monitor`, and the portal
builds it from the running Flatpak instances and nothing else (checked
against xdg-desktop-portal 1.22.1, `src/background.c`, and gnome-shell 51,
`js/ui/status/backgroundApps.js`, on Fedora 45). A Backtrack installed from a
distribution package, or run from a checkout, is never listed, whatever it
asks for, and GNOME offers such an application nothing in its place.

For the Flatpak build:

- **The daemon asks the Background portal** to run in the background, with
  the reason "Hourly backups", and to be started at login (`backtrackd`, as
  the autostart command) when *Run in background* is on. It asks at startup
  and whenever that setting changes, in place of enabling the systemd unit,
  which a sandbox does not have (`crates/backtrackd/src/background.rs`). The
  request runs on its own, because the portal may ask on screen first.
- **The launcher is named for the application ID**,
  `packaging/desktop/io.github.keithvassallomt.Backtrack.desktop`: the shell
  looks a background app up as `<app ID>.desktop` and launches it.

Outside a sandbox, nothing here changes: the systemd unit starts the daemon
at login, as before, and the portal is not asked.

### Checking it

Only in the Flatpak, so with the packaging (S13-T3's test matrix): with
*Run in background* on and the window closed, Backtrack is listed under
quick settings → Background Apps while the daemon runs, and choosing it there
opens the main window.

