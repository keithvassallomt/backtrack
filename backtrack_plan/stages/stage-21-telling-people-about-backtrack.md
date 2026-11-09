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
Added at Keith's request, 2026-11-08. A script that installs Backtrack. What
it does, which systems it covers and where it is published are to be agreed
with Keith before work starts; this section is then rewritten to match.

**Accept:** Keith has agreed the scope, and the criteria written for it here
then pass.

## Definition of Done
All tasks done.
