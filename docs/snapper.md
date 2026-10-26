# Snapshots between backups, with Snapper

Backtrack backs up every hour, unless you have chosen otherwise, and not at
all while your laptop runs on battery or while backups are paused. Anything
you change and lose in between is in no backup: a good document saved over
with a bad one at 12:40, or a file deleted with `rm`, is gone from the 13:00
backup and was never in the 12:00 one.

If your home folder is on btrfs, a program called Snapper can fill those
gaps. This guide takes you from never having heard of it to snapshots every
few minutes, shown in Backtrack's timeline among your backups.

## What Snapper is

btrfs is a filesystem that can take **snapshots**: a copy of a folder exactly
as it was at one moment, made in an instant. A snapshot takes no extra disk
space when it is made. It only starts to cost space as your files change,
because it keeps the versions you have since changed or deleted.

Snapper is a program that takes these snapshots on a schedule, every hour to
begin with, and removes old ones so that they do not fill your disk.

What it adds to Backtrack:

- **The hours Backtrack skips.** Snapper goes on taking snapshots on battery
  and while backups are paused, since a snapshot costs next to nothing.
- **The gaps between backups.** Snapper can take a snapshot every few
  minutes, so a file overwritten or deleted between two backups can still be
  found.

Backtrack reads Snapper's snapshots, and never takes, changes or removes one.
Each is shown in the timeline with the badge **Snapper snapshot**, and can be
browsed, previewed, compared and restored from like a backup, with or without
your backup destination.

**A snapshot is not a backup.** It is kept on the same disk as your files, so
if that disk fails, or the computer is lost, the snapshots go with it. Snapper
also removes them on its own schedule. Your backups are what keeps your files
safe; Snapper's snapshots are a quick way back to the last few hours.

## Whether this applies to you

Snapper needs your home folder to be a btrfs **subvolume**: a part of a btrfs
filesystem that can be snapshotted on its own. Open a terminal and run:

```sh
stat -f -c %T /home
stat -c %i /home
```

If the first line says `btrfs` and the second says `256`, `/home` is a btrfs
subvolume, and this guide applies. The folders you back up must be inside
`/home`, as `Documents`, `Pictures` and the rest of your home folder are.

Backtrack works this out too: Preferences → Storage → **Snapper** says
"Snapper is not set up", and offers to set it up, when your backed-up folders
are in a `/home` that is a btrfs subvolume, on a distribution the setup script
knows.

If the first line says anything else (`ext2/ext3`, which is how ext4 shows,
or `xfs`), your home folder is not on btrfs. This guide does not apply, and
nothing here converts a filesystem: Backtrack works fully without Snapper.

If the first line says `btrfs` and the second does not say `256`, your home
folder is an ordinary folder inside a larger subvolume, as Ubuntu's installer
lays btrfs out. Snapper could take snapshots of that whole subvolume, your
system included, but that is beyond this guide.

## Setting it up

Backtrack comes with a script that does all of this for you. In Backtrack,
open Preferences → Storage, and under **Snapper** choose **Set Up Snapper…**:
it opens a terminal running the script. From the Flatpak, it copies the
command that runs it instead, for you to paste into a terminal. You can also
run it yourself, from a terminal:

```sh
backtrack-snapper-setup
```

It works on Fedora, openSUSE and Arch Linux. It checks that this guide applies
to your computer, says what it is about to do, and asks you once.
Each command is shown as it runs, and `sudo` asks for your password for the
ones that need the administrator.

Run it again, and it offers to undo what it did. It keeps a note of what it
changed in `/var/lib/backtrack/snapper-setup`, so that the undo takes back
only that: a Snapper you had already, or openSUSE's own snapshots of the
system, are left as they were. If Snapper already takes snapshots of `/home`,
the script changes nothing but letting you read them, and that is all its
undo takes back.

The rest of this guide gives the commands the script runs, step by step, for
anyone who wants to know what changes, or to do it by hand.

## Installing Snapper

### Fedora

```sh
sudo dnf install -y snapper
```

### openSUSE

openSUSE installs Snapper for its own snapshots of the system, so there is
usually nothing to install. If it is not there:

```sh
sudo zypper --non-interactive install snapper
```

### Arch Linux

```sh
sudo pacman -S --needed --noconfirm snapper
```

## Taking snapshots of your home folder

Tell Snapper to take snapshots of `/home`, in a configuration named `home`,
and start its two timers: one takes a snapshot every hour, the other removes
old snapshots once an hour.

```sh
sudo snapper -c home create-config /home
sudo systemctl enable --now snapper-timeline.timer snapper-cleanup.timer
```

Check that both timers are waiting to run:

```sh
systemctl list-timers 'snapper-*'
```

Snapper keeps the snapshots in a folder at the top of `/home`, named
`.snapshots`, and keeps the first snapshot of each of the last 10 hours,
10 days, 10 months and 10 years; the next sections change both.

At this point, Backtrack's Preferences → Storage → **Snapper** says "Snapper's
snapshots cannot be read". `.snapshots` belongs to the system administrator,
and the next step lets you read it.

