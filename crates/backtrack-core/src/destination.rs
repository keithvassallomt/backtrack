// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Reading a repository address: where it is, and what a person calls it.
//!
//! A repository is configured as the one string Borg takes, which is a path, an
//! `ssh://` address, or the scp-style `user@host:path`. The daemon needs the
//! host to probe, and the daemon and the window both need a name to put in
//! "Backtrack can't sign in to nas.local.", so the reading is done once, here.

/// Host and port for a Borg remote destination, or `None` for a local path.
///
/// Borg accepts `ssh://[user@]host[:port]/path` and the scp-style
/// `[user@]host:path`. The second is the ambiguous one: a plain path containing
/// a colon must not be mistaken for it, so an `@` before the first `/` is
/// required — the same rule preflight uses to decide whether a destination
/// costs network traffic.
pub fn ssh_endpoint(repository: &str) -> Option<(String, u16)> {
    if let Some(rest) = repository.strip_prefix("ssh://") {
        let authority = rest.split('/').next()?;
        let host_port = authority.rsplit('@').next()?;
        return Some(split_host_port(host_port));
    }
    let (prefix, _) = repository.split_once(':')?;
    if !prefix.contains('@') || prefix.contains('/') {
        return None;
    }
    let host = prefix.rsplit('@').next()?;
    (!host.is_empty()).then(|| (host.to_string(), 22))
}

/// Split `host` or `host:port`, defaulting to SSH's port. IPv6 literals arrive
/// bracketed, and the brackets come off — `TcpStream::connect` wants the bare
/// address.
fn split_host_port(text: &str) -> (String, u16) {
    if let Some(rest) = text.strip_prefix('[') {
        if let Some((addr, tail)) = rest.split_once(']') {
            let port = tail
                .strip_prefix(':')
                .and_then(|p| p.parse().ok())
                .unwrap_or(22);
            return (addr.to_string(), port);
        }
    }
    match text.rsplit_once(':') {
        Some((host, port)) => match port.parse() {
            Ok(port) => (host.to_string(), port),
            // Not a port. Borg's scp-style form has already been handled by the
            // caller, so this is a hostname with a colon in it — malformed, but
            // passing it on unchanged gives a better error than inventing one.
            Err(_) => (text.to_string(), 22),
        },
        None => (text.to_string(), 22),
    }
}

/// What a person calls the place their backups go.
///
/// The server, for an SSH address or a share GIO mounted; the drive's label,
/// for a removable drive; and otherwise the path itself, which is long but is
/// at least exactly what they chose.
pub fn name(repository: &str) -> String {
    if let Some((host, _)) = ssh_endpoint(repository) {
        return host;
    }
    let mut parts = repository.split('/').filter(|part| !part.is_empty());
    let first = parts.next();
    let second = parts.next();
    let third = parts.next();
    let fourth = parts.next();
    match (first, second, third, fourth) {
        // /run/user/1000/gvfs/smb-share:server=nas.local,share=backups/...
        (Some("run"), Some("user"), Some(_), Some("gvfs")) => {
            if let Some(server) = parts.next().and_then(gvfs_server) {
                return server;
            }
        }
        // /run/media/k/WD Passport/..., and Debian's /media/k/WD Passport/...
        (Some("run"), Some("media"), Some(_), Some(label))
        | (Some("media"), Some(_), Some(label), _) => return label.to_string(),
        _ => {}
    }
    repository.to_string()
}

/// The server a GIO mount is of, from the name GIO gives its folder:
/// `smb-share:server=nas.local,share=backups`, `sftp:host=nas.local,user=k`.
fn gvfs_server(mount: &str) -> Option<String> {
    let (_, settings) = mount.split_once(':')?;
    settings.split(',').find_map(|setting| {
        let (key, value) = setting.split_once('=')?;
        matches!(key, "server" | "host").then(|| value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borg_remote_destinations_are_recognised_with_their_ports() {
        assert_eq!(
            ssh_endpoint("ssh://nas.local/./backups"),
            Some(("nas.local".into(), 22))
        );
        assert_eq!(
            ssh_endpoint("ssh://keith@nas.local:2222/./backups"),
            Some(("nas.local".into(), 2222))
        );
        assert_eq!(
            ssh_endpoint("keith@nas.local:backups"),
            Some(("nas.local".into(), 22))
        );
        assert_eq!(
            ssh_endpoint("ssh://[2001:db8::1]:2222/./backups"),
            Some(("2001:db8::1".into(), 2222)),
            "an IPv6 literal loses its brackets before it reaches connect()"
        );
    }

    #[test]
    fn a_local_path_is_not_a_remote_destination() {
        assert_eq!(ssh_endpoint("/mnt/usb/backups"), None);
        assert_eq!(
            ssh_endpoint("/mnt/odd:name/backups"),
            None,
            "a colon in a path is still a path"
        );
        assert_eq!(ssh_endpoint(""), None);
    }

    #[test]
    fn a_destination_is_called_what_a_person_would_call_it() {
        for (repository, expected) in [
            ("ssh://keith@nas.local:2222/./backups", "nas.local"),
            ("x1y2@x1y2.repo.borgbase.com:repo", "x1y2.repo.borgbase.com"),
            (
                "/run/user/1000/gvfs/smb-share:server=nas.local,share=backups/Backtrack/thinkpad",
                "nas.local",
            ),
            (
                "/run/user/1000/gvfs/sftp:host=files.example.org,user=k/Backtrack/thinkpad",
                "files.example.org",
            ),
            (
                "/run/media/keith/WD Passport/Backtrack/thinkpad",
                "WD Passport",
            ),
            ("/media/keith/USB/Backtrack/thinkpad", "USB"),
            (
                "/srv/backups/Backtrack/thinkpad",
                "/srv/backups/Backtrack/thinkpad",
            ),
        ] {
            assert_eq!(name(repository), expected, "{repository}");
        }
    }
}
