// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Pausing Borg while the computer goes to sleep (S15-T9).
//!
//! A backup to a network share can stop a laptop sleeping. When logind
//! announces sleep, NetworkManager takes the network down within about a
//! tenth of a second, and the kernel freezes processes only after that. A Borg
//! that is waiting on the share at that moment, or the SMB client closing a
//! file Borg has just used, can then never finish, and the kernel gives up on
//! the freeze after 20 seconds. The machine stays awake, in somebody's bag.
//!
//! So the daemon takes logind's announcement as the moment to stop Borg, with
//! SIGSTOP, while the network is still there: a stopped process starts no more
//! I/O, and what it already has in flight gets the rest of that moment to
//! finish. A delay lock makes logind wait until that is done. On wake, SIGCONT
//! lets Borg carry on exactly as it would after the kernel's own freeze.
//!
//! Stopping the job instead would be worse than doing nothing: Borg answers
//! SIGTERM by rolling back its transaction and removing its lock, which is a
//! burst of writes to the share just as the network goes.
//!
//! This narrows the window; it cannot close it. A read still waiting for the
//! server when the network goes is past anything user space can do.

use std::time::Duration;

use futures::StreamExt;
use rustix::process::{Pid, Signal};
use tracing::{debug, info, warn};

/// What the kernel calls every Borg the engine starts. The engine runs `borg`
/// from `PATH`, and Borg is a script, whose process is named after the script.
const BORG: &str = "borg";

/// How long Borg stays stopped if the wake is never announced: logind
/// restarting, or the system bus going away, must not leave a backup stopped
/// until the daemon next restarts. The clock behind it does not run while the
/// computer sleeps, so a long sleep does not use it up.
const RESUME_ANYWAY: Duration = Duration::from_secs(120);

/// What `systemd-inhibit --list` shows for the lock.
const WHO: &str = "Backtrack";
const WHY: &str = "Pausing Borg before the computer sleeps";

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Login1Manager {
    fn inhibit(
        &self,
        what: &str,
        who: &str,
        why: &str,
        mode: &str,
    ) -> zbus::Result<zbus::zvariant::OwnedFd>;

    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

/// The processes to stop for sleep.
trait Stoppable: Send + Sync {
    /// Stop them, and say which were stopped.
    fn stop(&self) -> Vec<Pid>;
    /// Let the ones [`Stoppable::stop`] stopped carry on.
    fn resume(&self, stopped: &[Pid]);
}

/// The daemon's own child processes with a given name.
struct Children {
    name: &'static str,
}

impl Stoppable for Children {
    fn stop(&self) -> Vec<Pid> {
        children_named(self.name)
            .into_iter()
            .filter(|&pid| rustix::process::kill_process(pid, Signal::STOP).is_ok())
            .collect()
    }

    fn resume(&self, stopped: &[Pid]) {
        for &pid in stopped {
            // One that has gone in the meantime has nothing to resume.
            let _ = rustix::process::kill_process(pid, Signal::CONT);
        }
    }
}

/// Follow logind's sleep announcements for the daemon's lifetime.
///
/// Everything here is survivable: without logind there is nothing to follow,
/// and without the lock the announcements are still acted on, only without
/// logind waiting for it.
pub async fn watch(connection: zbus::Connection) {
    follow(&connection, &Children { name: BORG }, RESUME_ANYWAY).await;
}

async fn follow(connection: &zbus::Connection, borg: &dyn Stoppable, resume_anyway: Duration) {
    let login1 = match Login1ManagerProxy::new(connection).await {
        Ok(proxy) => proxy,
        Err(e) => return debug!("not following sleep: {e}"),
    };
    // Subscribed to before the lock is taken: an announcement between the two
    // would otherwise be missed while the lock was held, and logind would wait
    // out its whole delay for nothing.
    let mut announcements = match login1.receive_prepare_for_sleep().await {
        Ok(stream) => stream,
        Err(e) => return debug!("not following sleep: {e}"),
    };
    let mut lock = take_lock(&login1).await;
    info!("Borg will be paused while the computer sleeps");

    while let Some(announcement) = announcements.next().await {
        // A wake with nothing paused, from a sleep that began before this
        // watcher did, has nothing to undo.
        if !announcement.args().is_ok_and(|a| a.start) {
            continue;
        }
        let stopped = borg.stop();
        if !stopped.is_empty() {
            info!(processes = stopped.len(), "Borg paused for sleep");
        }
        // Only now may the computer go to sleep.
        drop(lock.take());

        let woke = tokio::time::timeout(resume_anyway, async {
            while let Some(announcement) = announcements.next().await {
                if announcement.args().is_ok_and(|a| !a.start) {
                    return true;
                }
            }
            false
        })
        .await;
        borg.resume(&stopped);
        match woke {
            Ok(true) => {
                if !stopped.is_empty() {
                    info!("the computer woke; Borg carries on");
                }
            }
            Ok(false) => {
                warn!("logind stopped announcing sleep; Borg carries on");
                return;
            }
            Err(_) => warn!(
                seconds = resume_anyway.as_secs(),
                "no wake was announced; Borg carries on anyway"
            ),
        }
        lock = take_lock(&login1).await;
    }
}

/// Ask logind to wait for Borg to be paused before the computer sleeps. The
/// lock lasts until the descriptor is closed.
async fn take_lock(login1: &Login1ManagerProxy<'_>) -> Option<zbus::zvariant::OwnedFd> {
    match login1.inhibit("sleep", WHO, WHY, "delay").await {
        Ok(fd) => Some(fd),
        Err(e) => {
            warn!("logind will not wait for Borg to be paused before sleep: {e}");
            None
        }
    }
}

/// This process's children called `name`, found in `/proc`.
fn children_named(name: &str) -> Vec<Pid> {
    let me = rustix::process::getpid().as_raw_nonzero().get();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .as_deref()
                .and_then(parse_stat)
                .is_some_and(|(comm, parent)| parent == me && comm == name)
        })
        .filter_map(Pid::from_raw)
        .collect()
}

