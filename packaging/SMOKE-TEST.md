# Smoke test for each package

A fresh install, checked by hand on a clean machine or a new user account:
the same few steps whichever way Backtrack was installed. The builds and their
installs inside containers are machine-checked (`just rpm`, `just deb`,
`just aur-build`, `just flatpak`); this is the part a person looks at.
`scripts/smoke-check` walks through it one check at a time: the RPM on a
development VM as a new user, the deb from the testing PPA the same way on the
Ubuntu VM (`scripts/smoke-check deb`), and the AUR package on the computer it
is run on.

## Installing

| Channel | Install | Nautilus and Dolphin |
|---|---|---|
| Fedora 44+ (COPR) | `sudo dnf copr enable keithvassallo/backtrack` then `sudo dnf install backtrack` | `backtrack-nautilus`, `backtrack-dolphin` |
| Ubuntu 26.04+ (PPA) | `sudo add-apt-repository ppa:keithvassallomt/backtrack` then `sudo apt install backtrack` | `backtrack-nautilus`, `backtrack-dolphin` |
| Arch (AUR) | `yay -S backtrack`, or `sudo pacman -U` on the packages `just aur-build` leaves in `target/aur` | `backtrack-nautilus`, `backtrack-dolphin` |
| Anything (Flatpak) | as [flatpak/FRIENDLYHUB.md](flatpak/FRIENDLYHUB.md) says | Preferences → General → Install |

Before a channel is live, the packages from `just rpm` (`target/rpm/RPMS`) and
`just deb` (`target/deb`) install the same way from their files:
`sudo dnf install ./…rpm`, `sudo apt install ./…deb`.

## The checks

1. **The launcher.** Backtrack is in the desktop's application list, with its
   icon, and opens the Welcome wizard.
2. **The wizard to a first backup.** Choose a small folder and a destination
   (a folder on a second disk, or a USB stick), set a passphrase, save the
   recovery key, and start: the first backup finishes and its notification
   appears.
3. **Browse and restore.** Delete a file in the backed-up folder, find it in
   the timeline at the last backup, and restore it: it is back, with its old
   modification time.
4. **At login.** With "Run in background" on (the default), log out and back
   in: `systemctl --user is-active backtrackd` says `active` (packages), or
   GNOME's Background Apps lists Backtrack (Flatpak).
5. **The file manager.** With the integration installed and the file manager
   restarted (`nautilus -q`), right-click a backed-up file: Restore Previous
   Version… opens the window with the file selected.
6. **The tray (Plasma).** After logging in, the tray icon is there and its
   menu's Back Up Now runs a backup.
7. **Removing it.** Uninstall the packages (or the Flatpak): no `backtrackd`
   is left running after logging out and in, and the backups on the
   destination are still there.
