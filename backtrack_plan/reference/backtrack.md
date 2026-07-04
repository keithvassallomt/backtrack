# Conversation Transcript: Borg Backup / Time Machine-like Linux UI

## User
Is the type of backup that Apple's Time Machine does and Borg Backup does comparable?

## Assistant
Yes — **broadly comparable in purpose**, but **not identical in how they work or what they are optimised for**.

Both **Apple Time Machine** and **Borg Backup** are designed to give you **versioned backups**, meaning you can restore not only “the latest copy” of a file, but also older versions from previous backup points.

The similarity is mainly this:

| Feature | Time Machine | Borg Backup |
|---|---:|---:|
| Keeps historical snapshots | Yes | Yes |
| Lets you restore old file versions | Yes | Yes |
| Avoids storing full duplicate copies every time | Yes | Yes |
| Suitable for regular automatic backups | Yes | Yes |
| Can back up to network storage | Yes | Yes |
| Deduplication | Somewhat, depending on implementation/storage | Strong content-defined deduplication |
| Encryption | Limited / storage-dependent | Built-in, strong encryption |
| Cross-platform | Mostly Apple ecosystem | Linux/macOS/BSD, etc. |
| CLI/scriptable | Not really its focus | Very much yes |

The big difference is that **Time Machine is a user-friendly snapshot backup system**, tightly integrated into macOS. Its strength is convenience: plug in a disk or point it at a compatible network share, and macOS handles the rest with a nice restore interface.

**Borg is more technical but more powerful**. It stores backups as compressed, deduplicated archives inside a Borg repository. It is excellent when you want efficient storage, encryption, scripting, remote backup, pruning rules, and reliable unattended backups.

So in plain terms:

**Time Machine = “I want my Mac to automatically keep recoverable history with minimal fuss.”**

**Borg = “I want a serious, efficient, encrypted, scriptable backup system that I control.”**

For your Pika Backup setup specifically: Pika uses **Borg under the hood**, so the backup model is closer to Borg than Time Machine. It gives you Time-Machine-like historical restore capability, but with Borg’s deduplicated archive approach rather than Apple’s native snapshot experience.

One important practical difference: **Time Machine feels like browsing your computer as it used to be**, while **Borg/Pika feels more like restoring from a set of backup archives**. Same general goal, different philosophy.

---

## User
Would it be feasible to build a borg ui for linux which replicates the easy of use and UI of time machine?

## Assistant
That’s a genuinely interesting idea, because Borg already has most of the “backup engine” bits — the hard part is making restore feel humane rather than sysadmin-y. I’ll sanity-check the current landscape and then give you a practical feasibility view.

Yes — **feasible**, and actually a very good idea. Borg already provides enough low-level functionality to build something very Time-Machine-like on Linux. The gap is mostly **UX**, not backup capability.

Borg already supports the key pieces: encrypted, compressed, deduplicated backups; point-in-time archives; and mounting archives via FUSE so they can be browsed like a normal filesystem. Existing GUIs like **Pika Backup** and **Vorta** prove that Borg can be wrapped nicely, but they still feel more like “backup managers” than Apple’s “step back through time” restore experience. Pika is a GTK Borg frontend for simple personal backups, while Vorta presents Borg archives and supports restore by extracting or mounting.

The most Time-Machine-like version would probably work like this:

1. User opens the app.
2. It shows their current home folder.
3. A timeline/sidebar shows available Borg snapshots.
4. Clicking a date virtually swaps the file browser to that snapshot.
5. Deleted files appear again when viewing older snapshots.
6. User selects a file/folder and clicks **Restore**.
7. The app offers: restore to original location, restore elsewhere, or compare with current version.

That is absolutely buildable.

The hard parts are not impossible, but they are important:

| Challenge | Why it matters |
|---|---|
| **Fast browsing** | Mounting/listing large Borg archives can be slower than a native filesystem, especially over SMB/SSH/FUSE. |
| **Searching across snapshots** | “Where did this file go?” requires indexing archive metadata, otherwise it may be painfully slow. |
| **Restore safety** | You need careful handling of overwrites, permissions, symlinks, deleted folders, conflicts, and partial restores. |
| **Mount reliability** | Borg mount depends on FUSE; when that misbehaves, the whole “browse like Time Machine” experience suffers. |
| **Permissions/admin areas** | Restoring home-folder files is easy; system-level restore is much harder and potentially dangerous. |
| **Network availability** | In your case, `/mnt/backup` may not exist when away from home, so the UI must treat unavailable repositories gracefully. |

