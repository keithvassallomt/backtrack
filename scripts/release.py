#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>
"""Take a release from tag to every channel (S13-T9).

    release.py [--dry-run] [--friendlyhub-only] [DIR]

Run by the human only, through `just release` (or `just release-dry`), once
`just bump-version` and the CHANGELOG's move are merged. It never changes a
version: it reads the one `just bump-version` set, and refuses if anything
disagrees with it.

1. Preflight, before anything is published: the branch, a clean tree level
   with origin, `just verify-version`, `just check`, a dated CHANGELOG
   section, and every credential.
2. Tag and wait: the annotated tag, pushed; the release workflow watched; the
   GitHub release checked for every artifact.
3. COPR: the release's source RPM, built for Fedora 44 and later.
4. PPA: a source package per Ubuntu series, signed with the release key
   through the gpg agent (the key never leaves this machine), uploaded.
5. AUR: the PKGBUILD with the tag's checksum, built in a clean container,
   pushed.
6. FriendlyHub: the manifest pinned to the tag, its generated sources, and the
   metainfo with the release history, written to DIR (~/Downloads by default).
7. A summary of every channel.

Each publishing step skips a version already there, so a run that failed can
be run again once the cause is fixed. A pre-release (X.Y.Z-rc1) goes to the
testing COPR and PPA, and stops short of the AUR. `--dry-run` runs the
preflight (a missing credential is reported, not fatal) and every build and
generation step locally, and publishes nothing. `--friendlyhub-only` writes
the FriendlyHub files for a version already tagged.
"""

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time
import tomllib
import urllib.request
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import release_notes  # noqa: E402

APP_ID = "tools.backtrack.Backtrack"
GITHUB = "keithvassallomt/backtrack"
MANIFEST = ROOT / "packaging/flatpak/tools.backtrack.Backtrack.yml"
METAINFO = ROOT / "packaging/tools.backtrack.Backtrack.metainfo.xml"
CHANGELOG = ROOT / "CHANGELOG.md"


# ─── Rewrites, pure so they can be tested ──────────────────────────────────

# The manifest's source for a build of the checkout it sits in.
LOCAL_SOURCE = """      - type: dir
        path: ../..
        skip:
          - .git
          - target
"""


def friendlyhub_manifest(manifest, tag, commit):
    """The manifest FriendlyHub builds: Backtrack fetched by the tag's commit,
    with the metainfo beside it laid over the repository's copy, so the
    installed listing carries the release history too."""
    if manifest.count(LOCAL_SOURCE) != 1:
        raise ValueError("the manifest's source for this checkout has changed shape")
    pinned = (
        "      - type: git\n"
        f"        url: https://github.com/{GITHUB}.git\n"
        f"        tag: {tag}\n"
        f"        commit: {commit}\n"
        "      # The metainfo with the release history, over the repository's copy.\n"
        "      - type: file\n"
        f"        path: {APP_ID}.metainfo.xml\n"
        "        dest: packaging\n"
    )
    return manifest.replace(LOCAL_SOURCE, pinned)


def metainfo_with_releases(metainfo, releases):
    """The metainfo with a <releases> holding each (version, date, body)."""
    elements = re.sub(r"<!--.*?-->", "", metainfo, flags=re.S)
    if "<releases" in elements or elements.count("</component>") != 1:
        raise ValueError("the metainfo already has releases, or is not one component")
    blocks = [
        "\n".join("    " + line for line in release_notes.appstream(*release).splitlines())
        for release in releases
    ]
    element = "  <releases>\n" + "\n".join(blocks) + "\n  </releases>\n"
    return metainfo.replace("</component>", element + "</component>")


def pkgbuild_with_checksum(pkgbuild, sha256):
    """The PKGBUILD for the AUR: the tag's tarball checksum in place of SKIP."""
    if pkgbuild.count("sha256sums=('SKIP')") != 1:
        raise ValueError("the PKGBUILD's checksum line has changed shape")
    return pkgbuild.replace("sha256sums=('SKIP')", f"sha256sums=('{sha256}')")


def changelog_for_series(changelog, name, version):
    """The deb's changelog for one Ubuntu series: the top entry's version
    gains the PPA's suffix (0.1.0-1 becomes 0.1.0-1~ubuntu26.04.1, which sorts
    below the archive's own 0.1.0-1) and names the series."""
    top = re.compile(r"\Abacktrack \(([^)]+)\) [a-z-]+;")
    if not top.match(changelog):
        raise ValueError("the changelog's top entry has changed shape")
    return top.sub(lambda m: f"backtrack ({m.group(1)}~ubuntu{version}.1) {name};", changelog, 1)


