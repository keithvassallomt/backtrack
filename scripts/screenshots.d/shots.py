#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>
"""The screenshot user's side of scripts/screenshots, run in the GNOME VM as
that user, over SSH. See docs/screenshots.md.

  shots.py home            the home folder and four weeks of backups behind
                           it, made from nothing; then Backtrack's daemon
                           opens them and backs up once more
  shots.py daemon          start the daemon again, on the same backups
  shots.py app [ARGS…]     open Backtrack's window (ARGS as backtrack-gtk's)
  shots.py close           close every Backtrack window
  shots.py action NAME [S] activate one of the window's actions (win.NAME)
  shots.py keys TEXT       type TEXT into whatever has the keyboard; <Enter>,
                           <Escape>, <Tab>, <Down>, <Up> are keys
  shots.py scheme light|dark
  shots.py window W H      size and centre Backtrack's window
  shots.py shot NAME       the focused window, with its shadow, to OUT/NAME.png
  shots.py screen NAME     the whole screen, to OUT/NAME.png
  shots.py trash NAME      write NAME in Documents and move it to the Trash
  shots.py press LABEL     press the button labelled LABEL, through the
                           accessibility tree (the newest, when several)
  shots.py fill LABEL TEXT type TEXT into the field labelled LABEL
  shots.py files FILE      GNOME Files on FILE's folder, FILE selected, with
                           its right-click menu open
  shots.py area NAME CLASS the window of that class, with whatever menu it has
                           open, to OUT/NAME.png
  shots.py forget          the passphrase forgotten, as when it is not kept
  shots.py unlock          the passphrase given back, and kept again

It needs GNOME Shell in unsafe mode (the extension in this folder): that is
what lets a script size a window, type, and take a screenshot.
"""

import json
import os
import shutil
import subprocess
import sys
import time
import urllib.parse
import urllib.request
from datetime import UTC, datetime, timedelta

HOME = os.path.expanduser("~")
HERE = os.path.dirname(os.path.abspath(__file__))
BIN = os.environ.get("SHOTS_BIN", "/opt/backtrack-screenshots/bin")
OUT = os.environ.get("SHOTS_OUT", "/var/tmp/backtrack-screenshots")
DRIVE = "/run/media/{user}/Backup Drive".format(user=os.environ.get("USER", "sam"))
REPO = f"{DRIVE}/Backtrack"
PASSPHRASE = "correct horse battery staple"
ENV = {"BACKTRACK_DEV": "1"}
if os.environ.get("SHOTS_FRESH"):
    # A Backtrack that has never been set up, for the welcome wizard.
    ENV["XDG_DATA_HOME"] = os.path.join(OUT, "fresh")
BUS = "tools.backtrack.Backtrack.Daemon1.Dev"
DPATH = "/tools/backtrack/Backtrack/Daemon1"
DIFACE = "tools.backtrack.Backtrack.Daemon1"
# What Backtrack leaves out by default (crates/backtrack-core/src/config.rs).
EXCLUDES = [
    "**/.cache",
    "**/Cache",
    "**/Caches",
    "**/.local/share/Trash",
    "**/node_modules",
    "**/target/debug",
    "**/*.iso",
    "**/*.qcow2",
    "**/*.vdi",
    "**/*.vmdk",
    "**/.local/share/containers",
    "**/.local/share/docker",
]


def say(*a):
    print(*a, flush=True)


def run(cmd, env=None, check=True, **kw):
    e = dict(os.environ)
    e.update(env or {})
    r = subprocess.run(cmd, env=e, text=True, capture_output=True, **kw)
    if check and r.returncode != 0:
        raise SystemExit(f"failed ({r.returncode}): {cmd}\n{r.stdout[-1500:]}{r.stderr[-1500:]}")
    return r


# ── GNOME Shell and D-Bus ───────────────────────────────────────────────


def bus():
    from gi.repository import Gio

    return Gio.bus_get_sync(Gio.BusType.SESSION)


def call(name, path, iface, method, sig=None, args=(), timeout=600):
    from gi.repository import Gio, GLib

    params = GLib.Variant(sig, tuple(args)) if sig else None
    return (
        bus()
        .call_sync(
            name, path, iface, method, params, None, Gio.DBusCallFlags.NONE, timeout * 1000, None
        )
        .unpack()
    )


def shell(js):
    ok, out = call(
        "org.gnome.Shell", "/org/gnome/Shell", "org.gnome.Shell", "Eval", "(s)", [js], timeout=30
    )
    if not ok:
        raise SystemExit(f"GNOME Shell refused: {out}\n(is unsafe mode on?)")
    return json.loads(out) if out else None


