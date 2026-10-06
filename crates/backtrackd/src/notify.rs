// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Desktop notifications, sent by the daemon because the window may be gone.
//!
//! The daemon is what notices when something goes wrong, and it is usually the
//! only part of Backtrack running when it does: the window is closed most of
//! the time, which is the point of backups that run by themselves. So the
//! daemon sends notifications itself, through the freedesktop notification
//! service, and a click on one opens the application at the fix for what it
//! was about.
//!
//! What is sent follows the person's choice in General preferences, mapped as
//! health.md says: "Only when attention is needed" means `AT_RISK` and `BROKEN`,
//! "For every backup" adds a note after each one, and "Never" means never.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use backtrack_core::config::Notifications;
use backtrack_core::dbus::Reason;
use futures::StreamExt;
use tracing::{debug, info, warn};
use zbus::zvariant::Value;

use crate::service::Alert;

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;

    fn close_notification(&self, id: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: String) -> zbus::Result<()>;
}

/// What a notification is about. One of each is on screen at a time: a new
/// one replaces the last, so a problem that is still a problem a day later is
/// one notification that has been brought up to date, not two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Topic {
    /// The health of the backups.
    Health,
    /// How a backup went.
    Backup,
}

/// Which of the person's choices a notification falls under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `AT_RISK` or `BROKEN`.
    Attention,
    /// The end of the very first backup.
    FirstBackup,
    /// A backup that went well.
    Success,
    /// The end of a disaster recovery, either way.
    Recovery,
}

/// Whether the person's choice lets this kind of notification through.
///
/// The first backup is announced under "Only when attention is needed" too,
/// because the wizard promised it: the person was told they could close the
/// window, and its end is the thing they closed it waiting for.
pub fn permits(policy: Notifications, kind: Kind) -> bool {
    match (policy, kind) {
        (Notifications::None, _) => false,
        // A recovery is announced on the same terms as the first backup, and
        // for the same reason: the person was told they could close the window
        // while it ran.
        (_, Kind::Attention | Kind::FirstBackup | Kind::Recovery) => true,
        (Notifications::All, Kind::Success) => true,
        (Notifications::AttentionOnly, Kind::Success) => false,
    }
}

/// One notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub topic: Topic,
    pub title: String,
    pub body: String,
    /// What a click opens: the fix for this reason, or the window as it is.
    pub fix: Option<Reason>,
}

/// What to say about an alert the escalation rules raised. The title is the
/// banner's own words, so the person recognises the one from the other.
pub fn alert(alert: Alert, destination: &str) -> Note {
    let (title, body, fix) = match alert {
        Alert::AtRisk { days } => (
            format!(
                "No successful backups for {days} {}",
                if days == 1 { "day" } else { "days" }
            ),
            "Open Backtrack to see what is stopping them.".to_string(),
            Reason::NoRecentBackup,
        ),
        Alert::Broken(failure) => {
            use backtrack_core::engine::HealthFailure;
            let body = match failure {
                HealthFailure::PassphraseMissing => "Backups are paused until it is entered.",
                HealthFailure::PassphraseWrong => {
                    "Backups are paused until the current passphrase is entered."
                }
                HealthFailure::AuthExpired => {
                    "Backups are paused until Backtrack can sign in again."
                }
                HealthFailure::DestinationFull => {
                    "Free up space on it, keep fewer old backups, or choose somewhere else."
                }
                HealthFailure::LocalDiskFull => {
                    "Free up space on this computer so that changes can be protected again."
                }
                HealthFailure::RepoCorrupt => {
                    "Backtrack can check your backups and repair what is damaged."
                }
                HealthFailure::BorgMissing => {
                    "Backups cannot run until BorgBackup is installed again."
                }
            };
            (failure.copy(destination), body.to_string(), failure.into())
        }
    };
    Note {
        topic: Topic::Health,
        title,
        body,
        fix: Some(fix),
    }
}

/// What to say when the first backup ends, or nothing. A cancelled first
/// backup was somebody's own decision and needs no announcement.
pub fn first_backup(outcome: &str) -> Option<Note> {
    let (title, body) = match outcome {
        "completed" => (
            "Your first backup is complete",
            "Your files are protected. From now on, backups run on their own.",
        ),
        "failed" => (
            "Your first backup did not finish",
            "Open Backtrack to see what stopped it.",
        ),
        _ => return None,
    };
    Some(Note {
        topic: Topic::Backup,
        title: title.to_string(),
        body: body.to_string(),
        fix: None,
    })
}

