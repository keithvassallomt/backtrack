# Stage 20 — The Website at backtrack.tools

## Objective
backtrack.tools stops saying "coming soon" and becomes Backtrack's home.
Software centres open it from the listing's Homepage link (the AppStream
`homepage`). There, someone sees within seconds what Backtrack does,
installs it, and finds a guide when they need one.

Decided with Keith, 2026-11-07:
- **Pages:** the home page, one long page; `/install`; `/guides/`, which are
  Stage 19's guides; `/changelog`; and a 404 page.
- **Built with Zola**, a static site generator: one program, in Arch's
  `extra` repository. The site is deployed to S3 and CloudFront as now.
- **The guides, the install page and the changelog are rendered from this
  repository at a release tag**, never copied by hand, so the site cannot
  drift from the release it describes.
- **No analytics**, as the app has no telemetry (brief.md's non-goals).
- **Every line of copy is drafted and then run through the
  `humanizer:humanizer` skill** before it is committed. Keith's writing
  rules apply: no em-dash, and no sentence that begins with "And".
- **The colours come from the icon** and the AppStream branding (`#dcc6ff`,
  `#4b2f7f`), not from the coming-soon page's blue, and the look stays away
  from security tools' (brief.md's naming note).

## Where the work happens
In the backtrack.tools repository
(<https://github.com/keithvassallomt/backtrack.tools>), except S20-T10 and
S20-T11, which change this one. The checkboxes stay in this repository's
`progress.md`. Since the two repositories cannot share a commit, a task is
ticked in a commit here that names the backtrack.tools commit completing it.

## Prerequisites
Stage 19: the guides, `docs/README.md`'s list of them, the screenshots, and
S19-T2's answer for the questions. For S20-T12, the release the site
describes, on every channel the install page lists. Deploying needs the AWS
CLI signed in to the account that holds the domain (Keith).

## Order of work
Nothing here starts until Keith says so (2026-11-07). Then T1 and T2, which
can go live with the coming-soon page still in place; T3 once S19-T1's index
exists; T4 before any page is filled; T5 to T9; T10 and T11, ready for the
next release; T12 last.

## Tasks

### S20-T1 — The website built with Zola
The backtrack.tools repository laid out for Zola: `config.toml`,
`templates/`, `content/`, and `static/` (the icon, the screenshots, and
`.well-known/org.friendlyhub.VerifiedApps.txt`, by which FriendlyHub knows
the application ID belongs to this domain). `just build` builds into
`public/`; `just serve` shows it on `localhost:1111`, reloading as files
change; `just deploy` builds and then syncs `public/` as `infra/deploy`
syncs `site/` today. The coming-soon page is rebuilt this way first, so the
change of tooling goes live on its own. The repository's README says what is
where.

**Accept:** after a deploy, the coming-soon page reads as before;
`https://backtrack.tools/.well-known/org.friendlyhub.VerifiedApps.txt` serves
the same token (checked with `curl`); a build that fails stops the deploy
before anything is uploaded.

### S20-T2 — Pages below the top level, and a real 404, on CloudFront
CloudFront's default object covers only `/`, so `/guides/rewind/` does not
find its `index.html` today. `infra/www-redirect.js` also rewrites such a
path to its `index.html`, and redirects one without the trailing slash.
The 403 the bucket gives for a missing path is served as `/404.html` with
status 404, rather than as the home page. `infra/provision` brings the
existing distribution in line.

**Accept:** checked with `curl` against the live distribution: `/guides/`
and `/guides/rewind/` answer 200 with their own pages; `/guides/rewind`
redirects to `/guides/rewind/`; a missing path answers 404 with the 404
page; `www.backtrack.tools` still redirects to `backtrack.tools`.

### S20-T3 — Stage 19's guides published under /guides/
`just fetch-docs [REF]`, the newest release tag by default, copies the
guides that `docs/README.md` lists, in its order, into `content/guides/`
(not committed), with `docs/images/`. On the way it adds the header Zola
needs (the title from the guide's first heading, the position from the
index), turns links between guides into the site's (`rewind.md#section`
becomes `/guides/rewind/#section`), and makes a link to anything else in
the repository a link to it on GitHub at the same tag. `/guides/` lists the
guides in the index's order. Headings keep the anchors GitHub gives them, so
that a section Backtrack links to has the same name on both (S20-T11).

**Accept:** every guide in the index is rendered; a link check over the
built site finds no broken internal link, and fails on one planted to prove
it; the built pages have the anchors `from-a-package`, `from-the-flatpak`
and `giving-backtrack-access`.

### S20-T4 — The website's look
The template every page shares: a header with the icon, the name, Install,
Guides and GitHub; a footer with the licence, the changelog, the source, the
issue tracker and the copyright. Type and colour from the icon's palette,
light and dark following the system, readable at phone width, and every
font and file served from backtrack.tools itself. Landmarks, visible
keyboard focus, text alternatives for every picture, and contrast that meets
WCAG AA in both styles. Keith sees the design before any page is filled.

**Accept:** Keith approves the design; the contrast of every text colour
against its background is computed and meets AA in both styles.

