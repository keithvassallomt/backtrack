# Stage 9 — Onboarding Wizard & Preferences

## Objective
First-run setup that a non-technical user finishes in under three minutes with
their recovery key saved, and a Preferences window covering every setting the
product has. After this stage a fresh machine goes zero → protected via GUI alone.

## Prerequisites
Stages 2–4 (SetupRepo/ImportRepo, scheduler); Stage 6 shell. **Normative
references:** prototype.md Screens 9 (wizard) + 10 (menu & prefs) wireframes;
mockups [10](../mockups/10-wizard-welcome.png)–[13](../mockups/13-wizard-protect.png),
[15](../mockups/15-prefs-general.png)–[19](../mockups/19-prefs-advanced.png).
Wizard copy in the wireframes/mockups is the copy — do not rewrite it.

## Tasks

### S09-T1 — Wizard flow
`AdwNavigationView`, four pages per mockups 10–13. Triggered on first run (no
config) and from Preferences → Setup. Step 2: "My personal files" =
XDG dirs (Documents, Pictures, Music, Downloads, Desktop + dotfile-sane extras
list in config) with live size estimate (background du with cancel); "Let me
choose" = folder multi-picker. Advanced expander: exclusion list editor seeded
with defaults. Step 3: destination rows — detected removable drives (GVolumeMonitor)
with free space; Network folder (GVfs mount URI entry + mount via GIO, then treated
as path); SSH (user@host:path + connection test); Cloud greyed "Coming later".
Offline info line exactly as mockup 12 (the catches-up wording, NOT "pauses").
Space check line (est. vs free). "Already have backups? Import…" on step 1 →
ImportRepo path → passphrase → jumps to Stage 11's DR entry (until Stage 11 lands:
straight to indexing + main window).
**Accept:** GUI walkthrough on a clean `BACKTRACK_DEV` home creates repo, config,
units enabled; import path on demo repo reaches browsable main window.

### S09-T2 — Recovery-key gate
Step 4 per mockup 13: passphrase + confirm + zxcvbn-style strength meter (use a
Rust port or entropy heuristic; "strong" threshold documented), keyring checkbox
(default on), amber warning card, **Save Recovery Key… / Print… with Continue
disabled until one succeeds** (save = portal file dialog writing
`backtrack-recovery-key-<hostname>.txt` from `key_export`; print = GTK print
dialog with a rendered sheet: key, repo path, date, app URL).
**Accept:** Continue truly gated (UI test); exported file re-imports successfully
against the repo (round-trip test via borg key import in an integration test).

### S09-T3 — First backup kickoff
"Start First Backup": enables schedule, starts backup job, shows progress page
with the honest copy ("may take a few hours… you can close this window"), close
allowed (daemon continues), completion notification. Backfill/indexing status
shown if importing.
**Accept:** closing the window mid-first-backup doesn't kill the job; notification
arrives on completion (dev-mode small dataset).

### S09-T4 — Preferences
`AdwPreferencesWindow`, five pages exactly per mockups 15–19 and the Screen-10
ASCII (which is the authoritative settings inventory). Every row live-wired via
GetConfig/SetConfig (no Apply buttons); destructive/red rows confirm per HIG.
Notes: Dolphin row shows Install… hint when plugin missing (Stage 12 provides
detection); Storage page's offline group binds to Stage 5 status; Security page's
key export reuses S09-T2 machinery; Advanced page's Verify/Free Up Space run
daemon jobs with progress toasts; Reset All Settings = config reset + wizard
relaunch, explicitly NOT touching repos/index (copy says so).
**Accept:** settings snapshot test — every config.toml key has exactly one UI
control (generated cross-check list); each page screenshot-compared against its
mockup in both themes.

### S09-T5 — Run wizard again
Preferences → Setup row relaunches the wizard pre-filled from current config;
finishing updates config without recreating the repo (unless destination changed →
explicit confirmation explaining consequences: new repo starts fresh, old one
remains restorable via Import).
**Accept:** run-again changing only frequency touches nothing else (config diff
test); destination change path shows the confirmation and leaves the old repo
intact.

## Definition of Done
Zero-to-protected GUI run on a clean dev home in <3 min (scripted walkthrough
timed); recovery-key round-trip proven; all preference rows functional; CI green;
progress.md + CHANGELOG updated.