The sensible design would be **not** to rely entirely on live `borg mount` for everything. I’d do a hybrid:

**For browsing:** use cached metadata from `borg list --json` / archive indexes.

**For previewing/restoring:** mount or extract only when needed.

**For big restores:** use `borg extract`, because FUSE mounting can be slower for restoring lots of files. BorgBase’s docs make the same distinction: mount is good for browsing/finding individual files, while extract is better for larger restores.

So the app architecture could be:

```text
Borg repo
   ↓
Archive scanner/indexer
   ↓
Local metadata database
   ↓
Time-machine-style file browser
   ↓
Restore engine
   ├── borg extract for actual restore
   └── borg mount for preview / browse-on-demand
```

For Linux, a lovely target would be a **GNOME/GTK/libadwaita app**, especially because Pika already exists in that ecosystem. The differentiator would be: **not “configure backups” first**, but **“browse time and restore stuff” first**.

The MVP is very realistic:

- detect existing Borg repo
- list archives by date
- show file tree for selected archive
- search filenames
- restore selected file/folder to chosen location
- warn before overwriting
- handle repo unavailable without spamming the user

The “proper Time Machine clone” version is more ambitious:

- animated timeline
- compare current vs backed-up version
- deleted-file discovery
- preview text/images/PDFs
- Nautilus integration: right-click → “Restore previous version”
- automatic metadata indexing after each backup
- notifications only when action is actually needed
- restore audit log

The honest answer: **yes, technically very feasible**, but making it feel as effortless as Time Machine would take serious polish. Borg is a powerful engine; what Linux lacks is the opinionated, beautiful restore interface on top. Pika and Vorta are close in spirit, but there’s room for something that says:

> “Pick a folder. Pick a date. Bring it back.”

That would be genuinely useful.

---

## User
Would it be possible to have a Nautilus extension which adds a toolbar button to invoke a timeline slider? Then as the user slides back in time, we show the folder contents changing as they were back at that time? Similar to time machine

## Assistant
Yes, but there’s a subtle split here: the Time Machine *experience* is feasible, while embedding it directly into Nautilus exactly the way Apple does may be constrained by Nautilus’ extension APIs. I’ll separate “possible”, “awkward”, and “sensible architecture”.

Yes — **conceptually, absolutely**.

But I’d slightly adjust the implementation idea:

> **A Nautilus extension could invoke the timeline UI, but it probably should not try to mutate Nautilus itself live.**

The clean version is:

```text
Nautilus folder
   ↓
"Browse Backups" button / menu item
   ↓
Separate Borg Timeline window
   ↓
Slider changes snapshot
   ↓
File grid updates to show that folder at that point in time
   ↓
Restore selected files
```

That would be very feasible.

The more ambitious version — literally changing the current Nautilus window contents as the slider moves — is where things get ugly.

## The Nautilus limitation

Modern Nautilus extensions are mainly provider-based. The official `Nautilus.MenuProvider` interface lets extensions add menu items to Nautilus’ menus. The Python extension docs describe the same model: an extension becomes a provider that Nautilus asks for extra items or information.

That works nicely for things like:

```text
Right click folder → Browse previous versions
```

or perhaps:

```text
Current folder menu → Browse backup history
```

But adding a persistent custom toolbar button or injecting a whole live timeline widget into the Nautilus chrome is much less straightforward. The older `LocationWidgetProvider`, which would have been closer to “put a widget inside Nautilus”, was removed without replacement in the Nautilus 43 API migration. The nautilus-python news also notes that the Nautilus 43 port removed direct access to GTK widgets from the extension API.

So: **Nautilus can launch the experience. It probably should not host the whole experience.**

## Best UX design

I’d do this:

### In Nautilus

Add one or more entry points:

```text
Right-click background → Browse Backup History
Right-click file/folder → Restore Previous Version
```

Potentially also:

```text
Scripts / extension menu → Enter Timeline
```

Then open a dedicated GTK/libadwaita app that looks and behaves like a backup-aware file browser.

### In the Borg Timeline app

The window would receive the current Nautilus path, for example:

```text
/home/keith/Documents/Invoices
```

Then it would show:

```text
[ Today ]  ───────●────────  [ 14 Jun ] [ 13 Jun ] [ 12 Jun ]
```