# Rewind's window is not the application's, so it has the program's name.
FIND = (
    "global.display.list_all_windows().filter(w => "
    "['tools.backtrack.Backtrack', 'backtrack-gtk'].includes(w.get_wm_class()))"
)


def daemon(method, sig=None, args=()):
    return call(BUS, DPATH, DIFACE, method, sig, args)


# ── the home folder ─────────────────────────────────────────────────────

# CC0 photographs from Wikimedia Commons, credited in docs/screenshots.md.
PHOTOS = {
    "Pictures/Bella.jpg": "Dog on dirt road.jpg",
    "Pictures/Garden/Flowers by the pond.jpg": (
        "Flowers - Lan Su Chinese Garden - Portland, Oregon - DSC01252.jpg"
    ),
    "Pictures/Spain 2026/Parasols at sunset.jpg": (
        "Parasols, Evening, Beach, Rincon de la Victoria, Andalusia, Spain.jpg"
    ),
    "Pictures/Spain 2026/Harbour.jpg": (
        "Valenica Mar Harbour Spain - Image with boats in line up.jpg"
    ),
    "Pictures/Spain 2026/Boats.jpg": "Some Boats in harbour.jpg",
    "Pictures/Hiking/The lake.jpg": "Lake Mountain Landscape.jpg",
    "Pictures/Hiking/From the lodge.jpg": "View of Blue Mountain Lake from Mt. Magazine Lodge.jpg",
    "Pictures/Apple cake.jpg": "Apple cake with vanilla ice cream 2.jpg",
}


def fetch_photos(cache):
    os.makedirs(cache, exist_ok=True)
    for title in PHOTOS.values():
        dest = os.path.join(cache, title)
        if os.path.exists(dest):
            continue
        q = urllib.parse.urlencode(
            {
                "action": "query",
                "titles": f"File:{title}",
                "prop": "imageinfo",
                "iiprop": "url",
                "iiurlwidth": "2000",
                "format": "json",
            }
        )
        req = urllib.request.Request(
            f"https://commons.wikimedia.org/w/api.php?{q}",
            headers={"User-Agent": "backtrack-screenshots/1.0 (https://backtrack.tools)"},
        )
        page = next(iter(json.load(urllib.request.urlopen(req))["query"]["pages"].values()))
        url = page["imageinfo"][0]["thumburl"]
        req = urllib.request.Request(url, headers=req.headers)
        with urllib.request.urlopen(req) as r, open(dest + ".part", "wb") as f:
            shutil.copyfileobj(r, f)
        os.rename(dest + ".part", dest)
        say(f"  fetched {title}")


def html(title, *paragraphs):
    body = "".join(f"<p>{p}</p>" for p in paragraphs)
    return (
        f"<html><head><meta charset='utf-8'><title>{title}</title></head>"
        f"<body><h1>{title}</h1>{body}</body></html>"
    )


NEWSLETTER = [
    html(
        "Garden club newsletter",
        (
            "Autumn is here, and the allotments have never looked better. Thank you to everyone "
            "who came to the open day: we raised enough for two new water butts."
        ),
        (
            "The seed swap is on the first Saturday of next month, in the church hall from ten. "
            "Bring whatever you saved this year, labelled if you can."
        ),
        "Our next talk is on growing garlic, which goes in the ground in November.",
    ),
    html(
        "Garden club newsletter",
        (
            "Autumn is here, and the allotments have never looked better. Thank you to everyone "
            "who came to the open day: we raised enough for two new water butts, and a "
            "wheelbarrow."
        ),
        (
            "The seed swap is on the first Saturday of next month, in the church hall from ten. "
            "Bring whatever you saved this year, labelled if you can, and a pen for the labels you "
            "forgot."
        ),
        (
            "Our next talk is on growing garlic, which goes in the ground in November. Margaret "
            "will bring her famous elephant garlic."
        ),
        "Subscriptions for next year are due by the end of the month: still five pounds.",
    ),
    html("Garden club newsletter", "TODO: write this month's"),
]