/// The name and parent in a `/proc/<pid>/stat` line.
///
/// The name is in parentheses and may itself hold spaces and parentheses, so
/// it runs to the last `)`, not the first.
fn parse_stat(line: &str) -> Option<(&str, i32)> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    let comm = line.get(open + 1..close)?;
    let mut rest = line.get(close + 1..)?.split_whitespace();
    let _state = rest.next()?;
    let parent = rest.next()?.parse().ok()?;
    Some((comm, parent))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    /// What a lock was asked for with: what, who, why, mode.
    type Request = (String, String, String, String);

    /// A logind that hands out locks and remembers them, so a test can see
    /// whether the newest is still held.
    #[derive(Clone, Default)]
    struct FakeLogind {
        /// Our end of each lock handed out, newest last.
        locks: Arc<Mutex<Vec<UnixStream>>>,
        requests: Arc<Mutex<Vec<Request>>>,
    }

    impl FakeLogind {
        fn locks_taken(&self) -> usize {
            self.locks.lock().unwrap().len()
        }

        /// Whether the newest lock is still held: its other end is open for
        /// as long as the daemon keeps the descriptor.
        fn holding(&self) -> bool {
            let locks = self.locks.lock().unwrap();
            let Some(ours) = locks.last() else {
                return false;
            };
            let mut byte = [0u8; 1];
            match (&*ours).read(&mut byte) {
                Ok(0) => false,
                Ok(_) => true,
                Err(e) => e.kind() == std::io::ErrorKind::WouldBlock,
            }
        }
    }

    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl FakeLogind {
        fn inhibit(
            &self,
            what: &str,
            who: &str,
            why: &str,
            mode: &str,
        ) -> zbus::fdo::Result<zbus::zvariant::OwnedFd> {
            let (ours, theirs) = UnixStream::pair().unwrap();
            ours.set_nonblocking(true).unwrap();
            self.locks.lock().unwrap().push(ours);
            self.requests.lock().unwrap().push((
                what.to_string(),
                who.to_string(),
                why.to_string(),
                mode.to_string(),
            ));
            Ok(std::os::fd::OwnedFd::from(theirs).into())
        }

        #[zbus(signal)]
        async fn prepare_for_sleep(
            emitter: &zbus::object_server::SignalEmitter<'_>,
            start: bool,
        ) -> zbus::Result<()>;
    }

    /// What the watcher did to the processes it would stop, and whether the
    /// lock was still held when it stopped them.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Did {
        Stopped { lock_held: bool },
        Resumed(Vec<i32>),
    }

    struct Recorder {
        logind: FakeLogind,
        did: Mutex<Vec<Did>>,
    }

    impl Stoppable for Recorder {
        fn stop(&self) -> Vec<Pid> {
            self.did.lock().unwrap().push(Did::Stopped {
                lock_held: self.logind.holding(),
            });
            vec![Pid::from_raw(4242).unwrap()]
        }

        fn resume(&self, stopped: &[Pid]) {
            let pids = stopped.iter().map(|p| p.as_raw_nonzero().get()).collect();
            self.did.lock().unwrap().push(Did::Resumed(pids));
        }
    }

    impl Recorder {
        fn did(&self) -> Vec<Did> {
            self.did.lock().unwrap().clone()
        }
    }

    const PATH: &str = "/org/freedesktop/login1";

    /// A daemon-side connection to a fake logind, peer to peer.
    async fn connected(logind: FakeLogind) -> (zbus::Connection, zbus::Connection) {
        let (a, b) = tokio::net::UnixStream::pair().unwrap();
        let server = zbus::connection::Builder::unix_stream(a)
            .server(zbus::Guid::generate())
            .unwrap()
            .p2p()
            .serve_at(PATH, logind)
            .unwrap()
            .build();
        let client = zbus::connection::Builder::unix_stream(b).p2p().build();
        let (server, client) = futures::join!(server, client);
        (server.unwrap(), client.unwrap())
    }

    async fn announce(server: &zbus::Connection, start: bool) {
        let emitter = zbus::object_server::SignalEmitter::new(server, PATH).unwrap();
        FakeLogind::prepare_for_sleep(&emitter, start)
            .await
            .unwrap();
    }

    async fn until(what: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if what() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        what()
    }

    #[tokio::test]
    async fn borg_is_stopped_before_the_computer_may_sleep_and_resumed_on_wake() {
        let logind = FakeLogind::default();
        let (server, client) = connected(logind.clone()).await;
        let recorder = Arc::new(Recorder {
            logind: logind.clone(),
            did: Mutex::new(Vec::new()),
        });
        let watcher = {
            let recorder = Arc::clone(&recorder);
            tokio::spawn(async move { follow(&client, &*recorder, Duration::from_secs(60)).await })
        };

        assert!(
            until(|| logind.holding()).await,
            "a delay lock is taken at once"
        );
        assert_eq!(
            logind.requests.lock().unwrap()[0],
            ("sleep".into(), WHO.into(), WHY.into(), "delay".into())
        );

        announce(&server, true).await;
        assert!(
            until(|| !logind.holding()).await,
            "the lock is let go for sleep"
        );
        assert_eq!(
            recorder.did(),
            vec![Did::Stopped { lock_held: true }],
            "Borg is stopped while logind is still waiting, not after"
        );

        announce(&server, false).await;
        assert!(until(|| logind.locks_taken() == 2 && logind.holding()).await);
        assert_eq!(
            recorder.did(),
            vec![Did::Stopped { lock_held: true }, Did::Resumed(vec![4242])],
            "what was stopped is what carries on"
        );
        watcher.abort();
    }

    #[tokio::test]
    async fn borg_carries_on_when_no_wake_is_announced() {
        let logind = FakeLogind::default();
        let (server, client) = connected(logind.clone()).await;
        let recorder = Arc::new(Recorder {
            logind: logind.clone(),
            did: Mutex::new(Vec::new()),
        });
        let watcher = {
            let recorder = Arc::clone(&recorder);
            tokio::spawn(
                async move { follow(&client, &*recorder, Duration::from_millis(200)).await },
            )
        };
        assert!(until(|| logind.holding()).await);

        announce(&server, true).await;
        assert!(
            until(|| recorder.did().contains(&Did::Resumed(vec![4242]))).await,
            "a backup is not left stopped because the wake went unannounced"
        );
        assert!(
            until(|| logind.locks_taken() == 2 && logind.holding()).await,
            "and the lock is taken again for the next sleep"
        );
        watcher.abort();
    }

    /// The state letter in `/proc/<pid>/stat`: `T` is stopped.
    fn state(pid: u32) -> char {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let close = stat.rfind(')').unwrap();
        stat[close + 1..].trim_start().chars().next().unwrap()
    }

    #[tokio::test]
    async fn only_this_processs_children_of_that_name_are_stopped() {
        // A symlink names the process after itself, as Borg's script does,
        // without writing an executable this busy test process could be
        // forking around at the same moment.
        let dir = tempfile::tempdir().unwrap();
        let named = dir.path().join("bt-sleep-test");
        std::os::unix::fs::symlink(which("sleep"), &named).unwrap();
        let mut borg = tokio::process::Command::new(&named)
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut other = tokio::process::Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let (borg_pid, other_pid) = (borg.id().unwrap(), other.id().unwrap());
        assert!(until(|| state(borg_pid) != 'R').await);

        let children = Children {
            name: "bt-sleep-test",
        };
        let stopped = children.stop();
        assert_eq!(
            stopped
                .iter()
                .map(|p| p.as_raw_nonzero().get() as u32)
                .collect::<Vec<_>>(),
            vec![borg_pid]
        );
        assert!(until(|| state(borg_pid) == 'T').await, "it is stopped");
        assert_ne!(
            state(other_pid),
            'T',
            "a child with another name is left alone"
        );

        children.resume(&stopped);
        assert!(until(|| state(borg_pid) != 'T').await, "and it carries on");

        let _ = borg.kill().await;
        let _ = other.kill().await;
    }

    fn which(program: &str) -> std::path::PathBuf {
        std::env::var_os("PATH")
            .into_iter()
            .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
            .expect("on PATH")
    }

    #[test]
    fn a_name_with_spaces_and_parentheses_is_read_whole() {
        assert_eq!(
            parse_stat("4242 (borg) S 1217 4242 4242 0 -1 4194560"),
            Some(("borg", 1217))
        );
        assert_eq!(
            parse_stat("77 (a (b) c) R 1 77 77 0 -1"),
            Some(("a (b) c", 1))
        );
        assert_eq!(parse_stat("garbage"), None);
    }
}
