# File-manager integration

Backtrack adds a few items to the file manager's right-click menu. Each one
opens Backtrack's own window, or [Rewind](rewind.md), its full-screen view,
at the right place.

## Installing

Pick the steps for how you installed Backtrack. **Preferences → General**
shows what is installed: *Installed* and a switch, which hides the items
without uninstalling them.

### From a package

If you installed Backtrack from COPR (Fedora), the PPA (Ubuntu) or the AUR
(Arch).

**GNOME Files**

1. Install Backtrack's GNOME Files package, which brings nautilus-python
   with it:
   - Fedora: `sudo dnf install backtrack-nautilus`
   - Ubuntu: `sudo apt install backtrack-nautilus`
   - Arch: `backtrack-nautilus` from the AUR, as you installed `backtrack`
2. Quit GNOME Files so that it loads the new items: `nautilus -q`. Open it
   again.

**Dolphin**

1. Install Backtrack's Dolphin package:
   - Fedora: `sudo dnf install backtrack-dolphin`
   - Ubuntu: `sudo apt install backtrack-dolphin`
   - Arch: `backtrack-dolphin` from the AUR
2. Right-click a file. Nothing needs restarting.

### From the Flatpak

**GNOME Files**

1. Install nautilus-python from your distribution. GNOME Files needs it to
   run any extension, and a Flatpak cannot install it for you:
   - Fedora: `sudo dnf install nautilus-python`
   - Ubuntu: `sudo apt install python3-nautilus`
   - Arch: `sudo pacman -S python-nautilus`
2. In Backtrack, open **Preferences → General** and click **Install** beside
   GNOME Files.
3. Quit GNOME Files so that it loads the new items: `nautilus -q`. Open it
   again.

**Dolphin**

1. In Backtrack, open **Preferences → General** and click **Install** beside
   Dolphin.
2. Right-click a file. Nothing needs restarting.

**Remove** in the same place takes the items away again. An update of the
Flatpak updates them too.
