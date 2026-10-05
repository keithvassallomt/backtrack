# Development VMs

Some of Backtrack can only be tried on a particular desktop: GNOME Shell's
background apps in quick settings (S12-T4), and the tray and its autostart
on Plasma (S12-T3). The development VMs are those desktops. They are test
machines, not workspaces: the code is edited in the checkout on the host, and
`just vm-push` copies it into a VM over SSH and builds it there.

## What a VM needs

- Fedora 44 or later, Workstation (GNOME) or the KDE Plasma edition. Backtrack
  is built against GNOME 50's libraries (GTK 4.22, libadwaita 1.9) on every
  desktop, and Fedora 44 is the first release that has them.
- libvirt on `qemu:///system` with the default NAT network, so the host can
  reach it and `virsh` can look up its address. The VMs can also be anything
  else SSH reaches; pass `user@host` instead of a domain name.
- A user with sudo. By default the VM user is assumed to have your name; set
  `BACKTRACK_VM_USER`, or pass `user@vm`, if not.

## Once per VM

1. Start the VM and log in on its screen. Backtrack's daemon runs in your
   desktop session, so the VM needs someone logged in.
2. In the VM: `sudo systemctl enable --now sshd`.
3. Here: `ssh-copy-id $(scripts/vm address <vm>)`, with `<vm>` the libvirt
   domain name (`fedora44`).
4. Here, from your own terminal: `just vm-push <vm>`. It asks for the VM's
   sudo password once, to install the packages, and the first build takes a
   few minutes.

## Every change after that

`just vm-push <vm>`. No password, and it does only what the change needs:

1. Copies the checkout in with rsync, as `.gitignore` decides, deleting what
   was deleted here and never the VM's own build.
2. Runs `scripts/dev-machine` in the VM, which:
   - installs anything missing (packages, Rust),
   - builds the workspace,
   - installs the development daemon's units and restarts the daemon onto
     the new build,
   - on first run only, builds the demo backups (`just demo-repo`) and points
     the daemon at them, with a fixed development passphrase for the local
     spool,
   - installs the Nautilus extension where there is a Nautilus, and quits
     Nautilus so the next window loads it.

`scripts/dev-machine` works the same at the VM's own terminal (`just
dev-machine` there), or on any other computer being set up for development.

## Using it

| | |
|---|---|
| Open the window on the VM's screen | `just vm-app <vm>` (any window arguments after it, such as `--path`) |
| Run something in the VM's checkout | `scripts/vm run <vm> <command>` |
| What is installed and running there | `scripts/vm run <vm> scripts/dev-machine status` |
| The daemon's log | `scripts/vm run <vm> journalctl --user -u backtrackd -n 50` |

The VM's address is looked up on every call, so a new DHCP lease does not
matter. Its SSH host key is accepted the first time and checked after that.

## Keyrings

The daemon keeps passphrases in the desktop's Secret Service: GNOME Keyring
on GNOME and KWallet on Plasma. The first `just vm-push` stores the demo
passphrase there, and on Plasma KWallet may ask on the VM's screen to create
a wallet before it does. If the push stops at the `ImportRepo` call, look at
the VM's screen.
