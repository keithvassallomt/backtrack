# Stage 21 — Telling People About Backtrack

## Objective
The places where people meet Backtrack before they install it show it as it
is now, and Keith's blog has what it needs to announce it.

Added at Keith's request, 2026-11-07.

## Prerequisites
S19-T3: the screenshots of the release they illustrate, and the script that
takes them.

## Order of work
T1 once S19-T3 is done. T2 at any time. T3 once Keith has agreed its scope.

## Tasks

### S21-T1 — New screenshots for the FriendlyHub listing and the README
The FriendlyHub listing shows the screenshots in the AppStream file
(`packaging/tools.backtrack.Backtrack.metainfo.xml`): three from Stages 6 and
12, taken before Stages 15 to 18 changed the window. The README has none.

New ones, chosen from S19-T3's set. For the listing, the few a software
centre shows first, each with its caption: among them the window at an
earlier backup, Rewind, and search finding a deleted file. For the README,
one picture near the top that shows what Backtrack is. The listing's come
from new paths, so released listings keep the files they point at, and they
reach FriendlyHub with the next release. Where they are served from in the
end is S20-T10's question; this task does not wait for it.

**Accept:** Keith approves the choice; `appstreamcli validate` passes with
the network, which fetches every screenshot; the README shows its picture on
GitHub; the next release's FriendlyHub listing shows the new set.

### S21-T2 — A brief for a blog post announcing Backtrack
Keith's own agent writes his blog posts. This task gives it what an
announcement needs: what Backtrack does, as backtrack.tools describes it; why
Keith built it, in his words; the screenshots and the Rewind video to include,
with their captions; where to link; and what not to claim. The post is for
people who use Linux, not for developers. Changed at Keith's request,
2026-11-09: it replaces a list of post ideas for him to choose from.

**Accept:** the brief is a comment on this task's issue, and Keith has given
it to his agent.

### S21-T3 — An install script for Backtrack
Added at Keith's request, 2026-11-08; scope agreed 2026-11-09. One command,
`curl -fsSL https://backtrack.tools/install.sh | bash`, installs Backtrack
the way the install page would for the computer it runs on. It says what it
found and what it will do, and changes nothing until the user agrees. Its
output is a pleasure to watch: colour, spinners and a little animation,
falling back to plain text outside a terminal or with `NO_COLOR`.

- Fedora 44 or later, and Fedora-based systems on that base, such as Nobara:
  COPR. Fedora Atomic desktops: the Flatpak.
- Ubuntu 26.04 or later, and Ubuntu-based systems on that base, such as
  Linux Mint, Pop!_OS and Zorin OS: the PPA.
- Arch Linux and Arch-based systems such as EndeavourOS and CachyOS: the
  AUR, through yay or paru when one is installed, else with `makepkg` as the
  install page does. Manjaro and SteamOS: the Flatpak.
- Anything else, or an older Fedora or Ubuntu: the Flatpak, installing
  Flatpak first where the distribution's package manager is known.
- On Fedora, Ubuntu and Arch, the user can choose the Flatpak instead.
- It always offers the file-manager items for each file manager it finds
  (GNOME Files, Dolphin), switched on by default. For the Flatpak, that
  means nautilus-python on the host, and the last step in Preferences.
- If Backtrack is already installed, however it got there, it says so and
  stops. It refuses to run as root, and asks for a password once.

The script lives in backtrack.tools' repository and is served by the site.
The home page's Get Backtrack section has two tabs: **The quick way**,
shown first, with the command and a link to **What this script does**; and
**Choose your own way**, with the section as it was. What this script does
is a page made from the script and the comments in it, so the two cannot
drift apart: every step that changes the computer, explained. The install
guide (`docs/installing.md`) starts with the command too.

**Accept:** run from a terminal, the command installs Backtrack and the
chosen file-manager items on Fedora 45 (COPR), Ubuntu 26.04 (PPA), Arch
Linux with yay and with no helper (AUR) and openSUSE Tumbleweed (Flatpak),
and the Flatpak instead on one of the first three; on each it reports what
it found and installs nothing until told to; shellcheck passes; both tabs
work from the keyboard and Orca reads them as tabs; What this script does
shows every line of the script; `docs/installing.md` gives the command,
and /install/ shows it from the next release; Keith has watched it run and
likes how it looks.

## Definition of Done
All tasks done.