/// What to say when a disaster recovery has brought everything back.
/// `waiting` is how many files clash with this computer's and are waiting to
/// be asked about.
pub fn recovered(restored: u64, waiting: u64) -> Note {
    let body = match waiting {
        0 => format!(
            "{} restored. Backups start again now.",
            files(restored)
        ),
        n => format!(
            "{} restored. {} already on this computer {} different in the backup: open Backtrack to choose which to keep.",
            files(restored),
            files(n),
            if n == 1 { "is" } else { "are" }
        ),
    };
    Note {
        topic: Topic::Backup,
        title: "Your files are back".to_string(),
        body,
        fix: None,
    }
}

/// What to say when a disaster recovery stopped on an error.
pub fn recovery_stopped() -> Note {
    Note {
        topic: Topic::Backup,
        title: "Restoring your files stopped".to_string(),
        body: "Open Backtrack to see why and carry on from where it stopped.".to_string(),
        fix: None,
    }
}

fn files(count: u64) -> String {
    match count {
        1 => "1 file".to_string(),
        n => format!("{n} files"),
    }
}

/// The note after a backup that went well, for somebody who asked for one.
pub fn backup_complete() -> Note {
    Note {
        topic: Topic::Backup,
        title: "Backup complete".to_string(),
        body: "Your files are backed up.".to_string(),
        fix: None,
    }
}

/// Where notifications go.
///
/// A trait so the policy can be tested against a sink that records what it is
/// given, rather than against a desktop.
pub trait NotificationSink: Send + Sync {
    /// Show `note`, replacing whatever is showing on the same topic. Must not
    /// wait for the desktop: this is called from the loop that sends every
    /// other signal, and a notification service that is slow to answer must
    /// not hold them up.
    fn show(&self, note: Note);

    /// Take down whatever is showing on `topic`, because it is no longer true.
    fn withdraw(&self, topic: Topic);
}

/// Before the session bus is reached there is nobody to tell. What would have
/// been said is logged instead.
pub struct NoDesktop;

impl NotificationSink for NoDesktop {
    fn show(&self, note: Note) {
        debug!(title = note.title, "no desktop to show a notification on");
    }

    fn withdraw(&self, _topic: Topic) {}
}

/// How long the notification service gets to answer.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(5);

/// The session's notification service.
pub struct DesktopSink {
    desktop: Arc<Desktop>,
}

struct Desktop {
    connection: zbus::Connection,
    /// What is on screen for each topic, so the next one replaces it.
    shown: Mutex<HashMap<Topic, u32>>,
    /// What a click on each notification should open.
    fixes: Mutex<HashMap<u32, Option<Reason>>>,
}

impl DesktopSink {
    /// Send through `connection`, and start listening for clicks on it.
    ///
    /// The clicks arrive on the connection that sent the notifications, which
    /// is the daemon's own; the application is not involved until one does.
    pub fn start(connection: &zbus::Connection) -> DesktopSink {
        let desktop = Arc::new(Desktop {
            connection: connection.clone(),
            shown: Mutex::new(HashMap::new()),
            fixes: Mutex::new(HashMap::new()),
        });
        tokio::spawn(listen(Arc::clone(&desktop)));
        DesktopSink { desktop }
    }
}

impl NotificationSink for DesktopSink {
    fn show(&self, note: Note) {
        let desktop = Arc::clone(&self.desktop);
        tokio::spawn(async move {
            if tokio::time::timeout(PATIENCE, desktop.deliver(note))
                .await
                .is_err()
            {
                debug!("the notification service did not answer in time");
            }
        });
    }

