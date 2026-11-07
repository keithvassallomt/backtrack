# Stage 19 — Guides Anyone Can Follow

## Objective
Every guide in `docs/` reads as a walk-through that someone who has never
thought about backups can follow from the first step to the last, and the
guides that are missing exist: installing Backtrack, setting it up, getting a
file back, finding a lost file, moving to a new computer, and what its
warnings mean. Developer notes leave the guides for files of their own.

These are the guides backtrack.tools publishes (Stage 20), rendered from this
repository at a release tag, so they are written once, here.

Decided with Keith, 2026-11-07:
- **Non-technical and step by step.** A reader follows a guide like a
  tutorial. None of them is a reference to be searched.
- **Every guide, new or rewritten, is drafted and then run through the
  `humanizer:humanizer` skill before it is committed.**
- **Screenshots** are taken by a script wherever the desktop allows one to
  (S19-T3). Keith takes the rest from the list the script gives.

## How every guide is written
1. **A title that names what the reader will have done**: "Get a file back
   from an earlier backup", not "Restoring".
2. **Two or three opening sentences**: when this guide helps, and where it
   ends.
3. **Before you start**, only when something must already be true, as a
   short list.
4. **Numbered steps.** One thing to do in each, then what the reader sees
   when it worked. Labels in bold, exactly as the app shows them. A
   screenshot wherever the screen changes enough for the reader to get lost.
5. **If it did not go as described**: the usual wrong turns, each with its
   way back.
6. **Where to go next**: the guide a reader is likely to want after this one.

A guide leaves out how Backtrack works inside (D-Bus, the catalogue,
configuration files, unit and crate names), test checklists, and records of
who checked what on which machine. A terminal command appears only where no
button does the same job, in a block of its own, ready to paste, with a
sentence saying what it does.

## What the files keep
- **Their paths.** `docs/rewind.md`, `docs/trash.md`, `docs/snapper.md` and
  `docs/file-manager-integration.md` stay where they are: the CHANGELOG's
  0.1.0 entry links to them.
- **The headings Backtrack links to**, word for word: "From a package" and
  "From the Flatpak" in `file-manager-integration.md` (Preferences' How to
  Install…), and "Giving Backtrack access" in `snapper.md` (Preferences'
  Snapper row). Installed releases open these on GitHub's `main`, and the
  tests in `crates/backtrack-gtk/src/model/prefs.rs` hold them in place.
- **Plain Markdown that reads properly on GitHub**: no front matter and no
  syntax only the site understands. Links between guides are relative
  (`rewind.md`), and pictures live in `docs/images/`, linked relatively.
  Stage 20's build turns both into the site's own.
- **Keith's writing rules**: no em-dash anywhere, and no sentence that
  begins with "And".

## Prerequisites
Stages 0 to 18: the application as released in 0.1.0. The development VMs
(`docs/development-vms.md`): the GNOME VM for most guides, the Plasma VM for
Dolphin and the tray, and the Ubuntu VM for the PPA.

## Order of work
T1 first, since it settles which file holds what. T2 and T3 next, side by
side: T5 needs T2's answer, and every guide's pictures come from T3. Then T4
to T13 in order, the new guides before the rewrites, since the site needs
them most. T14 to T16 are faults T2 found; T14 comes before T5, since it
decides whether the setting-up guide may suggest carrying on into another
program's backups.

## Tasks

### S19-T1 — An index of the guides, with developer notes moved out of them
`docs/README.md`, new, which GitHub shows when someone opens `docs/`. Under
**Guides**, a Markdown list of links to the guides in the order a new user
meets them: installing, setting up, getting a file back, finding a lost file,
Rewind, the Trash, Snapper, the file manager's items, moving to a new
computer, and Backtrack's warnings. Under **For developers**, the rest of
`docs/`. Stage 20's build reads the Guides list to know which files to
publish and in what order, so it stays a plain list of links.

The developer half of `docs/file-manager-integration.md` (how Preferences
finds an integration, and the Nautilus and Dolphin sections: how each works,
installing for development, tests, the manual checklists and the record of
the runs) moves to `docs/file-manager-integration-internals.md`, unchanged.
The comments in `integrations/dolphin/*.desktop` and `scripts/fm-check` that
point at it follow. The README's Documentation section points at
`docs/README.md`.

**Accept:** every line removed from `file-manager-integration.md` is in the
new file (compared with `git diff --word-diff`); every link in the moved
text still resolves; `just check` passes, with the heading tests in
`prefs.rs` unchanged.

