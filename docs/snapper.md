# Take snapshots between backups with Snapper

Backtrack backs up every hour, so a file you change and lose between two
backups is in neither of them. If your home folder is on btrfs, a program
called Snapper can take a snapshot of it every five minutes, and Backtrack
shows those snapshots in its timeline next to your backups. This guide sets
Snapper up with one script and ends with its first snapshot in the timeline.

## What Snapper adds

A snapshot is a copy of your home folder exactly as it was at one moment.
btrfs makes one in an instant, and it takes up no space until your files
change after it. Snapper takes them on a schedule and removes old ones.

Backtrack reads Snapper's snapshots but never takes, changes or removes one.
In the timeline each has the badge **Snapper**, and you can browse it,
preview its files, compare them and restore from it as from a backup, with
the backup drive plugged in or not. Snapper also goes on taking snapshots
while the laptop is on battery and while backups are paused.

A snapshot is not a backup. It is kept on the same disk as your files, so if
that disk fails, or the computer is lost, the snapshots go with it. Your
backups keep your files safe; Snapper's snapshots are a quick way back to
the last few hours.

## Check whether it applies

1. In Backtrack, open the main menu, choose **Preferences**, then
   **Storage**.
2. Look for a section named **Snapper**.
   - If it says **Snapper is not set up**, with a **Set Up Snapper…** button,
     this guide applies. Carry on with [Set it up](#set-it-up).
   - If it says **Reading** and a number of snapshots taken by Snapper,
     Snapper is set up already and Backtrack shows its snapshots. There is
     nothing to do.
   - If it says **Snapper's snapshots cannot be read**, follow [Giving
     Backtrack access](#giving-backtrack-access).
   - If there is no Snapper section, your home folder is not on btrfs, or
     your distribution is not one the setup script knows (Fedora, openSUSE
     and Arch Linux). Backtrack works fully without Snapper, and there is
     nothing to do.

## Set it up

1. In **Preferences → Storage**, click **Set Up Snapper…**. A terminal opens
   and runs Backtrack's setup script.

   In the Flatpak, a window called **Paste the Command Into a Terminal**
   shows a command instead. Copy it, open a terminal (search your apps for
   **Terminal**), paste it with **Ctrl+Shift+V** and press **Enter**.
2. The script lists what it will do: install Snapper if it is missing, make a
   Snapper configuration for `/home` named "home", let you read its
   snapshots, take a snapshot every five minutes and keep a day of them, keep
   a week of daily snapshots and nothing older, and take a first snapshot
   now. It asks **Go ahead? [y/N]**. Type `y` and press **Enter**.
3. When it asks for your password, type it and press **Enter**. Nothing
   appears on the screen while you type. The script shows each command as it
   runs it.
4. It ends with "Done. Snapper takes a snapshot of /home every five minutes,
   and Backtrack shows them in its timeline". Close the terminal.
5. Back in Backtrack, the timeline shows the first snapshot, badged
   **Snapper**, within a few seconds. **Preferences → Storage** says
   **Reading 1 snapshot taken by Snapper**.

From then on a new snapshot appears every five minutes. Snapper keeps every
snapshot of the last day, then one a day for a week.

## Giving Backtrack access

**Preferences → Storage** says **Snapper's snapshots cannot be read** when
Snapper takes snapshots of your home folder but keeps them where only the
system administrator can read them. That happens when Snapper was set up
some other way than with this guide.

1. Click **Set Up Snapper…** in the Snapper section, as in [Set it
   up](#set-it-up). The script sees that Snapper is set up already, and
   offers only to let you read its snapshots, leaving the rest as it is.
2. Answer **Go ahead? [y/N]** with `y`, and type your password when it asks.
3. Within a few seconds, Backtrack's timeline shows the snapshots, and the
   Snapper section says how many it is reading.

If more than one person uses this computer and each wants Snapper's
snapshots in Backtrack, [Setting Snapper up by hand](snapper-by-hand.md#letting-you-read-the-snapshots)
says how to let each of them read them.

## Undo it

Running the setup script again offers to undo what it did: it removes every
snapshot Snapper has taken of your home folder, and its configuration, and
removes Snapper if the script installed it. Your files and your backups are
not touched. A Snapper you had before, or openSUSE's own snapshots of the
system, are left as they were.

1. Open a terminal and run the script. From Fedora's, Ubuntu's or Arch
   Linux's package:

   ```sh
   backtrack-snapper-setup
   ```

   From the Flatpak:

   ```sh
   bash ~/.var/app/tools.backtrack.Backtrack/data/backtrack/backtrack-snapper-setup
   ```

2. It lists what undoing takes away and asks **Undo it? [y/N]**. Type `y`,
   press **Enter**, and type your password when it asks.
3. Within a few seconds, Snapper's snapshots leave Backtrack's timeline, and
   **Preferences → Storage** says **Snapper is not set up**.

## If it did not go as described

- **Set Up Snapper… does nothing, or no terminal opens.** Backtrack copies the
  command instead and shows it in **Paste the Command Into a Terminal**.
  Paste it into a terminal yourself.
- **The script stops and says this computer is not one it knows.** It works
  on Fedora, openSUSE and Arch Linux only. Backtrack works fully without
  Snapper.
- **No snapshot appears in the timeline after setting it up.** Open
  **Preferences → Storage** and read what the Snapper section says. If it
  says the snapshots cannot be read, follow [Giving Backtrack
  access](#giving-backtrack-access).

## Where to go next

[Setting Snapper up by hand](snapper-by-hand.md) gives every command the
script runs, for anyone who wants to know exactly what it changes or to do
it without the script.