LETTER = [
    html(
        "To the council, about the hedge on Mill Lane",
        (
            "I am writing about the hedge along the footpath on Mill Lane, which has grown so far "
            "over the path that anyone with a pushchair has to walk in the road."
        ),
        "Could it be cut back before the clocks change, when the path is dark by five?",
    ),
    html(
        "To the council, about the hedge on Mill Lane",
        (
            "I am writing about the hedge along the footpath on Mill Lane, which has grown so far "
            "over the path that anyone with a pushchair or a wheelchair has to walk in the road."
        ),
        (
            "Could it be cut back before the clocks change, when the path is dark by five? I have "
            "attached two photographs taken this week."
        ),
    ),
]

PLAN = [
    html(
        "Kitchen renovation",
        "Units: keep the layout, replace the doors.",
        "Worktop: oak, oiled.",
        "Budget: about £6,000.",
    ),
    html(
        "Kitchen renovation",
        "Units: keep the layout, replace the doors.",
        "Worktop: quartz after all, oak marks too easily.",
        "Floor: the same tiles as the hall.",
        "Budget: about £7,500.",
    ),
]

BUDGET_ROWS = [
    "Month,Rent,Food,Energy,Transport,Savings",
    "January,950,310,140,85,200",
    "February,950,295,150,80,200",
    "March,950,320,130,90,200",
    "April,950,305,110,85,250",
    "May,950,330,95,110,250",
    "June,950,315,80,120,250",
    "July,950,340,75,140,150",
    "August,950,360,70,160,100",
    "September,950,300,85,90,250",
]
QUOTES = [
    [
        "Company,Units,Worktop,Fitting,Total",
        "Oak & Co,2400,1100,1800,5300",
        "Hartley Kitchens,2650,950,1600,5200",
    ],
    [
        "Company,Units,Worktop,Fitting,Total",
        "Oak & Co,2400,1900,1800,6100",
        "Hartley Kitchens,2650,1750,1600,6000",
        "Brook Joinery,2300,1800,1900,6000",
    ],
]

RECIPES = {
    "Lentil soup.txt": (
        "Lentil soup\n\n250 g red lentils\n1 onion, 2 carrots, 2 celery sticks\n1 l vegetable "
        "stock\nCumin, lemon\n\nSoften the vegetables, add the lentils and stock, simmer 25 "
        "minutes, blend half.\n"
    ),
    "Sourdough.txt": (
        "Sourdough\n\nFeed the starter the night before.\n500 g strong white flour, 350 g water, "
        "100 g starter, 10 g salt.\nFold every half hour for two hours, shape, prove overnight in "
        "the fridge.\nBake at 240 °C, 20 minutes lid on, 25 off.\n"
    ),
    "Weeknight curry.txt": (
        "Weeknight curry\n\nChickpeas, spinach, a tin of tomatoes, garam masala, a spoon of "
        "yoghurt.\n"
    ),
}

NOTES = [
    "Ring the garage about the MOT\nBook the dentist\n",
    "Ring the garage about the MOT\nBook the dentist\nSeed swap: bring the sweet peas\n",
    "Book the dentist\nSeed swap: bring the sweet peas\nAsk Hartley about a fitting date\n",
    (
        "Book the dentist\nSeed swap: bring the sweet peas\nAsk Hartley about a fitting "
        "date\nBella: worming tablet on Friday\n"
    ),
    (
        "Seed swap: bring the sweet peas\nAsk Hartley about a fitting date\nBella: worming tablet "
        "on Friday\nGarlic talk: 19th\n"
    ),
]


# What changed since the last backup, dated just before it as it would
# have been: the files are written now, and only the backups are dated back.
TOUCHED = set()


def write(path, text):
    full = os.path.join(HOME, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "w") as f:
        f.write(text)
    TOUCHED.add(full)


def place(name, source):
    full = os.path.join(HOME, name)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    shutil.copy(source, full)
    TOUCHED.add(full)


def office(work, items):
    """Convert (source text, source extension, destination, format) with one
    LibreOffice run per format, and move each result into place."""
    groups = {}
    for i, (text, ext, dest, fmt) in enumerate(items):
        src = os.path.join(work, f"{len(os.listdir(work))}-{i}.{ext}")
        with open(src, "w") as f:
            f.write(text)
        groups.setdefault((ext, fmt), []).append((src, dest))
    for (ext, fmt), pairs in groups.items():
        outdir = os.path.join(work, "out")
        os.makedirs(outdir, exist_ok=True)
        # HTML opens in Writer/Web, which cannot save a text document; as
        # Writer's own import it can.
        infilter = ["--infilter=HTML (StarWriter)"] if ext == "html" else []
        run(
            ["soffice", "--headless"]
            + infilter
            + ["--convert-to", fmt, "--outdir", outdir]
            + [s for s, _ in pairs],
            timeout=300,
        )
        ext = fmt.split(":")[0]
        for src, dest in pairs:
            made = os.path.join(outdir, os.path.splitext(os.path.basename(src))[0] + "." + ext)
            full = os.path.join(HOME, dest)
            os.makedirs(os.path.dirname(full), exist_ok=True)
            shutil.move(made, full)
            TOUCHED.add(full)


