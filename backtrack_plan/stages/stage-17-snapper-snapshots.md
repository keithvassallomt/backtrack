# Stage 17 — Snapper Snapshots in the Timeline

## Objective
Where Snapper already takes btrfs snapshots of the backed-up folders, the
timeline shows them among the backups, and they can be browsed, previewed,
compared and restored from. A script takes someone who has never heard of
Snapper from nothing to snapshots every few minutes, and back again, and a
guide says what it does.

Why it is worth having:
- **The hours Backtrack skips.** Backtrack does not back up on battery by
  default (`on_battery: false`), nor while paused. Snapper goes on through
  both, since a snapshot costs next to nothing.
- **The gap between backups.** At Snapper's default of one an hour it adds
  little, but its schedule is one setting away from every few minutes, which
  narrows the gap that Stage 16 covers only for files that went to the Trash:
  a file removed with `rm`, or a good document saved over with a bad one.

Decided with Keith, 2026-10-20: Backtrack reads Snapper's snapshots, and never
creates, changes or removes them. Creating snapshots itself would need root,
which a Flatpak cannot bring with it.

Revised with Keith, 2026-10-26: a script shipped with Backtrack sets Snapper
up and undoes it, run by the person in a terminal, with `sudo` (S17-T6).
Preferences starts it (S17-T7): in the desktop's terminal for a native
package, and for the Flatpak, which cannot open a terminal outside its
sandbox without the `org.freedesktop.Flatpak` permission and asks for none, by
copying the command that runs it. Backtrack itself still never takes, changes
or removes a snapshot. The guide (S17-T5) says what the script does, step by
step, for anyone who wants to know, or to do it by hand.

Ubuntu dropped with Keith, 2026-10-26: its installer puts `/home` inside one
flat btrfs subvolume, with no `@home`, so Snapper cannot take snapshots of it
alone, and the script supports Fedora, openSUSE and Arch Linux. It refuses
anywhere else, and wherever `/home` is not a subvolume of its own, by looking
at the filesystem rather than at the distribution's name.

## The rule
**Where Snapper's snapshots are.** Snapper keeps a configuration's snapshots
in a `.snapshots` folder at the top of the subvolume it covers, each as
`<N>/snapshot/` with an `<N>/info.xml` beside it giving its number, type, date
and description. For each backed-up folder, Backtrack looks in the
`.snapshots` folder of the subvolume containing it (found as Stage 5 finds it,
`snapshot::containing_subvolume`). No Snapper configuration file under `/etc`
is read, and no D-Bus call is made, so the Flatpak needs no new permission:
`--filesystem=host` already reaches the subvolume.

**Who can read them.** `.snapshots` belongs to root. A user can read it once
Snapper's configuration names them in `ALLOW_USERS` with `SYNC_ACL=yes`, which
the script (S17-T6) sets up. A `.snapshots` folder that exists but cannot be
read is reported as exactly that, so the window can say what to change.

**What they are.** A Snapper snapshot is a copy on this computer, on the same
disk as the files, and Snapper removes it on its own schedule. It is not a
backup, and its badge and tooltip say so, as the "local backup only" badge
does (S10-T7).

