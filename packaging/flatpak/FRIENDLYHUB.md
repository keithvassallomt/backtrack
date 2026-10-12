# Backtrack on FriendlyHub

Backtrack's Flatpak is published on [FriendlyHub](https://friendlyhub.org),
not Flathub. It is the main download: the one package that reaches every
distribution, including those whose own libraries are older than Backtrack's
GNOME 50 floor, such as Debian 13 and RHEL 10, since it brings its own runtime.

## What a release gives FriendlyHub

`just release` writes three things to
`~/Downloads/backtrack-X.Y.Z-friendlyhub/`, and `just friendlyhub-files [DIR]`
writes them again for a version already tagged:

- **`tools.backtrack.Backtrack.yml`**, the manifest. It builds the tag's commit,
  fetched from GitHub, rather than a checkout, and lays the metainfo below over
  the repository's copy, so the installed listing carries the release history
  too.
- **`tools.backtrack.Backtrack.metainfo.xml`**, the AppStream listing, with every
  dated release in `CHANGELOG.md` up to this one, written from its entries.
- **`cargo-sources.json`** and **`python3-requirements.json`**, every crate and
  Borg's Python packages, which `just flatpak-sources` generates. There is no
  `node-sources.json`: Backtrack has no JavaScript.

Keith submits the folder's files to FriendlyHub.

## Checking a set before submitting it

```sh
flatpak-builder --user --install --force-clean /tmp/backtrack-build \
    ~/Downloads/backtrack-X.Y.Z-friendlyhub/tools.backtrack.Backtrack.yml
appstreamcli validate --no-net \
    ~/Downloads/backtrack-X.Y.Z-friendlyhub/tools.backtrack.Backtrack.metainfo.xml
```

flatpak-builder downloads every source first and then builds without network,
so a set that builds this way is complete.

## Installing from FriendlyHub

Backtrack runs on the GNOME 51 runtime, which comes from Flathub, so both
remotes are needed:

```sh
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak remote-add --user --if-not-exists friendlyhub https://dl.friendlyhub.org/repo/friendlyhub.flatpakrepo
flatpak install --user friendlyhub tools.backtrack.Backtrack
```

Flatpak itself is in every distribution's archive. On Debian 13 it is
`sudo apt install flatpak`; Fedora, RHEL 10 and its rebuilds (Alma, Rocky)
Workstation installs have it already.