As you drag the slider, the app updates the visible folder contents from the matching Borg archive.

Something like:

```text
Documents/Invoices as of 2026-06-12 09:00

invoice-may.pdf
invoice-june-draft.pdf
old-client-folder/
tax-notes.txt
```

Deleted files could appear with a subtle “deleted after this backup” badge. Modified files could show “changed since current version”.

## How it would work technically

You would not want to run `borg mount` repeatedly every time the slider moves. That would feel sluggish and brittle.

Better architecture:

```text
Borg repository
   ↓
Indexer service
   ↓
SQLite metadata cache
   ↓
Timeline UI
   ↓
On-demand preview / restore
```

The indexer would store metadata like:

```text
archive_id
archive_timestamp
path
file_name
size
mtime
mode
type
hash/chunk reference if available
```

Then the slider can update the visible file list instantly from SQLite.

Only when the user previews or restores a file would you call Borg properly:

```bash
borg extract /path/to/repo::archive-name home/keith/Documents/Invoices/file.pdf
```

Or, for browsing/previewing whole folders, temporarily mount an archive:

```bash
borg mount /path/to/repo::archive-name /tmp/borg-timeline-mount
```

## The really nice version

The “bloody hell, this feels native” version would include:

| Feature | Feasibility |
|---|---:|
| Right-click “Restore Previous Version” | Easy |
| Open timeline for current folder | Easy |
| Slider through Borg archives | Very feasible |
| Show folder contents at selected time | Very feasible with indexing |
| Restore to original location | Feasible, needs safety checks |
| Restore elsewhere | Easy |
| Preview text/images/PDFs | Feasible |
| Compare current vs backed-up file | Feasible |
| Show deleted files | Feasible |
| Live-changing actual Nautilus window | Technically awkward / probably not worth it |
| Custom Nautilus toolbar button | Depends on Nautilus internals; not ideal via supported extension APIs |

## My honest take

The best design is **not a Nautilus toolbar takeover**.

It is:

> **A Nautilus extension that opens a dedicated “Borg Time Machine” browser for the current folder.**

That keeps Nautilus integration clean and stable, while giving you full control over the Time Machine-style UI. You’d still get the magic feeling: “I’m in this folder, I click backup history, and I slide back through time.”

Trying to force Nautilus itself to become the Time Machine interface would likely turn into fragile GNOME-version-specific hackery. Launching a polished companion app from Nautilus is the sane route — and genuinely doable.

---

## User
give me a mockup of what browsing the timeline would look like

## Assistant
Generated a high-fidelity visual mockup of a GNOME/libadwaita-style “Backup Timeline” application.

The mockup showed:

- A desktop application titled **Backup Timeline**
- Folder path: `/home/keith/Documents/Projects`
- A sidebar with locations and snapshot repository status
- A central file list showing files/folders as they existed at a selected snapshot
- Status badges such as **changed**, **deleted later**, and **older version**
- A right-hand details panel for the selected file, `requirements.txt`
- A bottom timeline slider showing snapshots from **7 Jun** through **Today**
- Current view set to **12 Jun 2026, 09:00**
- Top actions: **Restore**, **Restore Elsewhere**, and **Compare**

The generated image was saved as:

```text
/mnt/data/backup_timeline_ui_design_mockup.png
```

---

## User
We can't call it time machine, but we CAN call it ____________

## Assistant
We can't call it Time Machine, but we **CAN** call it:

**Backtrack**

That’s my favourite. Simple, memorable, and it says exactly what it does: go back through your files.

Other strong options:

**Rewind**  
Very clear and friendly, but maybe a bit generic.

**Chronicle**  
More elegant, suggests history and records.

**Restore Point**  
Very understandable, though more Windows-y.

**Backstep**  
Nice and simple, but slightly less polished than Backtrack.

**Wayback**  
Fun, but the Wayback Machine association is strong.

**PastPort**  
Cute: a port into the past. Maybe too startup-name-ish.

**Rollback**  
Technically accurate, but sounds a bit sysadmin.

**Snapshot Browser**  
Clear, but boring.

**Borgtrack**  
Nerdy and direct, but maybe too tied to Borg.

My vote:

> **Backtrack**  
> *Browse your files as they were.*

That sounds like a Linux app name, not a cheap Time Machine knockoff.

---

## User
Give me a transcript of this conversation (the entire thing) as a downloadable markdown file.