def ppa_parts(ppa):
    """("owner", "name") from "ppa:owner/name"."""
    owner, _, name = ppa.removeprefix("ppa:").partition("/")
    return owner, name


# ─── Running things ────────────────────────────────────────────────────────


@dataclass
class Run:
    version: str
    dry: bool
    friendlyhub: Path
    config: dict
    log: Path
    outcomes: list = field(default_factory=list)

    @property
    def tag(self):
        return f"v{self.version}"

    @property
    def prerelease(self):
        return "-" in self.version

    def say(self, message):
        print(message, flush=True)
        with self.log.open("a") as log:
            log.write(message + "\n")

    def run(self, *command, cwd=ROOT, capture=False, check=True, env=None):
        """Run `command`, logged; its output is returned when `capture`."""
        self.say("$ " + " ".join(str(part) for part in command))
        result = subprocess.run(
            [str(part) for part in command],
            cwd=cwd,
            text=True,
            capture_output=capture,
            env={**os.environ, **(env or {})},
        )
        if capture:
            with self.log.open("a") as log:
                log.write(result.stdout + result.stderr)
        if check and result.returncode != 0:
            raise Failed(f"{command[0]} failed (exit {result.returncode})")
        return result.stdout if capture else result.returncode

    def ok(self, *command):
        """Whether `command` succeeds; its output goes to the log only."""
        result = subprocess.run(
            [str(part) for part in command], cwd=ROOT, text=True, capture_output=True
        )
        with self.log.open("a") as log:
            log.write(
                f"$ {' '.join(str(part) for part in command)}\n{result.stdout}{result.stderr}"
            )
        return result.returncode == 0

    def publish(self, *command, **kwargs):
        """Run `command` unless this is a dry run, which only says it would."""
        if self.dry:
            self.say("would run: " + " ".join(str(part) for part in command))
            return ""
        return self.run(*command, **kwargs)

    def outcome(self, channel, state, where=""):
        self.outcomes.append((channel, state, where))
        self.say(f"{channel}: {state} {where}".rstrip())


class Failed(Exception):
    pass


def workspace_version():
    with (ROOT / "Cargo.toml").open("rb") as manifest:
        return tomllib.load(manifest)["workspace"]["package"]["version"]


def fetch(url):
    with urllib.request.urlopen(url, timeout=60) as response:
        return response.read()


# ─── 1. Preflight ──────────────────────────────────────────────────────────


def preflight(r):
    r.say(f"== Preflight for {r.version}")
    branch = r.run("git", "rev-parse", "--abbrev-ref", "HEAD", capture=True).strip()
    if branch != "main" and not r.prerelease:
        raise Failed(f"a release is made from main, and this is {branch}")
    if r.run("git", "status", "--porcelain", capture=True).strip():
        raise Failed("the working tree has changes; commit or stash them first")
    r.run("git", "fetch", "--quiet", "origin")
    upstream = r.run("git", "rev-parse", f"origin/{branch}", capture=True, check=False).strip()
    if upstream != r.run("git", "rev-parse", "HEAD", capture=True).strip():
        raise Failed(f"this branch is not level with origin/{branch}; push or pull first")
    r.run("just", "verify-version")
    try:
        release_notes.notes(CHANGELOG.read_text(), r.version)
    except release_notes.NotReleased as error:
        raise Failed(str(error)) from None
    r.run("just", "check")
    missing = [problem for problem in credentials(r) if problem]
    for problem in missing:
        r.say(f"credential: {problem}")
    if missing and not r.dry:
        raise Failed("a credential is missing or does not work; nothing has been published")


def credentials(r):
    """Each credential the run needs, checked; yields what is wrong."""
    if not r.ok("gh", "auth", "status"):
        yield "gh is not signed in (gh auth login)"
    if not r.ok("uvx", "--quiet", "copr-cli", "whoami"):
        yield "copr-cli has no working token (~/.config/copr, from copr.fedorainfracloud.org/api)"
    else:
        expiry = re.search(r"expiration date: (\d{4}-\d{2}-\d{2})", copr_config())
        if expiry:
            left = (datetime.fromisoformat(expiry.group(1)) - datetime.now()).days
            r.say(f"credential: the COPR token has {left} days left")
            if left < 0:
                yield "the COPR token has expired"
    key = r.config["ppa"]["key"]
    if not key:
        yield "no PPA signing key in packaging/release.toml"
    elif not r.ok(
        "gpg",
        "--batch",
        "--yes",
        "--local-user",
        key,
        "--clearsign",
        "--output",
        os.devnull,
        "/dev/null",
    ):
        yield f"gpg cannot sign with {key}"
    greeting = subprocess.run(
        ["ssh", "-o", "BatchMode=yes", "aur@aur.archlinux.org", "help"],
        capture_output=True,
        text=True,
    )
    if "Commands" not in greeting.stdout + greeting.stderr:
        yield "ssh is not let in to aur@aur.archlinux.org (an AUR account with this key)"