    fn withdraw(&self, topic: Topic) {
        let Some(id) = self.desktop.shown.lock().unwrap().remove(&topic) else {
            return;
        };
        self.desktop.fixes.lock().unwrap().remove(&id);
        let connection = self.desktop.connection.clone();
        tokio::spawn(async move {
            let closed = async {
                NotificationsProxy::new(&connection)
                    .await?
                    .close_notification(id)
                    .await
            };
            match tokio::time::timeout(PATIENCE, closed).await {
                Ok(Ok(())) => debug!(id, "a notification that is no longer true was taken down"),
                Ok(Err(error)) => debug!(%error, "the notification could not be taken down"),
                Err(_) => debug!("the notification service did not answer in time"),
            }
        });
    }
}

impl Desktop {
    async fn deliver(&self, note: Note) {
        let proxy = match NotificationsProxy::new(&self.connection).await {
            Ok(proxy) => proxy,
            Err(error) => {
                debug!(%error, "no notification service to tell");
                return;
            }
        };
        let replaces = self
            .shown
            .lock()
            .unwrap()
            .get(&note.topic)
            .copied()
            .unwrap_or(0);
        // `default` is the click on the notification itself. A notification
        // about a problem also gets a visible button, because health.md's
        // third principle is that every alert names its fix.
        let actions: &[&str] = if note.fix.is_some() {
            &["default", "Fix…", "fix", "Fix…"]
        } else {
            &["default", "Open Backtrack"]
        };
        // `desktop-entry` is what lets the desktop show the application's own
        // name and icon.
        let hints = HashMap::from([("desktop-entry", Value::from(backtrack_core::secret::APP_ID))]);
        match proxy
            .notify(
                "Backtrack",
                replaces,
                backtrack_core::secret::APP_ID,
                &note.title,
                &note.body,
                actions,
                hints,
                -1,
            )
            .await
        {
            Ok(id) => {
                info!(id, title = note.title, "notification shown");
                self.shown.lock().unwrap().insert(note.topic, id);
                let mut fixes = self.fixes.lock().unwrap();
                if replaces != 0 {
                    fixes.remove(&replaces);
                }
                fixes.insert(id, note.fix);
            }
            Err(error) => debug!(%error, "the notification could not be shown"),
        }
    }
}

/// Open the application for every click on one of ours.
async fn listen(desktop: Arc<Desktop>) {
    let clicks = async {
        NotificationsProxy::new(&desktop.connection)
            .await?
            .receive_action_invoked()
            .await
    };
    let mut clicks = match clicks.await {
        Ok(clicks) => clicks,
        Err(error) => {
            debug!(%error, "clicks on notifications will not be heard");
            return;
        }
    };
    while let Some(click) = clicks.next().await {
        let Ok(args) = click.args() else { continue };
        let fix = desktop.fixes.lock().unwrap().get(&args.id).copied();
        // Somebody else's notification, or one of ours already replaced.
        let Some(fix) = fix else { continue };
        info!(id = args.id, fix = ?fix, "a notification was clicked");
        launch(fix);
    }
}

/// Open the application, at the fix for `fix` if there is one.
///
/// Through the command line, whether or not a window is open: GApplication
/// hands a second launch to the instance already running, so the one path
/// covers both.
fn launch(fix: Option<Reason>) {
    let application = application();
    let (program, args) = command(&application, fix, which("systemd-run").is_some());
    let mut command = tokio::process::Command::new(&program);
    command.args(&args);
    match command.spawn() {
        Ok(mut child) => {
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
        }
        Err(error) => warn!(%error, program = %program.display(), "Backtrack could not be opened"),
    }
}

/// The command that opens the application.
///
/// In a scope of its own when systemd can give it one. Started directly, the
/// window would be a child of this daemon's unit, and stopping the daemon
/// (`systemctl --user restart`, an update) would close somebody's window
/// under them.
pub fn command(
    application: &Path,
    fix: Option<Reason>,
    systemd_run: bool,
) -> (PathBuf, Vec<String>) {
    let mut args: Vec<String> = Vec::new();
    if let Some(fix) = fix {
        args.push("--fix".to_string());
        args.push(fix.as_str().to_string());
    }
    if !systemd_run {
        return (application.to_path_buf(), args);
    }
    let mut scoped: Vec<String> = ["--user", "--scope", "--collect", "--quiet", "--"]
        .iter()
        .map(|arg| arg.to_string())
        .collect();
    scoped.push(application.display().to_string());
    scoped.extend(args);
    (PathBuf::from("systemd-run"), scoped)
}