### S19-T2 — Investigate: opening backups made by Vorta or Pika Backup
Whether the welcome wizard's **Already have backups?** opens a Borg
repository that Backtrack did not make, and what the window then shows. The
answer decides what the setting-up guide (S19-T5) and the site's questions
(Stage 20) can promise someone moving from Vorta or Pika Backup.

On the GNOME VM: a repository made by Vorta and one made by Pika Backup, each
with a few archives of a folder in the home folder, opened through the
wizard. For each: whether it opens at all; whether the timeline lists the
archives (Backtrack names its own `bt-<host>-<time>`); whether files can be
previewed and restored; what happens with an encryption mode Backtrack does
not set up itself (a key file rather than a key in the repository); and what
Backtrack's own next backup does to the other program's archives, since
pruning by Backtrack's rules could delete them.

**Accept:** the answer, with what was tried and what each program showed,
as a comment on this task's issue. A fault worth fixing becomes a task of
its own. S19-T5 and Stage 20 say only what was found.

### S19-T3 — Screenshots of Backtrack for the guides, the site and the software centres
One set of screenshots that the guides, the site's pages and the AppStream
listing all draw on, in `docs/images/`, of the release they illustrate.

- **A home folder that looks like somebody's.** A script makes a user on
  the GNOME VM with a home folder of believable documents, pictures and
  projects, and a few weeks of backups behind it, so that the sidebar's
  groups, the strip and Rewind's stack have something to show. Nothing in
  it says demo, fixture or test, and nothing comes from a real person's
  files. Borg's `--timestamp` and the wizard's import path are one way to
  give it a history. The script can rebuild the user from nothing.
- **Taken by a script** (`scripts/screenshots`) wherever the desktop lets a
  program take one, at one window size, in the light style and, where the
  site shows both, the dark: the window at an earlier backup, Rewind, the
  restore questions, a folder's restore summary with Undo, search with a
  deleted file, Compare with Today for a document and a picture, the
  wizard's steps and the recovery key, a warning banner with its Fix…, the
  new-computer restore, Preferences' file-manager and Snapper rows, a Trash
  entry in the timeline. The Plasma VM gives Dolphin's menu and the tray.
- **By hand, from a list.** Whatever the script cannot take, it lists, with
  how to set the screen up for it, for Keith to take.

`docs/screenshots.md`, under For developers, says how to take the set again
for the next release.

**Accept:** every picture the guides and Stage 20's pages call for is in
`docs/images/`; the script rebuilds the user and retakes its share on a
fresh run of the VM; Keith has taken the shots on its list.

### S19-T4 — A guide to installing Backtrack
`docs/installing.md`, new. Which way to install, decided by one question
(which distribution is this?), then each way step by step: the Flatpak from
FriendlyHub, for any distribution and the only one for those older than
Backtrack's GNOME 50 floor, such as Debian 13 and RHEL 10; COPR on Fedora;
the PPA on Ubuntu; the AUR on Arch. Then the file manager's items (a link to
S19-T13's guide), network folders on a desktop without GVfs (from
`packaging/flatpak/FRIENDLYHUB.md`), updating, and uninstalling, including
what it leaves behind: the backups, and the settings in the home folder.

`FRIENDLYHUB.md` keeps the release side and links here for installing. The
README's install table links here too, and Stage 20's install page is this
guide.

**Accept:** each way followed as written on a fresh VM of its distribution
(the Flatpak and COPR on the GNOME VM, the PPA on the Ubuntu VM), against the
testing channels before the release and the release channels after it, each
ending with Backtrack open at its welcome page; the AUR by Keith; what
uninstalling leaves behind checked, not assumed.

### S19-T5 — A guide to setting up your backups
`docs/getting-started.md`, new. The welcome wizard from the first launch to
the end of the first backup: what to back up, where to (a drive, a network
folder, a server over SSH), the passphrase and the recovery key (why it
matters, where to keep it), and the first backup and how long it takes.
Then what happens without being asked: a backup every hour, catching up
after sleep, waiting on battery and on a metered connection, and protection
while the drive is unplugged. Then how to change any of it in Preferences.
**Already have backups?** as S19-T2 found it.

**Accept:** followed as written on the GNOME VM from a fresh install to a
finished first backup, once to a drive (a virtual USB disk) and once to a
server over SSH.

