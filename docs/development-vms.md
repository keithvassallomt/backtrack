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
3. Here: `ssh-copy-id -f $(scripts/vm address <vm>)`, with `<vm>` the
   libvirt domain name, quoted if it has spaces (`"fedora45 (GNOME)"`).
   Without `-f`, `ssh-copy-id` first logs in with each key to see which are
   installed, and OpenSSH 9.8 and later answer a burst of failed logins by
   refusing the address for a while: the copy that follows fails with
   "Connection reset by peer". If it happens anyway, wait half a minute.
4. Here, from your own terminal: `just vm-push <vm>`. It asks for the VM's
   sudo password once, to install the packages, and the first build takes a
   few minutes.

## Once more per VM, to drive it without anyone at its screen

`just vm-start` boots a VM and waits until it is ready to use, so that a VM
can be started, tested and shut down from a script, or by Claude. That needs
three more changes in the VM. They are test machines, so none of them gives
anything away.

1. Let sudo run without a password, so that a push that needs packages does
   not stop to ask:

   ```
   echo "$USER ALL=(ALL) NOPASSWD: ALL" | sudo tee /etc/sudoers.d/90-dev
   ```

2. Log in automatically, as yourself:
   - GNOME: in `/etc/gdm/custom.conf`, under `[daemon]`,
     `AutomaticLoginEnable=True` and `AutomaticLogin=<you>`.
   - Plasma: in `/etc/plasmalogin.conf`, under `[Autologin]`,
     `User=<you>` and `Session=plasma`.
3. Let the keyring open without the password typed at the login screen,
   which an automatic login does not type:
   - GNOME: Fedora's Secret Service there is oo7-daemon, which keeps a
     keyring locked until it is given a password, even an empty one. From
     your own terminal, `scripts/vm run <vm> scripts/vm-keyring rekey` asks
     for your password in the VM, once, and changes the login keyring's to
     the development password, `backtrack dev`. `just vm-start` opens it
     with that after every boot. If you ever log in at the screen with your
     own password, the keyring asks for `backtrack dev`.
   - Plasma: in KDE Wallet Manager, select `kdewallet`, choose **Change
     Password**, and leave the new one empty. KWallet opens a wallet with
     an empty password without asking.

`just vm-start <vm>` then starts the VM, waits for SSH, waits for the
desktop to log in, and opens the keyring (`scripts/vm-keyring unlock`). If
one of those does not happen, it stops and says which. A VM that is already
up takes under a second.

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
| Start it, ready to use | `just vm-start <vm>` |
| Shut it down | `just vm-stop <vm>` |
| Open the window on the VM's screen | `just vm-app <vm>` (any window arguments after it, such as `--path`) |
| Run something in the VM's checkout | `scripts/vm run <vm> <command>` |
| What is installed and running there | `scripts/vm run <vm> scripts/dev-machine status` |
| The daemon's log | `scripts/vm run <vm> journalctl --user -u backtrackd -n 50` |

The VM's address is looked up from libvirt on every call. On libvirt's
default NAT network the VM's address is libvirt's to hand out, so it does
not change with the network the host is on, and a new lease would not matter
anyway. Its SSH host key is accepted the first time and checked after that.

## Keyrings

The daemon keeps passphrases in the desktop's Secret Service: oo7-daemon on
Fedora's GNOME and KWallet on Plasma. The first `just vm-push` stores the demo
passphrase there, and on Plasma KWallet may ask on the VM's screen to create
a wallet before it does. If the push stops at the `ImportRepo` call, look at
the VM's screen. A VM set up to log in automatically needs its keyring
opened without a password, as above.
