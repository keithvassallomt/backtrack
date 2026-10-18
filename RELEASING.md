# Releasing Backtrack

**The version-control contract: a version number is changed only by a human,
and only with `just bump-version`. No automated process and no AI contributor
changes one. In a release, an AI contributor does nothing, except fix red CI
when asked to.**

A release is published from one machine, the release machine, where every
credential lives. CI builds the artifacts and the GitHub release, and holds no
credential beyond its GitHub token; `just release` does the publishing.

| Channel | Who it is for | Published by |
|---|---|---|
| GitHub release | everyone: every artifact, `SHA256SUMS`, the notes | CI, on the tag |
| FriendlyHub (Flatpak) | every distribution, Debian and RHEL included | `just release` writes the files; Keith submits them |
| COPR | Fedora 44 and later | `just release` |
| PPA | Ubuntu 26.04 and later | `just release` |
| AUR | Arch | `just release` |

## Once, before the first release

- `gh auth login`, signed in as an account that can push tags to the repository.
- A Fedora account, and two COPR projects: the release one and a testing one
  for release candidates. An API token from
  <https://copr.fedorainfracloud.org/api/>, saved as `~/.config/copr`.
- A Launchpad account with the signing key registered, and two PPAs, release
  and testing. Put the key's fingerprint in `packaging/release.toml`.
- gpg-agent set to hold the signing key's passphrase for four hours. The PPA
  is signed an hour or more into the run, after the release workflow and
  COPR's builds, and the agent's defaults forget a passphrase after ten
  minutes unused. In `~/.gnupg/gpg-agent.conf`:

  ```
  default-cache-ttl 14400
  max-cache-ttl 14400
  ```

  then `gpgconf --reload gpg-agent`. The preflight checks it.
- An AUR account with this machine's SSH key.
- Check the account and project names in `packaging/release.toml`.

## Each release

1. Decide the version, by [VERSIONING.md](VERSIONING.md)'s rules.
2. Run `just bump-version X.Y.Z`. It sets every version location: the
   workspace, the AUR's PKGBUILD and `.SRCINFO`, the RPM spec and the deb's
   changelog.
3. In `CHANGELOG.md`, move the `[Unreleased]` entries under
   `## [X.Y.Z] - YYYY-MM-DD`, and leave `[Unreleased]` empty.
4. Commit, and merge to `main`.
5. Optionally, `just release-dry`: the preflight, every build and the
   FriendlyHub files, with nothing published.
6. Run `just release`. Its first checks sign a test message with the PPA
   key, so if the agent does not hold the key's passphrase, a dialog asks for
   it within seconds of starting: answer it, and the agent holds it for the
   rest of the run. It checks everything before publishing anything, tags
   `vX.Y.Z`, waits for the release workflow, then publishes to COPR, the PPA
   and the AUR, and writes the FriendlyHub files to
   `~/Downloads/backtrack-X.Y.Z-friendlyhub/` (`just release DIR` writes them
   elsewhere).
7. Read the summary it prints. If it stopped, fix what it names and run
   `just release` again: whatever already reached a channel is skipped.
8. Submit the FriendlyHub files, as
   [packaging/flatpak/FRIENDLYHUB.md](packaging/flatpak/FRIENDLYHUB.md) says.
9. After the release, bump nothing. The next bump comes when the next release
   is decided.

The log of every run is in `target/release/X.Y.Z.log`.

## Release candidates

Bump to a pre-release version (`just bump-version 0.2.0-rc1`) and run
`just release` from the release branch, which need not be `main`. CI makes a
GitHub pre-release; the testing COPR and the testing PPA get the build; the
AUR is left alone, and the run prints what it would have pushed there. The
FriendlyHub files are written as for a release, and whether a candidate goes
to FriendlyHub is a choice made when submitting them.

The packages spell a pre-release with a tilde (`0.2.0~rc1`), and the AUR
without the hyphen (`0.2.0rc1`), so that each sorts before the release.

## Credentials that expire

- **The COPR API token expires** (180 days, as COPR issues them). The
  preflight says how many days it has left; renew it at
  <https://copr.fedorainfracloud.org/api/> before it runs out.
- The signing key, if it was made with an expiry date: extend it, and send the
  new expiry to Launchpad's key server.
- The `gh` token, if it was made with one: `gh auth refresh`.

## When a channel refuses a release

- **The release workflow fails:** nothing has been published anywhere yet.
  Fix the cause on `main`, delete the tag (`git push --delete origin vX.Y.Z`,
  `git tag -d vX.Y.Z`), and run `just release` again.
- **A COPR build fails:** its log is linked from the project's page. A fix to
  the packaging alone goes out as the next patch release.
- **Launchpad rejects an upload:** its email says why. Launchpad never takes
  the same version twice, so a fixed upload is the next patch release.
- **The AUR refuses the push:** check that `ssh aur@aur.archlinux.org help`
  answers, which the preflight also does.
