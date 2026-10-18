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
from datetime import UTC, datetime
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


def release_run(runs, tag, commit):
    """The id of the release workflow's run for `tag` at `commit` among
    `gh run list`'s runs, or None. Runs are matched by workflow name, not
    looked up by file: GitHub finds a workflow by file only once it is on the
    default branch, and a release candidate's tag is on its release branch.
    CI runs on the tag's push too, so the name tells the two apart; a tag
    deleted and pushed again leaves its earlier runs listed, so the commit
    tells those apart."""
    return next(
        (
            x["databaseId"]
            for x in runs
            if x["headBranch"] == tag and x["headSha"] == commit and x["workflowName"] == "Release"
        ),
        None,
    )


def job_news(jobs, said, tz=None):
    """What has happened to the release run's jobs since `said`, a job's
    name to the last state said of it, which this updates: (job name, line)
    for a job that has started, passed, been skipped or failed (with the
    step it failed at, and its link). Times are local, or in `tz`."""
    lines = []
    for job in jobs:
        name = job["name"]
        if job["status"] == "completed":
            state = job["conclusion"]
        elif job["status"] == "in_progress":
            state = "started"
        else:
            continue
        if said.get(name) == state:
            continue
        said[name] = state
        if state == "started":
            lines.append((name, f"{clock(job['startedAt'], tz)}  {name}: started"))
        elif state == "skipped":
            lines.append((name, f"{clock(job['completedAt'], tz)}  {name}: skipped"))
        elif state == "success":
            took = duration(job["startedAt"], job["completedAt"])
            lines.append((name, f"{clock(job['completedAt'], tz)}  {name}: passed in {took}"))
        else:
            took = duration(job["startedAt"], job["completedAt"])
            step = next((x["name"] for x in job["steps"] if x["conclusion"] == "failure"), "")
            at = f" at “{step}”" if step else ""
            lines.append(
                (name, f"{clock(job['completedAt'], tz)}  {name}: {state}{at} after {took}")
            )
            lines.append((name, f"    {job['url']}"))
    return lines


def still_running(jobs, now):
    """The line naming the jobs still running at `now`, and for how long, or
    None when none is."""
    running = [
        f"{job['name']} ({duration(job['startedAt'], now)})"
        for job in jobs
        if job["status"] == "in_progress"
    ]
    return f"still running: {', '.join(running)}" if running else None


def clock(stamp, tz=None):
    """HH:MM:SS of one of GitHub's UTC timestamps, in local time or `tz`."""
    return datetime.fromisoformat(stamp).astimezone(tz).strftime("%H:%M:%S")


def duration(start, end):
    """How long from `start` to `end` (GitHub timestamps or datetimes), as
    "26s" or "11m 29s"."""
    start, end = (datetime.fromisoformat(t) if isinstance(t, str) else t for t in (start, end))
    minutes, seconds = divmod(max(0, round((end - start).total_seconds())), 60)
    return f"{minutes}m {seconds:02d}s" if minutes else f"{seconds}s"


# ─── Running things ────────────────────────────────────────────────────────

# Every line the run prints carries the tag of the step it belongs to, in
# that step's colour on a terminal (ANSI SGR codes). The release workflow's
# jobs run side by side, so each has its own tag, in the colour of the
# channel it builds for: Ubuntu's orange, Fedora's cyan, Arch's blue, and
# magenta for the Flatpak.
COLOURS = {
    "Preflight": "90",
    "GitHub": "97",
    "COPR": "36",
    "PPA": "38;5;208",
    "AUR": "38;5;33",
    "FriendlyHub": "35",
    "CI rpm": "36",
    "CI deb": "38;5;208",
    "CI flatpak": "35",
    "CI tarballs": "97",
    "CI": "97",
}
TAG_WIDTH = max(len(tag) for tag in COLOURS) + 2
JOB_TAGS = (
    ("RPMs", "CI rpm"),
    ("Debs", "CI deb"),
    ("Flatpak", "CI flatpak"),
    ("Source tarballs", "CI tarballs"),
)


def job_tag(name):
    """The tag for a release workflow job's lines, by the start of its name."""
    return next((tag for start, tag in JOB_TAGS if name.startswith(start)), "CI")