def edit_photo(src, dest):
    """The edit a person makes and saves over the original: cropped, warmer."""
    import gi

    gi.require_version("GdkPixbuf", "2.0")
    from gi.repository import GdkPixbuf

    pb = GdkPixbuf.Pixbuf.new_from_file(src)
    w, h = pb.get_width(), pb.get_height()
    crop = pb.new_subpixbuf(int(w * 0.18), int(h * 0.12), int(w * 0.64), int(h * 0.64))
    crop = crop.scale_simple(w, h, GdkPixbuf.InterpType.HYPER)
    crop.saturate_and_pixelate(crop, 1.6, False)
    crop.savev(dest, "jpeg", ["quality"], ["90"])
    TOUCHED.add(dest)


def backup(when):
    """A backup as Backtrack names and makes one, dated `when`."""
    for i, path in enumerate(sorted(TOUCHED)):
        t = (when - timedelta(minutes=40 - i % 30)).timestamp()
        os.utime(path, (t, t))
    TOUCHED.clear()
    stamp = when.astimezone(UTC).strftime("%Y%m%dT%H%M%SZ")
    host = os.uname().nodename.split(".")[0]
    args = [
        "borg",
        "create",
        "--one-file-system",
        "--compression",
        "zstd",
        "--timestamp",
        when.astimezone(UTC).strftime("%Y-%m-%dT%H:%M:%S"),
    ]
    for e in EXCLUDES:
        args += ["--exclude", e]
    # Exit 1 is Borg's warning (a file changed while it was read: Borg's own,
    # here), which Backtrack also counts as a backup made.
    r = run(
        args + [f"{REPO}::bt-{host}-{stamp}", HOME],
        env={"BORG_PASSPHRASE": PASSPHRASE},
        check=False,
    )
    if r.returncode not in (0, 1):
        raise SystemExit(f"borg create failed ({r.returncode}):\n{r.stderr[-1500:]}")
    say(f"  backup {when:%a %d %b %H:%M}")