## Prerequisites
Stage 5 (`snapshot::containing_subvolume`, and `walk::walk_as`, which reads a
snapshot as the subvolume it copies), Stage 1 (archives inserted into the
middle of the catalogue in time order, as `sync_archives` does for backfill),
Stage 15 (the timeline lists only the backups in which the folder differs,
Snapper's snapshots included; S15-T5, which routes preview and restore by the
archive's kind).

## Order of work
T1, T2 and T3 in order: each reads what the one before provides. T4 after T2.
T6 after T4, T7 after T6. T5 can be drafted at any point, and is finished with
T6, whose commands it gives.

## Tasks

### S17-T1 — Find Snapper's snapshots
A function in `backtrack-core` that, given the backed-up folders, finds the
`.snapshots` folder of each containing subvolume and lists its snapshots from
their `info.xml`: number, type, date, description. Snapshot 0 (Snapper's name
for the live filesystem) is left out. A snapshot whose `info.xml` is missing or
malformed, or whose `snapshot/` folder is not there, is skipped with a debug
log. A `.snapshots` folder that cannot be read is reported as such, distinct
from there being none.

**Accept:** unit tests on fixture folders shaped like Snapper's, with
`info.xml` files copied from real snapshots on a VM: the fields read, the date
read in the zone Snapper writes it in, snapshot 0 and broken entries skipped,
an unreadable `.snapshots` reported as unreadable, and a backed-up folder in a
subvolume with no `.snapshots` giving nothing.

### S17-T2 — Catalogue Snapper's snapshots
The daemon catalogues each new Snapper snapshot by walking it as the subvolume
it copies, through the same path as Stage 5's own snapshots, so every file is
recorded where it really lives. Snapshots are slotted into the catalogue in
time order among the backups, and a snapshot Snapper has removed leaves the
catalogue the way a pruned archive does.
- A new archive kind, `snapper`, beside `primary`, `spool` and `fs-snapshot`.
  `archives.repo` has a `CHECK` on its values, so this is a schema migration
  that rebuilds the table.
- Checked at startup, after every backup, and when the `.snapshots` folder
  changes (an inotify watch on each, a handful of folders).
- Never a write anywhere under `.snapshots`.

A catalogue walk reads the metadata of every backed-up file. It is timed on
Keith's home folder, and the figure decides the shortest schedule the guide
(S17-T5) recommends.

**Accept:** daemon tests on a loopback btrfs image (skipped with a warning
where one cannot be made, as Stage 5's are): a snapshot appears in the
catalogue with its files at their real paths, browsable through `folder_at`;
one older than the newest backup is slotted in by time; one removed leaves the
catalogue with the intervals around it rejoined; nothing under `.snapshots` is
modified. The walk's time on Keith's home folder is recorded.

### S17-T3 — Preview, compare and restore from a snapshot
Preview, the compare view and the restore pipeline (S07-T1) choose where an
archive's files come from by the archive's kind. For a snapshot folder
(Snapper's, and Stage 5's own btrfs snapshots), the fetch is a copy out of the
snapshot rather than a `borg extract`, and everything after it (skip
identical, conflicts, stash, Undo) is unchanged. S15-T5 makes the three choose
between the destination's repository and the spool's; this adds snapshot
folders, which neither repository holds.

**Accept:** daemon tests on the loopback image: a file previewed from a Snapper
snapshot and one from a Stage 5 snapshot; a file and a folder restored from
each, with a conflict and an Undo; a restore from a snapshot works with the
destination unreachable. On a btrfs VM with Snapper set up by the guide, a file
changed after a snapshot is restored from it from the window.

### S17-T4 — Snapper's snapshots in the window
- In the sidebar, a Snapper snapshot carries a badge of its own, with a tooltip
  saying that it was taken by Snapper on this computer, is not a backup, and
  goes when Snapper removes it.
- Preferences → Storage says whether Snapper's snapshots are being read, and
  how many. Where `.snapshots` exists but cannot be read, it says so and links
  to the guide's section on access. Where the backed-up folders are on btrfs
  with no Snapper snapshots, it links to the guide.

**Accept:** unit tests on the badge and on the three Preferences states; on a
btrfs VM, each state seen in turn: no Snapper, Snapper unreadable, Snapper read.

### S17-T5 — A guide to Snapper
`docs/snapper.md`, for a reader who has never heard of Snapper. Linked from
the README and from Preferences (S17-T4). It leads with the script (S17-T6)
and the button that starts it (S17-T7), then says what the script does, step
by step, as the commands it runs, for anyone who wants to know or to do it by
hand.
- What Snapper is, in plain terms, and what it adds to Backtrack; that its
  snapshots are on the same disk and are not a backup.
- Whether it applies: how to check that the backed-up folders are on btrfs and
  that `/home` is a subvolume, and what to do if not (this guide does not
  apply; nothing is converted).
- Installing it on Fedora, openSUSE and Arch Linux.
- Creating a configuration for `/home`, giving the user access (`ALLOW_USERS`
  and `SYNC_ACL`), and turning on the timeline and cleanup timers.
- Taking snapshots more often than hourly: the timer override, and the cleanup
  limits that decide how long the extra snapshots are kept, so that they are
  not thinned away before they are useful. The shortest interval recommended
  follows S17-T2's timing.
- Disk space: what snapshots cost, why deleting a file does not free its space
  until the snapshots holding it are gone, and how to see what they use.
- Undoing all of it.

Every command is checked on a VM before it goes in, not written from memory.

**Accept:** a test that each command the guide gives for a distribution is
one the script runs there; the Fedora sections followed by hand as written on
the GNOME VM, ending with Backtrack showing Snapper's snapshots every five
minutes, and the undo section leaving no Snapper configuration and no
snapshots behind (done 2026-10-26).

### S17-T6 — A script that sets Snapper up, and undoes it
`backtrack-snapper-setup`, installed by every package (the Flatpak's copy is
for S17-T7 to hand over). Run by the person, as themselves, in a terminal:
- Works out the distribution from `/etc/os-release`: Fedora, openSUSE
  (Tumbleweed and Leap) or Arch Linux. On anything else it says so, and
  stops.
- Checks that `/home` is a btrfs subvolume, and stops, saying why, if not.
- Says what it is about to do, and asks once. Each command is shown as it is
  run; `sudo` asks for the password in the terminal.
- Installs Snapper if it is not there; makes a configuration for `/home`;
  gives this user access (`ALLOW_USERS`, `SYNC_ACL`); starts the timeline and
  cleanup timers; takes a snapshot every five minutes, keeping a day of them;
  keeps a week of daily snapshots and nothing older; takes a first snapshot,
  and lists it.
- Records what it changed, and what it found already there (Snapper
  installed, timers on, openSUSE's own configuration for the system). Run
  again, it offers to undo exactly what it changed, and leaves what it found.
- A configuration for `/home` it did not make is not changed, apart from
  giving this user access to it if they have none, which is all its undo then
  takes back.

**Accept:** on each supported distribution (Fedora and openSUSE on fresh
btrfs VMs; Arch Linux on Keith's laptop, by his decision, 2026-10-26),
the script run from nothing ends with Backtrack showing Snapper's snapshots
every five minutes, and run again, its undo leaves the system as it was found:
no configuration, snapshots, timers or package it added, and anything it found
untouched (openSUSE's configuration for the system goes on working). On an
unsupported system, and on a `/home` that is not a subvolume, it stops before
changing anything.

### S17-T7 — Set Up Snapper from Preferences
- Preferences → Storage → Snapper offers **Set Up Snapper…** where Snapper is
  not set up, or its snapshots cannot be read, and the script would run here
  (a distribution it knows, with `/home` a subvolume of its own). Where it
  would not, Preferences says nothing of Snapper not being set up.
- From a native package, it opens the desktop's terminal running the script
  (GLib's terminal lookup, which must find GNOME's Ptyxis and Console and
  KDE's Konsole). From the Flatpak, it puts the script where a terminal
  outside the sandbox can run it, copies the command that runs it, and says
  where to paste it.
- The daemon announces a change in what it finds of Snapper (a signal, as
  `TrashChanged` is), so Preferences shows the result as soon as the script
  is done, without being reopened.

**Accept:** unit tests on what the button offers in each Snapper state and
for each kind of package; on the GNOME VM and the Plasma VM, the button opens
the terminal running the script, and Preferences shows Snapper's snapshots
when it ends; in the Flatpak on the GNOME VM, the copied command, pasted into
a terminal, does the same.

### S17-T8 — Reading a backup into the catalogue no longer slows with its size
*(defect, found while checking S17-T6 on Keith's laptop)* An ingest looks up,
for every file in the listing, the version ending at the archive before it.
Since the `versions_last_seq` index (S15), SQLite has looked that up through
it rather than through the file's path: nearly every file in the newest backup
ends there, so each lookup read all of them. Reading Keith's 55,000 files into
his catalogue took 6.5 seconds, for a backup and for a Snapper snapshot alike,
and it grows with the square of the number of files. The lookup is by path.

**Accept:** a test that both lookups an ingest makes go by path, shown to fail
on the query as it was; reading Keith's backed-up folders into a copy of his
catalogue takes a fraction of a second (6.5 s before).

## Definition of Done
All tasks done; the CHANGELOG's Unreleased section says what changed for the
user; Stage 5's own btrfs snapshots can be previewed and restored from, as a
side effect of S17-T3.