### S19-T6 — A guide to getting a file back from an earlier backup
`docs/restoring.md`, new. Two of brief.md's stories as one walk-through: a
file deleted and the Trash emptied, and a document spoiled by a rewrite.
Opening the folder in Backtrack, from the application or from the file
manager; stepping back to the backup before the mistake (Older and Newer,
the calendar, the strip); checking it in the preview; restoring it. What the
restore asks when the file is still there (Replace, Keep Both, Cancel) and
what each choice does. A whole folder and its summary. Undo, Recently
Replaced Files, and Restore To….

**Accept:** on the GNOME VM, a deleted file and an overwritten document,
each brought back by following the guide as written.

### S19-T7 — A guide to finding a lost file with search
`docs/searching.md`, new. The other two stories: a download nobody can find,
and the right version of a photo. Searching every backup at once, what the
results say (missing from this computer, when it existed), View in Timeline
and Restore Latest Version, and Compare with Today for a document and a
picture before restoring one.

**Accept:** on the GNOME VM, a file deleted in an unknown folder found and
restored, and an older picture chosen with Compare with Today, by following
the guide as written.

### S19-T8 — A guide to moving to a new computer
`docs/new-computer.md`, new. For when the old computer is lost or broken:
installing Backtrack on the new one, connecting the old backups, the
passphrase or the recovery key, choosing everything or some folders, pausing
and carrying on, what happens to files already on the new computer, and
backups starting again once it is done. `docs/disaster-recovery-drill.md`
stays the developer's drill.

**Accept:** followed as written on a fresh GNOME VM, against backups made by
another VM, through to the first backup of the new computer.