def home():
    if not os.path.ismount(DRIVE):
        raise SystemExit(f"{DRIVE} is not mounted (scripts/screenshots home mounts it)")
    stop_daemon()
    for d in ["Documents", "Downloads", "Pictures", "Projects", "Music", "Videos"]:
        shutil.rmtree(os.path.join(HOME, d), ignore_errors=True)
        os.makedirs(os.path.join(HOME, d))
    for d in [".local/share/backtrack-dev", ".local/share/Trash", ".cache/borg", ".config/borg"]:
        shutil.rmtree(os.path.join(HOME, d), ignore_errors=True)
    shutil.rmtree(REPO, ignore_errors=True)
    run(
        ["borg", "init", "--encryption=repokey-blake2", REPO],
        env={"BORG_NEW_PASSPHRASE": PASSPHRASE},
    )

    cache = os.path.join(OUT, "photos")
    fetch_photos(cache)
    work = os.path.join(OUT, "work")
    shutil.rmtree(work, ignore_errors=True)
    os.makedirs(work)

    def photo(name):
        return os.path.join(cache, PHOTOS[name])

    now = datetime.now().astimezone().replace(microsecond=0)

    def day(d, h, m):
        return (now - timedelta(days=d)).replace(hour=h, minute=m, second=0)

    # Four weeks ago: the home folder as it was.
    office(
        work,
        [
            ("\n".join(BUDGET_ROWS), "csv", "Documents/Household budget 2026.ods", "ods"),
            (NEWSLETTER[0], "html", "Documents/Garden club newsletter.odt", "odt:writer8"),
            (LETTER[0], "html", "Documents/Letter to the council.odt", "odt:writer8"),
            (PLAN[0], "html", "Projects/Kitchen renovation/Plan.odt", "odt:writer8"),
            ("\n".join(QUOTES[0]), "csv", "Projects/Kitchen renovation/Quotes.ods", "ods"),
            (
                html("Washing machine manual", "Read all instructions before first use."),
                "html",
                "Downloads/Washing machine manual.pdf",
                "pdf",
            ),
            (
                html("Bank statement, September", "Opening balance £2,431.18"),
                "html",
                "Downloads/Bank statement September.pdf",
                "pdf",
            ),
        ],
    )
    for name, text in RECIPES.items():
        write(f"Documents/Recipes/{name}", text)
    write("Documents/Notes.txt", NOTES[0])
    for name in ["Pictures/Bella.jpg", "Pictures/Garden/Flowers by the pond.jpg"]:
        place(name, photo(name))
    backup(day(27, 9, 12))
    backup(day(24, 19, 40))

    # Three weeks ago: back from Spain, with the boarding pass still there.
    for name in [n for n in PHOTOS if "Spain" in n]:
        place(name, photo(name))
    office(
        work,
        [
            (
                html("Boarding pass", "Málaga (AGP) to Bristol (BRS)", "Seat 14C"),
                "html",
                "Downloads/Boarding pass AGP-BRS.pdf",
                "pdf",
            )
        ],
    )
    backup(day(21, 18, 5))

    # The newsletter's good second draft, and the quotes in.
    office(
        work,
        [
            (NEWSLETTER[1], "html", "Documents/Garden club newsletter.odt", "odt:writer8"),
            ("\n".join(QUOTES[1]), "csv", "Projects/Kitchen renovation/Quotes.ods", "ods"),
            (
                "\n".join(BUDGET_ROWS + ["October,950,315,120,95,200"]),
                "csv",
                "Documents/Household budget 2026.ods",
                "ods",
            ),
        ],
    )
    write("Documents/Notes.txt", NOTES[1])
    backup(day(16, 20, 30))

    # Hiking, and a cake.
    for name in [
        "Pictures/Hiking/The lake.jpg",
        "Pictures/Hiking/From the lodge.jpg",
        "Pictures/Apple cake.jpg",
    ]:
        place(name, photo(name))
    write(
        "Documents/Recipes/Apple cake.txt",
        (
            "Apple cake\n\n3 apples, 200 g flour, 150 g sugar, 3 eggs, 150 g butter, "
            "cinnamon.\n180 °C for 45 minutes.\n"
        ),
    )
    backup(day(12, 17, 45))

    # Downloads cleared out, the boarding pass with it.
    os.remove(os.path.join(HOME, "Downloads/Boarding pass AGP-BRS.pdf"))
    os.remove(os.path.join(HOME, "Downloads/Bank statement September.pdf"))
    write("Documents/Notes.txt", NOTES[2])
    backup(day(9, 21, 10))

    # The sunset photo edited, and saved over the original.
    parasols = os.path.join(HOME, "Pictures/Spain 2026/Parasols at sunset.jpg")
    edit_photo(photo("Pictures/Spain 2026/Parasols at sunset.jpg"), parasols)
    backup(day(6, 19, 0))

    office(
        work,
        [
            (LETTER[1], "html", "Documents/Letter to the council.odt", "odt:writer8"),
            (PLAN[1], "html", "Projects/Kitchen renovation/Plan.odt", "odt:writer8"),
        ],
    )
    backup(day(4, 10, 20))

    # The newsletter spoiled: a new draft started over the finished one.
    office(work, [(NEWSLETTER[2], "html", "Documents/Garden club newsletter.odt", "odt:writer8")])
    write("Documents/Notes.txt", NOTES[3])
    backup(day(2, 11, 35))
    backup(day(1, 9, 5))

    # Today, hourly, and the letter deleted with the Trash emptied.
    for hours_ago in (5, 4, 3, 2):
        if hours_ago == 3:
            os.remove(os.path.join(HOME, "Documents/Letter to the council.odt"))
        if hours_ago == 2:
            write("Documents/Notes.txt", NOTES[4])
        backup((now - timedelta(hours=hours_ago)).replace(minute=0, second=0))

    shutil.rmtree(work)
    start_daemon()
    daemon("SetConfig", "(ss)", ["security.remember_passphrase", "true"])
    daemon("ImportRepo", "(ss)", [REPO, PASSPHRASE])
    say("  backups opened by Backtrack; it is backing up once more")
    wait_backup(lambda: daemon("SetConfig", "(ss)", ["backup.include", json.dumps([HOME])]))


def wait_backup(start, timeout=900):
    from gi.repository import Gio, GLib

    b = bus()
    loop = GLib.MainLoop()
    seen = []

    def on(_c, _s, _p, _i, _n, params):
        job, kind, outcome = params.unpack()
        seen.append((kind, outcome))
        if kind == "backup":
            loop.quit()

    sub = b.signal_subscribe(BUS, DIFACE, "JobFinished", DPATH, None, Gio.DBusSignalFlags.NONE, on)
    start()
    GLib.timeout_add_seconds(timeout, loop.quit)
    loop.run()
    b.signal_unsubscribe(sub)
    say(f"  jobs: {seen}")