def tagged(message, tag, colour):
    """`message` under `[tag]`, padded so that messages line up; the tag in
    its colour when `colour`."""
    if not tag:
        return message
    label = f"[{tag}]".ljust(TAG_WIDTH)
    if colour and tag in COLOURS:
        label = f"\033[1;{COLOURS[tag]}m{label}\033[0m"
    return f"{label} {message}"


def on_a_terminal():
    return sys.stdout.isatty() and "NO_COLOR" not in os.environ


@dataclass
class Run:
    version: str
    dry: bool
    friendlyhub: Path
    config: dict
    log: Path
    outcomes: list = field(default_factory=list)
    step: str = ""
    colour: bool = field(default_factory=on_a_terminal)

    @property
    def tag(self):
        return f"v{self.version}"

    @property
    def prerelease(self):
        return "-" in self.version

    def say(self, message, tag=None):
        """Print `message` and log it, under `tag`, or the current step's."""
        tag = self.step if tag is None else tag
        print(tagged(message, tag, self.colour), flush=True)
        with self.log.open("a") as log:
            log.write(tagged(message, tag, False) + "\n")

    def run(self, *command, cwd=ROOT, capture=False, check=True, env=None):
        """Run `command`, logged; its output is returned when `capture`, and
        otherwise said line by line, under the step's tag."""
        self.say("$ " + " ".join(str(part) for part in command))
        args = dict(cwd=cwd, text=True, errors="replace", env={**os.environ, **(env or {})})
        if capture:
            result = subprocess.run([str(part) for part in command], capture_output=True, **args)
            with self.log.open("a") as log:
                log.write(result.stdout + result.stderr)
            returncode = result.returncode
        else:
            with subprocess.Popen(
                [str(part) for part in command],
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                **args,
            ) as process:
                for line in process.stdout:
                    self.say(line.rstrip("\n"))
            returncode = process.returncode
        if check and returncode != 0:
            raise Failed(f"{command[0]} failed (exit {returncode})")
        return result.stdout if capture else returncode

    def read(self, *command):
        """`command`'s output, or None if it fails; neither goes to the log.
        For polling, which would otherwise fill it."""
        result = subprocess.run(
            [str(part) for part in command], cwd=ROOT, text=True, capture_output=True
        )
        return result.stdout if result.returncode == 0 else None

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


# Every request asks for the current answer. Launchpad's API answers
# anonymous requests through caches, one per front end, that can hold a
# listing from before an upload: the second run of the second candidate read
# such a listing and uploaded again.
FRESH = {"Cache-Control": "no-cache", "Pragma": "no-cache"}


def fetch(url):
    request = urllib.request.Request(url, headers=FRESH)
    with urllib.request.urlopen(request, timeout=60) as response:
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
    # The credentials before `just check`: the signing key's check may open a
    # passphrase dialog, which should come while whoever started the run is
    # still watching.
    missing = [problem for problem in credentials(r) if problem]
    for problem in missing:
        r.say(f"credential: {problem}")
    if missing and not r.dry:
        raise Failed("a credential is missing or does not work; nothing has been published")
    r.run("just", "check")


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
    if key:
        r.say(f"credential: signing with {key}; if gpg asks for its passphrase, answer it now")
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
    else:
        yield passphrase_held(r.read("gpgconf", "--list-options", "gpg-agent") or "")
    greeting = subprocess.run(
        ["ssh", "-o", "BatchMode=yes", "aur@aur.archlinux.org", "help"],
        capture_output=True,
        text=True,
    )
    if "Commands" not in greeting.stdout + greeting.stderr:
        yield "ssh is not let in to aur@aur.archlinux.org (an AUR account with this key)"


# The PPA's source packages are signed after the release workflow and COPR's
# builds, an hour or more after the preflight's signature asks for the key's
# passphrase, and gpg-agent must still hold it then. Four hours leaves room
# for a slow COPR queue.
PASSPHRASE_HELD_FOR = 4 * 3600


