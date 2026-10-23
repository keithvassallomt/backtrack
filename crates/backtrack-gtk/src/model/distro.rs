// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Which family of distributions this computer belongs to, read from its
//! `os-release`, for advice that names a package and the command that
//! installs it.

/// A family of distributions that installs packages the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Fedora,
    Debian,
    Arch,
    Suse,
}

impl Family {
    /// The command that installs `package` on this family.
    pub fn install(self, package: &str) -> String {
        match self {
            Family::Fedora => format!("sudo dnf install {package}"),
            Family::Debian => format!("sudo apt install {package}"),
            Family::Arch => format!("sudo pacman -S {package}"),
            Family::Suse => format!("sudo zypper install {package}"),
        }
    }
}

/// The family the distribution `os_release` describes belongs to, by its `ID`
/// and `ID_LIKE`. `None` for one this does not know.
pub fn family(os_release: &str) -> Option<Family> {
    let field = |key: &str| {
        os_release.lines().find_map(|line| {
            let value = line.strip_prefix(key)?.strip_prefix('=')?;
            Some(value.trim_matches('"').to_string())
        })
    };
    let ids = format!(
        "{} {}",
        field("ID").unwrap_or_default(),
        field("ID_LIKE").unwrap_or_default()
    );
    let is = |name: &str| ids.split_whitespace().any(|id| id == name);
    if is("fedora") || is("rhel") {
        Some(Family::Fedora)
    } else if is("debian") || is("ubuntu") {
        Some(Family::Debian)
    } else if is("arch") {
        Some(Family::Arch)
    } else if is("suse") || is("opensuse") {
        Some(Family::Suse)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_distribution_is_known_by_its_own_id_or_one_it_is_like() {
        for (release, expected) in [
            ("ID=fedora\n", Some(Family::Fedora)),
            (
                "ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n",
                Some(Family::Fedora),
            ),
            ("ID=ubuntu\nID_LIKE=debian\n", Some(Family::Debian)),
            (
                "ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n",
                Some(Family::Debian),
            ),
            ("ID=arch\n", Some(Family::Arch)),
            ("ID=endeavouros\nID_LIKE=arch\n", Some(Family::Arch)),
            (
                "ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n",
                Some(Family::Suse),
            ),
            ("ID=gentoo\n", None),
            ("", None),
        ] {
            assert_eq!(family(release), expected, "{release:?}");
        }
    }

    #[test]
    fn only_the_id_fields_are_read() {
        assert_eq!(family("VERSION_ID=arch\nID=gentoo\n"), None);
    }
}