# ── the daemon and the window ───────────────────────────────────────────


def stop_daemon():
    run(["systemctl", "--user", "stop", "backtrack-shots-*"], check=False)
    run(["systemctl", "--user", "reset-failed", "backtrack-shots-*"], check=False)


def start_daemon():
    stop_daemon()
    away = REPO + ".away"
    if "XDG_DATA_HOME" in ENV:
        shutil.rmtree(ENV["XDG_DATA_HOME"], ignore_errors=True)
        os.makedirs(ENV["XDG_DATA_HOME"])
        # The wizard is shown an empty drive, as a new user's would be,
        # unless it is to open the backups there (SHOTS_KEEP_REPO).
        if (
            not os.environ.get("SHOTS_KEEP_REPO")
            and os.path.isdir(REPO)
            and not os.path.exists(away)
        ):
            os.rename(REPO, away)
    elif os.path.isdir(away):
        shutil.rmtree(REPO, ignore_errors=True)
        os.rename(away, REPO)
    run(
        ["systemd-run", "--user", f"--unit=backtrack-shots-daemon-{time.time_ns()}", "--collect"]
        + [f"--setenv={k}={v}" for k, v in ENV.items()]
        + [f"{BIN}/backtrackd"]
    )
    for _ in range(100):
        try:
            daemon("GetConfig")
            return
        except Exception:
            time.sleep(0.2)
    raise SystemExit("the daemon never answered")


def no_overview():
    shell("Main.overview.hide()")
    time.sleep(0.8)


def app(args):
    run(
        ["systemd-run", "--user", f"--unit=backtrack-shots-app-{time.time_ns()}", "--collect"]
        + [f"--setenv={k}={v}" for k, v in ENV.items()]
        + [f"{BIN}/backtrack-gtk"]
        + args
    )
    for _ in range(100):
        if shell(f"{FIND}.length"):
            no_overview()
            time.sleep(1.5)
            return
        time.sleep(0.2)
    raise SystemExit("no Backtrack window appeared")


def close():
    run(["systemctl", "--user", "stop", "backtrack-shots-app-*"], check=False)


def action(name, param=None):
    from gi.repository import GLib

    params = [GLib.Variant("s", param)] if param is not None else []
    call(
        "tools.backtrack.Backtrack",
        "/tools/backtrack/Backtrack/window/1",
        "org.gtk.Actions",
        "Activate",
        "(sava{sv})",
        [name, params, {}],
        timeout=30,
    )
    time.sleep(1)


KEYS = {
    "<Enter>": "Return",
    "<Escape>": "Escape",
    "<Tab>": "Tab",
    "<Down>": "Down",
    "<Menu>": "Menu",
    "<Up>": "Up",
    "<Left>": "Left",
    "<Right>": "Right",
    "<BackSpace>": "BackSpace",
}


def keys(text):
    import re

    syms = []
    for part in re.split(r"(<[A-Za-z]+>)", text):
        if part in KEYS:
            syms.append(f"C.KEY_{KEYS[part]}")
        else:
            # Printable ASCII keysyms are the character's code; any other
            # character is its Unicode keysym, 0x01000000 + its code point.
            syms += [
                str(ord(c)) if 0x20 <= ord(c) <= 0x7E else hex(0x01000000 + ord(c)) for c in part
            ]
    shell(
        "(() => { const C = imports.gi.Clutter;"
        " const seat = global.stage.get_context().get_backend().get_default_seat();"
        " const k = seat.create_virtual_device(C.InputDeviceType.KEYBOARD_DEVICE);"
        " let t = global.get_current_time() * 1000;"
        " for (const s of [" + ",".join(syms) + "]) {"
        "   k.notify_keyval(t += 20000, s, C.KeyState.PRESSED);"
        "   k.notify_keyval(t += 20000, s, C.KeyState.RELEASED); }"
        " return true; })()"
    )
    time.sleep(0.8)


def scheme(which):
    run(
        [
            "gsettings",
            "set",
            "org.gnome.desktop.interface",
            "color-scheme",
            "prefer-dark" if which == "dark" else "default",
        ]
    )
    time.sleep(1.5)