### S19-T9 — A guide to what Backtrack's warnings mean
`docs/when-backtrack-needs-you.md`, new. Backtrack is quiet while your files
are safe; this guide covers the times it is not. Each banner and notification
in the words the app uses (health.md's catalogue): what it means, whether
your files are at risk, and what its Fix… does. Among them: no backup for a
day, the destination not connected, the passphrase needed, the keyring
locked, the destination full, and damage found by the monthly check. Also
pausing backups on purpose, and how to tell a pause from a problem.

**Accept:** each warning the guide covers brought about for real on the
GNOME VM (the drive unplugged, the keyring locked, a small destination
filled), and cleared by following the guide; those that cannot be caused
for real, named in the issue with how they were checked instead.

### S19-T10 — The Rewind guide as a walk-through
`docs/rewind.md`, rewritten by the rules above: open Rewind from wherever
the reader is, move back to the moment they want, find the file, restore
it, and undo it if it was the wrong one. The table of ways in becomes a step
that starts from each.

**Accept:** followed as written on the GNOME VM from the window and from
GNOME Files, and on the Plasma VM from Dolphin.

### S19-T11 — The Trash guide as a walk-through
`docs/trash.md`, rewritten: getting back something deleted between two
backups, step by step, then what the Trash cannot bring back, as a short
list in plain words. "How it works" moves to `docs/trash-internals.md`,
under For developers.

**Accept:** on the GNOME VM, a file written and moved to the Trash between
two backups brought back by following the guide as written, and the Undo
that puts it back in the Trash.

### S19-T12 — The Snapper guide as a walk-through
`docs/snapper.md`, rewritten for someone who has never opened a terminal on
purpose: what Snapper adds, in plain words, and that its snapshots are not
backups; checking whether it applies, by what Preferences → Storage says;
**Set Up Snapper…** and what the script asks; seeing the first snapshot in
the timeline; snapshots more often; and undoing it all. "Giving Backtrack
access" stays, as the section Preferences opens when the snapshots cannot
be read.

The commands, step by step, for anyone who wants to know what the script
does or to do it by hand, move to `docs/snapper-by-hand.md`.
`scripts/test_snapper_setup.py` then checks that file against the script, as
it checks `snapper.md` today, and the script's comments point at it.

**Accept:** followed as written on the GNOME VM, from no Snapper to
snapshots in the timeline, and its undo back to none; `just check` passes
with the test reading `snapper-by-hand.md`, and fails when a command is
taken out of that file.

### S19-T13 — The file-manager guide as a walk-through
`docs/file-manager-integration.md`, which after S19-T1 holds only the user's
part, rewritten: adding Backtrack's items to GNOME Files or Dolphin, from a
package and from the Flatpak, each step by step; using them (Restore
Previous Version…, Browse Backups of This Folder…, Rewind This File…, Rewind
This Folder…); and turning them off. "From a package" and "From the Flatpak"
stay as headings, word for word.

**Accept:** followed as written from a package and from the Flatpak, on the
GNOME VM and on the Plasma VM; `just check` passes.

### S19-T14 — Backtrack prunes only its own backups
`borg prune` (`crates/backtrack-core/src/engine/borg/mod.rs`) runs without
`--glob-archives`, so Backtrack's retention applies to every archive in the
repository, whoever made it. Carrying on into backups another program made,
which the wizard offers ("Existing Backtrack/Borg backup found — use it?"),
then deletes that program's backups: in S19-T2's test, Backtrack's first
backup removed 12 of Vorta's 51 archives and 12 of Pika Backup's 51. The
prune matches Backtrack's own archive names (`bt-…`) and nothing else, so an
archive of any other name is left alone, and retention among Backtrack's own
archives is as before.

**Accept:** a test against a real Borg repository holding archives of
another name, dated across weeks, keeps every one of them through a prune
that removes some of Backtrack's, and fails without the change; the existing
retention tests pass unchanged; a CHANGELOG entry under Unreleased.

### S19-T15 — Unencrypted backups made by other programs open from the wizard
Vorta's encryption "none", and Pika Backup's **Continue Unencrypted**, which
is Borg's repokey with an empty passphrase. Two things stop them (S19-T2).
Borg asks before it opens an unencrypted repository it has not seen on this
computer, nobody is at a terminal to answer, and `InspectDestination` fails
with "Something went wrong. The logs say what." That is every such
repository in the Flatpak, whose Borg keeps its own memory, every one on a
new computer, and every one Pika Backup made. The passphrase page's **Open
Backups** also stays insensitive until something is typed, which Pika
Backup's empty passphrase can never satisfy. The import answers Borg's
question, since the person chose these backups, and only the import does:
afterwards Borg knows the repository and asks again only if a different one
takes its place. A repository that needs no passphrase, or an empty one,
opens without one being typed.

**Accept:** on the GNOME VM, in the native build and in the Flatpak, a
repository made with Vorta's "none" and one made with Pika Backup's Continue
Unencrypted each open from **Already have backups?** with nothing typed; a
different unencrypted repository put in place of an imported one is still
refused; a CHANGELOG entry under Unreleased.

### S19-T16 — Backups whose key is in a key file can be opened with it
Vorta offers key-file encryption, which keeps the key in Borg's own folder
(`~/.config/borg/keys`) rather than in the repository. The native build on
the computer that made the backups finds it there, but Backtrack's Flatpak
looks in a folder of its own and a new computer has no key at all, so the
import stops at "Something went wrong. The logs say what." (S19-T2). The
wizard says that these backups need their key file, and lets the person
choose it, after which the passphrase opens them as it opens any others.
Vorta has no way to export the key, so the file is the one in Borg's keys
folder on the computer that made the backups, or one `borg key export`
wrote.

**Accept:** on the GNOME VM, a repository made with Vorta's key-file
encryption opened in the Flatpak and on a computer without the key, by
choosing the key file; without the file, the wizard says what is
missing rather than "Something went wrong"; a CHANGELOG entry under
Unreleased.

### S19-T17 — Rewind's window belongs to Backtrack
Rewind's window (`crates/backtrack-gtk/src/ui/rewind/mod.rs`) is an
`adw::Window` that is not the application's, so on Wayland it has the
program's name for its application ID (`backtrack-gtk`) rather than
`tools.backtrack.Backtrack`. GNOME Shell then shows it as a separate
application, without Backtrack's icon or name, in the dash, the window
switcher and the overview, whenever Rewind opens on its own: from the file
manager, the launcher or the tray. Found by S19-T3's screenshot script,
which looks windows up by their application.

**Accept:** opened from GNOME Files and from the launcher on the GNOME VM,
Rewind's window is listed under Backtrack, with its icon, in the window
switcher and the overview; GNOME Shell reports `tools.backtrack.Backtrack`
as its application ID; a CHANGELOG entry under Unreleased.

## Definition of Done
All tasks done; `docs/README.md` lists every guide in the order a new user
meets them; the README's Documentation section and install table point at
the guides; `just check` passes, including the tests that hold the headings
Backtrack links to; the CHANGELOG's Unreleased section names the new guides
and says that the existing ones were rewritten as walk-throughs.