/// The application's program: beside this one when it is there, which is
/// how both an installed system and a development build are laid out, and
/// otherwise wherever `PATH` finds it.
fn application() -> PathBuf {
    std::env::current_exe()
        .ok()
        .map(|daemon| daemon.with_file_name("backtrack-gtk"))
        .filter(|program| program.is_file())
        .unwrap_or_else(|| PathBuf::from("backtrack-gtk"))
}

/// Where `PATH` finds `program`, if anywhere.
fn which(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(test)]
mod tests {
    use backtrack_core::engine::HealthFailure;

    use super::*;

    #[test]
    fn the_policy_matrix() {
        use Kind::*;
        use Notifications::*;
        for (policy, kind, expected) in [
            (AttentionOnly, Attention, true),
            (AttentionOnly, FirstBackup, true),
            (AttentionOnly, Success, false),
            (All, Attention, true),
            (All, FirstBackup, true),
            (All, Success, true),
            (None, Attention, false),
            (None, FirstBackup, false),
            (None, Success, false),
        ] {
            assert_eq!(permits(policy, kind), expected, "{policy:?} / {kind:?}");
        }
    }

    #[test]
    fn a_problem_is_titled_in_the_banner_s_own_words_and_names_its_fix() {
        for failure in HealthFailure::ALL {
            let note = alert(Alert::Broken(*failure), "nas.local");
            assert_eq!(note.title, failure.copy("nas.local"));
            assert_eq!(note.fix, Some(Reason::from(*failure)));
            assert!(!note.body.is_empty());
            assert_eq!(note.topic, Topic::Health);
        }
        assert_eq!(
            alert(Alert::Broken(HealthFailure::AuthExpired), "nas.local").title,
            "Backtrack can't sign in to nas.local."
        );
    }

    #[test]
    fn at_risk_says_how_long_the_way_the_banner_does() {
        let note = alert(Alert::AtRisk { days: 3 }, "");
        assert_eq!(note.title, "No successful backups for 3 days");
        assert_eq!(note.fix, Some(Reason::NoRecentBackup));
        assert_eq!(
            alert(Alert::AtRisk { days: 1 }, "").title,
            "No successful backups for 1 day"
        );
    }

    #[test]
    fn a_first_backup_is_announced_and_a_cancelled_one_is_not() {
        let done = first_backup("completed").unwrap();
        assert_eq!(done.title, "Your first backup is complete");
        let failed = first_backup("failed").unwrap();
        assert!(failed.title.contains("did not finish"), "{}", failed.title);
        assert_eq!(first_backup("cancelled"), None);
    }

    #[test]
    fn a_click_opens_the_application_at_the_fix_for_what_it_was_about() {
        let app = Path::new("/usr/bin/backtrack-gtk");
        let (program, args) = command(app, Some(Reason::PassphraseMissing), false);
        assert_eq!(program, app);
        assert_eq!(args, vec!["--fix", "passphrase-missing"]);

        let (program, args) = command(app, None, false);
        assert_eq!(program, app);
        assert!(args.is_empty(), "no fix: just the window");
    }

    #[test]
    fn the_window_is_started_outside_the_daemon_s_unit_where_systemd_can() {
        let app = Path::new("/usr/bin/backtrack-gtk");
        let (program, args) = command(app, Some(Reason::DestinationFull), true);
        assert_eq!(program, PathBuf::from("systemd-run"));
        assert_eq!(
            args,
            vec![
                "--user",
                "--scope",
                "--collect",
                "--quiet",
                "--",
                "/usr/bin/backtrack-gtk",
                "--fix",
                "destination-full"
            ]
        );
    }

    #[test]
    fn every_alert_s_route_reads_back_as_the_reason_it_was_about() {
        // The other half of the click is the application parsing this token;
        // it parses with `Reason::parse`, which this proves round-trips.
        for alert in HealthFailure::ALL
            .iter()
            .map(|failure| Alert::Broken(*failure))
            .chain([Alert::AtRisk { days: 2 }])
        {
            let note = super::alert(alert, "x");
            let (_, args) = command(Path::new("backtrack-gtk"), note.fix, false);
            assert_eq!(Reason::parse(&args[1]), note.fix, "{alert:?}");
        }
    }
}