### S20-T5 — The home page
Top to bottom, as agreed with Keith on 2026-11-07:
- **The opening:** the icon, the name, "Browse your files as they were", a
  sentence on what Backtrack does, Get Backtrack and GitHub, with Rewind
  moving behind it (S20-T6).
- **brief.md's stories, as things that happen to people:** a file deleted
  and the Trash emptied, a document spoiled by a rewrite, a download nobody
  can find, the right version of a photo, a computer lost. Each has its
  screenshot and a link to its guide.
- **Rewind.**
- **Restores that cannot make things worse:** both versions' dates shown,
  Keep Both, nothing deleted, Undo, and 30 days of Recently Replaced Files.
- **Backups that look after themselves:** every hour, catching up after
  sleep, waiting on battery and metered connections, protection while the
  drive is away, the Trash and Snapper between backups, and quiet unless
  something needs attention.
- **At home on GNOME and KDE:** the file managers' items, the tray, the
  `backtrack` command, a screen reader and the keyboard.
- **Your backups are yours:** Borg underneath, encrypted and deduplicated,
  readable with Borg alone if Backtrack were gone; the recovery key; no
  telemetry; GPL-3.0-or-later.
- **What Backtrack does not do** (brief.md's non-goals): the system's files,
  more than one destination, one set of backups shared between computers,
  and cloud storage, though any server reachable over SSH works.
- **Questions** people ask first, among them S19-T2's answer on backups made
  by Vorta or Pika Backup.
- **Get Backtrack:** FriendlyHub's Flatpak first, then the distributions,
  linking to `/install`.

Whether the copy names Time Machine, as the comparison its readers know, is
Keith's decision, asked before writing: the app itself never uses the name
(Stage 18's Objective).

**Accept:** Keith approves the page; every claim on it is true of the
release the site describes, each checked against the app or its CHANGELOG
rather than from memory; the copy went through the humanizer skill.

### S20-T6 — Rewind, moving, on the home page
A short, muted, looping video of Rewind going back through a folder's
backups and restoring a file. Keith records it on S19-T3's screenshot user,
from the shot list this task writes. This task turns it into WebM and MP4
with a poster frame, and shows the still in its place when the system asks
for reduced motion. It stays small enough not to slow the page (under 3 MB).

**Accept:** Keith sees it play in Firefox and GNOME Web, and sees the still
in its place with reduced motion turned on.

### S20-T7 — The install page
`/install` is `docs/installing.md` (S19-T4) at the same tag, fetched by
S20-T3, in a layout that makes each way of installing easy to find and that
works without JavaScript.

**Accept:** the build checks that every command on the page is exactly the
guide's, and fails when one is changed; each way is followed from the live
page once the release is on its channels (S20-T12).

### S20-T8 — The changelog page
`/changelog` is `CHANGELOG.md` at the same tag: released versions only,
without Unreleased or the rules comment at the top, newest first, each
version with an anchor of its own, and the entries' links kept.

**Accept:** the page shows every released version; the build fails when an
Unreleased section reaches the page, proved by planting one.

### S20-T9 — Search engines and shared links
A title and description for each page. The home page's title says what
Backtrack is, so that a search result can be told from the BackTrack Linux
distribution's (brief.md's naming note). Also schema.org
`SoftwareApplication` data (name, Linux, category, licence, free, where to
download), Open Graph and Twitter card tags with a 1200 × 630 picture of
Rewind, `sitemap.xml`, `robots.txt`, and canonical URLs without `www`.

**Accept:** the structured data passes the Schema.org validator; Keith sees
the picture and title in a link preview in a messenger.

### S20-T10 — Software centres show screenshots served from backtrack.tools
The AppStream listing (`packaging/tools.backtrack.Backtrack.metainfo.xml`)
takes its screenshots from `https://backtrack.tools/screenshots/`, under
names that never change, chosen from S19-T3's set. Today it takes them from
raw.githubusercontent.com paths on `main`: those show the window from before
Stages 15 to 18, and moving the files would break every released listing.
The site keeps each name working from then on. This ships in the next
release.

**Accept:** `appstreamcli validate` passes with the network, which fetches
every screenshot; each new screenshot shows the release it is listed with.

### S20-T11 — Preferences opens the guides on backtrack.tools
How to Install… and the Snapper links
(`crates/backtrack-gtk/src/model/prefs.rs`) open the guides on
backtrack.tools, at the same sections, instead of their Markdown on GitHub.
The files and headings on GitHub stay, for the releases already installed
that link there. This ships in the next release, with a CHANGELOG entry.

**Accept:** the tests hold the sections in the guides, as they do today,
with S20-T3's anchor check covering the site; on the GNOME VM, each link
from Preferences opens its guide at its section.

### S20-T12 — The website goes live with the release
The site replaces the coming-soon page once the version `/install` names is
on every channel it lists (FriendlyHub, COPR, the PPA and the AUR), with the
guides, the install page and the changelog rendered from that release's tag.

**Accept:** the link check passes against the live site; Backtrack's
Homepage link in GNOME Software and in Discover opens it; Keith has looked
it over.

## Definition of Done
All tasks done; backtrack.tools serves the site, with no "coming soon"
anywhere; FriendlyHub's verification file is still served; the
backtrack.tools README says how to build, preview and deploy; the CHANGELOG's
Unreleased section covers S20-T10 and S20-T11.
