# Taking the screenshots

The guides, backtrack.tools and the software centres' listing share one set
of screenshots, in `docs/images/`. They are taken on the GNOME development
VM ([development-vms.md](development-vms.md)) by `scripts/screenshots`,
as a user of their own whose home folder looks like somebody's, so that the
set can be taken again for each release in a few minutes. A few need a
person at the VM's screen; they are listed under
[Taken by hand](#taken-by-hand).

## Once per VM

With the GNOME VM started (`just vm-start "fedora45 (GNOME)"`) and the build
to be shown pushed to it (`just vm-push "fedora45 (GNOME)"`):

```sh
scripts/screenshots setup
```

That makes the user **sam** (Sam Rivera, password `backtrack dev`, signed in
over SSH with your key), gives it an 8 GB virtual drive labelled **Backup
Drive** (`/var/lib/backtrack-screenshots/backup-drive.img`, mounted at
`/run/media/sam/Backup Drive`), and logs it in automatically in place of the
VM's own user. It copies this checkout's build to
`/opt/backtrack-screenshots/`, with GNOME Files' items pointing at it.

It also turns on GNOME Shell's unsafe mode for sam, through the extension in
`scripts/screenshots.d/unsafe-mode@dev.backtrack.tools`. Without it, GNOME
Shell answers its screenshot interface only for its own short list of
programs, and a script cannot size a window or type into it. `virsh
screenshot` is no way round it: the VM's virtio GL display has no surface
for QEMU to copy. Unsafe mode lets any program the user runs do all of
that, so it belongs on a development VM and nowhere else.

## Taking the set

```sh
scripts/screenshots take      # into target/screenshots/, full size
scripts/screenshots images    # sized for the guides, into docs/images/
```

`take` builds sam's home folder from nothing every time, since the shots
change it: four weeks of documents, photos, downloads and a kitchen
project, with a backup on each day something happened, made with Borg's
`--timestamp` under the names Backtrack gives its own. Backtrack's daemon
then opens those backups, as the wizard's **Already have backups?** does,
and backs up once more. The history holds the stories the guides tell: a
newsletter spoiled by a rewrite two days ago, a boarding pass cleared out of
Downloads, a holiday photo edited and saved over the original, a letter
deleted today with the Trash emptied, and a shopping list moved to
the Trash since the last backup.

It drives Backtrack through its command line (`--path`, `--select`,
`--rewind`), its window's actions, a virtual keyboard and pointer, and the
accessibility tree (to press a button by its label), and takes each window
with its shadow through GNOME Shell. The window is 1200 × 780 on a 3840 ×
2160 screen at 5/3; `images` brings each to its size on that screen.

| Image | What it shows |
|---|---|
| `window-earlier-backup.png`, `-dark` | Documents at an earlier backup: the newsletter changed since, the letter deleted |
| `restore-question.png` | Restoring the newsletter: Replace, Keep Both or Cancel |
| `compare-document.png` | Compare with Today on `Notes.txt` (a word processor's documents cannot be shown side by side) |
| `window-photo.png`, `-dark` | The holiday photo before it was edited, in the preview |
| `compare-picture.png` | Compare with Today on that photo |
| `rewind.png` | Rewind over the holiday photos, the whole screen |
| `search-deleted.png` | Search finding the boarding pass, no longer on the disk |
| `restore-folder-question.png` | Restoring a folder: what is identical and what would be replaced |
| `restore-folder-summary.png` | The folder restored with Keep Both Versions, and Undo |
| `preferences.png`, `-backup`, `-storage` | Preferences' General, Backup and Storage pages (the last with Snapper) |
| `files-menu.png` | GNOME Files' right-click menu on a backed-up document |
| `trash-in-timeline.png` | The Trash's entry above the latest backup |
| `warning-banner.png` | The banner when Backtrack needs the passphrase again, with Fix… |
| `wizard-welcome.png`, `-what`, `-where`, `-protect` | The welcome wizard's pages, the passphrase typed on the last |
| `wizard-key-saved.png` | The wizard's last step with the recovery key saved (by hand) |
| `wizard-first-backup.png` | The first backup running (by hand) |
| `new-computer.png` | Backups opened on a new computer, offering to restore everything (by hand) |
| `dolphin-menu.png` | Dolphin's Backtrack submenu, on the Plasma VM (by hand) |
| `tray-menu.png` | The tray icon's menu, on the Plasma VM (by hand) |

## Taken by hand

Each of these needs a person to choose a file or to use another desktop.
They are taken with sam logged in on the GNOME VM: after a `just
vm-start`, which logs the VM's own user in, `scripts/screenshots setup`
logs sam in again.
Take a window with **Alt+Print** on the VM's screen, which saves it to sam's
`Pictures/Screenshots`, then fetch it under its name with
`scripts/screenshots keep NAME`.

1. **The recovery key saved, and the first backup.**
   `scripts/screenshots wizard` brings the wizard to step 4 with the
   passphrase typed. On the VM's screen, click **Save Recovery Key…** and
   save it anywhere. Take the window, then `scripts/screenshots keep
   wizard-key-saved`. Click **Start First Backup**, take the window while
   it runs, then `scripts/screenshots keep wizard-first-backup`.
2. **Moving to a new computer.** `scripts/screenshots import` brings the
   wizard to "Where are your backups?". Click **Choose Folder…**, choose
   **Backup Drive**, then its **Backtrack** folder, and **Choose**. Type the
   passphrase `correct horse battery staple` and click **Open Backups**.
   Take the page that offers to restore everything, then
   `scripts/screenshots keep new-computer`. Close the window without
   restoring.
3. **Dolphin's menu and the tray, on the Plasma VM.** `scripts/screenshots
   done`, then `just vm-stop "fedora45 (GNOME)"`, `just vm-start "fedora45
   (KDE)"` and `just vm-push "fedora45 (KDE)"`. On its screen, open Dolphin
   on a folder of documents and right-click one with the **Backtrack**
   submenu open, then the tray icon's menu, each taken with Spectacle
   (**Meta+Print** for the active window, **Meta+Shift+Print** for a
   region). After each, fetch it with
   `BACKTRACK_SHOTS_VM="fedora45 (KDE)" scripts/screenshots keep
   dolphin-menu` (or `tray-menu`), which takes the newest screenshot in the
   VM's own user's `Pictures/Screenshots`.

Then `scripts/screenshots images` again.

## The Rewind video

The home page of backtrack.tools shows Rewind moving: a short, silent loop of
a folder going back through its backups and a file coming back. It is
recorded by hand, with sam logged in on the GNOME VM, after
`scripts/screenshots take`, whose backups it goes through.

1. `scripts/screenshots video` opens Rewind full screen on sam's
   **Documents**, at **Today (Now)**.
2. On the VM's screen, press **Ctrl+Shift+Alt+R** to start GNOME's screen
   recorder.
3. Record about fifteen seconds, without hurrying:
   - stay on Now for a second;
   - press **↑** about once a second until a file marked **deleted**
     appears;
   - click it, then **Restore**, and let the restore finish;
   - press **Home** to come back to Now, where the file is back;
   - stay there for a second.
4. Press **Ctrl+Shift+Alt+R** again to stop.
5. `scripts/screenshots keep-video` fetches the recording as
   `target/screenshots/rewind.webm`.

In the backtrack.tools repository, `scripts/make-video` with that file makes
the site's copies of it, and the still shown in its place to anyone whose
system asks for reduced motion.

## Putting the VM back

```sh
scripts/screenshots done
```

logs the VM's own user back in and unmounts the drive. sam, the drive's
image and the unsafe-mode extension stay, ready for the next set.

## The photographs

The photos in sam's Pictures are from Wikimedia Commons, each dedicated to
the public domain (CC0), fetched by `scripts/screenshots.d/shots.py` when
the home folder is first made:
[Dog on dirt road](https://commons.wikimedia.org/wiki/File:Dog_on_dirt_road.jpg),
[Flowers, Lan Su Chinese Garden](https://commons.wikimedia.org/wiki/File:Flowers_-_Lan_Su_Chinese_Garden_-_Portland,_Oregon_-_DSC01252.jpg),
[Parasols, Rincon de la Victoria](https://commons.wikimedia.org/wiki/File:Parasols,_Evening,_Beach,_Rincon_de_la_Victoria,_Andalusia,_Spain.jpg),
[Valencia harbour](https://commons.wikimedia.org/wiki/File:Valenica_Mar_Harbour_Spain_-_Image_with_boats_in_line_up.jpg),
[Some boats in harbour](https://commons.wikimedia.org/wiki/File:Some_Boats_in_harbour.jpg),
[Lake mountain landscape](https://commons.wikimedia.org/wiki/File:Lake_Mountain_Landscape.jpg),
[Blue Mountain Lake](https://commons.wikimedia.org/wiki/File:View_of_Blue_Mountain_Lake_from_Mt._Magazine_Lodge.jpg)
and
[Apple cake with vanilla ice cream](https://commons.wikimedia.org/wiki/File:Apple_cake_with_vanilla_ice_cream_2.jpg).