def passphrase_held(options):
    """What, in `gpgconf --list-options gpg-agent`, keeps the agent from
    holding the signing key's passphrase for PASSPHRASE_HELD_FOR; None if
    nothing does."""

    def value(name):
        """An option's value, or else its default."""
        # Each line: name:flags:level:description:type:alt-type:argname:default:argdef:value
        for line in options.splitlines():
            fields = line.split(":")
            if fields[0] == name and len(fields) > 9:
                return fields[9] or fields[7]
        return ""

    if value("ignore-cache-for-signing"):
        return "gpg-agent has ignore-cache-for-signing, so it asks for the passphrase every time"
    short = [
        name
        for name in ("default-cache-ttl", "max-cache-ttl")
        if int(value(name) or 0) < PASSPHRASE_HELD_FOR
    ]
    if short:
        settings = " and ".join(f"{name} {PASSPHRASE_HELD_FOR}" for name in short)
        return (
            "gpg-agent would forget the signing key's passphrase before the PPA step "
            f"signs; set {settings} in ~/.gnupg/gpg-agent.conf (RELEASING.md)"
        )
    return None


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
                "--event",
                "push",
                "--json",
                "databaseId,headBranch,headSha,workflowName",
                "--limit",
                "20",
                capture=True,
            )
        )
        run_id = release_run(runs, r.tag, head)
        if run_id:
            break
        time.sleep(10)
    if not run_id:
        raise Failed(f"the release workflow did not start for {r.tag}")
    follow(r, run_id)
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
    # A tag already on origin was pushed, and its release made, by an earlier run.
    state = "already published" if remote else "published"
    r.outcome("GitHub release", state, f"https://github.com/{GITHUB}/releases/tag/{r.tag}")


# How often the release run is looked at; how long a stretch with no news
# lasts before the jobs still running are named; and how many looks in a
# row may fail (GitHub unreachable) before the wait gives up.
LOOK_EVERY = 20
QUIET_FOR = 300
LOOKS_MAY_FAIL = 15


def follow(r, run_id):
    """Wait for the release run, saying each job's start and end as they
    come and, through a quiet stretch, which jobs are still running."""
    r.say(f"Following the release run: https://github.com/{GITHUB}/actions/runs/{run_id}")
    said, quiet, failed_looks = {}, 0, 0
    while True:
        out = r.read("gh", "run", "view", str(run_id), "--json", "status,conclusion,jobs")
        if out is None:
            failed_looks += 1
            if failed_looks == LOOKS_MAY_FAIL:
                raise Failed(f"GitHub did not answer for {LOOKS_MAY_FAIL * LOOK_EVERY}s")
            time.sleep(LOOK_EVERY)
            continue
        failed_looks = 0
        view = json.loads(out)
        news = job_news(view["jobs"], said)
        for name, line in news:
            r.say(line, tag=job_tag(name))
        if view["status"] == "completed":
            break
        quiet = 0 if news else quiet + LOOK_EVERY
        if quiet >= QUIET_FOR:
            line = still_running(view["jobs"], datetime.now(UTC))
            if line:
                r.say(line)
            quiet = 0
        time.sleep(LOOK_EVERY)
    if view["conclusion"] != "success":
        raise Failed(f"the release workflow ended in {view['conclusion']}")


# ─── 3. COPR ───────────────────────────────────────────────────────────────


def copr(r):
    project = r.config["copr"]["testing" if r.prerelease else "project"]
    r.say(f"== COPR: {project}")
    rpm_version = r.version.replace("-", "~")
    if not r.dry and copr_built(copr_builds(project), rpm_version):
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


def copr_builds(project):
    """The project's builds, from COPR's API: `copr-cli list-builds` leaves
    out the version each built."""
    owner, name = project.split("/")
    url = f"https://copr.fedorainfracloud.org/api_3/build/list?ownername={owner}&projectname={name}"
    return json.loads(fetch(url)).get("items", [])


def copr_built(builds, rpm_version):
    """Whether one of `builds` built `rpm_version` (any release of it)."""
    return any(
        b.get("state") == "succeeded"
        and (b.get("source_package") or {}).get("version", "").startswith(rpm_version + "-")
        for b in builds
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
        steps = [("FriendlyHub", friendlyhub)]
        if not only_friendlyhub:
            steps = [
                ("Preflight", preflight),
                ("GitHub", tag_and_wait),
                ("COPR", copr),
                ("PPA", ppa),
                ("AUR", aur),
                *steps,
            ]
        for tag, step in steps:
            r.step = tag
            step(r)
    except Failed as error:
        r.step = ""
        r.say(f"\nStopped: {error}")
        r.say(f"Fix the cause and run it again; finished steps are skipped. Log: {log}")
        return 1
    r.step = ""
    r.say("\n== Summary")
    for channel, state, where in r.outcomes:
        r.say(f"  {channel:<20} {state:<28} {where}")
    r.say(f"Log: {log}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