def window(w, h):
    shell(f"""(() => {{ const w = {FIND}.pop(); if (!w) return false;
        if (w.get_maximized && w.get_maximized()) w.unmaximize(imports.gi.Meta.MaximizeFlags.BOTH);
        const area = w.get_work_area_current_monitor();
        w.move_resize_frame(false, area.x + (area.width - {w}) / 2,
                            area.y + (area.height - {h}) / 2, {w}, {h});
        w.activate(global.get_current_time()); return true; }})()""")
    time.sleep(1.5)


def shot(name, whole=False):
    os.makedirs(OUT, exist_ok=True)
    no_overview()
    out = os.path.join(OUT, f"{name}.png")
    if whole:
        ok, path = call(
            "org.gnome.Shell.Screenshot",
            "/org/gnome/Shell/Screenshot",
            "org.gnome.Shell.Screenshot",
            "Screenshot",
            "(bbs)",
            [False, False, out],
        )
    else:
        ok, path = call(
            "org.gnome.Shell.Screenshot",
            "/org/gnome/Shell/Screenshot",
            "org.gnome.Shell.Screenshot",
            "ScreenshotWindow",
            "(bbbs)",
            [True, False, False, out],
        )
    if not ok:
        raise SystemExit(f"no screenshot of {name}")
    say(out)


def press(label):
    import warnings

    import gi

    warnings.simplefilter("ignore", DeprecationWarning)
    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi

    desk = Atspi.get_desktop(0)
    apps = [desk.get_child_at_index(i) for i in range(desk.get_child_count())]
    app = next((a for a in apps if a and a.get_name() == "backtrack-gtk"), None)
    if app is None:
        raise SystemExit("Backtrack's window is not in the accessibility tree")
    found = []

    showing = Atspi.StateType.SHOWING

    # A button's label has actions of its own (copy, select all), so only
    # what can be clicked or activated, and only what is on the screen.
    def walk(o, depth):
        if depth > 60:
            return
        try:
            if o.get_name() == label and o.get_state_set().contains(showing):
                action = o.get_action_iface()
                names = (
                    [action.get_action_name(i) for i in range(action.get_n_actions())]
                    if action
                    else []
                )
                for wanted in ("click", "activate"):
                    if wanted in names:
                        found.append((action, names.index(wanted)))
                        break
            for i in range(o.get_child_count()):
                child = o.get_child_at_index(i)
                if child:
                    walk(child, depth + 1)
        except Exception:
            pass

    labels = []

    def labelled(o, depth):
        if depth > 60:
            return
        try:
            if o.get_name() == label and o.get_state_set().contains(showing):
                labels.append(o)
            for i in range(o.get_child_count()):
                child = o.get_child_at_index(i)
                if child:
                    labelled(child, depth + 1)
        except Exception:
            pass

    walk(app, 0)
    if found:
        action, index = found[-1]
        action.do_action(index)
    else:
        # A sidebar's row (Preferences' pages) has no action of its own, and
        # changes the page when it is clicked: so it is clicked, where the
        # accessibility tree says it is in the window, which is where the
        # Shell says the window is.
        labelled(app, 0)
        if not labels:
            raise SystemExit(f"nothing labelled {label!r} to press or click")
        click(labels[-1])
    time.sleep(1.2)


def click(obj):
    """A real click on an accessible object: where the accessibility tree says
    it is in its window, which is where the Shell says the window is."""
    import gi

    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi

    box = obj.get_component_iface().get_extents(Atspi.CoordType.WINDOW)
    x, y = box.x + box.width / 2, box.y + box.height / 2
    pointer = (
        "const C = imports.gi.Clutter;"
        " const seat = global.stage.get_context().get_backend().get_default_seat();"
        " globalThis.shotsPointer ??= seat.create_virtual_device(C.InputDeviceType.POINTER_DEVICE);"
        " const p = globalThis.shotsPointer;"
        " const t = global.get_current_time() * 1000;"
    )
    # The pointer moves first and settles, so that the window has it before
    # the button goes down. GTK's window coordinates leave out the shadow,
    # as the frame does.
    shell(f"""(() => {{ {pointer}
        const w = {FIND}.find(w => w.has_focus()) || {FIND}.pop();
        const r = w.get_frame_rect();
        p.notify_absolute_motion(t, r.x + {x}, r.y + {y}); return true; }})()""")
    time.sleep(0.6)
    shell(f"""(() => {{ {pointer}
        p.notify_button(t, C.BUTTON_PRIMARY, C.ButtonState.PRESSED);
        p.notify_button(t + 50000, C.BUTTON_PRIMARY, C.ButtonState.RELEASED);
        return true; }})()""")
    time.sleep(0.8)