def copr_config():
    path = Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")) / "copr"
    return path.read_text() if path.exists() else ""


# ─── 2. Tag, and the release workflow ──────────────────────────────────────

EXPECTED = ("{n}.tar.gz", "{n}-vendor.tar.xz", "{n}.flatpak", ".src.rpm", ".x86_64.rpm", ".deb")


def tag_and_wait(r):
    r.say(f"== Tag {r.tag} and the release workflow")
    head = r.run("git", "rev-parse", "HEAD", capture=True).strip()
    remote = r.run("git", "ls-remote", "origin", f"refs/tags/{r.tag}^{{}}", capture=True).split()
    if remote and remote[0] != head:
        raise Failed(f"{r.tag} is already on origin, at another commit")
    if not remote:
        r.publish("git", "tag", "--annotate", r.tag, "--message", f"Backtrack {r.version}")
        r.publish("git", "push", "origin", r.tag)
    if r.dry:
        r.say("dry run: building every artifact here instead")
        for recipe in ("dist", "rpm", "deb", "flatpak", "flatpak-bundle"):
            r.run("just", recipe)
        r.outcome("GitHub release", "built locally", "target/")
        return
    run_id = None
    for _ in range(60):
        runs = json.loads(
            r.run(
                "gh",
                "run",
                "list",
                "--workflow",
                "release.yml",
                "--json",
                "databaseId,headBranch",
                "--limit",
                "20",
                capture=True,
            )
        )
        run_id = next((x["databaseId"] for x in runs if x["headBranch"] == r.tag), None)
        if run_id:
            break
        time.sleep(10)
    if not run_id:
        raise Failed(f"the release workflow did not start for {r.tag}")
    r.run("gh", "run", "watch", str(run_id), "--exit-status")
    assets = r.run(
        "gh",
        "release",
        "view",
        r.tag,
        "--json",
        "assets",
        "--jq",
        ".assets[].name",
        capture=True,
    ).split()
    name = f"backtrack-{r.version}"
    absent = [e for e in EXPECTED if not any(a.endswith(e.format(n=name)) for a in assets)]
    if absent or "SHA256SUMS" not in assets:
        raise Failed(f"the release lacks {absent or ['SHA256SUMS']}")
    r.outcome("GitHub release", "published", f"https://github.com/{GITHUB}/releases/tag/{r.tag}")


# ─── 3. COPR ───────────────────────────────────────────────────────────────


def copr(r):
    project = r.config["copr"]["testing" if r.prerelease else "project"]
    r.say(f"== COPR: {project}")
    rpm_version = r.version.replace("-", "~")
    if not r.dry:
        builds = json.loads(
            r.run(
                "uvx",
                "--quiet",
                "copr-cli",
                "list-builds",
                project,
                "--output-format",
                "json",
                capture=True,
            )
            or "[]"
        )
        if any(
            b.get("state") == "succeeded"
            and (b.get("source_package") or {}).get("version", "").startswith(rpm_version + "-")
            for b in builds
        ):
            r.outcome("COPR", "already built", project)
            return
    work = ROOT / "target/release/copr"
    shutil.rmtree(work, ignore_errors=True)
    work.mkdir(parents=True)
    if r.dry:
        srpms = sorted((ROOT / "target/rpm/SRPMS").glob("*.src.rpm"))
    else:
        r.run("gh", "release", "download", r.tag, "--pattern", "*.src.rpm", "--dir", work)
        srpms = sorted(work.glob("*.src.rpm"))
    if not srpms:
        raise Failed("no source RPM to submit")
    r.publish("uvx", "--quiet", "copr-cli", "build", project, srpms[-1])
    r.outcome(
        "COPR",
        "would build" if r.dry else "built",
        f"https://copr.fedorainfracloud.org/coprs/{project}",
    )


# ─── 4. PPA ────────────────────────────────────────────────────────────────


