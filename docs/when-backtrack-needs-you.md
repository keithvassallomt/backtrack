# When Backtrack needs you

Backtrack stays quiet while your files are safe. When something stops it
from backing up, it says so, and offers a way to put it right. This guide
lists each of its warnings in Backtrack's own words: what it means, whether
your files are at risk, and what its **Fix…** button does.

## How Backtrack tells you

- A banner at the top of its window. A yellow one means your files have
  not been backed up for a while. A red one means backups cannot run until
  something is put right. A plain one is for your information and needs
  nothing from you.
- A notification, for the yellow and red ones only. Its **Fix…** button
  goes straight to the fix; clicking the notification opens Backtrack.
- The tray icon, on desktops with a tray, which says **Backups need your
  attention**.

Every warning goes away by itself once the problem has gone, and so does its
notification. If you open a fix that is no longer needed, Backtrack says
"That has been put right already".

To change which notifications you get, open **Preferences → General →
Notifications**: **Only when attention is needed**, **For every backup**, or
**Never**.

## The warnings

### No successful backups for … days

Shown as **No successful backups yet** if Backtrack has never managed one.
Nothing has been backed up for over a day: your newest changes are in no
backup. The notification comes when it starts, again after three days, and
then once a week.

**Fix…** says what is in the way: the backup destination not being
reachable, the last attempt failing and why, or the computer being off or
asleep when backups were due. Put that right, then click **Back Up Now**.

### Backup drive not reachable — protecting changes on this computer.

A plain banner, with nothing to do. The backup drive is unplugged, or the
network folder or server cannot be reached. Meanwhile Backtrack keeps the
files you change on this computer, and sends them on when the destination is
back. Plug the drive in, or reconnect, whenever it suits you.

If the destination stays away for a whole day, this becomes **No successful
backups for … days**.

### Backtrack needs your backup passphrase again.

Red. Backups are paused, because the keyring no longer gives Backtrack your
passphrase. Most often the keyring is locked: unlocking it, for example by
logging in again, clears this within a minute or so.

**Fix…** opens **Enter your backup passphrase**. Type the passphrase, leave
**Remember it so backups run automatically** ticked, and click **Unlock
Backups**. Backtrack checks it against the backups before keeping it, and
backs up straight away.

### The saved passphrase no longer matches the backup.

Red. The passphrase kept on this computer no longer opens your backups,
usually because it was changed on another computer that backs up to the same
place. **Fix…** opens the same **Enter your backup passphrase** window: type
the current passphrase and click **Unlock Backups**.

If the backups' key itself has changed since you saved the recovery key,
click **Lost the passphrase? Use your recovery key…**, choose the recovery
key file, and type the passphrase that was in use when you saved it. A
recovery key cannot bring back a passphrase that has been forgotten.

### Backtrack can't sign in to …

Red, naming the server or the network folder. Backups are paused until
Backtrack can sign in.

- For a network folder, **Fix…** offers **Sign In…**, which brings up your
  desktop's sign-in window for it. Backups carry on once you have signed in.
- For an SSH server, the server refused this computer's SSH key. Check that
  the key is still allowed on the server ([Set up an SSH key for a
  server](getting-started.md#set-up-an-ssh-key-for-a-server)), then click
  **Try Again**.

### Backtrack can't write to …

Red. Backtrack can reach the destination but is not allowed to write to it.
A drive may have been mounted read-only, or the folder's permissions may
have changed. **Fix…** says which; put it right, then click **Try Again**,
or **Sign In…** for a network folder.

### The backup drive is full.

Red. There is no room for new backups. **Fix…** offers three ways out:

- **Free Up Space** removes old backups your schedule no longer keeps, and
  the space they used, then backs up.
- **Keep Fewer Backups…** opens **Preferences → Storage**, where you can keep
  fewer old backups.
- **Change Destination…** moves your backups' future somewhere with more
  room.

### Not enough space on this computer to keep protecting changes.

This computer's own disk is nearly full, so Backtrack cannot keep your
changes here while the destination is away. It is red once there is almost
no room left. **Fix…** shows what Backtrack keeps on this computer and lets
you give some of it up:

- **Space for local snapshots**: how much room the changes kept while the
  destination is away may take.
- **Files restores replaced**: kept for thirty days so that a restore can be
  undone. **Empty…** deletes them, apart from the last hour's.
- **Previews and comparisons**: **Clear** removes them; they are made again
  when next needed.

Backups try again by themselves once there is room.

### The backup needs repair.

Red. Backtrack checks your backups once a month, and this check found part
of them damaged.

1. Click **Fix…**, then **Check Backups**. The check reads all of your
   backups and can take hours. You can close the window: it carries on.
2. If it finds nothing wrong, you see **Your backups are in good health**.
   Click **Back Up Now**.
3. If it finds damage, **Repair your backups?** explains what repairing
   does: it removes whatever cannot be saved, so that everything else can be
   used again. Nothing on this computer changes. Click **Repair**.
4. When it has finished, **Your backups are repaired**. Click **Back Up
   Now**.

If the backups cannot be repaired, **Start Fresh…** begins a new set of
backups in the same place and keeps the damaged ones beside them, renamed,
without deleting anything. On an SSH server, **Choose a New Location…** does
the same somewhere new.

To run the check yourself, open **Preferences → Advanced** and click
**Verify Repository Health…**.

### Backtrack's backup engine is missing.

Red. BorgBackup, the program Backtrack backs up with, is missing or too old.
**Fix…** gives the command that installs it on your distribution. Run it in
a terminal, then click **Check Again**. In the Flatpak, Borg comes with
Backtrack: reinstalling Backtrack puts it back.

### In Preferences only

Two notices appear only in Preferences, unless they last a week:

- **… backups not yet browsable**, on **Advanced**. Those backups are stored
  safely, but their file lists have not been read yet. Backtrack keeps
  trying; **Fix…** or **Rebuild** reads every backup's file list again.
- **Catalogue rebuilding…**, while that rebuild runs. It goes away by itself.

## Pausing backups on purpose

To stop backups for a while, open the main menu, choose **Pause Backups**,
then **For 1 hour**, **Until tomorrow** or **Until I resume**. A plain banner
says **Backups are paused until** a time, or **until you resume them**, with
a **Resume** button. **Resume Backups** in the main menu does the same, and a
timed pause ends by itself.

A pause is not a problem: its banner has no colour, offers **Resume** rather
than **Fix…**, and sends no notification. **Back Up Now** still backs up
while backups are paused.

## Waiting is not a problem either

The line at the bottom of the window sometimes adds why the next backup is
waiting. None of these needs anything from you:

- **waiting for charger**: the laptop is on battery, and Backtrack waits for
  mains power, for up to three hours after the last backup.
- **waiting for an unmetered network**: the connection is marked as metered.
- **your backup destination is away**: the drive is unplugged or the
  destination cannot be reached.

[Set up your backups](getting-started.md#change-your-settings) says where to
change each of these.

## Where to go next

[Get a file back from an earlier backup](restoring.md).
