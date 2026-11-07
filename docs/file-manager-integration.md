# Add Backtrack to your file manager's menu

Backtrack can add four items to the right-click menu of GNOME Files and
Dolphin, so that a file's earlier versions are one click away from wherever
you are looking at it. This guide adds them, shows what each one does, and
how to turn them off again.

## See whether they are installed

1. In Backtrack, open the main menu and choose **Preferences**. The first
   page, **General**, has a section named **File manager integration**.
2. Look at the row for your file manager, **GNOME Files (Nautilus)** or
   **Dolphin**.
   - **Installed**, with a switch: the items are there. Skip to [Use the
     items](#use-the-items).
   - **Not installed**, with **How to Install…**: follow the steps below for
     how you installed Backtrack. **How to Install…** opens them too.

## Installing

### From a package

If you installed Backtrack from COPR (Fedora), the PPA (Ubuntu) or the AUR
(Arch Linux). On Fedora, the items for your desktop's file manager come with
Backtrack, so they may be installed already.

**GNOME Files**

1. Install Backtrack's package for GNOME Files. In a terminal, run the
   command for your distribution:
   - Fedora: `sudo dnf install backtrack-nautilus`
   - Ubuntu: `sudo apt install backtrack-nautilus`
   - Arch Linux: install `backtrack-nautilus` from the AUR, as you installed
     `backtrack`
2. Quit GNOME Files, so that it loads the new items when it opens again:

   ```sh
   nautilus -q
   ```

3. Open GNOME Files again. **Preferences → General** in Backtrack now says
   **Installed** beside **GNOME Files (Nautilus)**.

**Dolphin**

1. Install Backtrack's package for Dolphin:
   - Fedora: `sudo dnf install backtrack-dolphin`
   - Ubuntu: `sudo apt install backtrack-dolphin`
   - Arch Linux: install `backtrack-dolphin` from the AUR
2. Right-click a file in Dolphin. Nothing needs restarting.

### From the Flatpak

**GNOME Files**

1. Install nautilus-python from your distribution. GNOME Files needs it to
   run any extension, and a Flatpak cannot install it for you:
   - Fedora: `sudo dnf install nautilus-python`
   - Ubuntu: `sudo apt install python3-nautilus`
   - Arch Linux: `sudo pacman -S python-nautilus`
2. In Backtrack, open **Preferences → General** and click **Install** beside
   **GNOME Files (Nautilus)**. It now says **Installed**.
3. Quit GNOME Files, so that it loads the new items when it opens again:

   ```sh
   nautilus -q
   ```

**Dolphin**

1. In Backtrack, open **Preferences → General** and click **Install** beside
   **Dolphin**.
2. Right-click a file in Dolphin. Nothing needs restarting.

When the Flatpak updates, it updates the items too.

## Use the items

In GNOME Files, right-click a file or a folder inside the folders Backtrack
backs up. In Dolphin, the same items are in a **Backtrack** submenu of the
right-click menu.

- **Restore Previous Version…**, on a file or a folder, opens Backtrack's
  window on the folder it is in, with it selected. Step back through the
  backups to the version you want, and restore it. [Get a file back from an
  earlier backup](restoring.md) goes through it.
- **Browse Backups of This Folder…**, on a folder or on the empty space of
  the folder you are in, opens Backtrack's window on that folder.
- **Rewind This File…**, on a file, opens [Rewind](rewind.md), Backtrack's
  full-screen view of a folder's history, on the folder it is in, with the
  file selected.
- **Rewind This Folder…**, on a folder or on the empty space of the folder
  you are in, opens Rewind on that folder.

In GNOME Files the items appear only where Backtrack has something to show:
inside the folders it backs up, on this computer's own disks. A file on a
network location, on a USB stick or in the Trash has none.

## Turn them off

To hide the items without uninstalling them, open **Preferences → General**
in Backtrack and turn off the switch beside **GNOME Files (Nautilus)** or
**Dolphin**. Turn it on again to bring them back.

To remove them completely:

- From a package, remove its package, for example with `sudo dnf remove
  backtrack-nautilus` on Fedora or `sudo apt remove backtrack-nautilus` on
  Ubuntu.
- From the Flatpak, click **Remove** beside the file manager in
  **Preferences → General**.

## If it did not go as described

- **GNOME Files shows no items after installing them.** Quit it with
  `nautilus -q` and open it again: it loads extensions only when it starts.
- **There are no items on a file.** The file is outside the folders Backtrack
  backs up, or on a network location or a removable drive. **Preferences →
  Backup** lists the folders it backs up.
- **The menu does not change after you flip the switch.** GNOME Files builds
  the menu when you select something. Click another file, then right-click
  the one you want.
- **The Flatpak offers How to Install… for GNOME Files instead of Install.**
  nautilus-python is not installed. Do step 1 of [From the
  Flatpak](#from-the-flatpak), then look again.

## Where to go next

[Go back through a folder's history with Rewind](rewind.md), or [Get a file
back from an earlier backup](restoring.md).