def ppa(r):
    target = r.config["ppa"]["testing" if r.prerelease else "release"]
    owner, name = ppa_parts(target)
    r.say(f"== PPA: {target}")
    upstream = r.version.replace("-", "~")
    published = [] if r.dry else ppa_published(owner, name)
    dist = ROOT / "target/dist"
    if not r.dry:
        shutil.rmtree(dist, ignore_errors=True)
        dist.mkdir(parents=True)
        r.run("gh", "release", "download", r.tag, "--pattern", "*.tar.*", "--dir", dist)
    key = r.config["ppa"]["key"]
    for series in r.config["ppa"]["series"]:
        work = ROOT / "target/release/ppa" / series["name"]
        shutil.rmtree(work, ignore_errors=True)
        (work / "debian").mkdir(parents=True)
        shutil.copytree(ROOT / "packaging/deb", work / "debian", dirs_exist_ok=True)
        changelog = work / "debian/changelog"
        changelog.write_text(
            changelog_for_series(changelog.read_text(), series["name"], series["version"])
        )
        debian_version = re.match(r"backtrack \(([^)]+)\)", changelog.read_text()).group(1)
        if debian_version in published:
            r.outcome(f"PPA {series['name']}", "already uploaded", debian_version)
            continue
        shutil.copy(
            dist / f"backtrack-{r.version}.tar.gz", work / f"backtrack_{upstream}.orig.tar.gz"
        )
        shutil.copy(
            dist / f"backtrack-{r.version}-vendor.tar.xz",
            work / f"backtrack_{upstream}.orig-vendor.tar.xz",
        )
        mounts = [
            "-v",
            f"{work}:/build:Z",
            "-v",
            f"{ROOT}/packaging/deb/source-in-container.sh:/source.sh:ro,Z",
        ]
        env = ["-e", f"UPLOAD={'' if r.dry else target}", "-e", f"KEY={'' if r.dry else key}"]
        if not r.dry:
            # The key stays here: the container signs through the gpg agent's
            # restricted socket, with only the public half imported.
            agent = r.run("gpgconf", "--list-dirs", "agent-extra-socket", capture=True).strip()
            (work / "key.asc").write_text(r.run("gpg", "--armor", "--export", key, capture=True))
            mounts += ["-v", f"{agent}:/root/.gnupg/S.gpg-agent:Z"]
        r.run(
            "podman", "run", "--rm", *mounts, *env, "docker.io/library/ubuntu:26.04", "/source.sh"
        )
        r.outcome(
            f"PPA {series['name']}",
            "source package built" if r.dry else "uploaded",
            f"https://launchpad.net/~{owner}/+archive/ubuntu/{name}",
        )


def ppa_published(owner, name):
    """The Debian versions of backtrack the PPA already has, any state."""
    url = (
        f"https://api.launchpad.net/1.0/~{owner}/+archive/ubuntu/{name}"
        "?ws.op=getPublishedSources&source_name=backtrack&exact_match=true"
    )
    try:
        entries = json.loads(fetch(url)).get("entries", [])
    except Exception:
        return []
    return [entry["source_package_version"] for entry in entries]


# ─── 5. AUR ────────────────────────────────────────────────────────────────


def aur(r):
    r.say("== AUR")
    if r.prerelease:
        r.say("a pre-release stops short of the AUR")
    if r.dry or r.prerelease:
        r.run("just", "aur-build")
        r.outcome("AUR", "built from this checkout; nothing pushed")
        return
    work = ROOT / "target/release/aur"
    if (work / ".git").exists():
        r.run("git", "-C", work, "pull", "--quiet", "--rebase")
    else:
        shutil.rmtree(work, ignore_errors=True)
        r.run("git", "clone", "--quiet", r.config["aur"]["repository"], work)
    srcinfo = work / ".SRCINFO"
    if srcinfo.exists() and f"\tpkgver = {r.version.replace('-', '')}\n" in srcinfo.read_text():
        r.outcome("AUR", "already at this version")
        return
    tarball = fetch(f"https://github.com/{GITHUB}/archive/refs/tags/{r.tag}.tar.gz")
    pkgbuild = pkgbuild_with_checksum(
        (ROOT / "packaging/aur/PKGBUILD").read_text(), hashlib.sha256(tarball).hexdigest()
    )
    (work / "PKGBUILD").write_text(pkgbuild)
    srcinfo.write_text(r.run("makepkg", "--printsrcinfo", cwd=work, capture=True))
    check = ROOT / "target/release/aur-check"
    shutil.rmtree(check, ignore_errors=True)
    check.mkdir(parents=True)
    shutil.copy(work / "PKGBUILD", check / "PKGBUILD")
    r.run(
        "podman",
        "run",
        "--rm",
        "-v",
        f"{check}:/build:Z",
        "-v",
        f"{ROOT}/packaging/aur/build-in-container.sh:/build.sh:ro,Z",
        "docker.io/library/archlinux:latest",
        "/build.sh",
    )
    r.run("git", "-C", work, "add", "PKGBUILD", ".SRCINFO")
    r.run("git", "-C", work, "commit", "--quiet", "--message", f"Backtrack {r.version}")
    r.run("git", "-C", work, "push", "--quiet")
    r.outcome("AUR", "pushed", "https://aur.archlinux.org/pkgbase/backtrack")