## Giving Backtrack access

Let your own user read Snapper's snapshots of `/home`:

```sh
sudo snapper -c home set-config ALLOW_USERS="$USER" SYNC_ACL=yes
```

`ALLOW_USERS` names who may use this configuration, and `SYNC_ACL` gives them
permission to read `.snapshots`. It also lets you use Snapper for this
configuration without `sudo`. To check, run this as yourself:

```sh
snapper -c home list
```

It lists the snapshots taken so far. To see one in Backtrack straight away,
take one by hand:

```sh
snapper -c home create --description "My first snapshot"
```

Within a few seconds, Backtrack's timeline shows it, badged **Snapper
snapshot**, and Preferences → Storage → **Snapper** says "Reading 1 snapshot
taken by Snapper". Snapshots that Snapper takes later appear the same way, as
they are taken.

If more than one person uses this computer, name each of them, separated by
spaces: `ALLOW_USERS="alice bob"`.

## Taking snapshots more often

Every hour adds little to Backtrack's own hourly backups. Every five minutes
narrows the gap they leave to five minutes. Change the timer's schedule:

```sh
sudo mkdir -p /etc/systemd/system/snapper-timeline.timer.d
printf '[Timer]\nOnCalendar=\nOnCalendar=*:0/5\n' | sudo tee /etc/systemd/system/snapper-timeline.timer.d/every-5-minutes.conf
sudo systemctl daemon-reload
systemctl list-timers snapper-timeline.timer
```

The last command shows the next snapshot due within five minutes. The
schedule is shared by every Snapper configuration that takes timeline
snapshots. openSUSE's own configuration for the system takes none, so only
`/home` gains the extra snapshots.

Snapper's cleanup keeps only the first snapshot of each hour, and removes
the others once they are an hour old. That would throw the five-minute
snapshots away before they had been much use. Keep them for a day instead:

```sh
sudo snapper -c home set-config TIMELINE_MIN_AGE=86400
```

That keeps every snapshot of the last 24 hours (288 of them, at one every five
minutes), and the first of each hour, day, month and year after that, within
the limits above.

Five minutes is the shortest interval this guide recommends. Backtrack reads
every snapshot Snapper takes into its catalogue: under a second for a home
folder of 55,000 files, longer for a larger one, and while it does, a backup
waits its turn.

## Disk space

A snapshot costs nothing when it is taken. It starts to cost space as files
change after it, because it keeps the versions it holds.

**Deleting a file does not free its space until every snapshot holding it is
gone.** With the limits above, Snapper keeps the first snapshot of each year
for ten years, so a large file deleted today can stay on your disk for a
decade. Backtrack already keeps your long history, at your backup
destination, so Snapper only needs to keep the recent past. To keep a week of
daily snapshots and nothing older:

```sh
sudo snapper -c home set-config TIMELINE_LIMIT_DAILY=7 TIMELINE_LIMIT_MONTHLY=0 TIMELINE_LIMIT_YEARLY=0
```

To see what each snapshot holds that nothing else does, which is what
removing it would free (the `Exclusive` column), run the command below. It
reads every file in `/home`, so on a large home folder it can take many
minutes:

```sh
sudo btrfs filesystem du -s --human-readable /home/.snapshots/*/snapshot
```

To see how much room is left on the disk:

```sh
df -h /home
```

To remove a snapshot now, give its number from `snapper -c home list`:

```sh
snapper -c home delete 42
```

## Undoing all of it

To stop taking snapshots and remove every one Snapper has taken of `/home`,
with its configuration:

```sh
sudo systemctl stop snapperd.service
sudo snapper -c home delete-config
sudo rm -f /etc/systemd/system/snapper-timeline.timer.d/every-5-minutes.conf
sudo rmdir --ignore-fail-on-non-empty /etc/systemd/system/snapper-timeline.timer.d
sudo systemctl daemon-reload
```

Stopping Snapper's background service first works around a fault in Snapper
0.13: when its settings were changed since the service started, it writes the
configuration back as it deletes it.

Then stop Snapper's timers, if you started them for this. On openSUSE, leave
them on: they take the system's own snapshots too.

```sh
sudo systemctl disable --now snapper-timeline.timer snapper-cleanup.timer
```

Then remove Snapper itself, if you installed it for this.

On Fedora:

```sh
sudo dnf remove -y snapper
```

On openSUSE:

Keep Snapper unless you installed it yourself for this guide, since openSUSE
uses it for its snapshots of the system. Removing it also removes what it was
installed with:

```sh
sudo zypper --non-interactive remove --clean-deps snapper
```

On Arch Linux:

```sh
sudo pacman -Rns --noconfirm snapper
```

Check that no configuration is left:

```sh
ls /etc/snapper/configs
```

It prints nothing, or says there is no such folder. If it lists `home`,
remove what is left by hand with `sudo rm /etc/snapper/configs/home`.

Within a few seconds, Snapper's snapshots leave Backtrack's timeline, and
Preferences → Storage → **Snapper** says "Snapper is not set up". Your backups
are not touched.
