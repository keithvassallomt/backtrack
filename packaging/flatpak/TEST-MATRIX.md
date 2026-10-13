# Flatpak portal and sandbox test matrix

S13-T3's checklist, run by hand on the development VMs with
`scripts/flatpak-check`, which installs the Flatpak fresh on each and says
what to do at the screen. Each run adds its column; a row is green when both
desktops pass it.

The VMs are Fedora 45: GNOME 51 and Plasma 6.7, both on Wayland. The Flatpak
is built from the checkout with `just flatpak-bundle`.

## Run 2: 2026-10-13, unattended

`scripts/matrix-check`, on the same VMs with the Flatpak from commit d87c1ba,
for the rows that need nobody at the screen. It drives the Flatpak's daemon
over D-Bus as the wizard does, so the sandbox's own Borg does each backup.

| Check | GNOME | Plasma |
|---|---|---|
| SMB destination through gvfs: a share mounted from inside the sandbox, and backed up to | pass | n/a: this Plasma has no gvfs, which the Flatpak mounts shares through |
| SSH destination, through the sandbox's ssh | pass | pass |
| Xorg session | n/a: none installed | n/a: none installed |
| A drive unplugged, seen to go, plugged in again, seen, and backed up to at once | pass | pass |
| Another Flatpak's data under `~/.var/app` is in the backup | pass | pass |
| S14-T7: a killed daemon brought back by the same supervisor | pass (6 s) | pass (6 s) |
| S14-T8: with the destination away, a btrfs snapshot of a subvolume the user owns | pass | pass |
| Background portal: the daemon starts at login | pass | pass |

On GNOME the drive was mounted through gvfs, as the desktop mounts one
(GNOME mounts nothing by itself while the screen is locked); on Plasma,
which has no gvfs, with a plain mount at the same place.

## Run 1: 2026-10-12

Last build tested: commit 15edada. Fixes made during the run were rebuilt and
checked again on the same VM before the run went on.

| Check | GNOME | Plasma |
|---|---|---|
| FileChooser portal: the wizard's folder chooser | pass | pass |
| FileChooser portal: the wizard's destination folder button | not run (added during the run) | pass |
| FileChooser portal: saving the recovery key | pass | pass |
| FileChooser portal: Restore To… | pass | not run |
| Notification portal: a health notification, and clicking it opens the fix (S14-T2) | pass | not run |
| Background portal: the daemon starts at login | not run | pass |
| Background portal: listed in GNOME's Background Apps, and clicking it opens the window (S12-T4) | pass | n/a |
| Keyring: the passphrase is saved, and a restarted daemon reads it back | pass | pass with the wallet open; fails while it is locked (S10-T9, #106) |
| SMB destination through gvfs | not run | not run |
| SSH destination | not run | not run |
| Xorg session | not run | not run |
| A drive plugged in after the daemon started is seen and backed up to | not run | not run |
| Another Flatpak's data under `~/.var/app` is in the backup | not run | not run |
| S14-T1: the daemon starts by D-Bus activation | pass | pass |
| S14-T2: the tray's Open Backtrack opens the window | n/a | pass |
| S14-T3: the tray icon, started by the daemon, in the panel's text colour | n/a | pass |
| S14-T4: Install, the file action, Remove (Nautilus on GNOME, Dolphin on Plasma) | pass | pass |
| S14-T4: the folder action | not run | not run |
| S14-T5: the Fix page for a missing Borg names no distribution command | pass | not run |
| S14-T7: the supervisor restarts a killed daemon | not run | not run |
| S14-T8: btrfs snapshots inside the sandbox | not run | not run |

### Found and fixed during the run

- The wizard's folder destination took a local folder only as a typed path,
  under the name "Network folder": now "Network/local folder", with a folder
  button that opens the chooser (e0c0604).
- The Fix page for a missing Borg began with the native packages' sentence
  in the Flatpak too (1fe2940).
- Preferences did not recognise the Flatpak's own Nautilus copy, so it never
  offered Remove for it (08ad055).
- The symbolic icon stayed black on Plasma's dark panel, since Plasma does
  not recolour GTK-style symbolic icons (7abfaad).
- Preferences listed an integration for a file manager that is not
  installed, and its instructions button looked like Install; on Plasma the
  header showed the logo twice (15edada).

### Found, and done since (2026-10-13)

Checked again in the Flatpak: S10-T8 on Plasma, S10-T9 on both VMs.

- S10-T8 (#105): a local destination Borg may not write to read "Backtrack
  can't sign in to …"; it now reads "Backtrack can't write to …", with its
  own fix (1a12cdb). The banner drew the name as "." until the next refresh
  (0bc3c4b).
- S10-T9 (#106): a locked keyring read "The saved passphrase no longer
  matches the backup."; it now reads "Backtrack needs your backup
  passphrase again.", recovers by itself once the keyring is open, and the
  fix unlocks the keyring to save the passphrase (633f3fe).
- S14-T10 (#104), a suggestion from the run: Background Apps shows a line
  under Backtrack, the tray's first line, kept current by the daemon
  (d87c1ba). Checked on GNOME through a backup and a failure.