# ─── 6. FriendlyHub ────────────────────────────────────────────────────────


def friendlyhub(r):
    out = r.friendlyhub.expanduser() / f"backtrack-{r.version}-friendlyhub"
    r.say(f"== FriendlyHub: {out}")
    commit = r.run("git", "rev-list", "-n", "1", r.tag, capture=True, check=False).strip()
    if not commit:
        if not r.dry:
            raise Failed(f"{r.tag} does not exist yet")
        commit = r.run("git", "rev-parse", "HEAD", capture=True).strip()
        r.say(f"dry run: {r.tag} is not tagged yet, so the manifest names HEAD ({commit[:12]})")
    shutil.rmtree(out, ignore_errors=True)
    out.mkdir(parents=True)
    (out / MANIFEST.name).write_text(friendlyhub_manifest(MANIFEST.read_text(), r.tag, commit))
    text = CHANGELOG.read_text()
    try:
        if r.prerelease:
            # Its own notes, then the releases before it.
            history = [(r.version, *release_notes.notes(text, r.version))]
            history += [h for h in released_before(text, r.version)]
        else:
            history = release_notes.released(text, r.version)
    except release_notes.NotReleased as error:
        if not r.dry:
            raise Failed(str(error)) from None
        r.say(f"dry run: {error}; the metainfo carries an undated stand-in")
        history = [(r.version, datetime.now().date().isoformat(), "### Added\n\n- (dry run)")]
    metainfo = out / METAINFO.name
    metainfo.write_text(metainfo_with_releases(METAINFO.read_text(), history))
    for generated in ("cargo-sources.json", "python3-requirements.json"):
        shutil.copy(MANIFEST.parent / generated, out / generated)
    r.run("appstreamcli", "validate", "--no-net", metainfo)
    r.outcome("FriendlyHub", "files written; Keith submits them", str(out))


def released_before(text, version):
    """The dated releases older than a pre-release of `version`'s base."""
    base = version.split("-", 1)[0]
    dated = [
        m.group("version")
        for m in map(release_notes.HEADING.match, text.splitlines())
        if m and release_notes.DATE.match(m.group("date") or "") and m.group("version") != base
    ]
    return release_notes.released(text, dated[0]) if dated else []


# ─── The run ───────────────────────────────────────────────────────────────


def main(argv):
    dry = "--dry-run" in argv
    only_friendlyhub = "--friendlyhub-only" in argv
    rest = [arg for arg in argv if not arg.startswith("--")]
    with (ROOT / "packaging/release.toml").open("rb") as file:
        config = tomllib.load(file)
    version = workspace_version()
    log = ROOT / "target/release" / f"{version}.log"
    log.parent.mkdir(parents=True, exist_ok=True)
    directory = Path(rest[0]) if rest else Path(config["friendlyhub"]["directory"])
    r = Run(version=version, dry=dry, friendlyhub=directory, config=config, log=log)
    r.say(f"\n# {datetime.now():%Y-%m-%d %H:%M} release of {version}{' (dry run)' if dry else ''}")
    try:
        if only_friendlyhub:
            friendlyhub(r)
        else:
            preflight(r)
            tag_and_wait(r)
            copr(r)
            ppa(r)
            aur(r)
            friendlyhub(r)
    except Failed as error:
        r.say(f"\nStopped: {error}")
        r.say(f"Fix the cause and run it again; finished steps are skipped. Log: {log}")
        return 1
    r.say("\n== Summary")
    for channel, state, where in r.outcomes:
        r.say(f"  {channel:<20} {state:<28} {where}")
    r.say(f"Log: {log}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
