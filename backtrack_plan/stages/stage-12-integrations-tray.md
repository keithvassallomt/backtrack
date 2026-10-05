# Stage 12 — File-Manager Integration, Tray & Background Presence

## Objective
The right-click magic in Nautilus and Dolphin, a status tray for non-GNOME
desktops, and correct background presence on GNOME (quick-settings launchability).
Everything here is deliberately thin — the research
([../reference/open-questions.md](../reference/open-questions.md) Q1) proved deep
file-manager integration is a trap (Deja Dup got burned); plugins launch the app,
nothing more.

## Prerequisites
Stages 6 (launch contract), 3 (D-Bus). Mockups
[2](../mockups/2-nautilus-context-menu.png),
[3](../mockups/3-dolphin-integration.png) (context-menu part only — the
`backtrack:/` location shown there is v1.x, do NOT build it now).

## Tasks

### S12-T1 — Nautilus extension
`integrations/nautilus/backtrack.py` (nautilus-python, API 4.x):
`MenuProvider.get_file_items` → one item "Restore Previous Version…" (single
selection, local files only) launching
`backtrack --path <parent> --select <file>`; `get_background_items` →
"Browse Backups of This Folder…" → `backtrack --path <dir>`. Only appears for
paths under the configured backup roots (read roots from a tiny
`~/.config/backtrack/roots.json` the daemon maintains — do NOT D-Bus-call from
the extension synchronously). Install path `~/.local/share/nautilus-python/extensions/`
(dev) / packaged path (Stage 13). `just install-nautilus-dev` recipe + docs on
restarting nautilus (`nautilus -q`).
**Accept:** manual checklist (screenshots vs mockup 2) documented in the PR:
both items appear in the right places, launch correctly selected, and do NOT
appear outside backup roots or on non-local mounts.

### S12-T2 — Dolphin service menu
`integrations/dolphin/backtrack.desktop` (KF6 servicemenus location):
`X-KDE-Submenu=Backtrack` with the same two actions (file/dir mimetypes +
`inode/directory` for background). Same roots caveat is impossible in a static
.desktop — accepted: entries always show; app opens with a friendly "this folder
isn't backed up" state when outside roots (implement that state in the GTK app:
info page + "add to backups?" link to prefs).
**Accept:** on a KDE VM/container: menu present per mockup 3, actions launch;
outside-roots launch shows the friendly state (also reachable on GNOME for tests).

### S12-T3 — Tray (non-GNOME)
`ksni` (StatusNotifierItem) in a small `backtrack-tray` bin (or feature of
backtrack-gtk — decide by binary size, note decision): icon reflects health state
(normal / attention glyph per Stage 10 states), menu: status line (disabled item,
e.g. "Last backup: today 16:00"), Back Up Now, Pause ▸ (three durations), Open
Backtrack, Quit tray. Autostart on KDE/XFCE etc. via desktop file with
`OnlyShowIn` excluding GNOME. Tray talks D-Bus only.
**Accept:** on KDE: icon, live status updates on StatusChanged, all actions work;
not started on GNOME sessions.

### S12-T4 — GNOME background presence
Via ashpd Background portal: daemon requests background+autostart with reason
"Hourly backups"; ensure the app (not just daemon) registers proper app-id so
GNOME quick settings' background-apps section lists Backtrack and its entry
launches the app (this is the agreed GNOME quick-settings answer — launch is one
click; status is the first thing the launched window shows via the Stage 10
banner/status line). Flatpak-ready (portal is mandatory there — Stage 13 tests it
in-sandbox).
**Accept:** on GNOME: Backtrack appears under quick settings → background apps
while daemon active; activating it opens the main window.

### S12-T5 — Plugin detection
App checks for nautilus extension / dolphin servicemenu presence (path probes per
desktop) → Preferences General rows show Installed / Install… (opens distro
package instructions page; exact package names from Stage 13) per mockup 15.
**Accept:** detection correct in all four combinations (each plugin present/absent).

### S12-T6 — Development VMs
Added during the stage: the tray (T3) and GNOME's background apps (T4) can
only be tried on their own desktops, and iterating on them means a VM per
desktop that follows the checkout. `scripts/vm` on the host and
`scripts/dev-machine` in the VM, behind `just vm-push <vm>`, `just vm-app
<vm>` and `just dev-machine`: one command takes a fresh Fedora VM (GNOME or
Plasma) to a built checkout, the development daemon installed and pointed at
the demo backups, and the desktop's file-manager integration installed; later
pushes copy the change in and rebuild with no prompt. `just setup` takes its
package lists from the same script. Documented in `docs/development-vms.md`.
**Accept:** on Keith's GNOME VM and on a Plasma VM, after enabling sshd and
copying a key: the first `just vm-push` (from a terminal, for sudo) leaves
`dev-machine status` reporting a running daemon on the demo backups; a second
push after a source change rebuilds with no prompt; `just vm-app` opens the
window on the VM's screen.

## Definition of Done
Demo videos/screenshots for GNOME (menu + background presence) and KDE (menu +
tray) attached to the PR; the outside-roots friendly state implemented; CI green
(python file linted with ruff; .desktop validated with desktop-file-validate);
progress.md + CHANGELOG updated.