def fill(label, text):
    import warnings

    import gi

    warnings.simplefilter("ignore", DeprecationWarning)
    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi

    desk = Atspi.get_desktop(0)
    apps = [desk.get_child_at_index(i) for i in range(desk.get_child_count())]
    app = next(a for a in apps if a and a.get_name() == "backtrack-gtk")
    fields = []

    def walk(o, depth, inside):
        if depth > 60:
            return
        try:
            named = inside or (
                o.get_name() == label and o.get_state_set().contains(Atspi.StateType.SHOWING)
            )
            if named and o.get_role_name() in ("text", "password text", "entry"):
                fields.append(o)
            for i in range(o.get_child_count()):
                child = o.get_child_at_index(i)
                if child:
                    walk(child, depth + 1, named)
        except Exception:
            pass

    walk(app, 0, False)
    if not fields:
        raise SystemExit(f"no field labelled {label!r}")
    click(fields[0])
    keys(text)


def files(path):
    run(["nautilus", "-q"], check=False)
    time.sleep(1)
    subprocess.Popen(
        ["nautilus", "--select", path],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    for _ in range(100):
        if shell(
            "global.display.list_all_windows().some(w => w.get_wm_class() === 'org.gnome.Nautilus')"
        ):
            break
        time.sleep(0.2)
    no_overview()
    time.sleep(2)
    shell("""(() => { const w = global.display.list_all_windows().find(w =>
            w.get_wm_class() === 'org.gnome.Nautilus');
        const area = w.get_work_area_current_monitor();
        w.move_resize_frame(false, area.x + (area.width - 1100) / 2,
            area.y + (area.height - 700) / 2, 1100, 700);
        w.activate(global.get_current_time()); return true; })()""")
    time.sleep(1.5)
    keys("<Menu>")
    time.sleep(1.5)


def area(name, wm_class):
    """The window's frame and a margin, so that a menu open over it is in."""
    rect = shell(f"""(() => {{ const w = global.display.list_all_windows()
            .find(w => w.get_wm_class() === '{wm_class}');
        const r = w.get_frame_rect(); return [r.x, r.y, r.width, r.height]; }})()""")
    x, y, w, h = rect
    m = 24
    os.makedirs(OUT, exist_ok=True)
    out = os.path.join(OUT, f"{name}.png")
    ok, _ = call(
        "org.gnome.Shell.Screenshot",
        "/org/gnome/Shell/Screenshot",
        "org.gnome.Shell.Screenshot",
        "ScreenshotArea",
        "(iiiibs)",
        [x - m, y - m, w + 2 * m, h + 2 * m, False, out],
    )
    if not ok:
        raise SystemExit(f"no screenshot of {name}")
    say(out)


def forget():
    daemon("SetConfig", "(ss)", ["security.remember_passphrase", "false"])
    start_daemon()


def unlock():
    daemon("ImportRepo", "(ss)", [REPO, PASSPHRASE])
    daemon("SetConfig", "(ss)", ["security.remember_passphrase", "true"])


def trash(name):
    path = os.path.join(HOME, "Documents", name)
    write(path, "Milk, eggs, flour\nApples for the cake\nBella's food\n")
    run(["gio", "trash", path])


if __name__ == "__main__":
    cmd, rest = (sys.argv[1], sys.argv[2:]) if len(sys.argv) > 1 else ("", [])
    if cmd == "home":
        home()
    elif cmd == "daemon":
        start_daemon()
    elif cmd == "app":
        app(rest)
    elif cmd == "close":
        close()
    elif cmd == "action":
        action(rest[0], rest[1] if len(rest) > 1 else None)
    elif cmd == "keys":
        keys(rest[0])
    elif cmd == "scheme":
        scheme(rest[0])
    elif cmd == "window":
        window(int(rest[0]), int(rest[1]))
    elif cmd == "shot":
        shot(rest[0])
    elif cmd == "screen":
        shot(rest[0], whole=True)
    elif cmd == "trash":
        trash(rest[0])
    elif cmd == "press":
        press(rest[0])
    elif cmd == "fill":
        fill(rest[0], rest[1])
    elif cmd == "files":
        files(rest[0])
    elif cmd == "area":
        area(rest[0], rest[1])
    elif cmd == "forget":
        forget()
    elif cmd == "unlock":
        unlock()
    else:
        raise SystemExit(__doc__)
